//! Update checking, caching, and self-replacement.

pub mod apply;
pub mod assets;
pub mod check;
pub mod refresh;
pub mod state;

use check::{clean_version, UpdateResult};
use state::read_state;

use crate::core::version::VERSION;

/// Re-derive the update verdict against the version actually running.
///
/// The cache may have been written by a different (older) binary — e.g. before
/// `orbital update`, or by another copy on the same machine — so its frozen
/// `has_update` and `current` can't be trusted. Only `latest`/`url`/`assets`
/// are reused.
pub fn reconcile(result: Option<UpdateResult>) -> Option<UpdateResult> {
    reconcile_against(result, VERSION)
}

fn reconcile_against(result: Option<UpdateResult>, current: &str) -> Option<UpdateResult> {
    let mut result = result?;
    let running = semver::Version::parse(current).ok();
    let latest = result.latest.as_deref().and_then(clean_version);

    result.has_update = match (latest, running) {
        (Some(latest), Some(running)) => latest > running,
        _ => false,
    };
    result.current = current.to_string();
    Some(result)
}

/// The cached update verdict, re-checked against this binary's version. Never
/// touches the network, so it's safe on any startup path.
pub fn cached_update() -> Option<UpdateResult> {
    reconcile(read_state()?.result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cached(has_update: bool, latest: Option<&str>) -> Option<UpdateResult> {
        Some(UpdateResult {
            has_update,
            current: "unknown".to_string(),
            latest: latest.map(str::to_string),
            url: None,
            assets: Vec::new(),
        })
    }

    #[test]
    fn passes_none_through() {
        assert!(reconcile(None).is_none());
    }

    #[test]
    fn clears_a_stale_has_update_when_latest_is_not_newer() {
        // Cache written by an older binary: claims an update, but `latest` <= us.
        let result = reconcile_against(cached(true, Some("1.0.0")), "1.0.0").unwrap();
        assert_eq!(result.current, "1.0.0");
        assert!(!result.has_update);
    }

    #[test]
    fn reports_an_update_when_the_cached_latest_really_is_newer() {
        let result = reconcile_against(cached(false, Some("2.0.0")), "1.0.0").unwrap();
        assert!(result.has_update);
        assert_eq!(result.latest.as_deref(), Some("2.0.0"));
        assert_eq!(result.current, "1.0.0");
    }

    #[test]
    fn treats_a_missing_or_invalid_latest_as_no_update() {
        assert!(
            !reconcile_against(cached(true, None), "1.0.0")
                .unwrap()
                .has_update
        );
        assert!(
            !reconcile_against(cached(true, Some("nightly")), "1.0.0")
                .unwrap()
                .has_update
        );
    }

    #[test]
    fn reconciling_against_this_binary_uses_its_own_version() {
        let result = reconcile(cached(true, Some("0.0.1"))).unwrap();
        assert_eq!(result.current, VERSION);
    }
}
