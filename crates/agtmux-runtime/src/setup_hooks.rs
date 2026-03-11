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
    "SessionStart",
    "SessionEnd",
    "PermissionRequest",
    "UserPromptSubmit",
    "PostToolUseFailure",
    "PreCompact",
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

/// Perform the surgical per-type merge of agtmux hook entries into an existing settings value.
///
/// For each of the 11 HOOK_TYPES:
/// 1. Gets or creates the `hooks` object (does NOT replace it).
/// 2. Gets or creates the array for this hook_type.
/// 3. Removes any existing agtmux entries (identified by `AGTMUX_HOOK_TYPE=` in `command`).
/// 4. Appends the new agtmux entry.
///
/// This preserves all entries from other tools in every hook_type array.
pub(crate) fn merge_hooks_into_settings(
    settings: &mut serde_json::Value,
    quoted_script: &str,
) -> anyhow::Result<()> {
    let obj = settings
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("settings.json is not a JSON object"))?;

    let hooks_obj = obj
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("\"hooks\" in settings.json is not an object"))?;

    for hook_type in HOOK_TYPES {
        let command = format!("AGTMUX_HOOK_TYPE={hook_type} {quoted_script}");
        let new_entry = serde_json::json!({"type": "command", "command": command});

        let arr = hooks_obj
            .entry(*hook_type)
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("hook entry for {hook_type} is not an array"))?;

        // Remove existing agtmux entries (idempotent)
        arr.retain(|entry| {
            entry
                .get("command")
                .and_then(|c| c.as_str())
                .map(|cmd| !cmd.contains("AGTMUX_HOOK_TYPE="))
                .unwrap_or(true)
        });
        arr.push(new_entry);
    }

    Ok(())
}

/// Perform the surgical removal of agtmux hook entries from an existing settings value.
///
/// Only removes entries whose `command` field contains `AGTMUX_HOOK_TYPE=`.
/// Empty hook-type arrays and an empty `hooks` object are cleaned up.
pub(crate) fn remove_hooks_from_settings(settings: &mut serde_json::Value) -> anyhow::Result<()> {
    let obj = settings
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("settings.json is not a JSON object"))?;

    if let Some(hooks_val) = obj.get_mut("hooks") {
        if let Some(hooks) = hooks_val.as_object_mut() {
            // Remove agtmux entries from each hook type
            for hook_type in HOOK_TYPES {
                if let Some(arr) = hooks.get_mut(*hook_type).and_then(|v| v.as_array_mut()) {
                    arr.retain(|entry| {
                        entry
                            .get("command")
                            .and_then(|c| c.as_str())
                            .map(|cmd| !cmd.contains("AGTMUX_HOOK_TYPE="))
                            .unwrap_or(true)
                    });
                }
            }
            // Remove hook_type keys where array is now empty.
            // Restrict to HOOK_TYPES only — do NOT remove empty arrays belonging to
            // third-party tools registered under keys we don't own.
            let hook_type_set: std::collections::HashSet<&str> =
                HOOK_TYPES.iter().copied().collect();
            let to_remove: Vec<String> = hooks
                .iter()
                .filter_map(|(k, v)| {
                    if hook_type_set.contains(k.as_str())
                        && v.as_array().map(|a| a.is_empty()).unwrap_or(false)
                    {
                        Some(k.clone())
                    } else {
                        None
                    }
                })
                .collect();
            for k in &to_remove {
                hooks.remove(k.as_str());
            }
        }
        // Remove hooks key if object is now empty
        if obj
            .get("hooks")
            .and_then(|h| h.as_object())
            .map(|h| h.is_empty())
            .unwrap_or(false)
        {
            obj.remove("hooks");
        }
    }

    Ok(())
}

