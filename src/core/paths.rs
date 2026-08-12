//! Where orbital keeps its state, following each platform's convention.

use std::path::PathBuf;

use super::version::BIN;

/// Directory for orbital's persisted state (e.g. `~/.config/orbital` on Linux).
pub fn config_dir() -> PathBuf {
    platform_config_dir().unwrap_or_else(|| PathBuf::from(".").join(format!(".{BIN}")))
}

/// Cached update-check result lives here.
pub fn state_file() -> PathBuf {
    config_dir().join("state.json")
}

#[cfg(windows)]
fn platform_config_dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join(BIN).join("Config"))
}

#[cfg(target_os = "macos")]
fn platform_config_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("Preferences")
            .join(BIN),
    )
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_config_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join(BIN));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config").join(BIN))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_file_sits_in_the_config_dir() {
        let state = state_file();
        assert_eq!(state.file_name().unwrap(), "state.json");
        assert_eq!(state.parent().unwrap(), config_dir());
    }

    #[test]
    fn config_dir_is_namespaced_to_the_binary() {
        let dir = config_dir();
        assert!(
            dir.components().any(|c| c.as_os_str() == BIN),
            "expected {BIN} in {dir:?}"
        );
    }
}
