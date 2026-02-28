//! `agtmux json` — machine-readable JSON output.

use crate::client::rpc_call;
use crate::context::build_branch_map;

/// Normalize activity_state for JSON output.
///
/// Server values → lowercase with underscores:
/// - "Running" → "running"
/// - "Idle" → "idle"
/// - "WaitingApproval" → "waiting_approval"
/// - "WaitingInput" → "waiting_input"
/// - "Error" → "error"
/// - null/unknown → null
pub(crate) fn normalize_activity_state(state: Option<&str>) -> serde_json::Value {
    match state {
        Some("Running") => serde_json::Value::String("running".to_string()),
        Some("Idle") => serde_json::Value::String("idle".to_string()),
        Some("WaitingApproval") => serde_json::Value::String("waiting_approval".to_string()),
        Some("WaitingInput") => serde_json::Value::String("waiting_input".to_string()),
        Some("Error") => serde_json::Value::String("error".to_string()),
        Some("Unknown") => serde_json::Value::String("unknown".to_string()),
        _ => serde_json::Value::Null,
    }
}

/// Normalize provider for JSON output.
///
/// - "ClaudeCode" → "claude"
/// - "Codex" → "codex"
/// - null → null
pub(crate) fn normalize_provider(provider: Option<&str>) -> serde_json::Value {
    match provider {
        Some("ClaudeCode") => serde_json::Value::String("claude".to_string()),
        Some("Codex") => serde_json::Value::String("codex".to_string()),
        Some(other) => serde_json::Value::String(other.to_lowercase()),
        None => serde_json::Value::Null,
    }
}

/// Calculate age_secs from updated_at ISO timestamp.
pub(crate) fn calculate_age_secs(updated_at: Option<&str>) -> serde_json::Value {
    match updated_at {
        Some(ts) => match chrono::DateTime::parse_from_rfc3339(ts) {
            Ok(dt) => {
                let secs = (chrono::Utc::now() - dt.with_timezone(&chrono::Utc)).num_seconds();
                serde_json::Value::Number(serde_json::Number::from(secs.max(0)))
            }
            Err(_) => serde_json::Value::Null,
        },
        None => serde_json::Value::Null,
    }
}

/// Normalize presence for JSON output.
fn normalize_presence(presence: Option<&str>) -> &str {
    match presence {
        Some(p) => p,
        None => "unknown",
    }
}

/// Convert a single pane to JSON schema v1 representation.
fn pane_to_json_v1(
    pane: &serde_json::Value,
    branch_map: &std::collections::HashMap<String, String>,
) -> serde_json::Value {
    let git_branch = pane["current_path"]
        .as_str()
        .and_then(|p| branch_map.get(p))
        .map(|b| serde_json::Value::String(b.clone()))
        .unwrap_or(serde_json::Value::Null);

    serde_json::json!({
        "pane_id": pane["pane_id"],
        "session_name": pane["session_name"],
        "session_id": pane["session_id"],
        "window_name": pane["window_name"],
        "window_id": pane["window_id"],
        "presence": normalize_presence(pane["presence"].as_str()),
        "provider": normalize_provider(pane["provider"].as_str()),
        "activity_state": normalize_activity_state(pane["activity_state"].as_str()),
        "evidence_mode": pane.get("evidence_mode").and_then(|v| v.as_str()).unwrap_or("none"),
        "conversation_title": pane.get("conversation_title").cloned().unwrap_or(serde_json::Value::Null),
        "current_path": pane["current_path"],
        "git_branch": git_branch,
        "current_cmd": pane["current_cmd"],
        "updated_at": pane.get("updated_at").cloned().unwrap_or(serde_json::Value::Null),
        "age_secs": calculate_age_secs(pane.get("updated_at").and_then(|v| v.as_str())),
    })
}

/// Build the full JSON schema v1 output.
pub(crate) fn build_json_v1(
    panes: &[serde_json::Value],
    branch_map: &std::collections::HashMap<String, String>,
) -> serde_json::Value {
    let json_panes: Vec<serde_json::Value> = panes
        .iter()
        .map(|p| pane_to_json_v1(p, branch_map))
        .collect();

    serde_json::json!({
        "version": 1,
        "panes": json_panes,
    })
}

