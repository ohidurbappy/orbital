//! Fuzzy matching used by the interactive menu's search box.

use super::Command;

/// Score how well `query` fuzzy-matches `text` (case-insensitive subsequence).
///
/// Returns `None` when the query is not a subsequence of the text. Higher is a
/// better match: contiguous runs, word-boundary hits, and an early first match
/// all score higher.
pub fn fuzzy_score(text: &str, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let q: Vec<char> = query.to_lowercase().chars().collect();

    let mut score: i32 = 0;
    let mut ti: usize = 0;
    // Sentinel that can never be `found - 1`, so the first match never counts
    // as contiguous.
    let mut prev_match: i64 = -2;

    for ch in q {
        let found = ti + t[ti..].iter().position(|&c| c == ch)?;

        score += 1;
        if found as i64 == prev_match + 1 {
            score += 5; // contiguous run
        }
        if found == 0 {
            score += 8; // matches very start
        } else if !is_word_char(t[found - 1]) {
            score += 3; // word boundary
        }
        score -= (found - ti) as i32; // penalize skipped chars

        prev_match = found as i64;
        ti = found + 1;
    }
    Some(score)
}

fn is_word_char(ch: char) -> bool {
    ch.is_ascii_lowercase() || ch.is_ascii_digit()
}

/// Filter and rank commands by a query. Matches against the command name (or
/// its aliases) and description, weighting name matches highest. An empty query
/// returns every command in registry order.
pub fn filter_commands<'a>(commands: &'a [Command], query: &str) -> Vec<&'a Command> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return commands.iter().collect();
    }

    let mut scored: Vec<(usize, i32, &Command)> = commands
        .iter()
        .enumerate()
        .filter_map(|(index, command)| {
            score_command(command, trimmed).map(|score| (index, score, command))
        })
        .collect();

    // Best score first; registry order breaks ties.
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(_, _, command)| command).collect()
}

fn score_command(command: &Command, query: &str) -> Option<i32> {
    let best_name = std::iter::once(command.name)
        .chain(command.aliases.iter().copied())
        .filter_map(|n| fuzzy_score(n, query))
        .max();
    let desc_score = fuzzy_score(command.description, query);

    match (best_name, desc_score) {
        (None, None) => None,
        // Name matches dominate; a description-only match still surfaces, ranked lower.
        (name, desc) => Some(std::cmp::max(
            name.map(|s| s.saturating_mul(3)).unwrap_or(i32::MIN),
            desc.unwrap_or(i32::MIN),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(
        name: &'static str,
        description: &'static str,
        aliases: &'static [&'static str],
    ) -> Command {
        Command {
            name,
            description,
            aliases,
            run: None,
            view: |_| Ok(()),
            stdin: crate::commands::Stdin::Never,
        }
    }

    fn fixture() -> Vec<Command> {
        vec![
            cmd("ip", "Print local IP address(es)", &["ipaddr"]),
            cmd(
                "sysinfo",
                "Show system information (neofetch-style)",
                &["sys", "neofetch"],
            ),
            cmd(
                "update",
                "Download and install the latest release",
                &["upgrade"],
            ),
        ]
    }

    fn names(commands: &[Command], query: &str) -> Vec<&'static str> {
        filter_commands(commands, query)
            .iter()
            .map(|c| c.name)
            .collect()
    }

    #[test]
    fn empty_query_scores_zero() {
        assert_eq!(fuzzy_score("anything", ""), Some(0));
    }

    #[test]
    fn non_subsequence_does_not_match() {
        assert_eq!(fuzzy_score("ip", "xyz"), None);
    }

    #[test]
    fn matches_subsequences() {
        assert!(fuzzy_score("sysinfo", "sfo").is_some());
    }

    #[test]
    fn prefix_scores_above_scattered() {
        let prefix = fuzzy_score("sysinfo", "sys").unwrap();
        let scattered = fuzzy_score("sysinfo", "sfo").unwrap();
        assert!(prefix > scattered);
    }

    #[test]
    fn empty_query_returns_everything_in_order() {
        assert_eq!(names(&fixture(), ""), ["ip", "sysinfo", "update"]);
    }

    #[test]
    fn filters_by_name() {
        assert_eq!(names(&fixture(), "sys"), ["sysinfo"]);
    }

    #[test]
    fn matches_aliases() {
        assert_eq!(names(&fixture(), "neofetch"), ["sysinfo"]);
        assert_eq!(names(&fixture(), "upgrade"), ["update"]);
    }

    #[test]
    fn matches_description_text() {
        assert_eq!(names(&fixture(), "release"), ["update"]);
    }

    #[test]
    fn ranks_name_match_above_description_only_match() {
        // "in" appears in "sysinfo" and in "install"; the name match wins.
        assert_eq!(names(&fixture(), "in")[0], "sysinfo");
    }

    #[test]
    fn no_matches_returns_empty() {
        assert!(names(&fixture(), "zzzzz").is_empty());
    }
}
