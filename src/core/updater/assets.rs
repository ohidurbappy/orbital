//! Mapping this machine to the release asset built for it.

use crate::core::updater::check::ReleaseAsset;
use crate::core::version::BIN;

/// The release asset name expected for a given os/arch. Assets are shipped
/// gzipped and gunzipped by `apply_update`. Must match the names produced by
/// the release workflow.
///
/// `os` and `arch` use Rust's [`std::env::consts`] spelling (`macos`, `windows`,
/// `x86_64`, `aarch64`); the asset names keep the shorter labels the installers
/// and release notes use.
pub fn asset_name_for(os: &str, arch: &str) -> Option<String> {
    let platform = match os {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "windows",
        _ => return None,
    };
    let cpu = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    // Only windows-x64 is built.
    if platform == "windows" && cpu != "x64" {
        return None;
    }
    Some(if platform == "windows" {
        format!("{BIN}-{platform}-{cpu}.exe.gz")
    } else {
        format!("{BIN}-{platform}-{cpu}.gz")
    })
}

pub fn find_asset<'a>(
    assets: &'a [ReleaseAsset],
    os: &str,
    arch: &str,
) -> Option<&'a ReleaseAsset> {
    let name = asset_name_for(os, arch)?;
    assets.iter().find(|a| a.name == name)
}

/// Find the asset matching the running machine.
pub fn find_current_asset(assets: &[ReleaseAsset]) -> Option<&ReleaseAsset> {
    find_asset(assets, std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str, url: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_string(),
            browser_download_url: url.to_string(),
            size: 1,
        }
    }

    #[test]
    fn maps_platforms_and_arches_to_gzipped_asset_names() {
        assert_eq!(
            asset_name_for("macos", "aarch64").unwrap(),
            "orbital-darwin-arm64.gz"
        );
        assert_eq!(
            asset_name_for("linux", "x86_64").unwrap(),
            "orbital-linux-x64.gz"
        );
        assert_eq!(
            asset_name_for("windows", "x86_64").unwrap(),
            "orbital-windows-x64.exe.gz"
        );
    }

    #[test]
    fn returns_nothing_for_unsupported_combinations() {
        assert!(asset_name_for("windows", "aarch64").is_none());
        assert!(asset_name_for("freebsd", "x86_64").is_none());
        assert!(asset_name_for("linux", "x86").is_none());
    }

    #[test]
    fn finds_the_matching_asset_for_a_platform() {
        let assets = [
            asset("orbital-linux-x64.gz", "u1"),
            asset("orbital-darwin-arm64.gz", "u2"),
        ];
        assert_eq!(
            find_asset(&assets, "macos", "aarch64")
                .unwrap()
                .browser_download_url,
            "u2"
        );
    }

    #[test]
    fn returns_nothing_when_no_asset_matches() {
        let assets = [asset("orbital-linux-x64.gz", "u1")];
        assert!(find_asset(&assets, "windows", "x86_64").is_none());
    }

    #[test]
    fn this_platform_has_an_asset_name() {
        // Every target we build for must be able to name its own asset,
        // otherwise self-update silently degrades to "no-asset".
        assert!(asset_name_for(std::env::consts::OS, std::env::consts::ARCH).is_some());
    }
}
