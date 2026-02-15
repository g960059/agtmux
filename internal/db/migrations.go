package db

import (
	"context"
	"database/sql"
	"fmt"
)

type Migration struct {
	Version int
	UpSQL   string
	DownSQL string
}

var migrations = []Migration{
	{
		Version: 1,
		UpSQL: `
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_migrations (
	version INTEGER PRIMARY KEY,
	applied_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS targets (
	target_id TEXT PRIMARY KEY,
	target_name TEXT NOT NULL UNIQUE,
	kind TEXT NOT NULL CHECK(kind IN ('local','ssh')),
	connection_ref TEXT NOT NULL CHECK(connection_ref = '' OR (length(connection_ref) BETWEEN 1 AND 128 AND connection_ref NOT GLOB '*[^A-Za-z0-9._-]*')),
	is_default INTEGER NOT NULL DEFAULT 0,
	last_seen_at TEXT,
	health TEXT NOT NULL DEFAULT 'ok' CHECK(health IN ('ok','degraded','down')),
	updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS panes (
	target_id TEXT NOT NULL,
	pane_id TEXT NOT NULL,
	session_name TEXT NOT NULL,
	window_id TEXT NOT NULL,
	window_name TEXT NOT NULL,
	updated_at TEXT NOT NULL,
	PRIMARY KEY(target_id, pane_id),
	FOREIGN KEY(target_id) REFERENCES targets(target_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS runtimes (
	runtime_id TEXT PRIMARY KEY,
	target_id TEXT NOT NULL,
	pane_id TEXT NOT NULL,
	tmux_server_boot_id TEXT NOT NULL,
	pane_epoch INTEGER NOT NULL,
	agent_type TEXT NOT NULL,
	pid INTEGER,
	started_at TEXT NOT NULL,
	ended_at TEXT,
	UNIQUE(target_id, tmux_server_boot_id, pane_id, pane_epoch),
	FOREIGN KEY(target_id, pane_id) REFERENCES panes(target_id, pane_id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS runtimes_active_per_pane
ON runtimes(target_id, pane_id)
WHERE ended_at IS NULL;

CREATE TABLE IF NOT EXISTS actions (
	action_id TEXT PRIMARY KEY,
	action_type TEXT NOT NULL CHECK(action_type IN ('attach','send','view-output','kill')),
	request_ref TEXT NOT NULL,
	target_id TEXT NOT NULL,
	pane_id TEXT NOT NULL,
	runtime_id TEXT,
	requested_at TEXT NOT NULL,
	completed_at TEXT,
	result_code TEXT NOT NULL,
	error_code TEXT,
	metadata_json TEXT,
	UNIQUE(action_type, request_ref),
	FOREIGN KEY(target_id, pane_id) REFERENCES panes(target_id, pane_id) ON DELETE CASCADE,
	FOREIGN KEY(runtime_id) REFERENCES runtimes(runtime_id)
);

CREATE TABLE IF NOT EXISTS action_snapshots (
	snapshot_id TEXT PRIMARY KEY,
	action_id TEXT NOT NULL,
	target_id TEXT NOT NULL,
	pane_id TEXT NOT NULL,
	runtime_id TEXT NOT NULL,
	state_version INTEGER NOT NULL,
	observed_at TEXT NOT NULL,
	expires_at TEXT NOT NULL,
	nonce TEXT NOT NULL,
	FOREIGN KEY(action_id) REFERENCES actions(action_id) ON DELETE CASCADE,
	FOREIGN KEY(target_id, pane_id) REFERENCES panes(target_id, pane_id) ON DELETE CASCADE,
	FOREIGN KEY(runtime_id) REFERENCES runtimes(runtime_id)
);

CREATE UNIQUE INDEX IF NOT EXISTS action_snapshots_action_id_unique
ON action_snapshots(action_id);

CREATE TABLE IF NOT EXISTS events (
	event_id TEXT PRIMARY KEY,
	runtime_id TEXT NOT NULL,
	event_type TEXT NOT NULL,
	source TEXT NOT NULL CHECK(source IN ('hook','notify','wrapper','poller')),
	source_event_id TEXT,
	source_seq INTEGER,
	event_time TEXT NOT NULL,
	ingested_at TEXT NOT NULL,
	dedupe_key TEXT NOT NULL,
	action_id TEXT,
	raw_payload TEXT,
	UNIQUE(runtime_id, source, dedupe_key),
	FOREIGN KEY(runtime_id) REFERENCES runtimes(runtime_id) ON DELETE CASCADE,
	FOREIGN KEY(action_id) REFERENCES actions(action_id)
);

CREATE TABLE IF NOT EXISTS event_inbox (
	inbox_id TEXT PRIMARY KEY,
	target_id TEXT NOT NULL,
	pane_id TEXT NOT NULL,
	runtime_id TEXT,
	event_type TEXT NOT NULL,
	source TEXT NOT NULL CHECK(source IN ('hook','notify','wrapper','poller')),
	dedupe_key TEXT NOT NULL,
	event_time TEXT NOT NULL,
	ingested_at TEXT NOT NULL,
	pid INTEGER,
	start_hint TEXT,
	status TEXT NOT NULL CHECK(status IN ('pending_bind','bound','dropped_unbound')),
	reason_code TEXT,
	raw_payload TEXT,
	UNIQUE(target_id, pane_id, source, dedupe_key),
	FOREIGN KEY(target_id, pane_id) REFERENCES panes(target_id, pane_id) ON DELETE CASCADE,
	FOREIGN KEY(runtime_id) REFERENCES runtimes(runtime_id)
);

CREATE TABLE IF NOT EXISTS runtime_source_cursors (
	runtime_id TEXT NOT NULL,
	source TEXT NOT NULL CHECK(source IN ('hook','notify','wrapper','poller')),
	last_source_seq INTEGER,
	last_order_event_time TEXT NOT NULL,
	last_order_ingested_at TEXT NOT NULL,
	last_order_event_id TEXT NOT NULL,
	PRIMARY KEY(runtime_id, source),
	FOREIGN KEY(runtime_id) REFERENCES runtimes(runtime_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS states (
	target_id TEXT NOT NULL,
	pane_id TEXT NOT NULL,
	runtime_id TEXT NOT NULL,
	state TEXT NOT NULL CHECK(state IN ('running','waiting_input','waiting_approval','completed','idle','error','unknown')),
	reason_code TEXT,
	confidence TEXT NOT NULL CHECK(confidence IN ('high','medium','low')),
	state_version INTEGER NOT NULL,
	last_source_seq INTEGER,
	last_seen_at TEXT NOT NULL,
	updated_at TEXT NOT NULL,
	PRIMARY KEY(target_id, pane_id),
	FOREIGN KEY(target_id, pane_id) REFERENCES panes(target_id, pane_id) ON DELETE CASCADE,
	FOREIGN KEY(runtime_id) REFERENCES runtimes(runtime_id)
);

CREATE TABLE IF NOT EXISTS adapters (
	adapter_name TEXT PRIMARY KEY,
	agent_type TEXT NOT NULL,
	version TEXT NOT NULL,
	capabilities TEXT NOT NULL,
	enabled INTEGER NOT NULL DEFAULT 1,
	updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS events_runtime_source_ingested_at
ON events(runtime_id, source, ingested_at DESC);

CREATE INDEX IF NOT EXISTS events_ingested_at
ON events(ingested_at DESC);

CREATE INDEX IF NOT EXISTS event_inbox_status_ingested_at
ON event_inbox(status, ingested_at);

CREATE INDEX IF NOT EXISTS states_updated_at
ON states(updated_at DESC);

CREATE INDEX IF NOT EXISTS states_state_updated_at
ON states(state, updated_at DESC);
`,
		DownSQL: `
DROP TABLE IF EXISTS adapters;
DROP TABLE IF EXISTS states;
DROP TABLE IF EXISTS runtime_source_cursors;
DROP TABLE IF EXISTS event_inbox;
DROP TABLE IF EXISTS events;
DROP TABLE IF EXISTS action_snapshots;
DROP TABLE IF EXISTS actions;
DROP INDEX IF EXISTS runtimes_active_per_pane;
DROP TABLE IF EXISTS runtimes;
DROP TABLE IF EXISTS panes;
DROP TABLE IF EXISTS targets;
DROP TABLE IF EXISTS schema_migrations;
`,
	},
	{
		Version: 2,
		UpSQL: `
ALTER TABLE states ADD COLUMN state_source TEXT CHECK(state_source IS NULL OR state_source IN ('hook','notify','wrapper','poller'));
ALTER TABLE states ADD COLUMN last_event_type TEXT;
ALTER TABLE states ADD COLUMN last_event_at TEXT;
`,
		DownSQL: `
-- SQLite deployments may not support DROP COLUMN safely across environments.
-- RollbackAll() remains safe because migration v1 DownSQL drops full tables.
-- Operational rollback for this migration is database recreation + state re-sync.
SELECT 1;
`,
	},
	{
		Version: 3,
		UpSQL: `
ALTER TABLE panes ADD COLUMN current_cmd TEXT NOT NULL DEFAULT '';
`,
		DownSQL: `
-- SQLite deployments may not support DROP COLUMN safely across environments.
-- RollbackAll() remains safe because migration v1 DownSQL drops full tables.
-- Operational rollback for this migration is database recreation + state re-sync.
SELECT 1;
`,
	},
	{
		Version: 4,
		UpSQL: `
ALTER TABLE panes ADD COLUMN pane_title TEXT NOT NULL DEFAULT '';
ALTER TABLE panes ADD COLUMN history_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE panes ADD COLUMN last_activity_at TEXT;
`,
		DownSQL: `
-- SQLite deployments may not support DROP COLUMN safely across environments.
-- RollbackAll() remains safe because migration v1 DownSQL drops full tables.
-- Operational rollback for this migration is database recreation + state re-sync.
SELECT 1;
`,
	},
	{
		Version: 5,
		UpSQL: `
ALTER TABLE panes ADD COLUMN current_path TEXT NOT NULL DEFAULT '';
`,
		DownSQL: `
-- SQLite deployments may not support DROP COLUMN safely across environments.
-- RollbackAll() remains safe because migration v1 DownSQL drops full tables.
-- Operational rollback for this migration is database recreation + state re-sync.
SELECT 1;
`,
	},
}

