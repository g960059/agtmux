//! Legacy sync-v2 runtime wire builders.
//!
//! These helpers intentionally isolate the remaining v2 bootstrap/change feed
//! payload assembly from the live server module so the runtime wire boundary
//! makes the compat-only surface explicit.

use agtmux_core_v5::types::{EvidenceMode, PanePresence};
use agtmux_daemon_v5::projection::{ReplayCursor, StateChange};

use crate::poll_loop::DaemonState;

pub(crate) fn parse_replay_cursor(value: &serde_json::Value) -> Option<ReplayCursor> {
    Some(ReplayCursor {
        epoch: value.get("epoch")?.as_u64()?,
        seq: value.get("seq")?.as_u64()?,
    })
}

pub(crate) fn build_ui_bootstrap_v2(state: &DaemonState) -> serde_json::Value {
    let replay_cursor = state.daemon.replay_cursor();
    serde_json::json!({
        "epoch": replay_cursor.epoch,
        "snapshot_seq": replay_cursor.seq,
        "panes": build_sync_v2_pane_list(state),
        "sessions": build_session_list(state),
        "generated_at": chrono::Utc::now(),
        "replay_cursor": {
            "epoch": replay_cursor.epoch,
            "seq": replay_cursor.seq,
        },
    })
}

pub(crate) fn build_ui_changes_v2(
    state: &mut DaemonState,
    cursor: Option<ReplayCursor>,
    limit: usize,
) -> serde_json::Value {
    let now = chrono::Utc::now();
    let Some(cursor) = cursor else {
        let replay_cursor = state.daemon.replay_cursor();
        state.daemon.record_replay_resync("invalid_cursor", now);
        return build_resync_required(replay_cursor.epoch, replay_cursor.seq, "invalid_cursor");
    };

    match state.daemon.replay_changes(cursor, limit) {
        Ok(batch) => {
            let changes = batch
                .changes
                .iter()
                .map(|change| build_change_entry(change))
                .collect::<Vec<_>>();
            let epoch = batch.epoch;
            let from_seq = batch.from_seq;
            let to_seq = batch.to_seq;
            let next_epoch = batch.next_cursor.epoch;
            let next_seq = batch.next_cursor.seq;
            state.daemon.acknowledge_replay_cursor(cursor);
            serde_json::json!({
                "epoch": epoch,
                "changes": changes,
                "from_seq": from_seq,
                "to_seq": to_seq,
                "next_cursor": {
                    "epoch": next_epoch,
                    "seq": next_seq,
                },
            })
        }
        Err(resync) => {
            state.daemon.record_replay_resync(resync.reason, now);
            build_resync_required(
                resync.current_epoch,
                resync.latest_snapshot_seq,
                resync.reason,
            )
        }
    }
}

fn build_session_list(state: &DaemonState) -> serde_json::Value {
    serde_json::to_value(state.daemon.list_sessions()).unwrap_or_else(|_| serde_json::json!([]))
}

fn build_resync_required(
    current_epoch: u64,
    latest_snapshot_seq: u64,
    reason: &str,
) -> serde_json::Value {
    serde_json::json!({
        "resync_required": {
            "current_epoch": current_epoch,
            "latest_snapshot_seq": latest_snapshot_seq,
            "reason": reason,
        }
    })
}

fn sync_v2_pane_session_key(pane_id: &str) -> String {
    format!("poller-{pane_id}")
}

fn build_change_entry(change: &StateChange) -> serde_json::Value {
    let sync_session_key = change
        .pane_id
        .as_deref()
        .map(sync_v2_pane_session_key)
        .unwrap_or_else(|| change.session_key.clone());

    let mut entry = serde_json::json!({
        "seq": change.version,
        "session_key": sync_session_key,
        "timestamp": change.timestamp,
    });

    if let Some(ref pane_id) = change.pane_id {
        entry["pane_id"] = serde_json::Value::String(pane_id.clone());
    }
    if let Some(ref pane_state) = change.pane_state {
        let mut pane_value = serde_json::to_value(pane_state).unwrap_or(serde_json::Value::Null);
        if let Some(pane_id) = change.pane_id.as_deref()
            && let Some(pane_obj) = pane_value.as_object_mut()
        {
            pane_obj.insert(
                "session_key".to_string(),
                serde_json::Value::String(sync_v2_pane_session_key(pane_id)),
            );
        }
        entry["pane"] = pane_value;
    }
    if let Some(ref session_state) = change.session_state {
        entry["session"] = serde_json::to_value(session_state).unwrap_or(serde_json::Value::Null);
    }

    entry
}

