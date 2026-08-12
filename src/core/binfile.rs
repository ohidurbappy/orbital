//! Putting an executable on disk, safely, on every platform.
//!
//! Shared by `orbital update` (which replaces the running binary in place) and
//! `orbital install` (which writes a copy into a directory on PATH). Both need
//! the same three properties, and each is a bug if skipped:
//!
//! * **A fresh file, never a copy.** `fs::copy` inherits things we don't want:
//!   on Windows it carries alternate data streams, so a binary downloaded by a
//!   browser passes its mark-of-the-web on and the installed copy keeps
//!   tripping SmartScreen; it also carries the read-only attribute. On macOS it
//!   goes through `fclonefileat`/`copyfile(COPYFILE_ALL)` and propagates
//!   extended attributes including `com.apple.quarantine`. Writing bytes into a
//!   newly created file inherits none of that.
//! * **Staged in the destination directory, then renamed.** Renaming is atomic
//!   and can't leave a half-written binary behind, and staging next to the
//!   target means the rename never crosses a filesystem (`EXDEV`) the way
//!   staging in the temp dir would.
//! * **Never opened for writing at its final path.** On Unix that would be
//!   `ETXTBSY` against a running binary, and on macOS mutating a live mapping
//!   fails code-signature validation and kills the running process.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Attempts and backoff for operations Windows can transiently refuse.
const RETRIES: usize = 5;
const BACKOFF_MS: u64 = 50;

/// Write `bytes` as an executable at `target`, replacing whatever is there.
///
/// The parent directory must already exist — creating it is the caller's
/// decision, since `update` must never invent a directory while `install` must.
pub fn place_executable(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = target.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no parent directory", target.display()),
        )
    })?;

    let staged = Staged::write(dir, target, bytes)?;
    replace(staged.path(), target)?;
    staged.placed();
    Ok(())
}

/// A binary written to a temporary name, removed again unless it gets placed.
struct Staged {
    path: PathBuf,
    placed: bool,
}

impl Staged {
    fn write(dir: &Path, target: &Path, bytes: &[u8]) -> io::Result<Self> {
        // A leading dot and a `.tmp` suffix so a leftover is never mistaken for
        // a command on PATH, and the pid so two installs can't collide.
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let path = dir.join(format!(".{name}.{}.tmp", std::process::id()));

        // O_EXCL: a planted symlink at this path must not redirect the write.
        let mut file = match File::options().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                // Debris from a crashed run that happened to share our pid.
                fs::remove_file(&path)?;
                File::options().write(true).create_new(true).open(&path)?
            }
            Err(err) => return Err(err),
        };
        let staged = Self {
            path,
            placed: false,
        };

        file.write_all(bytes)?;
        // Durability: rename() orders the directory entry but says nothing
        // about the data, and the tool that repairs orbital is orbital.
        file.sync_all()?;
        drop(file);

        set_executable(staged.path())?;
        Ok(staged)
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn placed(mut self) {
        self.placed = true;
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.placed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
fn set_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // Normalised rather than inherited, and explicit rather than umask-derived:
    // a binary without the execute bit fails later, in the user's shell.
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Move `staged` onto `target`.
fn replace(staged: &Path, target: &Path) -> io::Result<()> {
    if cfg!(windows) && target.exists() {
        // Windows won't let a running .exe be replaced, so move it aside first.
        // The aside name has to be one that isn't itself locked — a previous
        // update can leave an `orbital.exe.old` that someone is running.
        let aside = free_aside_name(target)?;
        retry(|| fs::rename(target, &aside))?;
        if let Err(err) = retry(|| fs::rename(staged, target)) {
            // Put the original back rather than leaving nothing on PATH.
            let _ = fs::rename(&aside, target);
            return Err(err);
        }
        // Usually locked if the old binary is still running; the next install
        // clears it.
        let _ = fs::remove_file(&aside);
    } else {
        // Unix: replacing the file a process is executing is safe — the running
        // image keeps the old inode.
        retry(|| fs::rename(staged, target))?;
        sync_dir(target);
    }
    Ok(())
}

/// The first `<target>.old`-style name that is free, so moving the old binary
/// aside can't be blocked by an even older one.
fn free_aside_name(target: &Path) -> io::Result<PathBuf> {
    for attempt in 0..64 {
        let candidate = if attempt == 0 {
            with_suffix(target, ".old")
        } else {
            with_suffix(target, &format!(".{attempt}.old"))
        };
        // Clear a stale one if we can; if it's locked, move to the next name.
        if candidate.exists() {
            let _ = fs::remove_file(&candidate);
        }
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("could not free a name to move {} aside", target.display()),
    ))
}

