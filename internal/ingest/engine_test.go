package ingest

import (
	"errors"
	"fmt"
	"math/rand"
	"strings"
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/adapter"
	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/db"
	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/testutil"
)

type mockNormalizeAdapter struct{}

func (mockNormalizeAdapter) Definition() adapter.Definition {
	return adapter.Definition{
		Name:            "mock-normalize",
		AgentType:       "mock",
		ContractVersion: "v1",
	}
}

func (mockNormalizeAdapter) Normalize(signal adapter.Signal) (adapter.NormalizedState, bool) {
	if signal.EventType != "adapter-only-signal" {
		return adapter.NormalizedState{}, false
	}
	return adapter.NormalizedState{
		State:      model.StateWaitingApproval,
		Reason:     "adapter_signal",
		Confidence: "high",
	}, true
}

func TestStaleRuntimeGuard(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := NewEngine(store, cfg)

	// Seed pane/target only, no runtime named missing-runtime.
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")
	_ = rt

	err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "e1",
		EventType:  "running",
		Source:     model.SourceNotify,
		DedupeKey:  "d1",
		RuntimeID:  "missing-runtime",
		TargetID:   "host",
		PaneID:     "%1",
		EventTime:  time.Now().Add(-time.Second),
		IngestedAt: time.Now(),
	})
	if err == nil {
		t.Fatalf("expected stale runtime rejection")
	}
	if !strings.Contains(err.Error(), model.ErrRuntimeStale) {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestEndedRuntimeRejectedAsStale(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")
	if err := store.EndRuntime(ctx, rt.RuntimeID, time.Now().UTC()); err != nil {
		t.Fatalf("end runtime: %v", err)
	}

	err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "e-ended",
		EventType:  "running",
		Source:     model.SourceNotify,
		DedupeKey:  "d-ended",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  time.Now().Add(-time.Second),
		IngestedAt: time.Now(),
	})
	if err == nil {
		t.Fatalf("expected stale runtime rejection for ended runtime")
	}
	if !strings.Contains(err.Error(), model.ErrRuntimeStale) {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestOrderingDeterminismAcrossShuffles(t *testing.T) {
	seed := int64(42)
	rng := rand.New(rand.NewSource(seed))
	hashByRun := ""

	for run := 0; run < 20; run++ {
		store, ctx := testutil.NewStore(t)
		cfg := config.DefaultConfig()
		engine := NewEngine(store, cfg)
		rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")

		base := time.Now().UTC()
		events := []model.EventEnvelope{
			{EventID: "e1", EventType: "running", Source: model.SourceNotify, DedupeKey: "d1", RuntimeID: rt.RuntimeID, EventTime: base.Add(1 * time.Second), IngestedAt: base.Add(1 * time.Second), SourceSeq: ptrI64(1)},
			{EventID: "e2", EventType: "running", Source: model.SourceNotify, DedupeKey: "d2", RuntimeID: rt.RuntimeID, EventTime: base.Add(2 * time.Second), IngestedAt: base.Add(2 * time.Second), SourceSeq: ptrI64(2)},
			{EventID: "e3", EventType: "completed", Source: model.SourceNotify, DedupeKey: "d3", RuntimeID: rt.RuntimeID, EventTime: base.Add(3 * time.Second), IngestedAt: base.Add(3 * time.Second), SourceSeq: ptrI64(3)},
		}
		rng.Shuffle(len(events), func(i, j int) { events[i], events[j] = events[j], events[i] })

		for _, ev := range events {
			err := engine.Ingest(ctx, ev)
			if err != nil && !errors.Is(err, db.ErrOutOfOrder) {
				t.Fatalf("ingest event %s: %v", ev.EventID, err)
			}
		}
		st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
		if err != nil {
			t.Fatalf("get state: %v", err)
		}
		hash := string(st.State) + ":" + st.ReasonCode + ":" + st.RuntimeID
		if hashByRun == "" {
			hashByRun = hash
		}
		if hash != hashByRun {
			t.Fatalf("non deterministic final state: got=%s want=%s", hash, hashByRun)
		}
	}
}

func TestTargetDownShortCircuitsToUnknown(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")

	if err := store.UpsertTarget(ctx, model.Target{
		TargetID:      rt.TargetID,
		TargetName:    rt.TargetID,
		Kind:          model.TargetKindLocal,
		ConnectionRef: "",
		Health:        model.TargetHealthDown,
		UpdatedAt:     time.Now().UTC(),
	}); err != nil {
		t.Fatalf("set target down: %v", err)
	}

	if err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "down-short-circuit",
		EventType:  "running",
		Source:     model.SourceNotify,
		DedupeKey:  "down-short-circuit",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  time.Now().UTC(),
		IngestedAt: time.Now().UTC(),
	}); err != nil {
		t.Fatalf("ingest event: %v", err)
	}

	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.State != model.StateUnknown || st.ReasonCode != "target_unreachable" {
		t.Fatalf("expected unknown/target_unreachable, got %s/%s", st.State, st.ReasonCode)
	}
}

