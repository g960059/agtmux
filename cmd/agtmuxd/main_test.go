package main

import (
	"context"
	"path/filepath"
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/db"
	"github.com/g960059/agtmux/internal/model"
)

func TestSyncAdapterRegistry(t *testing.T) {
	ctx := context.Background()
	store, err := db.Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := db.ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	if err := syncAdapterRegistry(ctx, store); err != nil {
		t.Fatalf("sync adapter registry: %v", err)
	}

	adapters, err := store.ListAdapters(ctx)
	if err != nil {
		t.Fatalf("list adapters: %v", err)
	}
	if len(adapters) < 3 {
		t.Fatalf("expected at least 3 synced adapters, got %d", len(adapters))
	}
}

func TestSyncAdapterRegistryPreservesEnabledFlag(t *testing.T) {
	ctx := context.Background()
	store, err := db.Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := db.ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	if err := store.UpsertAdapter(ctx, model.AdapterRecord{
		AdapterName:  "codex-notify-wrapper",
		AgentType:    "codex",
		Version:      "v1",
		Capabilities: []string{"event_driven"},
		Enabled:      false,
		UpdatedAt:    time.Now().UTC(),
	}); err != nil {
		t.Fatalf("seed adapter: %v", err)
	}

	if err := syncAdapterRegistry(ctx, store); err != nil {
		t.Fatalf("sync adapter registry: %v", err)
	}

	row, err := store.GetAdapterByName(ctx, "codex-notify-wrapper")
	if err != nil {
		t.Fatalf("get synced adapter: %v", err)
	}
	if row.Enabled {
		t.Fatalf("expected enabled flag preserved as false, got %+v", row)
	}
}

func TestEnsureActiveRuntimeUsesAgentClassificationAndPID(t *testing.T) {
	ctx := context.Background()
	store, err := db.Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := db.ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC()
	tg := model.Target{TargetID: "t1", TargetName: "t1", Kind: model.TargetKindLocal, Health: model.TargetHealthOK, UpdatedAt: now}
	if err := store.UpsertTarget(ctx, tg); err != nil {
		t.Fatalf("upsert target: %v", err)
	}

	pid := int64(1001)
	pane := model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		CurrentCmd:  "zsh",
		CurrentPID:  &pid,
		UpdatedAt:   now,
	}
	if err := store.UpsertPane(ctx, pane); err != nil {
		t.Fatalf("upsert pane: %v", err)
	}

	rt1, err := ensureActiveRuntime(ctx, store, tg, pane, agentTypeNone, now)
	if err != nil {
		t.Fatalf("ensure active runtime #1: %v", err)
	}
	if rt1.AgentType != agentTypeNone {
		t.Fatalf("expected agent_type=%s, got %+v", agentTypeNone, rt1)
	}
	if rt1.PID == nil || *rt1.PID != pid {
		t.Fatalf("expected pid=%d, got %+v", pid, rt1.PID)
	}

	rt2, err := ensureActiveRuntime(ctx, store, tg, pane, agentTypeNone, now.Add(2*time.Second))
	if err != nil {
		t.Fatalf("ensure active runtime #2: %v", err)
	}
	if rt2.RuntimeID != rt1.RuntimeID {
		t.Fatalf("expected runtime reuse, rt1=%s rt2=%s", rt1.RuntimeID, rt2.RuntimeID)
	}

	pane.CurrentCmd = "codex"
	rt3, err := ensureActiveRuntime(ctx, store, tg, pane, "codex", now.Add(3*time.Second))
	if err != nil {
		t.Fatalf("ensure active runtime #3: %v", err)
	}
	if rt3.RuntimeID == rt2.RuntimeID {
		t.Fatalf("expected runtime rotation on agent change")
	}
	if rt3.AgentType != "codex" {
		t.Fatalf("expected codex runtime, got %+v", rt3)
	}

	active, err := store.ListActiveRuntimesForPane(ctx, "t1", "%1")
	if err != nil {
		t.Fatalf("list active runtimes: %v", err)
	}
	if len(active) != 1 {
		t.Fatalf("expected exactly one active runtime, got %d", len(active))
	}
	if active[0].AgentType != "codex" {
		t.Fatalf("expected codex active runtime, got %+v", active[0])
	}
}
