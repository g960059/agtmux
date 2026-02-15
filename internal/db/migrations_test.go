package db

import (
	"context"
	"database/sql"
	"path/filepath"
	"testing"
	"time"

	_ "modernc.org/sqlite"
)

func openTempDB(t *testing.T) (*sql.DB, context.Context) {
	t.Helper()
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "test.db")
	db, err := sql.Open("sqlite", "file:"+path+"?_pragma=foreign_keys(1)")
	if err != nil {
		t.Fatalf("open sqlite: %v", err)
	}
	t.Cleanup(func() {
		_ = db.Close()
	})
	return db, ctx
}

func TestApplyAndRollbackMigrations(t *testing.T) {
	db, ctx := openTempDB(t)
	if err := ApplyMigrations(ctx, db); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	mustExist := []string{"targets", "panes", "runtimes", "events", "event_inbox", "runtime_source_cursors", "states", "actions", "action_snapshots", "adapters"}
	for _, table := range mustExist {
		var name string
		if err := db.QueryRowContext(ctx, `SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?`, table).Scan(&name); err != nil {
			t.Fatalf("expected table %s to exist: %v", table, err)
		}
	}

	if err := RollbackAll(ctx, db); err != nil {
		t.Fatalf("rollback migrations: %v", err)
	}

	for _, table := range mustExist {
		var count int
		if err := db.QueryRowContext(ctx, `SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?`, table).Scan(&count); err != nil {
			t.Fatalf("count table %s: %v", table, err)
		}
		if count != 0 {
			t.Fatalf("table %s still exists after rollback", table)
		}
	}
}

func TestCoreConstraints(t *testing.T) {
	db, ctx := openTempDB(t)
	if err := ApplyMigrations(ctx, db); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC().Format(time.RFC3339Nano)
	_, err := db.ExecContext(ctx, `INSERT INTO targets(target_id, target_name, kind, connection_ref, updated_at) VALUES('t1','t1','local','',?)`, now)
	if err != nil {
		t.Fatalf("insert target: %v", err)
	}
	_, err = db.ExecContext(ctx, `INSERT INTO targets(target_id, target_name, kind, connection_ref, updated_at) VALUES('t_bad','t_bad','ssh','ssh://user:pass@vm1',?)`, now)
	if err == nil {
		t.Fatalf("expected connection_ref check constraint failure")
	}
	_, err = db.ExecContext(ctx, `INSERT INTO panes(target_id, pane_id, session_name, window_id, window_name, updated_at) VALUES('t1','%1','s1','@1','w1',?)`, now)
	if err != nil {
		t.Fatalf("insert pane: %v", err)
	}
	_, err = db.ExecContext(ctx, `INSERT INTO events(event_id, runtime_id, event_type, source, event_time, ingested_at, dedupe_key) VALUES('e1','missing-runtime','x','hook',?,?, 'd1')`, now, now)
	if err == nil {
		t.Fatalf("expected FK violation for missing runtime")
	}

	_, err = db.ExecContext(ctx, `INSERT INTO actions(action_id, action_type, request_ref, target_id, pane_id, requested_at, result_code) VALUES('a1','attach','r1','t1','%1',?,'pending')`, now)
	if err != nil {
		t.Fatalf("insert first action: %v", err)
	}
	_, err = db.ExecContext(ctx, `INSERT INTO actions(action_id, action_type, request_ref, target_id, pane_id, requested_at, result_code) VALUES('a2','attach','r1','t1','%1',?,'pending')`, now)
	if err == nil {
		t.Fatalf("expected unique violation on (action_type, request_ref)")
	}
}

func TestActiveRuntimeUniqueness(t *testing.T) {
	db, ctx := openTempDB(t)
	if err := ApplyMigrations(ctx, db); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}
	now := time.Now().UTC().Format(time.RFC3339Nano)
	_, _ = db.ExecContext(ctx, `INSERT INTO targets(target_id, target_name, kind, connection_ref, updated_at) VALUES('t1','t1','local','',?)`, now)
	_, _ = db.ExecContext(ctx, `INSERT INTO panes(target_id, pane_id, session_name, window_id, window_name, updated_at) VALUES('t1','%1','s1','@1','w1',?)`, now)

	_, err := db.ExecContext(ctx, `INSERT INTO runtimes(runtime_id, target_id, pane_id, tmux_server_boot_id, pane_epoch, agent_type, started_at) VALUES('r1','t1','%1','boot',1,'codex',?)`, now)
	if err != nil {
		t.Fatalf("insert first runtime: %v", err)
	}
	_, err = db.ExecContext(ctx, `INSERT INTO runtimes(runtime_id, target_id, pane_id, tmux_server_boot_id, pane_epoch, agent_type, started_at) VALUES('r2','t1','%1','boot',2,'codex',?)`, now)
	if err == nil {
		t.Fatalf("expected unique index violation on active runtime")
	}
}
