use std::os::unix::net::UnixStream;
use std::process::Command;

struct CheckResult {
    passed: bool,
    label: String,
}

impl CheckResult {
    fn pass(label: impl Into<String>) -> Self {
        Self { passed: true, label: label.into() }
    }

    fn fail(label: impl Into<String>) -> Self {
        Self { passed: false, label: label.into() }
    }
}

impl std::fmt::Display for CheckResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tag = if self.passed { "PASS" } else { "FAIL" };
        write!(f, "[{}] {}", tag, self.label)
    }
}

fn check_tmux_server() -> CheckResult {
    match Command::new("tmux").arg("list-sessions").output() {
        Ok(output) if output.status.success() => {
            CheckResult::pass("tmux server is running")
        }
        Ok(_) => CheckResult::fail("tmux server is not running (run: tmux start-server)"),
        Err(_) => CheckResult::fail("tmux is not installed or not in PATH"),
    }
}

fn check_daemon_binary() -> CheckResult {
    if Command::new("which").arg("agtmux").output().map(|o| o.status.success()).unwrap_or(false) {
        return CheckResult::pass("agtmux binary found in PATH");
    }
    if std::path::Path::new("./target/release/agtmux").exists() {
        return CheckResult::pass("agtmux binary found at ./target/release/agtmux");
    }
    if std::path::Path::new("./target/debug/agtmux").exists() {
        return CheckResult::pass("agtmux binary found at ./target/debug/agtmux");
    }
    CheckResult::fail("agtmux binary not found (run: cargo build)")
}

fn check_stale_sockets() -> CheckResult {
    let entries: Vec<_> = glob::glob("/tmp/agtmux/*.sock")
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();

    if entries.is_empty() {
        return CheckResult::pass("no sockets found (clean state)");
    }

    let mut stale = Vec::new();
    let mut live = Vec::new();

    for path in &entries {
        match UnixStream::connect(path) {
            Ok(_) => live.push(path.display().to_string()),
            Err(_) => stale.push(path.display().to_string()),
        }
    }

    if !stale.is_empty() {
        return CheckResult::fail(format!(
            "stale socket(s) detected: {} (remove manually)",
            stale.join(", ")
        ));
    }

    CheckResult::pass(format!(
        "daemon already running (live socket(s): {})",
        live.join(", ")
    ))
}

fn check_cli_tool(name: &str, env_key: &str) -> CheckResult {
    let has_binary = Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let has_key = std::env::var(env_key).is_ok();

    match (has_binary, has_key) {
        (true, true) => CheckResult::pass(format!("{name} CLI found and {env_key} is set")),
        (true, false) => CheckResult::fail(format!("{name} CLI found but {env_key} is not set")),
        (false, _) => CheckResult::fail(format!("{name} CLI not found in PATH")),
    }
}

pub fn run_preflight(check_real_agents: bool) -> i32 {
    let mut results = vec![
        check_tmux_server(),
        check_daemon_binary(),
        check_stale_sockets(),
    ];

    if check_real_agents {
        results.push(check_cli_tool("claude", "ANTHROPIC_API_KEY"));
        results.push(check_cli_tool("codex", "OPENAI_API_KEY"));
    }

    let mut any_fail = false;
    for r in &results {
        println!("{}", r);
        if !r.passed {
            any_fail = true;
        }
    }

    if any_fail { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_result_formatting() {
        let pass = CheckResult::pass("tmux server is running");
        assert_eq!(pass.to_string(), "[PASS] tmux server is running");

        let fail = CheckResult::fail("tmux server is not running (run: tmux start-server)");
        assert_eq!(
            fail.to_string(),
            "[FAIL] tmux server is not running (run: tmux start-server)"
        );
    }

    #[test]
    fn daemon_binary_returns_a_result() {
        let result = check_daemon_binary();
        // Must be either pass or fail, never panic.
        assert!(result.to_string().starts_with("[PASS]") || result.to_string().starts_with("[FAIL]"));
    }

    #[test]
    fn stale_sockets_does_not_panic() {
        let result = check_stale_sockets();
        assert!(result.to_string().starts_with("[PASS]") || result.to_string().starts_with("[FAIL]"));
    }

    #[test]
    fn cli_tool_missing_binary() {
        let result = check_cli_tool("nonexistent_tool_xyz_42", "NONEXISTENT_KEY_42");
        assert_eq!(result.to_string(), "[FAIL] nonexistent_tool_xyz_42 CLI not found in PATH");
    }
}
