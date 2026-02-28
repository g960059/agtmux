//! Display helpers for CLI output: path shortening, git branch, text truncation.

use std::collections::HashMap;

/// Return the last two path segments, collapsing $HOME to `~`.
///
/// ```text
/// "/Users/me/ghq/org/agtmux/worktrees/feature-T139" -> "agtmux/feature-T139"
/// "/Users/me/.claude/projects/repo"                  -> "projects/repo"
/// ```
pub fn short_path(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let collapsed = if !home.is_empty() && path.starts_with(&home) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_string()
    };

    let trimmed = collapsed.trim_end_matches('/');
    let segments: Vec<&str> = trimmed
        .rsplit('/')
        .take(2)
        .filter(|s| !s.is_empty())
        .collect();

    match segments.len() {
        0 => collapsed,
        1 => segments[0].to_string(),
        _ => format!("{}/{}", segments[1], segments[0]),
    }
}

/// Synchronously get the git branch for a directory path (best-effort).
/// Returns `None` if the path is not a git repo or git is unavailable.
pub fn git_branch_for_path(path: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["-C", path, "rev-parse", "--abbrev-ref", "HEAD"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

/// Right-truncate a branch name to `max_len` characters, appending `...` if truncated.
pub fn truncate_branch(branch: &str, max_len: usize) -> String {
    if branch.len() <= max_len {
        branch.to_string()
    } else {
        // We need at least 1 visible char + ellipsis
        let truncated = &branch[..max_len.saturating_sub(1)];
        format!("{truncated}\u{2026}")
    }
}

/// If all non-None items are the same value, return `Some(value)`.
/// Returns `None` if the iterator is empty or values are mixed.
pub fn consensus_str<'a>(items: impl Iterator<Item = Option<&'a str>>) -> Option<String> {
    let mut seen: Option<&str> = None;
    let mut has_any = false;

    for item in items {
        let val = item?; // None item -> short-circuit to None
        has_any = true;
        match seen {
            None => seen = Some(val),
            Some(prev) if prev == val => {}
            Some(_) => return None, // mixed
        }
    }

    if has_any {
        seen.map(|s| s.to_string())
    } else {
        None
    }
}

/// Relative-time helper: seconds -> human string.
pub fn relative_time(seconds: i64) -> String {
    let s = seconds.unsigned_abs();
    if s < 60 {
        "just now".to_string()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86400 {
        format!("{}h", s / 3600)
    } else if s < 86400 * 30 {
        format!("{}d", s / 86400)
    } else {
        format!("{}w", s / (86400 * 7))
    }
}

/// Build a map of cwd -> git branch by running `git rev-parse` for each unique cwd.
pub fn build_branch_map(panes: &[serde_json::Value]) -> HashMap<String, String> {
    let mut cwds: std::collections::HashSet<String> = std::collections::HashSet::new();
    for pane in panes {
        if let Some(p) = pane["current_path"].as_str() {
            cwds.insert(p.to_string());
        }
    }

    let mut map = HashMap::new();
    for cwd in cwds {
        if let Some(branch) = git_branch_for_path(&cwd) {
            map.insert(cwd, branch);
        }
    }
    map
}

/// Resolve --color flag to bool.
pub fn resolve_color(color: &str) -> bool {
    use std::io::IsTerminal;
    match color {
        "always" => true,
        "never" => false,
        _ => std::io::stdout().is_terminal(),
    }
}

/// Provider short name for display.
pub fn provider_short(provider: &str) -> &str {
    match provider {
        "ClaudeCode" => "Claude",
        "Codex" => "Codex",
        _ => provider,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_path_basic() {
        // Last 2 segments
        let result = short_path("/some/deep/nested/repo/subdir");
        assert_eq!(result, "repo/subdir");
    }

    #[test]
    fn short_path_home_collapse() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/test".to_string());
        let input = format!("{home}/projects/myrepo");
        let result = short_path(&input);
        assert_eq!(result, "projects/myrepo");
    }

    #[test]
    fn short_path_single_segment() {
        let result = short_path("/only");
        assert_eq!(result, "only");
    }

    #[test]
    fn truncate_branch_short() {
        assert_eq!(truncate_branch("main", 20), "main");
        assert_eq!(truncate_branch("feat/short", 20), "feat/short");
    }

    #[test]
    fn truncate_branch_exact() {
        let branch = "a".repeat(20);
        assert_eq!(truncate_branch(&branch, 20), branch);
    }

    #[test]
    fn truncate_branch_long() {
        let result = truncate_branch("feat/very-long-branch-name", 20);
        assert!(result.len() <= 24, "truncated with ellipsis"); // ellipsis is multi-byte
        assert!(result.ends_with('\u{2026}'), "ends with ellipsis");
        assert!(
            result.starts_with("feat/very-long-bran"),
            "preserves prefix"
        );
    }

    #[test]
    fn consensus_str_uniform() {
        let items = vec![Some("main"), Some("main"), Some("main")];
        assert_eq!(consensus_str(items.into_iter()), Some("main".to_string()));
    }

    #[test]
    fn consensus_str_mixed() {
        let items = vec![Some("main"), Some("dev"), Some("main")];
        assert_eq!(consensus_str(items.into_iter()), None);
    }

    #[test]
    fn consensus_str_empty() {
        let items: Vec<Option<&str>> = vec![];
        assert_eq!(consensus_str(items.into_iter()), None);
    }

    #[test]
    fn consensus_str_with_none() {
        let items = vec![Some("main"), None, Some("main")];
        assert_eq!(consensus_str(items.into_iter()), None);
    }

    #[test]
    fn consensus_str_single() {
        let items = vec![Some("dev")];
        assert_eq!(consensus_str(items.into_iter()), Some("dev".to_string()));
    }

    #[test]
    fn relative_time_just_now() {
        assert_eq!(relative_time(30), "just now");
    }

    #[test]
    fn relative_time_minutes() {
        assert_eq!(relative_time(180), "3m");
    }

    #[test]
    fn relative_time_hours() {
        assert_eq!(relative_time(7200), "2h");
    }
}
