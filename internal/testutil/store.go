package testutil

import (
	"context"
	"path/filepath"
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/db"
	"github.com/g960059/agtmux/internal/model"
)

func NewStore(t *testing.T) (*db.Store, context.Context) {
	t.Helper()
	ctx := context.Background()
	store, err := db.Open(ctx, filepath.Join(t.TempDir(), "agtmux-test.db"))
	if err != nil {
		t.Fatalf("open test store: %v", err)
	}
	t.Cleanup(func() {
		_ = store.Close()
	})
	if err := db.ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}
	return store, ctx
}

func SeedTargetPaneRuntime(t *testing.T, store *db.Store, ctx context.Context, targetID, paneID string) model.Runtime {
	return SeedTargetPaneRuntimeWithAgent(t, store, ctx, targetID, paneID, "codex")
}

func SeedTargetPaneRuntimeWithAgent(t *testing.T, store *db.Store, ctx context.Context, targetID, paneID, agentType string) model.Runtime {
	t.Helper()
	if agentType == "" {
		agentType = "unknown"
	}
	now := time.Now().UTC()
	target := model.Target{
		TargetID:      targetID,
		TargetName:    targetID,
		Kind:          model.TargetKindLocal,
		ConnectionRef: "",
		Health:        model.TargetHealthOK,
		UpdatedAt:     now,
	}
	if err := store.UpsertTarget(ctx, target); err != nil {
		t.Fatalf("seed target: %v", err)
	}
	pane := model.Pane{
		TargetID:    targetID,
		PaneID:      paneID,
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}
	if err := store.UpsertPane(ctx, pane); err != nil {
		t.Fatalf("seed pane: %v", err)
	}
	pid := int64(1001)
	runtime := model.Runtime{
		RuntimeID:        "runtime-1",
		TargetID:         targetID,
		PaneID:           paneID,
		TmuxServerBootID: "boot-1",
		PaneEpoch:        1,
		AgentType:        agentType,
		PID:              &pid,
		StartedAt:        now,
	}
	if err := store.InsertRuntime(ctx, runtime); err != nil {
		t.Fatalf("seed runtime: %v", err)
	}
	return runtime
}