/// Build sync-v2 pane list with exact-identity contract (FR-065 / FR-066).
///
/// Does NOT reuse the human-facing inventory serializer, which emits legacy
/// aliases such as `session_id` that are forbidden in sync-v2 payloads.
fn build_sync_v2_pane_list(state: &DaemonState) -> serde_json::Value {
    let managed_panes = state.daemon.list_panes();
    let managed_ids = managed_panes
        .iter()
        .map(|pane| pane.pane_instance_id.pane_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let now = chrono::Utc::now();
    let mut result: Vec<serde_json::Value> = Vec::new();

    for pane in &managed_panes {
        let Some(tmux_info) = state
            .last_panes
            .iter()
            .find(|tmux_pane| tmux_pane.pane_id == pane.pane_instance_id.pane_id)
        else {
            // FR-065: exact-location fields are required for managed sync-v2 rows.
            continue;
        };

        let age_secs = (now - pane.updated_at).num_seconds().max(0) as u64;

        result.push(serde_json::json!({
            "pane_id": pane.pane_instance_id.pane_id,
            "session_name": tmux_info.session_name.as_str(),
            "session_key": sync_v2_pane_session_key(&pane.pane_instance_id.pane_id),
            "window_id": tmux_info.window_id.as_str(),
            "pane_instance_id": {
                "pane_id": pane.pane_instance_id.pane_id,
                "generation": pane.pane_instance_id.generation,
                "birth_ts": pane.pane_instance_id.birth_ts,
            },
            "window_name": tmux_info.window_name.as_str(),
            "session_group": serde_json::Value::Null,
            "presence": "managed",
            "evidence_mode": pane.evidence_mode,
            "activity_state": format!("{:?}", pane.activity_state),
            "provider": pane.provider.map(|provider| provider.as_str()),
            "conversation_title": state.conversation_titles.get(&pane.session_key),
            "current_path": tmux_info.current_path.as_str(),
            "git_branch": serde_json::Value::Null,
            "current_cmd": tmux_info.current_cmd.as_str(),
            "updated_at": pane.updated_at,
            "age_secs": age_secs,
        }));
    }

    for tmux_pane in &state.last_panes {
        if managed_ids.contains(tmux_pane.pane_id.as_str()) {
            continue;
        }

        let (generation, birth_ts) = state
            .generation_tracker
            .get(&tmux_pane.pane_id)
            .unwrap_or((0, now));

        result.push(serde_json::json!({
            "pane_id": tmux_pane.pane_id.as_str(),
            "session_name": tmux_pane.session_name.as_str(),
            "session_key": sync_v2_pane_session_key(&tmux_pane.pane_id),
            "window_id": tmux_pane.window_id.as_str(),
            "pane_instance_id": {
                "pane_id": tmux_pane.pane_id.as_str(),
                "generation": generation,
                "birth_ts": birth_ts,
            },
            "window_name": tmux_pane.window_name.as_str(),
            "session_group": serde_json::Value::Null,
            "presence": PanePresence::Unmanaged,
            "evidence_mode": EvidenceMode::None,
            "activity_state": "Unknown",
            "provider": serde_json::Value::Null,
            "conversation_title": serde_json::Value::Null,
            "current_path": tmux_pane.current_path.as_str(),
            "git_branch": serde_json::Value::Null,
            "current_cmd": tmux_pane.current_cmd.as_str(),
            "updated_at": now,
            "age_secs": 0,
        }));
    }

    serde_json::Value::Array(result)
}
