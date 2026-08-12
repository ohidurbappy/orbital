//! The "an update is available" notice shown above whatever the user ran.

use crate::core::updater::check::UpdateResult;
use crate::core::version::BIN;
use crate::style;

/// Render the banner, or nothing when there's no newer release. The trailing
/// blank line matches the margin the TypeScript version reserved below it.
pub fn banner_lines(update: Option<&UpdateResult>) -> Vec<String> {
    let update = match update {
        Some(update) if update.has_update => update,
        _ => return Vec::new(),
    };
    let latest = match update.latest.as_deref() {
        Some(latest) => latest,
        None => return Vec::new(),
    };

    let plain = format!(
        "↑ Update available: {} → {latest} — run {BIN} update",
        update.current
    );
    let styled = format!(
        "{}{}{}{}{}{}",
        style::yellow("↑ Update available: "),
        style::dim(&update.current),
        " → ",
        style::bold_green(latest),
        " — run ",
        style::bold_cyan(&format!("{BIN} update")),
    );

    let inner = plain.chars().count() + 2;
    vec![
        format!("╭{}╮", "─".repeat(inner)),
        format!("│ {styled} │"),
        format!("╰{}╯", "─".repeat(inner)),
        String::new(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(has_update: bool, latest: Option<&str>) -> UpdateResult {
        UpdateResult {
            has_update,
            current: "1.0.0".to_string(),
            latest: latest.map(str::to_string),
            url: None,
            assets: Vec::new(),
        }
    }

    #[test]
    fn renders_nothing_without_an_update() {
        assert!(banner_lines(None).is_empty());
        assert!(banner_lines(Some(&update(false, Some("2.0.0")))).is_empty());
    }

    #[test]
    fn renders_nothing_when_the_latest_version_is_missing() {
        assert!(banner_lines(Some(&update(true, None))).is_empty());
    }

    #[test]
    fn shows_both_versions_and_the_update_command() {
        let lines = banner_lines(Some(&update(true, Some("2.0.0"))));
        assert_eq!(lines.len(), 4);
        assert!(lines[1].contains("1.0.0"));
        assert!(lines[1].contains("2.0.0"));
        assert!(lines[1].contains("orbital update"));
    }

    #[test]
    fn the_box_borders_match_the_content_width() {
        let lines = banner_lines(Some(&update(true, Some("2.0.0"))));
        let width = |s: &str| s.chars().count();
        // Colour is disabled under test, so every line is its visible width.
        assert_eq!(width(&lines[0]), width(&lines[1]));
        assert_eq!(width(&lines[0]), width(&lines[2]));
    }
}
