package db

import (
	"context"
	"database/sql"
	"path/filepath"
	"strings"
	"testing"
	"time"

	_ "modernc.org/sqlite"
)

func TestIndexBaselineUtility(t *testing.T) {
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "idx.db")
	db, err := sql.Open("sqlite", "file:"+path+"?_pragma=foreign_keys(1)")
	if err != nil {
		t.Fatalf("open sqlite: %v", err)
	}
	defer db.Close() //nolint:errcheck

	if err := ApplyMigrations(ctx, db); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}
	now := time.Now().UTC().Format(time.RFC3339Nano)
	_, _ = db.ExecContext(ctx, `INSERT INTO targets(target_id, target_name, kind, connection_ref, updated_at) VALUES('t1','t1','local','',?)`, now)
	_, _ = db.ExecContext(ctx, `INSERT INTO panes(target_id, pane_id, session_name, window_id, window_name, updated_at) VALUES('t1','%1','s1','@1','w1',?)`, now)
	_, _ = db.ExecContext(ctx, `INSERT INTO runtimes(runtime_id,target_id,pane_id,tmux_server_boot_id,pane_epoch,agent_type,started_at) VALUES('r1','t1','%1','b1',1,'codex',?)`, now)
	_, _ = db.ExecContext(ctx, `INSERT INTO states(target_id,pane_id,runtime_id,state,confidence,state_version,last_seen_at,updated_at) VALUES('t1','%1','r1','running','high',1,?,?)`, now, now)
	_, _ = db.ExecContext(ctx, `INSERT INTO events(event_id,runtime_id,event_type,source,event_time,ingested_at,dedupe_key) VALUES('e1','r1','running','hook',?,?, 'd1')`, now, now)

	assertPlanUsesIndex(t, db, `EXPLAIN QUERY PLAN SELECT * FROM states WHERE state='running' ORDER BY updated_at DESC LIMIT 10`, "states_state_updated_at")
	assertPlanUsesIndex(t, db, `EXPLAIN QUERY PLAN SELECT * FROM events WHERE runtime_id='r1' AND source='hook' ORDER BY ingested_at DESC LIMIT 10`, "events_runtime_source_ingested_at")
}

func assertPlanUsesIndex(t *testing.T, db *sql.DB, query, expectedIndex string) {
	t.Helper()
	rows, err := db.Query(query)
	if err != nil {
		t.Fatalf("query plan failed: %v", err)
	}
	defer rows.Close()
	var matched bool
	for rows.Next() {
		var id, parent, notused int
		var detail string
		if err := rows.Scan(&id, &parent, &notused, &detail); err != nil {
			t.Fatalf("scan plan row: %v", err)
		}
		if strings.Contains(detail, expectedIndex) {
			matched = true
		}
	}
	if err := rows.Err(); err != nil {
		t.Fatalf("plan rows error: %v", err)
	}
	if !matched {
		t.Fatalf("expected query plan to use index %q", expectedIndex)
	}
}