/// Apply hook configuration to the settings file (surgical per-type merge).
///
/// For each of the 11 HOOK_TYPES:
/// 1. Gets or creates the `hooks` object (does NOT replace it).
/// 2. Gets or creates the array for this hook_type.
/// 3. Removes any existing agtmux entries (identified by `AGTMUX_HOOK_TYPE=` in `command`).
/// 4. Appends the new agtmux entry.
///
/// This preserves all entries from other tools in every hook_type array.
pub fn apply_hooks(opts: &SetupHooksOpts) -> anyhow::Result<PathBuf> {
    let path = settings_path(&opts.scope)?;
    let script = resolve_hook_script(opts.hook_script.as_deref())?;
    let quoted = shell_quote(&script);

    // Read existing settings or start fresh
    let mut settings: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        serde_json::from_str(&content)?
    } else {
        serde_json::json!({})
    };

    merge_hooks_into_settings(&mut settings, &quoted)?;

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let output = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&path, format!("{output}\n"))?;

    Ok(path)
}

/// Remove agtmux hook entries from the settings file.
///
/// Only removes entries whose `command` field contains `AGTMUX_HOOK_TYPE=`.
/// Other tools' hook entries are preserved.
/// Empty hook-type arrays and an empty `hooks` object are cleaned up.
/// If the settings file does not exist, returns Ok(path) without error.
pub fn remove_hooks(scope: &str) -> anyhow::Result<PathBuf> {
    let path = settings_path(scope)?;
    if !path.exists() {
        return Ok(path); // nothing to do, not an error
    }
    let content = std::fs::read_to_string(&path)?;
    let mut settings: serde_json::Value = serde_json::from_str(&content)?;

    remove_hooks_from_settings(&mut settings)?;

    let output = serde_json::to_string_pretty(&settings)?;
    std::fs::write(&path, format!("{output}\n"))?;
    Ok(path)
}

/// Status of a single hook type in the settings file.
#[derive(Debug, PartialEq)]
pub enum HookStatus {
    Registered,
    Missing,
}

/// Result of checking hook configuration.
pub struct HookCheckResult {
    pub statuses: Vec<(&'static str, HookStatus)>,
}

impl HookCheckResult {
    /// Returns true if all hooks are registered.
    pub fn all_registered(&self) -> bool {
        self.statuses
            .iter()
            .all(|(_, s)| *s == HookStatus::Registered)
    }

