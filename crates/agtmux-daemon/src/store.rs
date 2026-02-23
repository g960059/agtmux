//! SQLite persistence for pane state, allowing state to survive daemon restarts.

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, Result};

use agtmux_core::engine::ResolvedActivity;
use agtmux_core::types::{AttentionResult, AttentionState, Provider};

use crate::orchestrator::PaneState;
use crate::serde_helpers::{parse_enum, serde_variant_name};

/// SQLite-backed persistence store for pane states.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (or create) a database at the given filesystem path and run migrations.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Open an in-memory database. Useful for testing.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Create the schema if it does not already exist.
    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS pane_states (
                pane_id            TEXT PRIMARY KEY,
                provider           TEXT,
                provider_confidence REAL NOT NULL DEFAULT 0.0,
                activity_state     TEXT NOT NULL,
                activity_confidence REAL NOT NULL DEFAULT 0.0,
                activity_source    TEXT NOT NULL DEFAULT 'unknown',
                attention_state    TEXT NOT NULL DEFAULT 'none',
                attention_reason   TEXT NOT NULL DEFAULT '',
                attention_since    TEXT,
                last_event_type    TEXT NOT NULL DEFAULT '',
                updated_at         TEXT NOT NULL
            );",
        )?;
        Ok(())
    }

    /// Upsert a single pane state row.
    pub fn save_pane_state(&self, state: &PaneState) -> Result<()> {
        let provider_str: Option<String> = state.provider.as_ref().map(|p| serde_variant_name(p));
        let activity_state_str = serde_variant_name(&state.activity.state);
        let activity_source_str = serde_variant_name(&state.activity.source);
        let attention_state_str = serde_variant_name(&state.attention.state);
        let attention_since_str: Option<String> =
            state.attention.since.map(|dt| dt.to_rfc3339());
        let updated_at_str = state.updated_at.to_rfc3339();

        self.conn.execute(
            "INSERT OR REPLACE INTO pane_states
                (pane_id, provider, provider_confidence,
                 activity_state, activity_confidence, activity_source,
                 attention_state, attention_reason, attention_since,
                 last_event_type, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                state.pane_id,
                provider_str,
                state.provider_confidence,
                activity_state_str,
                state.activity.confidence,
                activity_source_str,
                attention_state_str,
                state.attention.reason,
                attention_since_str,
                state.last_event_type,
                updated_at_str,
            ],
        )?;
        Ok(())
    }

    /// Load all pane state rows from the database.
    pub fn load_all_pane_states(&self) -> Result<Vec<PaneState>> {
        let mut stmt = self.conn.prepare(
            "SELECT pane_id, provider, provider_confidence,
                    activity_state, activity_confidence, activity_source,
                    attention_state, attention_reason, attention_since,
                    last_event_type, updated_at
             FROM pane_states",
        )?;

        let rows = stmt.query_map([], |row| {
            let pane_id: String = row.get(0)?;
            let provider_str: Option<String> = row.get(1)?;
            let provider_confidence: f64 = row.get(2)?;
            let activity_state_str: String = row.get(3)?;
            let activity_confidence: f64 = row.get(4)?;
            let activity_source_str: String = row.get(5)?;
            let attention_state_str: String = row.get(6)?;
            let attention_reason: String = row.get(7)?;
            let attention_since_str: Option<String> = row.get(8)?;
            let last_event_type: String = row.get(9)?;
            let updated_at_str: String = row.get(10)?;

            let provider: Option<Provider> =
                provider_str.as_deref().and_then(parse_enum);
            let activity_state = parse_enum(&activity_state_str)
                .unwrap_or(agtmux_core::types::ActivityState::Unknown);
            let activity_source = parse_enum(&activity_source_str)
                .unwrap_or(agtmux_core::types::SourceType::Poller);
            let attention_state: AttentionState =
                parse_enum(&attention_state_str).unwrap_or(AttentionState::None);
            let attention_since: Option<DateTime<Utc>> = attention_since_str
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            let updated_at: DateTime<Utc> = DateTime::parse_from_rfc3339(&updated_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            Ok(PaneState {
                pane_id,
                provider,
                provider_confidence,
                activity: ResolvedActivity {
                    state: activity_state,
                    confidence: activity_confidence,
                    source: activity_source,
                    reason_code: String::new(),
                },
                attention: AttentionResult {
                    state: attention_state,
                    reason: attention_reason,
                    since: attention_since,
                },
                last_event_type,
                updated_at,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Delete a single pane state row by pane_id.
    pub fn remove_pane_state(&self, pane_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM pane_states WHERE pane_id = ?1",
            params![pane_id],
        )?;
        Ok(())
    }

    /// Delete all pane state rows. Useful for testing or full reset.
    pub fn clear(&self) -> Result<()> {
        self.conn.execute("DELETE FROM pane_states", [])?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use agtmux_core::engine::ResolvedActivity;
    use agtmux_core::types::*;
    use chrono::Utc;

    /// Helper to create a PaneState for testing.
    fn make_pane_state(pane_id: &str) -> PaneState {
        PaneState {
            pane_id: pane_id.to_string(),
            provider: Some(Provider::Claude),
            provider_confidence: 0.92,
            activity: ResolvedActivity {
                state: ActivityState::Running,
                confidence: 0.88,
                source: SourceType::Hook,
                reason_code: "running".to_string(),
            },
            attention: AttentionResult {
                state: AttentionState::None,
                reason: String::new(),
                since: None,
            },
            last_event_type: "tool-execution".to_string(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn open_in_memory_creates_table() {
        let store = Store::open_in_memory().expect("should open in-memory db");
        // Verify the table exists by attempting to query it.
        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM pane_states", [], |row| row.get(0))
            .expect("pane_states table should exist");
        assert_eq!(count, 0);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let store = Store::open_in_memory().unwrap();

        let original = PaneState {
            pane_id: "%1".to_string(),
            provider: Some(Provider::Codex),
            provider_confidence: 0.85,
            activity: ResolvedActivity {
                state: ActivityState::WaitingApproval,
                confidence: 0.95,
                source: SourceType::Api,
                reason_code: "approval_needed".to_string(),
            },
            attention: AttentionResult {
                state: AttentionState::ActionRequiredApproval,
                reason: "tool approval".to_string(),
                since: Some(Utc::now()),
            },
            last_event_type: "approval-request".to_string(),
            updated_at: Utc::now(),
        };

        store.save_pane_state(&original).unwrap();
        let loaded = store.load_all_pane_states().unwrap();

        assert_eq!(loaded.len(), 1);
        let l = &loaded[0];
        assert_eq!(l.pane_id, "%1");
        assert_eq!(l.provider, Some(Provider::Codex));
        assert!((l.provider_confidence - 0.85).abs() < f64::EPSILON);
        assert_eq!(l.activity.state, ActivityState::WaitingApproval);
        assert!((l.activity.confidence - 0.95).abs() < f64::EPSILON);
        assert_eq!(l.activity.source, SourceType::Api);
        assert_eq!(l.attention.state, AttentionState::ActionRequiredApproval);
        assert_eq!(l.attention.reason, "tool approval");
        assert!(l.attention.since.is_some());
        assert_eq!(l.last_event_type, "approval-request");
        // updated_at should roundtrip within 1 second (RFC3339 has sub-second precision)
        let delta = (l.updated_at - original.updated_at).num_milliseconds().abs();
        assert!(delta < 1000, "updated_at should roundtrip accurately, delta={delta}ms");
    }

    #[test]
    fn upsert_overwrites_existing_row() {
        let store = Store::open_in_memory().unwrap();

        let mut state = make_pane_state("%1");
        store.save_pane_state(&state).unwrap();

        // Update the state and save again.
        state.activity = ResolvedActivity {
            state: ActivityState::Error,
            confidence: 0.99,
            source: SourceType::Hook,
            reason_code: "error_detected".to_string(),
        };
        state.attention = AttentionResult {
            state: AttentionState::ActionRequiredError,
            reason: "process crashed".to_string(),
            since: Some(Utc::now()),
        };
        store.save_pane_state(&state).unwrap();

        let loaded = store.load_all_pane_states().unwrap();
        assert_eq!(loaded.len(), 1, "upsert should not create duplicate rows");
        assert_eq!(loaded[0].activity.state, ActivityState::Error);
        assert_eq!(loaded[0].attention.state, AttentionState::ActionRequiredError);
        assert_eq!(loaded[0].attention.reason, "process crashed");
    }

    #[test]
    fn remove_pane_state_deletes_row() {
        let store = Store::open_in_memory().unwrap();

        store.save_pane_state(&make_pane_state("%1")).unwrap();
        store.save_pane_state(&make_pane_state("%2")).unwrap();
        assert_eq!(store.load_all_pane_states().unwrap().len(), 2);

        store.remove_pane_state("%1").unwrap();

        let loaded = store.load_all_pane_states().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].pane_id, "%2");
    }

    #[test]
    fn load_all_pane_states_multiple_panes() {
        let store = Store::open_in_memory().unwrap();

        let state1 = PaneState {
            pane_id: "%1".to_string(),
            provider: Some(Provider::Claude),
            provider_confidence: 0.9,
            activity: ResolvedActivity {
                state: ActivityState::Running,
                confidence: 0.88,
                source: SourceType::Hook,
                reason_code: String::new(),
            },
            attention: AttentionResult {
                state: AttentionState::None,
                reason: String::new(),
                since: None,
            },
            last_event_type: String::new(),
            updated_at: Utc::now(),
        };

        let state2 = PaneState {
            pane_id: "%2".to_string(),
            provider: Some(Provider::Codex),
            provider_confidence: 0.85,
            activity: ResolvedActivity {
                state: ActivityState::WaitingInput,
                confidence: 0.92,
                source: SourceType::Poller,
                reason_code: String::new(),
            },
            attention: AttentionResult {
                state: AttentionState::ActionRequiredInput,
                reason: "needs input".to_string(),
                since: Some(Utc::now()),
            },
            last_event_type: "prompt".to_string(),
            updated_at: Utc::now(),
        };

        let state3 = PaneState {
            pane_id: "%3".to_string(),
            provider: None,
            provider_confidence: 0.0,
            activity: ResolvedActivity {
                state: ActivityState::Unknown,
                confidence: 0.5,
                source: SourceType::Poller,
                reason_code: String::new(),
            },
            attention: AttentionResult {
                state: AttentionState::None,
                reason: String::new(),
                since: None,
            },
            last_event_type: String::new(),
            updated_at: Utc::now(),
        };

        store.save_pane_state(&state1).unwrap();
        store.save_pane_state(&state2).unwrap();
        store.save_pane_state(&state3).unwrap();

        let loaded = store.load_all_pane_states().unwrap();
        assert_eq!(loaded.len(), 3);

        // Verify each pane is present (order not guaranteed by SQLite without ORDER BY).
        let ids: Vec<&str> = loaded.iter().map(|p| p.pane_id.as_str()).collect();
        assert!(ids.contains(&"%1"));
        assert!(ids.contains(&"%2"));
        assert!(ids.contains(&"%3"));

        // Verify the None provider roundtrips correctly.
        let p3 = loaded.iter().find(|p| p.pane_id == "%3").unwrap();
        assert_eq!(p3.provider, None);
        assert!((p3.provider_confidence - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn clear_removes_all_data() {
        let store = Store::open_in_memory().unwrap();

        store.save_pane_state(&make_pane_state("%1")).unwrap();
        store.save_pane_state(&make_pane_state("%2")).unwrap();
        store.save_pane_state(&make_pane_state("%3")).unwrap();
        assert_eq!(store.load_all_pane_states().unwrap().len(), 3);

        store.clear().unwrap();

        let loaded = store.load_all_pane_states().unwrap();
        assert_eq!(loaded.len(), 0, "clear should remove all rows");
    }

    #[test]
    fn remove_nonexistent_pane_is_noop() {
        let store = Store::open_in_memory().unwrap();
        // Removing a pane that does not exist should succeed silently.
        store.remove_pane_state("%nonexistent").unwrap();
        assert_eq!(store.load_all_pane_states().unwrap().len(), 0);
    }

    #[test]
    fn save_pane_with_none_provider() {
        let store = Store::open_in_memory().unwrap();

        let state = PaneState {
            pane_id: "%1".to_string(),
            provider: None,
            provider_confidence: 0.0,
            activity: ResolvedActivity {
                state: ActivityState::Idle,
                confidence: 0.5,
                source: SourceType::Poller,
                reason_code: String::new(),
            },
            attention: AttentionResult {
                state: AttentionState::None,
                reason: String::new(),
                since: None,
            },
            last_event_type: String::new(),
            updated_at: Utc::now(),
        };

        store.save_pane_state(&state).unwrap();
        let loaded = store.load_all_pane_states().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].provider, None);
        assert!(loaded[0].attention.since.is_none());
    }
}