func TestAdapterNormalizationOverridesFallbackWhenHandled(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	reg := adapter.NewRegistry(mockNormalizeAdapter{})
	engine := NewEngineWithRegistry(store, cfg, reg)
	rt := testutil.SeedTargetPaneRuntimeWithAgent(t, store, ctx, "host", "%1", "mock")

	if err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "adapter-override",
		EventType:  "adapter-only-signal",
		Source:     model.SourceNotify,
		DedupeKey:  "adapter-override",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  time.Now().UTC(),
		IngestedAt: time.Now().UTC(),
	}); err != nil {
		t.Fatalf("ingest event: %v", err)
	}

	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.State != model.StateWaitingApproval || st.ReasonCode != "adapter_signal" || st.Confidence != "high" {
		t.Fatalf("expected adapter-driven state, got %+v", st)
	}
}

func TestAdapterNormalizationFallsBackWhenNotHandled(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	reg := adapter.NewRegistry(mockNormalizeAdapter{})
	engine := NewEngineWithRegistry(store, cfg, reg)
	rt := testutil.SeedTargetPaneRuntimeWithAgent(t, store, ctx, "host", "%1", "mock")

	if err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "adapter-fallback",
		EventType:  "running",
		Source:     model.SourceNotify,
		DedupeKey:  "adapter-fallback",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  time.Now().UTC(),
		IngestedAt: time.Now().UTC(),
	}); err != nil {
		t.Fatalf("ingest event: %v", err)
	}

	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.State != model.StateRunning || st.ReasonCode != "active" {
		t.Fatalf("expected fallback running/active state, got %+v", st)
	}
}

func TestIngestPersistsStateProvenanceFields(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")
	eventTime := time.Now().UTC().Add(-2 * time.Second)
	ingestedAt := eventTime.Add(500 * time.Millisecond)

	if err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "provenance-event",
		EventType:  "input_requested",
		Source:     model.SourceNotify,
		DedupeKey:  "provenance-event",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  eventTime,
		IngestedAt: ingestedAt,
	}); err != nil {
		t.Fatalf("ingest event: %v", err)
	}

	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.StateSource != model.SourceNotify {
		t.Fatalf("expected state_source=notify, got %s", st.StateSource)
	}
	if st.LastEventType != "input-requested" {
		t.Fatalf("expected normalized last_event_type=input-requested, got %s", st.LastEventType)
	}
	if st.LastEventAt == nil || !st.LastEventAt.Equal(eventTime) {
		t.Fatalf("expected last_event_at=%s, got %+v", eventTime, st.LastEventAt)
	}
}