/// Entry point for `agtmux json`.
pub async fn cmd_json(socket_path: &str, health: bool) -> anyhow::Result<()> {
    if health {
        let result = rpc_call(socket_path, "list_source_health").await?;
        let json = serde_json::to_string_pretty(&result)?;
        println!("{json}");
        return Ok(());
    }

    let panes = rpc_call(socket_path, "list_panes").await?;
    let arr = panes.as_array().cloned().unwrap_or_default();
    let branch_map = build_branch_map(&arr);

    let output = build_json_v1(&arr, &branch_map);
    let json = serde_json::to_string_pretty(&output)?;
    println!("{json}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_activity_state_running() {
        assert_eq!(
            normalize_activity_state(Some("Running")),
            serde_json::Value::String("running".to_string())
        );
    }

    #[test]
    fn normalize_activity_state_waiting_approval() {
        assert_eq!(
            normalize_activity_state(Some("WaitingApproval")),
            serde_json::Value::String("waiting_approval".to_string())
        );
    }

    #[test]
    fn normalize_activity_state_waiting_input() {
        assert_eq!(
            normalize_activity_state(Some("WaitingInput")),
            serde_json::Value::String("waiting_input".to_string())
        );
    }

    #[test]
    fn normalize_activity_state_idle() {
        assert_eq!(
            normalize_activity_state(Some("Idle")),
            serde_json::Value::String("idle".to_string())
        );
    }

    #[test]
    fn normalize_activity_state_error() {
        assert_eq!(
            normalize_activity_state(Some("Error")),
            serde_json::Value::String("error".to_string())
        );
    }

    #[test]
    fn normalize_activity_state_null() {
        assert_eq!(normalize_activity_state(None), serde_json::Value::Null);
    }

    #[test]
    fn normalize_provider_claude_code() {
        assert_eq!(
            normalize_provider(Some("ClaudeCode")),
            serde_json::Value::String("claude".to_string())
        );
    }

    #[test]
    fn normalize_provider_codex() {
        assert_eq!(
            normalize_provider(Some("Codex")),
            serde_json::Value::String("codex".to_string())
        );
    }

    #[test]
    fn normalize_provider_null() {
        assert_eq!(normalize_provider(None), serde_json::Value::Null);
    }

    #[test]
    fn json_schema_version_is_1() {
        let panes: Vec<serde_json::Value> = vec![];
        let branch_map = std::collections::HashMap::new();
        let output = build_json_v1(&panes, &branch_map);
        assert_eq!(output["version"], 1);
        assert!(output["panes"].is_array());
    }

    #[test]
    fn json_schema_pane_fields() {
        let pane = serde_json::json!({
            "pane_id": "%0",
            "session_name": "work",
            "session_id": "$0",
            "window_id": "@0",
            "window_name": "api",
            "presence": "managed",
            "provider": "ClaudeCode",
            "evidence_mode": "deterministic",
            "activity_state": "WaitingApproval",
            "current_cmd": "claude",
            "current_path": "/Users/me/repo",
            "updated_at": "2026-02-28T10:30:00Z",
        });
        let branch_map: std::collections::HashMap<String, String> =
            [("/Users/me/repo".to_string(), "feat/oauth".to_string())].into();

        let output = build_json_v1(&[pane], &branch_map);
        let p = &output["panes"][0];

        assert_eq!(p["pane_id"], "%0");
        assert_eq!(p["session_name"], "work");
        assert_eq!(p["provider"], "claude");
        assert_eq!(p["activity_state"], "waiting_approval");
        assert_eq!(p["evidence_mode"], "deterministic");
        assert_eq!(p["git_branch"], "feat/oauth");
        assert_eq!(p["presence"], "managed");
    }

    #[test]
    fn age_secs_calculation() {
        use chrono::Utc;
        // Create a timestamp 120 seconds ago
        let ts = (Utc::now() - chrono::Duration::seconds(120))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let age = calculate_age_secs(Some(&ts));
        // Should be approximately 120, allow +-2s for test execution time
        let secs = age.as_i64().expect("should be a number");
        assert!(
            (118..=122).contains(&secs),
            "age_secs should be ~120, got {secs}"
        );
    }

    #[test]
    fn age_secs_null_for_missing() {
        assert_eq!(calculate_age_secs(None), serde_json::Value::Null);
    }

    #[test]
    fn age_secs_null_for_invalid() {
        assert_eq!(
            calculate_age_secs(Some("not-a-date")),
            serde_json::Value::Null
        );
    }
}
