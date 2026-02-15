package inbox

import (
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/ingest"
	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/testutil"
)

func TestPendingBindResolvesOnSafeCandidate(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := ingest.NewEngine(store, cfg)
	resolver := NewResolver(store, engine, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")

	startHint := rt.StartedAt
	pid := *rt.PID
	if err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "inbox-1",
		EventType:  "waiting_input",
		Source:     model.SourceNotify,
		DedupeKey:  "inbox-key-1",
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		PID:        &pid,
		StartHint:  &startHint,
		EventTime:  time.Now().UTC(),
		IngestedAt: time.Now().UTC(),
	}); err != nil {
		t.Fatalf("ingest pending event: %v", err)
	}

	if err := resolver.Resolve(ctx, time.Now().UTC()); err != nil {
		t.Fatalf("resolve pending: %v", err)
	}

	var status, runtimeID string
	if err := store.DB().QueryRowContext(ctx, `SELECT status, COALESCE(runtime_id, '') FROM event_inbox WHERE dedupe_key = 'inbox-key-1'`).Scan(&status, &runtimeID); err != nil {
		t.Fatalf("query inbox status: %v", err)
	}
	if status != string(model.InboxBound) {
		t.Fatalf("expected bound, got %s", status)
	}
	if runtimeID != rt.RuntimeID {
		t.Fatalf("expected runtime_id %s, got %s", rt.RuntimeID, runtimeID)
	}
}

func TestPendingBindDropsWhenNoCandidate(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := ingest.NewEngine(store, cfg)
	resolver := NewResolver(store, engine, cfg)

	// Target/pane exist but no active runtime.
	_ = testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")
	if _, err := store.DB().ExecContext(ctx, `DELETE FROM runtimes WHERE target_id='host' AND pane_id='%1'`); err != nil {
		t.Fatalf("delete seeded runtime: %v", err)
	}

	if err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "inbox-2",
		EventType:  "running",
		Source:     model.SourceHook,
		DedupeKey:  "inbox-key-2",
		TargetID:   "host",
		PaneID:     "%1",
		EventTime:  time.Now().UTC(),
		IngestedAt: time.Now().UTC(),
	}); err != nil {
		t.Fatalf("ingest pending event: %v", err)
	}

	if err := resolver.Resolve(ctx, time.Now().UTC()); err != nil {
		t.Fatalf("resolve pending: %v", err)
	}

	var status, reason string
	if err := store.DB().QueryRowContext(ctx, `SELECT status, COALESCE(reason_code,'') FROM event_inbox WHERE dedupe_key = 'inbox-key-2'`).Scan(&status, &reason); err != nil {
		t.Fatalf("query inbox status: %v", err)
	}
	if status != string(model.InboxDroppedUnbound) || reason != "bind_no_candidate" {
		t.Fatalf("expected dropped_unbound/bind_no_candidate, got %s/%s", status, reason)
	}
}

func TestPendingBindDropsOnTTLExpired(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	cfg.PendingBindTTL = 10 * time.Millisecond
	engine := ingest.NewEngine(store, cfg)
	resolver := NewResolver(store, engine, cfg)
	_ = testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")
	if _, err := store.DB().ExecContext(ctx, `DELETE FROM runtimes WHERE target_id='host' AND pane_id='%1'`); err != nil {
		t.Fatalf("delete seeded runtime: %v", err)
	}
	now := time.Now().UTC()

	if err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "inbox-3",
		EventType:  "running",
		Source:     model.SourceHook,
		DedupeKey:  "inbox-key-3",
		TargetID:   "host",
		PaneID:     "%1",
		EventTime:  now,
		IngestedAt: now,
	}); err != nil {
		t.Fatalf("ingest pending event: %v", err)
	}

	time.Sleep(15 * time.Millisecond)
	if err := resolver.Resolve(ctx, time.Now().UTC()); err != nil {
		t.Fatalf("resolve pending: %v", err)
	}

	var status, reason string
	if err := store.DB().QueryRowContext(ctx, `SELECT status, COALESCE(reason_code,'') FROM event_inbox WHERE dedupe_key = 'inbox-key-3'`).Scan(&status, &reason); err != nil {
		t.Fatalf("query inbox status: %v", err)
	}
	if status != string(model.InboxDroppedUnbound) || reason != "bind_ttl_expired" {
		t.Fatalf("expected dropped_unbound/bind_ttl_expired, got %s/%s", status, reason)
	}
}