    /// Returns list of missing hook type names.
    pub fn missing(&self) -> Vec<&'static str> {
        self.statuses
            .iter()
            .filter_map(|(name, s)| {
                if *s == HookStatus::Missing {
                    Some(*name)
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Check the current hook registration status in the given settings.json.
/// Returns HookCheckResult with per-hook status.
pub fn check_hooks(scope: &str) -> anyhow::Result<HookCheckResult> {
    let path = settings_path(scope)?;
    check_hooks_at_path(&path)
}

/// Check the current hook registration status at an explicit settings.json path.
/// Returns HookCheckResult with per-hook status.
pub(crate) fn check_hooks_at_path(path: &Path) -> anyhow::Result<HookCheckResult> {
    // Read existing settings; if not present, all hooks are missing.
    let settings: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(path)?;
        serde_json::from_str(&content)?
    } else {
        serde_json::json!({})
    };

    let hooks_obj = settings.get("hooks").and_then(|h| h.as_object());

    let statuses = HOOK_TYPES
        .iter()
        .map(|&hook_type| {
            let status = if hooks_obj
                .map(|h| h.contains_key(hook_type))
                .unwrap_or(false)
            {
                HookStatus::Registered
            } else {
                HookStatus::Missing
            };
            (hook_type, status)
        })
        .collect();

    Ok(HookCheckResult { statuses })
}

// ─── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── Pure value-logic tests (no file I/O, no CWD, safe to run in parallel) ──

    /// Build a fresh empty settings value.
    fn empty_settings() -> serde_json::Value {
        serde_json::json!({})
    }

    /// Build settings pre-populated with one foreign entry for hook_type.
    fn settings_with_foreign(hook_type: &str) -> serde_json::Value {
        serde_json::json!({
            "hooks": {
                hook_type: [{"type": "command", "command": "other-tool-hook.sh"}]
            }
        })
    }

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("valid unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agtmux-{label}-{nonce}"));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    // ── merge_hooks_into_settings (T-E08) ─────────────────────────────

    #[test]
    fn merge_writes_all_hook_types() {
        let mut s = empty_settings();
        merge_hooks_into_settings(&mut s, "agtmux-claude-hook.sh").expect("merge ok");

        let hooks_obj = s["hooks"].as_object().expect("hooks object");
        for hook_type in HOOK_TYPES {
            let arr = hooks_obj[*hook_type].as_array().expect("array");
            assert_eq!(arr.len(), 1);
            assert_eq!(arr[0]["type"], "command");
            let cmd = arr[0]["command"].as_str().expect("cmd");
            assert!(cmd.contains(hook_type));
            assert!(cmd.contains("agtmux-claude-hook.sh"));
        }
    }

    #[test]
    fn merge_escapes_path_with_spaces() {
        let mut s = empty_settings();
        // shell_quote is called externally before merge; pass the quoted string directly
        let quoted = shell_quote("/path/with spaces/hook.sh");
        merge_hooks_into_settings(&mut s, &quoted).expect("merge ok");
        let cmd = s["hooks"]["PreToolUse"][0]["command"]
            .as_str()
            .expect("cmd");
        assert!(
            cmd.contains("'/path/with spaces/hook.sh'"),
            "path with spaces should be single-quoted, got: {cmd}"
        );
    }

    #[test]
    fn merge_escapes_path_with_quotes() {
        let mut s = empty_settings();
        let quoted = shell_quote("/path/it's/hook.sh");
        merge_hooks_into_settings(&mut s, &quoted).expect("merge ok");
        let cmd = s["hooks"]["PreToolUse"][0]["command"]
            .as_str()
            .expect("cmd");
        assert!(
            cmd.contains("'\\''"),
            "path with single quotes should be escaped, got: {cmd}"
        );
        assert!(
            !cmd.contains("it's/hook"),
            "raw single quote should not appear unescaped, got: {cmd}"
        );
    }

    #[test]
    fn apply_hooks_preserves_other_tools_hooks() {
        let mut s = settings_with_foreign("PreToolUse");
        merge_hooks_into_settings(&mut s, "agtmux-claude-hook.sh").expect("merge ok");

        let arr = s["hooks"]["PreToolUse"].as_array().expect("array");

        let has_foreign = arr.iter().any(|e| {
            e.get("command")
                .and_then(|c| c.as_str())
                .map(|cmd| cmd == "other-tool-hook.sh")
                .unwrap_or(false)
        });
        let has_agtmux = arr.iter().any(|e| {
            e.get("command")
                .and_then(|c| c.as_str())
                .map(|cmd| cmd.contains("AGTMUX_HOOK_TYPE="))
                .unwrap_or(false)
        });
        assert!(has_foreign, "foreign entry should be preserved");
        assert!(has_agtmux, "agtmux entry should be added");
    }

    #[test]
    fn apply_hooks_is_idempotent_with_surgical_merge() {
        let mut s = empty_settings();
        merge_hooks_into_settings(&mut s, "agtmux-claude-hook.sh").expect("first merge ok");
        merge_hooks_into_settings(&mut s, "agtmux-claude-hook.sh").expect("second merge ok");

        for hook_type in HOOK_TYPES {
            let arr = s["hooks"][hook_type]
                .as_array()
                .unwrap_or_else(|| panic!("expected array for {hook_type}"));
            let agtmux_count = arr
                .iter()
                .filter(|e| {
                    e.get("command")
                        .and_then(|c| c.as_str())
                        .map(|cmd| cmd.contains("AGTMUX_HOOK_TYPE="))
                        .unwrap_or(false)
                })
                .count();
            assert_eq!(
                agtmux_count, 1,
                "expected exactly 1 agtmux entry for {hook_type}, got {agtmux_count}"
            );
        }
    }

    // ── remove_hooks_from_settings (T-E09) ────────────────────────────

    #[test]
    fn remove_hooks_removes_only_agtmux_entries() {
        let mut s = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {"type": "command", "command": "AGTMUX_HOOK_TYPE=PreToolUse agtmux-claude-hook.sh"},
                    {"type": "command", "command": "other-tool-hook.sh"}
                ]
            }
        });
        remove_hooks_from_settings(&mut s).expect("remove ok");

        let arr = s["hooks"]["PreToolUse"].as_array().expect("array");
        assert_eq!(arr.len(), 1, "only foreign entry should remain");
        assert_eq!(arr[0]["command"].as_str().unwrap(), "other-tool-hook.sh");
    }

    #[test]
    fn remove_hooks_cleans_empty_arrays() {
        // Build a settings with only agtmux entries for all hook types
        let mut hooks_map = serde_json::Map::new();
        for hook_type in HOOK_TYPES {
            hooks_map.insert(
                (*hook_type).to_string(),
                serde_json::json!([{
                    "type": "command",
                    "command": format!("AGTMUX_HOOK_TYPE={hook_type} agtmux-claude-hook.sh")
                }]),
            );
        }
        let mut s = serde_json::json!({"hooks": hooks_map});
        remove_hooks_from_settings(&mut s).expect("remove ok");

        assert!(
            s.get("hooks").is_none(),
            "hooks key should be removed when all arrays are empty"
        );
    }

    #[test]
    fn remove_hooks_noop_on_empty_settings() {
        let mut s = empty_settings();
        remove_hooks_from_settings(&mut s).expect("remove on empty ok");
        // No hooks key was present and none should appear
        assert!(s.get("hooks").is_none());
    }

    #[test]
    fn remove_hooks_preserves_other_settings_keys() {
        let mut s = serde_json::json!({
            "model": "claude-opus-4-6",
            "hooks": {
                "PreToolUse": [
                    {"type": "command", "command": "AGTMUX_HOOK_TYPE=PreToolUse agtmux-claude-hook.sh"}
                ]
            }
        });
        remove_hooks_from_settings(&mut s).expect("remove ok");

        assert_eq!(
            s.get("model").and_then(|v| v.as_str()),
            Some("claude-opus-4-6"),
            "non-hooks settings keys must be preserved"
        );
    }

    // ── round-trip: merge then remove ────────────────────────────────

    #[test]
    fn merge_then_remove_leaves_settings_unchanged() {
        let original = serde_json::json!({
            "model": "claude-opus-4-6",
            "hooks": {
                "PreToolUse": [{"type": "command", "command": "other-tool-hook.sh"}]
            }
        });
        let mut s = original.clone();
        merge_hooks_into_settings(&mut s, "agtmux-claude-hook.sh").expect("merge ok");
        remove_hooks_from_settings(&mut s).expect("remove ok");

        assert_eq!(s, original, "round-trip must restore original state");
    }

    // ── settings_path / resolve_hook_script / HookCheckResult ────────

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

    #[test]
    fn check_hooks_all_missing_when_no_settings() {
        let result = HookCheckResult {
            statuses: HOOK_TYPES
                .iter()
                .map(|&ht| (ht, HookStatus::Missing))
                .collect(),
        };
        assert!(!result.all_registered());
        assert_eq!(result.missing().len(), HOOK_TYPES.len());
    }

    #[test]
    fn check_hooks_all_registered() {
        let result = HookCheckResult {
            statuses: HOOK_TYPES
                .iter()
                .map(|&ht| (ht, HookStatus::Registered))
                .collect(),
        };
        assert!(result.all_registered());
        assert!(result.missing().is_empty());
    }

    #[test]
    fn check_hooks_partial_missing() {
        let result = HookCheckResult {
            statuses: vec![
                ("PreToolUse", HookStatus::Registered),
                ("SessionEnd", HookStatus::Missing),
            ],
        };
        assert!(!result.all_registered());
        assert_eq!(result.missing(), vec!["SessionEnd"]);
    }

    #[test]
    fn check_hooks_integration_partial_registered() {
        let temp = temp_dir("check-hooks-partial");
        let settings_path = temp.join(".claude").join("settings.json");
        fs::create_dir_all(settings_path.parent().expect("settings parent"))
            .expect("create .claude dir");
        fs::write(
            &settings_path,
            serde_json::to_string_pretty(&serde_json::json!({
                "hooks": {
                    "PreToolUse": [{
                        "type": "command",
                        "command": "AGTMUX_HOOK_TYPE=PreToolUse agtmux-claude-hook.sh"
                    }]
                }
            }))
            .expect("serialize settings"),
        )
        .expect("write settings");

        let result = check_hooks_at_path(&settings_path).expect("check hooks");

        assert!(!result.all_registered());
        assert_eq!(result.statuses[0], ("PreToolUse", HookStatus::Registered));
        let missing = result.missing();
        assert!(missing.contains(&"PostToolUse"));
        assert!(missing.contains(&"SessionStart"));
        assert!(!missing.contains(&"PreToolUse"));

        let _ = fs::remove_dir_all(temp);
    }

    // ── MT-5: B-1 fix — remove_hooks must not delete third-party empty arrays ──

    #[test]
    fn remove_hooks_does_not_delete_third_party_empty_arrays() {
        // A third-party tool left an empty array under a non-HOOK_TYPES key.
        // --unregister must NOT delete it.
        let mut settings = serde_json::json!({
            "hooks": {
                "CustomHook": [],           // third-party empty array
                "PreToolUse": [{
                    "type": "command",
                    "command": "AGTMUX_HOOK_TYPE=PreToolUse /some/hook.sh"
                }]
            }
        });
        remove_hooks_from_settings(&mut settings).expect("remove ok");
        // "CustomHook" must survive
        let hooks = settings["hooks"].as_object().expect("hooks object");
        assert!(
            hooks.contains_key("CustomHook"),
            "third-party empty array must be preserved"
        );
        // "PreToolUse" was the only agtmux entry → removed → array empty → key removed
        assert!(
            !hooks.contains_key("PreToolUse"),
            "agtmux PreToolUse key should be removed"
        );
    }

    // ── MT-1: merge_hooks_into_settings with non-object hooks value ──────────

    #[test]
    fn merge_hooks_errors_when_hooks_key_is_not_object() {
        let mut settings = serde_json::json!({ "hooks": null });
        let result = merge_hooks_into_settings(&mut settings, "/hook.sh");
        assert!(result.is_err(), "null hooks value should be an error");
    }

    #[test]
    fn merge_hooks_errors_when_hooks_key_is_array() {
        let mut settings = serde_json::json!({ "hooks": [] });
        let result = merge_hooks_into_settings(&mut settings, "/hook.sh");
        assert!(result.is_err(), "array hooks value should be an error");
    }

    // ── MT-2: remove_hooks_from_settings with non-object hooks value ─────────

    #[test]
    fn remove_hooks_noop_when_hooks_is_null() {
        let mut settings = serde_json::json!({ "hooks": null });
        remove_hooks_from_settings(&mut settings).expect("remove on null hooks ok");
        // hooks key still present and still null (unchanged)
        assert_eq!(settings["hooks"], serde_json::Value::Null);
    }

    // ── MT-6: merge_hooks_into_settings when hook_type value is not an array ─

    #[test]
    fn merge_hooks_errors_when_hook_type_value_is_not_array() {
        let mut settings = serde_json::json!({
            "hooks": { "PreToolUse": "not-an-array" }
        });
        let result = merge_hooks_into_settings(&mut settings, "/hook.sh");
        assert!(
            result.is_err(),
            "non-array hook_type value should be an error"
        );
    }
}
