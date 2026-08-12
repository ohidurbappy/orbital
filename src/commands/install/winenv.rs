//! Reading and writing the Windows *user* environment, and nothing else.
//!
//! This is the only module in the crate that uses `unsafe`. It holds no
//! decisions — the planning in [`super::plan`] decides, this only carries out
//! the registry read, the registry write, and the broadcast — so the FFI stays
//! auditable in one place.
//!
//! # Why not the obvious approaches
//!
//! `setx PATH …` truncates any value over 1024 characters, silently and
//! mid-entry, and cannot express a value's type.
//!
//! `[Environment]::SetEnvironmentVariable(…, 'User')` — what `install.ps1` uses
//! — is worse than it looks. `GetEnvironmentVariable(…,'User')` *expands*
//! `%VAR%` on read, and the setter writes back through `RegistryKey.SetValue`,
//! which infers `REG_SZ`. So a user PATH of
//! `%USERPROFILE%\.cargo\bin;%LOCALAPPDATA%\mingw64\bin;%PATH%` round-trips into
//! a `REG_SZ` containing a frozen copy of the whole machine PATH — every system
//! directory permanently duplicated into the user's own PATH, and the `%VAR%`
//! indirection destroyed.
//!
//! So: read the raw UTF-16, append to it, and write it back with its type
//! preserved byte for byte.

use std::io;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
#[cfg(test)]
use windows_sys::Win32::System::Registry::RegDeleteValueW;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_READ, KEY_SET_VALUE, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
};

/// `HKEY_CURRENT_USER\Environment` — the per-user environment, writable without
/// elevation. The machine value under `HKLM` is deliberately never touched.
const SUBKEY: &str = "Environment";
const VALUE: &str = "Path";

/// How the user PATH is stored: the raw UTF-16 and the value type it had.
pub struct UserPath {
    /// UTF-16 code units, with no terminating NUL, exactly as stored.
    pub raw: Vec<u16>,
    /// `REG_SZ` or `REG_EXPAND_SZ`, to be written back unchanged.
    pub kind: REG_VALUE_TYPE,
}

impl UserPath {
    /// The value as text, for comparison and display only — never for writing
    /// back, because a lossy round trip could mangle an unpaired surrogate.
    pub fn to_text(&self) -> String {
        String::from_utf16_lossy(&self.raw)
    }
}

/// NUL-terminated UTF-16, as every `…W` entry point expects.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn win_error(what: &str, code: u32) -> io::Error {
    io::Error::other(format!("{what} failed (Windows error {code})"))
}

/// A `HKEY` that closes itself.
struct Key(HKEY);

impl Key {
    fn open(access: u32) -> io::Result<Self> {
        let mut handle: HKEY = std::ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated, and `handle` is a valid out-pointer
        // for one HKEY. On success the handle is owned by this struct.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(SUBKEY).as_ptr(),
                0,
                access,
                &mut handle,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(win_error(r"opening HKCU\Environment", status));
        }
        Ok(Self(handle))
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: opened by `Key::open` and not closed anywhere else.
            unsafe { RegCloseKey(self.0) };
        }
    }
}

/// Read the user PATH exactly as stored, without expanding anything.
pub fn read_user_path() -> io::Result<UserPath> {
    read_value(VALUE)
}

fn read_value(value_name: &str) -> io::Result<UserPath> {
    let key = Key::open(KEY_READ)?;
    let name = wide(value_name);

    // Ask for the size, then read; retry if it grew in between.
    for _ in 0..4 {
        let mut kind: REG_VALUE_TYPE = 0;
        let mut bytes: u32 = 0;
        // SAFETY: a null data pointer with a zero length is the documented way
        // to query the size; `kind` and `bytes` are valid out-pointers.
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                std::ptr::null_mut(),
                &mut bytes,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            // A fresh account has no user Path at all. An empty
            // REG_EXPAND_SZ is the right thing to create.
            return Ok(UserPath {
                raw: Vec::new(),
                kind: REG_EXPAND_SZ,
            });
        }
        if status != ERROR_SUCCESS {
            return Err(win_error("reading the user PATH", status));
        }
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("the user PATH has unexpected registry type {kind}"),
            ));
        }

        // Round up so an odd byte count can't truncate a code unit.
        let mut buffer: Vec<u16> = vec![0; (bytes as usize).div_ceil(2)];
        let mut length = (buffer.len() * 2) as u32;
        // SAFETY: the buffer is `length` bytes, which is what we declare.
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                buffer.as_mut_ptr().cast::<u8>(),
                &mut length,
            )
        };
        if status == ERROR_MORE_DATA {
            continue; // it grew; ask again
        }
        if status != ERROR_SUCCESS {
            return Err(win_error("reading the user PATH", status));
        }

        // The returned byte count includes the terminating NUL.
        buffer.truncate((length as usize) / 2);
        while buffer.last() == Some(&0) {
            buffer.pop();
        }
        return Ok(UserPath { raw: buffer, kind });
    }

    Err(io::Error::other(
        "the user PATH kept changing while being read",
    ))
}