func TestPollerDoesNotOverrideFreshEventDrivenState(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	cfg.StaleSignalTTL = 3 * time.Second
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")
	base := time.Now().UTC()

	if err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "ev-notify-waiting",
		EventType:  "input-requested",
		Source:     model.SourceNotify,
		DedupeKey:  "ev-notify-waiting",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  base,
		IngestedAt: base,
	}); err != nil {
		t.Fatalf("ingest notify event: %v", err)
	}

	if err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "ev-poller-running-fresh",
		EventType:  "running",
		Source:     model.SourcePoller,
		DedupeKey:  "ev-poller-running-fresh",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  base.Add(2 * time.Second),
		IngestedAt: base.Add(2 * time.Second),
	}); err != nil {
		t.Fatalf("ingest fresh poller event: %v", err)
	}

	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state after fresh poller: %v", err)
	}
	if st.State != model.StateWaitingInput || st.StateSource != model.SourceNotify {
		t.Fatalf("fresh poller should not override notify state, got %+v", st)
	}

	if err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "ev-poller-running-stale",
		EventType:  "running",
		Source:     model.SourcePoller,
		DedupeKey:  "ev-poller-running-stale",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  base.Add(5 * time.Second),
		IngestedAt: base.Add(5 * time.Second),
	}); err != nil {
		t.Fatalf("ingest stale poller event: %v", err)
	}

	st, err = store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state after stale poller: %v", err)
	}
	if st.State != model.StateRunning || st.StateSource != model.SourcePoller {
		t.Fatalf("stale poller should be allowed after ttl, got %+v", st)
	}
}

func TestIngestDuplicateRetryIsIdempotentWithoutStableEventID(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")
	base := time.Now().UTC()

	first := model.EventEnvelope{
		EventID:    "ev-dup-first",
		EventType:  "agent-turn-complete",
		Source:     model.SourceNotify,
		DedupeKey:  "dup-retry-1",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  base,
		IngestedAt: base,
	}
	if err := engine.Ingest(ctx, first); err != nil {
		t.Fatalf("ingest first: %v", err)
	}

	retry := model.EventEnvelope{
		EventID:    "ev-dup-retry",
		EventType:  "agent-turn-complete",
		Source:     model.SourceNotify,
		DedupeKey:  "dup-retry-1",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  base.Add(3 * time.Second),
		IngestedAt: base.Add(4 * time.Second),
	}
	if err := engine.Ingest(ctx, retry); err != nil {
		t.Fatalf("ingest retry should be idempotent, got %v", err)
	}

	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.State != model.StateCompleted {
		t.Fatalf("expected completed after idempotent retry, got %+v", st)
	}
}

func TestIngestDuplicateReplayRepairsMissingCursorAndState(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")
	base := time.Now().UTC()

	stored := model.EventEnvelope{
		EventID:    "ev-repair-stored",
		EventType:  "running",
		Source:     model.SourceNotify,
		DedupeKey:  "dup-repair-1",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  base,
		IngestedAt: base,
	}
	if err := store.InsertEvent(ctx, stored, ""); err != nil {
		t.Fatalf("insert stored event: %v", err)
	}

	replay := model.EventEnvelope{
		EventID:    "ev-repair-replay",
		EventType:  "running",
		Source:     model.SourceNotify,
		DedupeKey:  "dup-repair-1",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  base.Add(2 * time.Second),
		IngestedAt: base.Add(2 * time.Second),
	}
	if err := engine.Ingest(ctx, replay); err != nil {
		t.Fatalf("replay ingest should repair cursor/state, got %v", err)
	}

	cursor, err := store.GetSourceCursor(ctx, rt.RuntimeID, model.SourceNotify)
	if err != nil {
		t.Fatalf("get source cursor: %v", err)
	}
	if cursor.LastOrderEventID != "ev-repair-stored" {
		t.Fatalf("expected cursor from stored event, got %+v", cursor)
	}
	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.State != model.StateRunning || st.StateSource != model.SourceNotify {
		t.Fatalf("expected repaired running state, got %+v", st)
	}
}

