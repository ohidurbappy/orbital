//! Keeping the update cache warm without ever blocking the user's command.

use std::process::{Command, Stdio};

use crate::core::updater::check::check_for_update;
use crate::core::updater::state::{is_stale, now_ms, read_state, write_state, UpdateState};

/// The hidden argument the detached child is invoked with.
pub const REFRESH_ARG: &str = "__refresh-update";

/// True when running out of a Cargo target directory rather than an installed
/// binary. Self-update refuses to overwrite a development build.
pub fn is_dev_build() -> bool {
    if cfg!(debug_assertions) {
        return true;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(_) => return false,
    };
    let mut components = exe.components().rev().skip(1); // skip the file name
    let parent = components.next().map(|c| c.as_os_str().to_owned());
    let grandparent = components.next().map(|c| c.as_os_str().to_owned());
    matches!(
        parent.as_deref().and_then(|p| p.to_str()),
        Some("debug" | "release")
    ) && grandparent.as_deref().and_then(|p| p.to_str()) == Some("target")
}

/// Perform a check and persist the result. Used by the hidden refresh command.
pub fn run_refresh() {
    let result = check_for_update();
    write_state(&UpdateState {
        last_check: now_ms(),
        result: Some(result),
    });
}

/// Fire a detached child process to refresh the update cache, then return
/// immediately so a one-shot command never blocks on the network. No-op when
/// the cache is still fresh.
pub fn spawn_background_refresh() {
    spawn_background_refresh_at(now_ms());
}

fn spawn_background_refresh_at(now: u64) {
    if !is_stale(read_state().as_ref(), now) {
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(_) => return,
    };

    let mut command = Command::new(exe);
    command
        .arg(REFRESH_ARG)
        .env("ORBITAL_REFRESH", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }

    // Best-effort; never break the foreground command. The child is left
    // unwaited on purpose — we exit immediately after this.
    let _ = command.spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tests_run_from_a_target_directory_and_count_as_dev() {
        // The test binary itself lives under target/, which is exactly the case
        // self-update must refuse.
        assert!(is_dev_build());
    }
}