func ApplyMigrations(ctx context.Context, db *sql.DB) error {
	if _, err := db.ExecContext(ctx, `CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)`); err != nil {
		return fmt.Errorf("create schema_migrations: %w", err)
	}

	for _, m := range migrations {
		var exists int
		err := db.QueryRowContext(ctx, `SELECT 1 FROM schema_migrations WHERE version = ?`, m.Version).Scan(&exists)
		if err == nil {
			continue
		}
		if err != nil && err != sql.ErrNoRows {
			return fmt.Errorf("check migration %d: %w", m.Version, err)
		}

		tx, err := db.BeginTx(ctx, nil)
		if err != nil {
			return fmt.Errorf("begin tx for migration %d: %w", m.Version, err)
		}
		if _, err := tx.ExecContext(ctx, m.UpSQL); err != nil {
			tx.Rollback() //nolint:errcheck
			return fmt.Errorf("apply migration %d: %w", m.Version, err)
		}
		if _, err := tx.ExecContext(ctx, `INSERT INTO schema_migrations(version, applied_at) VALUES (?, datetime('now'))`, m.Version); err != nil {
			tx.Rollback() //nolint:errcheck
			return fmt.Errorf("record migration %d: %w", m.Version, err)
		}
		if err := tx.Commit(); err != nil {
			return fmt.Errorf("commit migration %d: %w", m.Version, err)
		}
	}
	return nil
}

func RollbackAll(ctx context.Context, db *sql.DB) error {
	for i := len(migrations) - 1; i >= 0; i-- {
		m := migrations[i]
		tx, err := db.BeginTx(ctx, nil)
		if err != nil {
			return fmt.Errorf("begin rollback tx %d: %w", m.Version, err)
		}
		if _, err := tx.ExecContext(ctx, m.DownSQL); err != nil {
			tx.Rollback() //nolint:errcheck
			return fmt.Errorf("rollback migration %d: %w", m.Version, err)
		}
		if err := tx.Commit(); err != nil {
			return fmt.Errorf("commit rollback %d: %w", m.Version, err)
		}
	}
	return nil
}
