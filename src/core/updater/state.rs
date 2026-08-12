//! The on-disk update-check cache, so a check runs at most once per interval.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::core::paths::state_file;
use crate::core::updater::check::UpdateResult;

/// Re-check at most this often (10 minutes), in milliseconds.
pub const CHECK_INTERVAL_MS: u64 = 10 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateState {
    /// Epoch ms of the last completed check.
    pub last_check: u64,
    pub result: Option<UpdateResult>,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn read_state() -> Option<UpdateState> {
    let text = fs::read_to_string(state_file()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_state(state: &UpdateState) {
    // Persisting the cache is best-effort; never break the CLI over it.
    let path = state_file();
    if let Some(dir) = path.parent() {
        if fs::create_dir_all(dir).is_err() {
            return;
        }
    }
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = fs::write(path, json);
    }
}

/// True when the cache is missing or older than the interval.
pub fn is_stale(state: Option<&UpdateState>, now: u64) -> bool {
    match state {
        None => true,
        // Saturating: a cache stamped in the future (clock change) reads fresh
        // rather than wrapping into "ancient".
        Some(state) => now.saturating_sub(state.last_check) >= CHECK_INTERVAL_MS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(last_check: u64) -> UpdateState {
        UpdateState {
            last_check,
            result: None,
        }
    }

    #[test]
    fn missing_state_is_stale() {
        assert!(is_stale(None, 1000));
    }

    #[test]
    fn is_fresh_within_the_interval() {
        let now = 1_000_000;
        assert!(!is_stale(Some(&state(now - 1000)), now));
    }

    #[test]
    fn is_stale_once_the_interval_has_elapsed() {
        let now = 1_000_000;
        assert!(is_stale(Some(&state(now - CHECK_INTERVAL_MS)), now));
    }

    #[test]
    fn a_future_timestamp_is_not_treated_as_ancient() {
        assert!(!is_stale(Some(&state(2_000_000)), 1_000_000));
    }

    #[test]
    fn state_round_trips_through_json() {
        let original = UpdateState {
            last_check: 42,
            result: Some(UpdateResult::none("1.0.0")),
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: UpdateState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.last_check, 42);
        assert_eq!(parsed.result.unwrap().current, "1.0.0");
    }
}