/// Append `suffix` to a path's file name, keeping any extension it has.
pub fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.to_path_buf().into_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Retry the operations Windows refuses transiently: antivirus holds a
/// freshly-written executable open for a moment, and the rename then fails with
/// a sharing violation (os error 32) or access denied (os error 5).
fn retry<T>(mut operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut delay = Duration::from_millis(BACKOFF_MS);
    for _ in 0..RETRIES - 1 {
        match operation() {
            Ok(value) => return Ok(value),
            Err(err) if is_transient(&err) => {
                std::thread::sleep(delay);
                delay *= 2;
            }
            Err(err) => return Err(err),
        }
    }
    operation()
}

fn is_transient(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(5) | Some(32))
}

/// Make the rename itself durable. Best-effort: a filesystem that refuses to
/// sync a directory is not a reason to fail an install that already worked.
fn sync_dir(target: &Path) {
    if let Some(dir) = target.parent() {
        if let Ok(handle) = File::open(dir) {
            let _ = handle.sync_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway directory, the same pattern `serve::files` uses.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("orbital-binfile-{}-{tag}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn writes_a_new_executable() {
        let tree = TempTree::new("new");
        let target = tree.path().join("orbital");
        place_executable(&target, b"binary").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"binary");
    }

    #[test]
    fn replaces_an_existing_executable() {
        let tree = TempTree::new("replace");
        let target = tree.path().join("orbital");
        fs::write(&target, b"old").unwrap();
        place_executable(&target, b"new").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }

    #[test]
    fn leaves_no_staging_file_behind() {
        let tree = TempTree::new("clean");
        place_executable(&tree.path().join("orbital"), b"x").unwrap();
        let leftovers: Vec<String> = fs::read_dir(tree.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "orbital")
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    }

    #[test]
    fn a_stale_aside_does_not_block_a_replace() {
        let tree = TempTree::new("aside");
        let target = tree.path().join("orbital.exe");
        fs::write(&target, b"old").unwrap();
        // Debris from an earlier update.
        fs::write(tree.path().join("orbital.exe.old"), b"older").unwrap();
        place_executable(&target, b"new").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
    }

    #[test]
    fn fails_when_the_directory_does_not_exist() {
        let tree = TempTree::new("nodir");
        let target = tree.path().join("missing").join("orbital");
        assert!(place_executable(&target, b"x").is_err());
        // …and does not create it: that is the caller's decision.
        assert!(!tree.path().join("missing").exists());
    }

    #[cfg(unix)]
    #[test]
    fn sets_the_execute_bit() {
        use std::os::unix::fs::PermissionsExt;
        let tree = TempTree::new("mode");
        let target = tree.path().join("orbital");
        place_executable(&target, b"x").unwrap();
        let mode = fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "mode was {:o}", mode & 0o777);
    }

    #[test]
    fn suffixes_a_name_that_already_has_an_extension() {
        assert_eq!(
            with_suffix(Path::new("/bin/orbital.exe"), ".old"),
            PathBuf::from("/bin/orbital.exe.old")
        );
        assert_eq!(
            with_suffix(Path::new("/bin/orbital"), ".old"),
            PathBuf::from("/bin/orbital.old")
        );
    }

    #[test]
    fn a_transient_error_is_retried_and_a_real_one_is_not() {
        let mut attempts = 0;
        let result: io::Result<()> = retry(|| {
            attempts += 1;
            if attempts < 3 {
                Err(io::Error::from_raw_os_error(32)) // sharing violation
            } else {
                Ok(())
            }
        });
        assert!(result.is_ok());
        assert_eq!(attempts, 3);

        let mut attempts = 0;
        let result: io::Result<()> = retry(|| {
            attempts += 1;
            Err(io::Error::new(io::ErrorKind::NotFound, "gone"))
        });
        assert!(result.is_err());
        assert_eq!(attempts, 1, "a permanent error must not be retried");
    }
}
