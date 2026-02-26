//! Generate and apply Claude Code hook configuration for agtmux integration.

use std::path::{Path, PathBuf};

use crate::cli::SetupHooksOpts;

/// Hook types that agtmux integrates with.
const HOOK_TYPES: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "Stop",
    "SubagentStop",
];

/// Resolve the settings.json path based on scope.
pub fn settings_path(scope: &str) -> anyhow::Result<PathBuf> {
    match scope {
        "project" => Ok(PathBuf::from(".claude/settings.json")),
        "user" => {
            let home = std::env::var("HOME")
                .map_err(|_| anyhow::anyhow!("HOME not set; cannot resolve user scope"))?;
            Ok(PathBuf::from(home).join(".claude/settings.json"))
        }
        _ => anyhow::bail!("invalid scope: {scope:?} (expected \"project\" or \"user\")"),
    }
}

/// Detect the hook script path.
///
/// Resolution order:
/// 1. Explicit `--hook-script` argument
/// 2. `scripts/agtmux-claude-hook.sh` relative to the current directory
/// 3. `agtmux-claude-hook.sh` on PATH (assumed installed)
pub fn resolve_hook_script(explicit: Option<&str>) -> anyhow::Result<String> {
    if let Some(path) = explicit {
        return Ok(path.to_string());
    }

    let local = Path::new("scripts/agtmux-claude-hook.sh");
    if local.exists() {
        // Use absolute path for reliability
        let abs = std::fs::canonicalize(local)?;
        return Ok(abs.to_string_lossy().into_owned());
    }

    // Fallback: assume it's on PATH
    Ok("agtmux-claude-hook.sh".to_string())
}

/// Shell-quote a path for safe embedding in a shell command string.
///
/// Wraps in single quotes if the path contains whitespace, quotes, or backslashes.
/// Single quotes inside the path are escaped as `'\''`.
fn shell_quote(path: &str) -> String {
    if path.contains(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '\\') {
        format!("'{}'", path.replace('\'', "'\\''"))
    } else {
        path.to_string()
    }
}

/// Generate the hooks configuration object for Claude Code settings.json.
pub fn generate_hooks_config(script_path: &str) -> serde_json::Value {
    let mut hooks = serde_json::Map::new();
    let quoted = shell_quote(script_path);

    for hook_type in HOOK_TYPES {
        let command = format!("AGTMUX_HOOK_TYPE={hook_type} {quoted}");
        hooks.insert(
            (*hook_type).to_string(),
            serde_json::json!([{
                "type": "command",
                "command": command,
            }]),
        );
    }

    serde_json::Value::Object(hooks)
}

/// Apply hook configuration to the settings file (merge, not overwrite).
pub fn apply_hooks(opts: &SetupHooksOpts) -> anyhow::Result<PathBuf> {
    let path = settings_path(&opts.scope)?;
    let script = resolve_hook_script(opts.hook_script.as_deref())?;
    let hooks = generate_hooks_config(&script);

    // Read existing settings or start fresh
    let mut settings: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        serde_json::from_str(&content)?
    } else {
        serde_json::json!({})
    };

    // Merge hooks into settings
    let obj = settings
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("settings.json is not a JSON object"))?;
    obj.insert("hooks".to_string(), hooks);

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let output = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&path, format!("{output}\n"))?;

    Ok(path)
}

// ─── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_hooks_config_all_types() {
        let config = generate_hooks_config("/usr/local/bin/agtmux-claude-hook.sh");
        let obj = config.as_object().expect("should be object");

        // All 5 hook types present
        for hook_type in HOOK_TYPES {
            assert!(
                obj.contains_key(*hook_type),
                "missing hook type: {hook_type}"
            );
            let arr = obj[*hook_type].as_array().expect("should be array");
            assert_eq!(arr.len(), 1);
            assert_eq!(arr[0]["type"], "command");
            let cmd = arr[0]["command"].as_str().expect("command string");
            assert!(cmd.contains(hook_type));
            assert!(cmd.contains("agtmux-claude-hook.sh"));
        }
    }

    #[test]
    fn settings_path_project() {
        let path = settings_path("project").expect("ok");
        assert_eq!(path, PathBuf::from(".claude/settings.json"));
    }

    #[test]
    fn settings_path_user() {
        let path = settings_path("user").expect("ok");
        assert!(path.to_string_lossy().contains(".claude/settings.json"));
        assert!(path.to_string_lossy().contains('/'));
    }

    #[test]
    fn settings_path_invalid_scope() {
        let result = settings_path("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn resolve_hook_script_explicit() {
        let result = resolve_hook_script(Some("/custom/path.sh")).expect("ok");
        assert_eq!(result, "/custom/path.sh");
    }

    // ── T-118 F2: path escaping tests ────────────────────────────────

    #[test]
    fn generate_hooks_config_escapes_path_with_spaces() {
        let config = generate_hooks_config("/path/with spaces/hook.sh");
        let obj = config.as_object().expect("object");
        let cmd = obj["PreToolUse"][0]["command"].as_str().expect("cmd");
        assert!(
            cmd.contains("'/path/with spaces/hook.sh'"),
            "path with spaces should be single-quoted, got: {cmd}"
        );
    }

    #[test]
    fn generate_hooks_config_escapes_path_with_quotes() {
        let config = generate_hooks_config("/path/it's/hook.sh");
        let obj = config.as_object().expect("object");
        let cmd = obj["PreToolUse"][0]["command"].as_str().expect("cmd");
        // Single quote inside path should be escaped as '\''
        assert!(
            cmd.contains("'\\''"),
            "path with single quotes should be escaped, got: {cmd}"
        );
        assert!(
            !cmd.contains("it's/hook"),
            "raw single quote should not appear unescaped, got: {cmd}"
        );
    }
}