func TestIngestDuplicateReplayPreservesPayloadDerivedState(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntimeWithAgent(t, store, ctx, "host", "%1", "codex")
	base := time.Now().UTC()

	stored := model.EventEnvelope{
		EventID:    "ev-replay-payload-stored",
		EventType:  "agent-turn-complete",
		Source:     model.SourceNotify,
		DedupeKey:  "dup-replay-payload-1",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  base,
		IngestedAt: base,
	}
	if err := store.InsertEvent(ctx, stored, ""); err != nil {
		t.Fatalf("insert stored event: %v", err)
	}

	replay := model.EventEnvelope{
		EventID:    "ev-replay-payload-retry",
		EventType:  "agent-turn-complete",
		Source:     model.SourceNotify,
		DedupeKey:  "dup-replay-payload-1",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  base.Add(2 * time.Second),
		IngestedAt: base.Add(2 * time.Second),
		RawPayload: `{"type":"approval-requested"}`,
	}
	if err := engine.Ingest(ctx, replay); err != nil {
		t.Fatalf("replay ingest should use retry payload for state, got %v", err)
	}

	cursor, err := store.GetSourceCursor(ctx, rt.RuntimeID, model.SourceNotify)
	if err != nil {
		t.Fatalf("get source cursor: %v", err)
	}
	if cursor.LastOrderEventID != stored.EventID {
		t.Fatalf("expected cursor from stored event ordering, got %+v", cursor)
	}

	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.State != model.StateWaitingApproval || st.ReasonCode != "approval_requested" {
		t.Fatalf("expected payload-derived waiting approval state, got %+v", st)
	}
}

func TestIngestPartialFailureThenReplayPayloadDependentClassification(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntimeWithAgent(t, store, ctx, "host", "%1", "codex")
	base := time.Now().UTC()

	stored := model.EventEnvelope{
		EventID:    "ev-partial-payload-stored",
		EventType:  "agent-turn-complete",
		Source:     model.SourceNotify,
		DedupeKey:  "dup-partial-payload-1",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  base,
		IngestedAt: base,
	}
	if err := store.InsertEvent(ctx, stored, ""); err != nil {
		t.Fatalf("insert stored event: %v", err)
	}
	storedKey := BuildOrderKey(stored, cfg.SkewBudget)
	if err := store.UpsertSourceCursor(ctx, db.SourceCursor{
		RuntimeID:           rt.RuntimeID,
		Source:              model.SourceNotify,
		LastSourceSeq:       stored.SourceSeq,
		LastOrderEventTime:  storedKey.EventTime,
		LastOrderIngestedAt: storedKey.IngestedAt,
		LastOrderEventID:    storedKey.EventID,
	}); err != nil {
		t.Fatalf("upsert source cursor: %v", err)
	}

	replay := model.EventEnvelope{
		EventID:    "ev-partial-payload-retry",
		EventType:  "agent-turn-complete",
		Source:     model.SourceNotify,
		DedupeKey:  "dup-partial-payload-1",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  base.Add(3 * time.Second),
		IngestedAt: base.Add(3 * time.Second),
		RawPayload: `{"type":"input-requested"}`,
	}
	if err := engine.Ingest(ctx, replay); err != nil {
		t.Fatalf("replay ingest after partial failure should classify from retry payload, got %v", err)
	}

	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.State != model.StateWaitingInput || st.ReasonCode != "input_required" {
		t.Fatalf("expected payload-derived waiting input state after replay, got %+v", st)
	}
}

func TestPollerSuppressionFutureEventTimeIsClamped(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	cfg.StaleSignalTTL = 2 * time.Second
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")
	base := time.Now().UTC()

	if err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "ev-future-notify",
		EventType:  "input-requested",
		Source:     model.SourceNotify,
		DedupeKey:  "ev-future-notify",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  base.Add(24 * time.Hour),
		IngestedAt: base,
	}); err != nil {
		t.Fatalf("ingest future notify: %v", err)
	}

	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state after notify: %v", err)
	}
	if st.LastEventAt == nil || st.LastEventAt.After(base.Add(10*time.Second)) {
		t.Fatalf("expected clamped last_event_at around ingested time, got %+v", st.LastEventAt)
	}

	if err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "ev-future-poller",
		EventType:  "running",
		Source:     model.SourcePoller,
		DedupeKey:  "ev-future-poller",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  base.Add(3 * time.Second),
		IngestedAt: base.Add(3 * time.Second),
	}); err != nil {
		t.Fatalf("ingest poller after ttl: %v", err)
	}

	st, err = store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state after poller: %v", err)
	}
	if st.State != model.StateRunning || st.StateSource != model.SourcePoller {
		t.Fatalf("expected poller state after ttl despite future event_time, got %+v", st)
	}
}