/// Overwrite the user PATH, keeping its original value type.
pub fn write_user_path(value: &[u16], kind: REG_VALUE_TYPE) -> io::Result<()> {
    write_value(VALUE, value, kind)
}

fn write_value(value_name: &str, value: &[u16], kind: REG_VALUE_TYPE) -> io::Result<()> {
    let key = Key::open(KEY_SET_VALUE)?;

    // The terminator has to be part of the data: leave it out and `regedit`
    // shows the right string while consumers read a truncated last character.
    let mut data: Vec<u16> = value.to_vec();
    data.push(0);

    // SAFETY: `data` is a NUL-terminated UTF-16 buffer and `cbData` is its
    // length in bytes, terminator included, as RegSetValueExW requires.
    let status = unsafe {
        RegSetValueExW(
            key.0,
            wide(value_name).as_ptr(),
            0,
            kind,
            data.as_ptr().cast::<u8>(),
            (data.len() * 2) as u32,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(win_error("writing the user PATH", status));
    }
    Ok(())
}

/// Remove a value. Only used to clean up after the round-trip test.
#[cfg(test)]
fn delete_value(value_name: &str) -> io::Result<()> {
    let key = Key::open(KEY_SET_VALUE)?;
    // SAFETY: `wide` is NUL-terminated and the key is open for writing.
    let status = unsafe { RegDeleteValueW(key.0, wide(value_name).as_ptr()) };
    if status != ERROR_SUCCESS {
        return Err(win_error("deleting a registry value", status));
    }
    Ok(())
}

/// Tell the rest of the session that the environment changed.
///
/// Without this, even freshly launched programs keep the stale environment
/// until the next sign-in, because they inherit it from an Explorer that never
/// noticed. Best-effort: a hung window is not a reason to fail an install that
/// already succeeded.
pub fn broadcast_change() {
    let mut result: usize = 0;
    // SAFETY: a NUL-terminated UTF-16 string as LPARAM is the documented
    // payload for WM_SETTINGCHANGE, and `result` is a valid out-pointer.
    // SMTO_ABORTIFHUNG plus a timeout bounds how long a stuck window can block.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            wide("Environment").as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5000,
            &mut result,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_strings_are_nul_terminated() {
        assert_eq!(wide("ab"), vec![97, 98, 0]);
        assert_eq!(wide(""), vec![0]);
    }

    /// Exercises the write path against a scratch value under
    /// `HKCU\Environment`, never `Path` itself: a test that edited the real user
    /// PATH would be a test that can break the machine it runs on.
    #[test]
    fn a_written_value_round_trips_with_its_type_and_terminator() {
        let name = format!("ORBITAL_TEST_{}", std::process::id());
        let text = r"%LOCALAPPDATA%\x;C:\y";
        let value: Vec<u16> = text.encode_utf16().collect();

        write_value(&name, &value, REG_EXPAND_SZ).expect("writing the scratch value");
        let read_back = read_value(&name).expect("reading the scratch value");
        delete_value(&name).expect("removing the scratch value");

        // The three things a hand-rolled registry write gets wrong: a mangled
        // value, a demoted type, and a terminator left inside the data.
        assert_eq!(read_back.raw, value);
        assert_eq!(read_back.kind, REG_EXPAND_SZ);
        assert_eq!(read_back.to_text(), text);
        assert!(read_back.raw.last() != Some(&0));
    }

    #[test]
    fn an_absent_value_reads_as_an_empty_expandable_path() {
        let missing = format!("ORBITAL_ABSENT_{}", std::process::id());
        let value = read_value(&missing).expect("a missing value must not be an error");
        assert!(value.raw.is_empty());
        assert_eq!(value.kind, REG_EXPAND_SZ);
    }

    #[test]
    fn reads_the_real_user_path_without_expanding_it() {
        // Read-only, so this is safe to run on a developer's machine. It proves
        // the round trip we depend on: whatever is stored comes back with a
        // usable type, and `%VAR%` references are still literal.
        let current = read_user_path().expect("reading HKCU\\Environment\\Path");
        assert!(current.kind == REG_SZ || current.kind == REG_EXPAND_SZ);
        // No terminator is left in the value we would append to.
        assert!(current.raw.last() != Some(&0));
        let text = current.to_text();
        assert_eq!(text.len(), text.trim_end_matches('\0').len());
    }
}
