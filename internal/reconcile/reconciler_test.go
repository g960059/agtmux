package reconcile

import (
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/ingest"
	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/testutil"
)

func TestUnknownSafeConvergenceOnTargetDown(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := ingest.NewEngine(store, cfg)
	r := NewReconciler(store, engine, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")

	now := time.Now().UTC()
	if err := store.UpsertState(ctx, model.StateRow{
		TargetID:     rt.TargetID,
		PaneID:       rt.PaneID,
		RuntimeID:    rt.RuntimeID,
		State:        model.StateRunning,
		ReasonCode:   "active",
		Confidence:   "high",
		StateVersion: 1,
		LastSeenAt:   now,
		UpdatedAt:    now,
	}); err != nil {
		t.Fatalf("seed state: %v", err)
	}

	targetDown := model.Target{
		TargetID:      rt.TargetID,
		TargetName:    rt.TargetID,
		Kind:          model.TargetKindLocal,
		ConnectionRef: "",
		Health:        model.TargetHealthDown,
		UpdatedAt:     now,
	}
	if err := store.UpsertTarget(ctx, targetDown); err != nil {
		t.Fatalf("set target down: %v", err)
	}

	if err := r.Tick(ctx, now.Add(2*time.Second)); err != nil {
		t.Fatalf("reconcile tick: %v", err)
	}

	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.State != model.StateUnknown || st.ReasonCode != "target_unreachable" {
		t.Fatalf("expected unknown/target_unreachable, got %s/%s", st.State, st.ReasonCode)
	}
}

func TestUnknownSafeConvergenceOnStaleSignals(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	cfg.StaleSignalTTL = 100 * time.Millisecond
	engine := ingest.NewEngine(store, cfg)
	r := NewReconciler(store, engine, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")

	now := time.Now().UTC()
	if err := store.UpsertState(ctx, model.StateRow{
		TargetID:     rt.TargetID,
		PaneID:       rt.PaneID,
		RuntimeID:    rt.RuntimeID,
		State:        model.StateRunning,
		ReasonCode:   "active",
		Confidence:   "high",
		StateVersion: 1,
		LastSeenAt:   now.Add(-2 * time.Second),
		UpdatedAt:    now.Add(-2 * time.Second),
	}); err != nil {
		t.Fatalf("seed state: %v", err)
	}

	if err := r.Tick(ctx, now); err != nil {
		t.Fatalf("reconcile tick: %v", err)
	}
	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.State != model.StateUnknown || st.ReasonCode != "stale_signal" {
		t.Fatalf("expected unknown/stale_signal, got %s/%s", st.State, st.ReasonCode)
	}
}

func TestDownTransitionCanReapplyAfterRepromotion(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := ingest.NewEngine(store, cfg)
	r := NewReconciler(store, engine, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")

	now := time.Now().UTC()
	if err := store.UpsertTarget(ctx, model.Target{
		TargetID:      rt.TargetID,
		TargetName:    rt.TargetID,
		Kind:          model.TargetKindLocal,
		ConnectionRef: "",
		Health:        model.TargetHealthDown,
		UpdatedAt:     now,
	}); err != nil {
		t.Fatalf("set target down: %v", err)
	}
	if err := store.UpsertState(ctx, model.StateRow{
		TargetID:     rt.TargetID,
		PaneID:       rt.PaneID,
		RuntimeID:    rt.RuntimeID,
		State:        model.StateRunning,
		ReasonCode:   "active",
		Confidence:   "high",
		StateVersion: 1,
		LastSeenAt:   now,
		UpdatedAt:    now,
	}); err != nil {
		t.Fatalf("seed state: %v", err)
	}

	if err := r.Tick(ctx, now.Add(1*time.Second)); err != nil {
		t.Fatalf("first reconcile tick: %v", err)
	}
	// Simulate a temporary promotion from another source while target remains down.
	if err := store.UpsertState(ctx, model.StateRow{
		TargetID:     rt.TargetID,
		PaneID:       rt.PaneID,
		RuntimeID:    rt.RuntimeID,
		State:        model.StateRunning,
		ReasonCode:   "active",
		Confidence:   "high",
		StateVersion: 3,
		LastSeenAt:   now.Add(2 * time.Second),
		UpdatedAt:    now.Add(2 * time.Second),
	}); err != nil {
		t.Fatalf("simulate repromotion: %v", err)
	}

	if err := r.Tick(ctx, now.Add(3*time.Second)); err != nil {
		t.Fatalf("second reconcile tick: %v", err)
	}

	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.State != model.StateUnknown || st.ReasonCode != "target_unreachable" {
		t.Fatalf("expected unknown/target_unreachable after reapply, got %s/%s", st.State, st.ReasonCode)
	}
}