func TestDisabledAdapterSkipsNormalization(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntimeWithAgent(t, store, ctx, "host", "%1", "codex")
	now := time.Now().UTC()
	if err := store.UpsertAdapter(ctx, model.AdapterRecord{
		AdapterName:  "codex-notify-wrapper",
		AgentType:    "codex",
		Version:      "v1",
		Capabilities: []string{"event_driven"},
		Enabled:      false,
		UpdatedAt:    now,
	}); err != nil {
		t.Fatalf("upsert adapter: %v", err)
	}

	if err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "disabled-adapter-event",
		EventType:  "running",
		Source:     model.SourceNotify,
		DedupeKey:  "disabled-adapter-event",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  now,
		IngestedAt: now,
	}); err != nil {
		t.Fatalf("ingest event: %v", err)
	}
	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.State != model.StateUnknown || st.ReasonCode != "unsupported_signal" {
		t.Fatalf("expected unknown/unsupported_signal for disabled adapter, got %+v", st)
	}
}

func TestIncompatibleAdapterVersionSkipsNormalization(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntimeWithAgent(t, store, ctx, "host", "%1", "codex")
	now := time.Now().UTC()
	if err := store.UpsertAdapter(ctx, model.AdapterRecord{
		AdapterName:  "codex-notify-wrapper",
		AgentType:    "codex",
		Version:      "v2",
		Capabilities: []string{"event_driven"},
		Enabled:      true,
		UpdatedAt:    now,
	}); err != nil {
		t.Fatalf("upsert adapter: %v", err)
	}

	if err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "incompatible-adapter-event",
		EventType:  "running",
		Source:     model.SourceNotify,
		DedupeKey:  "incompatible-adapter-event",
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  now,
		IngestedAt: now,
	}); err != nil {
		t.Fatalf("ingest event: %v", err)
	}
	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.State != model.StateUnknown || st.ReasonCode != "unsupported_signal" {
		t.Fatalf("expected unknown/unsupported_signal for incompatible adapter, got %+v", st)
	}
}

func TestDedupeBehavior(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")

	ev := model.EventEnvelope{
		EventID:    "e-dup-1",
		EventType:  "waiting_input",
		Source:     model.SourceHook,
		DedupeKey:  "dup-key",
		RuntimeID:  rt.RuntimeID,
		EventTime:  time.Now().UTC(),
		IngestedAt: time.Now().UTC(),
	}
	if err := engine.Ingest(ctx, ev); err != nil {
		t.Fatalf("first ingest: %v", err)
	}
	if err := engine.Ingest(ctx, ev); err != nil {
		t.Fatalf("second ingest should be deduped: %v", err)
	}

	count, err := store.CountRows(ctx, "events")
	if err != nil {
		t.Fatalf("count events: %v", err)
	}
	if count != 1 {
		t.Fatalf("expected one event row, got %d", count)
	}
}

func TestDuplicateConflictRejected(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")

	ev1 := model.EventEnvelope{
		EventID:    "dup-conflict-1",
		EventType:  "running",
		Source:     model.SourceNotify,
		DedupeKey:  "dup-conflict-key",
		RuntimeID:  rt.RuntimeID,
		SourceSeq:  ptrI64(1),
		EventTime:  time.Now().UTC(),
		IngestedAt: time.Now().UTC(),
	}
	if err := engine.Ingest(ctx, ev1); err != nil {
		t.Fatalf("first ingest: %v", err)
	}

	ev2 := ev1
	ev2.EventID = "dup-conflict-2"
	ev2.SourceSeq = ptrI64(2)
	ev2.EventType = "waiting_input"
	err := engine.Ingest(ctx, ev2)
	if err == nil {
		t.Fatalf("expected duplicate conflict")
	}
	if !strings.Contains(err.Error(), model.ErrIdempotencyConflict) {
		t.Fatalf("expected idempotency conflict, got %v", err)
	}
}

