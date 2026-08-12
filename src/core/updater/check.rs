//! Ask GitHub whether a newer release exists.

use std::time::Duration;

use serde::Deserialize;

use crate::core::version::{user_agent, REPO, VERSION};

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct UpdateResult {
    pub has_update: bool,
    pub current: String,
    pub latest: Option<String>,
    pub assets: Vec<ReleaseAsset>,
}

impl UpdateResult {
    /// The "we know nothing" answer, used whenever a check can't complete.
    pub fn none(current: &str) -> Self {
        Self {
            has_update: false,
            current: current.to_string(),
            latest: None,
            assets: Vec::new(),
        }
    }
}

/// The subset of GitHub's release payload we care about.
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: Option<String>,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

fn latest_release_url() -> String {
    format!("https://api.github.com/repos/{REPO}/releases/latest")
}

/// Strip a leading `v` and surrounding whitespace, then parse — the equivalent
/// of `semver.clean` in the TypeScript version.
pub fn clean_version(tag: &str) -> Option<semver::Version> {
    let trimmed = tag.trim();
    let stripped = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'));
    semver::Version::parse(stripped.unwrap_or(trimmed)).ok()
}

/// Compare a release payload against the running version. Kept separate from
/// the HTTP call so the comparison logic is testable without a network.
pub fn parse_release(body: &str, current: &str) -> UpdateResult {
    let no_update = UpdateResult::none(current);

    let release: Release = match serde_json::from_str(body) {
        Ok(release) => release,
        Err(_) => return no_update,
    };

    let latest = match release.tag_name.as_deref().and_then(clean_version) {
        Some(version) => version,
        None => return no_update,
    };
    let running = match semver::Version::parse(current) {
        Ok(version) => version,
        Err(_) => return no_update,
    };

    UpdateResult {
        has_update: latest > running,
        current: current.to_string(),
        latest: Some(latest.to_string()),
        assets: release.assets,
    }
}

/// Query GitHub for the latest release and compare it to the running version.
///
/// Network and parse errors surface a "no update" result rather than an error —
/// update checks must never break the actual command the user ran.
pub fn check_for_update() -> UpdateResult {
    check_version(VERSION)
}

fn check_version(current: &str) -> UpdateResult {
    let no_update = UpdateResult::none(current);

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into();

    let response = agent
        .get(latest_release_url())
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", user_agent())
        .call();

    match response {
        Ok(mut res) => match res.body_mut().read_to_string() {
            Ok(body) => parse_release(&body, current),
            Err(_) => no_update,
        },
        Err(_) => no_update,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_an_update_when_the_latest_tag_is_greater() {
        let body = r#"{"tag_name":"v1.2.0","html_url":"http://x","assets":[]}"#;
        let res = parse_release(body, "1.0.0");
        assert!(res.has_update);
        assert_eq!(res.latest.as_deref(), Some("1.2.0"));
    }

    #[test]
    fn reports_no_update_when_already_current() {
        let res = parse_release(r#"{"tag_name":"v1.0.0"}"#, "1.0.0");
        assert!(!res.has_update);
    }

    #[test]
    fn ignores_invalid_tags() {
        let res = parse_release(r#"{"tag_name":"nightly"}"#, "1.0.0");
        assert!(!res.has_update);
        assert!(res.latest.is_none());
    }

    #[test]
    fn treats_an_unparseable_body_as_no_update() {
        let res = parse_release("<html>502</html>", "1.0.0");
        assert!(!res.has_update);
        assert_eq!(res.current, "1.0.0");
        assert!(res.latest.is_none());
    }

    #[test]
    fn keeps_the_assets_from_the_release() {
        let body = r#"{"tag_name":"v2.0.0","assets":[
            {"name":"orbital-linux-x64.gz","browser_download_url":"u1","size":42}
        ]}"#;
        let res = parse_release(body, "1.0.0");
        assert_eq!(res.assets.len(), 1);
        assert_eq!(res.assets[0].name, "orbital-linux-x64.gz");
        assert_eq!(res.assets[0].size, 42);
    }

    #[test]
    fn cleans_tag_prefixes() {
        assert_eq!(clean_version("v1.2.3").unwrap().to_string(), "1.2.3");
        assert_eq!(clean_version(" 1.2.3 ").unwrap().to_string(), "1.2.3");
        assert!(clean_version("nightly").is_none());
    }

    #[test]
    fn this_binarys_version_is_valid_semver() {
        assert!(
            semver::Version::parse(VERSION).is_ok(),
            "invalid VERSION: {VERSION}"
        );
    }
}
