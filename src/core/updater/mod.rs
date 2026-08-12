//! Checking GitHub for a newer release, and installing it.

pub mod apply;
pub mod assets;
pub mod check;

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