func TestDuplicateStormConvergence(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")

	base := time.Now().UTC()
	for i := 0; i < 200; i++ {
		seq := int64((i % 5) + 1)
		ev := model.EventEnvelope{
			EventID:    fmt.Sprintf("storm-event-%d", i),
			EventType:  []string{"running", "running", "waiting_input", "running", "completed"}[i%5],
			Source:     model.SourceNotify,
			DedupeKey:  []string{"d1", "d2", "d3", "d4", "d5"}[i%5],
			RuntimeID:  rt.RuntimeID,
			SourceSeq:  &seq,
			EventTime:  base.Add(time.Duration(seq) * time.Second),
			IngestedAt: base.Add(time.Duration(i%7) * time.Millisecond),
		}
		err := engine.Ingest(ctx, ev)
		if err == nil {
			continue
		}
		if errors.Is(err, db.ErrOutOfOrder) || strings.Contains(err.Error(), model.ErrIdempotencyConflict) {
			continue
		}
		t.Fatalf("ingest storm event %d: %v", i, err)
	}
	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.State != model.StateCompleted {
		t.Fatalf("expected converged completed state, got %s", st.State)
	}
	count, err := store.CountRows(ctx, "events")
	if err != nil {
		t.Fatalf("count events: %v", err)
	}
	if count > 5 {
		t.Fatalf("expected deduped event rows <= 5, got %d", count)
	}
}

func TestReconcileGuardDropsStaleVersionEvent(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")
	now := time.Now().UTC()

	if err := store.UpsertState(ctx, model.StateRow{
		TargetID:     rt.TargetID,
		PaneID:       rt.PaneID,
		RuntimeID:    rt.RuntimeID,
		State:        model.StateRunning,
		ReasonCode:   "active",
		Confidence:   "high",
		StateVersion: 2,
		LastSeenAt:   now,
		UpdatedAt:    now,
	}); err != nil {
		t.Fatalf("seed state: %v", err)
	}

	err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "reconcile-stale-v",
		EventType:  string(model.ReconcileDemotionDue),
		Source:     model.SourcePoller,
		DedupeKey:  fmt.Sprintf("reconcile:%s:%s:%s:state-v1", model.ReconcileDemotionDue, rt.RuntimeID, rt.PaneID),
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  now.Add(3 * time.Second),
		IngestedAt: now.Add(3 * time.Second),
	})
	if err != nil {
		t.Fatalf("ingest reconcile event: %v", err)
	}

	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.StateVersion != 2 || st.State != model.StateRunning {
		t.Fatalf("expected stale reconcile event ignored, got version=%d state=%s", st.StateVersion, st.State)
	}
}

func TestReconcileGuardDropsRuntimeMismatch(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")
	now := time.Now().UTC()

	if err := store.UpsertState(ctx, model.StateRow{
		TargetID:     rt.TargetID,
		PaneID:       rt.PaneID,
		RuntimeID:    rt.RuntimeID,
		State:        model.StateRunning,
		ReasonCode:   "active",
		Confidence:   "high",
		StateVersion: 4,
		LastSeenAt:   now,
		UpdatedAt:    now,
	}); err != nil {
		t.Fatalf("seed state: %v", err)
	}

	err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "reconcile-stale-runtime",
		EventType:  string(model.ReconcileTargetHealthChange),
		Source:     model.SourcePoller,
		DedupeKey:  fmt.Sprintf("reconcile:%s:%s:%s:state-v4", model.ReconcileTargetHealthChange, "other-runtime", rt.PaneID),
		RuntimeID:  rt.RuntimeID,
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  now.Add(3 * time.Second),
		IngestedAt: now.Add(3 * time.Second),
	})
	if err != nil {
		t.Fatalf("ingest reconcile runtime mismatch event: %v", err)
	}

	st, err := store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.StateVersion != 4 || st.State != model.StateRunning {
		t.Fatalf("expected runtime mismatch reconcile event ignored, got version=%d state=%s", st.StateVersion, st.State)
	}
}

func ptrI64(v int64) *int64 {
	return &v
}
