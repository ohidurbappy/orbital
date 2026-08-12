//! Downloading a release asset and replacing the running binary with it.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;

use crate::core::updater::assets::find_current_asset;
use crate::core::updater::check::check_for_update;
use crate::core::updater::is_dev_build;
use crate::core::version::{user_agent, BIN};

/// Ceiling for the downloaded asset. Well above any real build, but bounded so
/// a misbehaving server can't exhaust memory.
const MAX_ASSET_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Updated,
    UpToDate,
    Unsupported,
    NoAsset,
    Error,
}

impl Status {
    /// The colour the outcome message is printed in.
    pub fn color(self) -> &'static str {
        match self {
            Status::Updated => "green",
            Status::UpToDate => "cyan",
            Status::Unsupported | Status::NoAsset => "yellow",
            Status::Error => "red",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApplyOutcome {
    pub status: Status,
    pub message: String,
}

impl ApplyOutcome {
    pub fn new(status: Status, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

/// Phases an update goes through, reported via the progress callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Checking,
    Downloading,
    Installing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    /// Total download size in bytes, known once the release asset is resolved.
    pub total_bytes: Option<u64>,
}

/// The label shown next to the spinner for a phase.
pub fn progress_label(progress: Progress) -> String {
    match progress.phase {
        Phase::Downloading => {
            let size = progress
                .total_bytes
                .map(|b| format!(" ({:.1} MB)", b as f64 / 1024.0 / 1024.0))
                .unwrap_or_default();
            format!("Downloading update{size}…")
        }
        Phase::Installing => "Installing…".to_string(),
        Phase::Checking => "Checking for updates…".to_string(),
    }
}

/// Download the release asset for this platform and replace the running binary.
/// Returns a structured outcome; never panics on a failed update. `on_progress`
/// is called as the update moves through its phases so the UI can reflect them.
pub fn apply_update(on_progress: &dyn Fn(Progress)) -> ApplyOutcome {
    if is_dev_build() {
        return ApplyOutcome::new(
            Status::Unsupported,
            "Self-update only applies to an installed binary (not a cargo build).",
        );
    }

    on_progress(Progress {
        phase: Phase::Checking,
        total_bytes: None,
    });
    let result = check_for_update();
    let latest = match (result.has_update, result.latest.as_deref()) {
        (true, Some(latest)) => latest.to_string(),
        _ => {
            return ApplyOutcome::new(
                Status::UpToDate,
                format!("Already on the latest version ({}).", result.current),
            )
        }
    };

    let asset = match find_current_asset(&result.assets) {
        Some(asset) => asset.clone(),
        None => {
            return ApplyOutcome::new(
                Status::NoAsset,
                format!(
                    "No release asset for {}/{} in {latest}.",
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ),
            )
        }
    };

    on_progress(Progress {
        phase: Phase::Downloading,
        total_bytes: Some(asset.size),
    });
    let gzipped = match download(&asset.browser_download_url) {
        Ok(bytes) => bytes,
        Err(message) => return ApplyOutcome::new(Status::Error, message),
    };

    // Release assets are gzipped to cut download size; decompress back to the
    // raw executable before swapping it in.
    let mut bytes = Vec::new();
    if let Err(err) = GzDecoder::new(&gzipped[..]).read_to_end(&mut bytes) {
        return ApplyOutcome::new(Status::Error, format!("Update failed: {err}"));
    }

    on_progress(Progress {
        phase: Phase::Installing,
        total_bytes: Some(asset.size),
    });
    match install(&bytes) {
        Ok(()) => ApplyOutcome::new(
            Status::Updated,
            format!("Updated to {latest}. Restart {BIN} to use the new version."),
        ),
        Err(message) => ApplyOutcome::new(Status::Error, message),
    }
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .header("User-Agent", user_agent())
        .header("Accept", "application/octet-stream")
        .call();

    match response {
        Ok(mut res) => res
            .body_mut()
            .with_config()
            .limit(MAX_ASSET_BYTES)
            .read_to_vec()
            .map_err(|err| format!("Download failed: {err}")),
        Err(ureq::Error::StatusCode(code)) => Err(format!("Download failed: HTTP {code}")),
        Err(err) => Err(format!("Download failed: {err}")),
    }
}

/// Write the new binary next to the current one and move it into place.
fn install(bytes: &[u8]) -> Result<(), String> {
    let target: PathBuf = std::env::current_exe().map_err(|e| format!("Update failed: {e}"))?;
    let tmp = with_suffix(&target, ".new");

    fs::write(&tmp, bytes).map_err(|e| format!("Update failed: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Update failed: {e}"))?;
    }

    if cfg!(windows) {
        // Can't overwrite a running .exe; rename it aside, then move the new
        // one in. The leftover .old is removed on the next update.
        let old = with_suffix(&target, ".old");
        let _ = fs::remove_file(&old);
        fs::rename(&target, &old).map_err(|e| format!("Update failed: {e}"))?;
        if let Err(err) = fs::rename(&tmp, &target) {
            // Put the original back rather than leaving nothing on PATH.
            let _ = fs::rename(&old, &target);
            return Err(format!("Update failed: {err}"));
        }
    } else {
        // Unix: replacing the file the process is executing is safe (inode swap).
        fs::rename(&tmp, &target).map_err(|e| format!("Update failed: {e}"))?;
    }

    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.to_path_buf().into_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_each_phase() {
        assert_eq!(
            progress_label(Progress {
                phase: Phase::Checking,
                total_bytes: None
            }),
            "Checking for updates…"
        );
        assert_eq!(
            progress_label(Progress {
                phase: Phase::Installing,
                total_bytes: None
            }),
            "Installing…"
        );
    }

    #[test]
    fn shows_the_download_size_when_known() {
        let label = progress_label(Progress {
            phase: Phase::Downloading,
            total_bytes: Some(3 * 1024 * 1024 + 512 * 1024),
        });
        assert_eq!(label, "Downloading update (3.5 MB)…");
    }

    #[test]
    fn omits_the_size_when_unknown() {
        let label = progress_label(Progress {
            phase: Phase::Downloading,
            total_bytes: None,
        });
        assert_eq!(label, "Downloading update…");
    }

    #[test]
    fn suffixes_the_binary_path() {
        let path = PathBuf::from("/usr/local/bin/orbital");
        assert_eq!(
            with_suffix(&path, ".new"),
            PathBuf::from("/usr/local/bin/orbital.new")
        );
    }

    #[test]
    fn refuses_to_replace_a_development_build() {
        // The test binary is a cargo build, so this must never touch the disk.
        let outcome = apply_update(&|_| {});
        assert_eq!(outcome.status, Status::Unsupported);
    }
}
