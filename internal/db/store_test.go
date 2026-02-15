package db

import (
	"context"
	"path/filepath"
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/model"
)

func TestUpsertTargetRejectsSecretConnectionRef(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	err = store.UpsertTarget(ctx, TargetForTest("t1", "ssh://user:pass@vm1", time.Now().UTC()))
	if err == nil {
		t.Fatalf("expected connection_ref secret rejection")
	}

	err = store.UpsertTarget(ctx, TargetForTest("t2", "vm1", time.Now().UTC()))
	if err != nil {
		t.Fatalf("expected alias connection_ref accepted: %v", err)
	}

	invalidRefs := []string{
		"ssh://user:pass@vm1",
		"user@vm1",
		"api_key=abc",
		"/home/me/.ssh/id_ed25519",
		"with space",
	}
	invalidIDs := []string{"bad-a", "bad-b", "bad-c", "bad-d", "bad-e"}
	for i, invalid := range invalidRefs {
		err = store.UpsertTarget(ctx, TargetForTest(invalidIDs[i], invalid, time.Now().UTC()))
		if err == nil {
			t.Fatalf("expected invalid connection_ref to be rejected: %q", invalid)
		}
	}
}

func TargetForTest(id, connectionRef string, now time.Time) model.Target {
	return model.Target{
		TargetID:      id,
		TargetName:    id,
		Kind:          model.TargetKindSSH,
		ConnectionRef: connectionRef,
		Health:        model.TargetHealthOK,
		UpdatedAt:     now,
	}
}

func ptrTime(t time.Time) *time.Time {
	v := t
	return &v
}

func TestUpsertAndListAdapters(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC()
	err = store.UpsertAdapter(ctx, model.AdapterRecord{
		AdapterName:  "codex-notify-wrapper",
		AgentType:    "Codex",
		Version:      "v1",
		Capabilities: []string{"supports_completed", "event_driven", "supports_completed", ""},
		Enabled:      true,
		UpdatedAt:    now,
	})
	if err != nil {
		t.Fatalf("upsert adapter: %v", err)
	}

	adapters, err := store.ListAdapters(ctx)
	if err != nil {
		t.Fatalf("list adapters: %v", err)
	}
	if len(adapters) != 1 {
		t.Fatalf("expected 1 adapter, got %d", len(adapters))
	}
	first := adapters[0]
	if first.AdapterName != "codex-notify-wrapper" || first.AgentType != "codex" || first.Version != "v1" || !first.Enabled {
		t.Fatalf("unexpected adapter row: %+v", first)
	}
	if len(first.Capabilities) != 2 || first.Capabilities[0] != "event_driven" || first.Capabilities[1] != "supports_completed" {
		t.Fatalf("unexpected capabilities normalization: %+v", first.Capabilities)
	}

	enabledOnly := true
	filteredEnabled, err := store.ListAdaptersFiltered(ctx, &enabledOnly)
	if err != nil {
		t.Fatalf("list adapters filtered enabled=true: %v", err)
	}
	if len(filteredEnabled) != 1 || filteredEnabled[0].AdapterName != "codex-notify-wrapper" {
		t.Fatalf("unexpected enabled filter result: %+v", filteredEnabled)
	}

	updatedAt := now.Add(1 * time.Minute)
	err = store.UpsertAdapter(ctx, model.AdapterRecord{
		AdapterName:  "codex-notify-wrapper",
		AgentType:    "codex",
		Version:      "v2",
		Capabilities: []string{"supports_waiting_input"},
		Enabled:      false,
		UpdatedAt:    updatedAt,
	})
	if err != nil {
		t.Fatalf("upsert adapter update: %v", err)
	}

	adapters, err = store.ListAdapters(ctx)
	if err != nil {
		t.Fatalf("list adapters after update: %v", err)
	}
	if len(adapters) != 1 {
		t.Fatalf("expected 1 adapter after update, got %d", len(adapters))
	}
	after := adapters[0]
	if after.Version != "v2" || after.Enabled {
		t.Fatalf("unexpected adapter update row: %+v", after)
	}
	if len(after.Capabilities) != 1 || after.Capabilities[0] != "supports_waiting_input" {
		t.Fatalf("unexpected updated capabilities: %+v", after.Capabilities)
	}
	if !after.UpdatedAt.Equal(updatedAt) {
		t.Fatalf("expected updated_at=%s, got %s", updatedAt, after.UpdatedAt)
	}

	gotByName, err := store.GetAdapterByName(ctx, "codex-notify-wrapper")
	if err != nil {
		t.Fatalf("get adapter by name: %v", err)
	}
	if gotByName.Version != "v2" {
		t.Fatalf("unexpected get by name: %+v", gotByName)
	}

	gotByAgent, err := store.GetAdapterByAgentType(ctx, "codex")
	if err != nil {
		t.Fatalf("get adapter by agent_type: %v", err)
	}
	if gotByAgent.AdapterName != "codex-notify-wrapper" {
		t.Fatalf("unexpected get by agent_type: %+v", gotByAgent)
	}
}

func TestUpsertAdapterValidation(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	cases := []model.AdapterRecord{
		{AdapterName: "", AgentType: "codex", Version: "v1"},
		{AdapterName: "codex-notify-wrapper", AgentType: "", Version: "v1"},
		{AdapterName: "codex-notify-wrapper", AgentType: "codex", Version: ""},
	}
	for _, tc := range cases {
		if err := store.UpsertAdapter(ctx, tc); err == nil {
			t.Fatalf("expected validation error for %+v", tc)
		}
	}
}

func TestSetAdapterEnabledByName(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC()
	if err := store.UpsertAdapter(ctx, model.AdapterRecord{
		AdapterName:  "gemini-wrapper-parser",
		AgentType:    "gemini",
		Version:      "v1",
		Capabilities: []string{"event_driven"},
		Enabled:      true,
		UpdatedAt:    now,
	}); err != nil {
		t.Fatalf("seed adapter: %v", err)
	}

	updated, err := store.SetAdapterEnabledByName(ctx, "gemini-wrapper-parser", false, now.Add(1*time.Minute))
	if err != nil {
		t.Fatalf("set adapter enabled false: %v", err)
	}
	if updated.Enabled {
		t.Fatalf("expected adapter disabled, got %+v", updated)
	}

	disabledOnly := false
	filteredDisabled, err := store.ListAdaptersFiltered(ctx, &disabledOnly)
	if err != nil {
		t.Fatalf("list adapters filtered enabled=false: %v", err)
	}
	if len(filteredDisabled) != 1 || filteredDisabled[0].AdapterName != "gemini-wrapper-parser" {
		t.Fatalf("unexpected disabled filter result: %+v", filteredDisabled)
	}

	if _, err := store.SetAdapterEnabledByName(ctx, "missing", true, now); err != ErrNotFound {
		t.Fatalf("expected ErrNotFound for missing adapter, got %v", err)
	}
}

func seedTargetPaneForAction(t *testing.T, store *Store, ctx context.Context, targetID, paneID string, now time.Time) {
	t.Helper()
	if err := store.UpsertTarget(ctx, model.Target{
		TargetID:      targetID,
		TargetName:    targetID,
		Kind:          model.TargetKindLocal,
		ConnectionRef: "",
		Health:        model.TargetHealthOK,
		UpdatedAt:     now,
	}); err != nil {
		t.Fatalf("seed target: %v", err)
	}
	if err := store.UpsertPane(ctx, model.Pane{
		TargetID:    targetID,
		PaneID:      paneID,
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}
}

func TestUpsertPanePersistsCurrentCmd(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC()
	if err := store.UpsertTarget(ctx, model.Target{
		TargetID:      "t1",
		TargetName:    "local",
		Kind:          model.TargetKindLocal,
		ConnectionRef: "",
		Health:        model.TargetHealthOK,
		UpdatedAt:     now,
	}); err != nil {
		t.Fatalf("upsert target: %v", err)
	}
	if err := store.UpsertPane(ctx, model.Pane{
		TargetID:       "t1",
		PaneID:         "%1",
		SessionName:    "s1",
		WindowID:       "@1",
		WindowName:     "w1",
		CurrentCmd:     "nvim",
		CurrentPath:    "/tmp/worktree",
		PaneTitle:      "review output",
		HistoryBytes:   128,
		LastActivityAt: ptrTime(now.Add(-1 * time.Minute)),
		UpdatedAt:      now,
	}); err != nil {
		t.Fatalf("upsert pane: %v", err)
	}

	panes, err := store.ListPanes(ctx)
	if err != nil {
		t.Fatalf("list panes: %v", err)
	}
	if len(panes) != 1 {
		t.Fatalf("expected 1 pane, got %d", len(panes))
	}
	if panes[0].CurrentCmd != "nvim" {
		t.Fatalf("expected current_cmd=nvim, got %+v", panes[0].CurrentCmd)
	}
	if panes[0].CurrentPath != "/tmp/worktree" {
		t.Fatalf("expected current_path=/tmp/worktree, got %+v", panes[0].CurrentPath)
	}
	if panes[0].PaneTitle != "review output" {
		t.Fatalf("expected pane_title=review output, got %+v", panes[0].PaneTitle)
	}
	if panes[0].HistoryBytes != 128 {
		t.Fatalf("expected history_bytes=128, got %+v", panes[0].HistoryBytes)
	}
	if panes[0].LastActivityAt == nil || !panes[0].LastActivityAt.Equal(now.Add(-1*time.Minute)) {
		t.Fatalf("expected last_activity_at set, got %+v", panes[0].LastActivityAt)
	}
}

func TestListSendActionsForPanesFiltersAndOrders(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC()
	seedTargetPaneForAction(t, store, ctx, "t1", "%1", now)
	seedTargetPaneForAction(t, store, ctx, "t1", "%2", now)
	seedTargetPaneForAction(t, store, ctx, "t2", "%1", now)

	insert := func(id, req, targetID, paneID string, at time.Time) {
		meta := `{"text":"` + req + `"}`
		if err := store.InsertAction(ctx, model.Action{
			ActionID:     id,
			ActionType:   model.ActionTypeSend,
			RequestRef:   req,
			TargetID:     targetID,
			PaneID:       paneID,
			RequestedAt:  at,
			ResultCode:   "completed",
			MetadataJSON: &meta,
		}); err != nil {
			t.Fatalf("insert action %s: %v", id, err)
		}
	}

	insert("a1", "one", "t1", "%1", now.Add(1*time.Second))
	insert("a2", "two", "t1", "%1", now.Add(2*time.Second))
	insert("a3", "three", "t1", "%2", now.Add(3*time.Second))
	insert("a4", "four", "t2", "%1", now.Add(4*time.Second))

	got, err := store.ListSendActionsForPanes(ctx, []string{"t1"}, []string{"%1", "%2"})
	if err != nil {
		t.Fatalf("list send actions for panes: %v", err)
	}
	if len(got) != 3 {
		t.Fatalf("expected 3 actions, got %d", len(got))
	}
	if got[0].RequestRef != "one" || got[1].RequestRef != "two" || got[2].RequestRef != "three" {
		t.Fatalf("unexpected order/filter result: %+v", got)
	}
}

func TestListLatestRuntimeEventsReturnsLatestNonPollerPerRuntime(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC()
	seedTargetPaneForAction(t, store, ctx, "t1", "%1", now)
	seedTargetPaneForAction(t, store, ctx, "t1", "%2", now)

	rt1 := model.Runtime{
		RuntimeID:        "rt-ev-1",
		TargetID:         "t1",
		PaneID:           "%1",
		TmuxServerBootID: "boot-a",
		PaneEpoch:        1,
		AgentType:        "codex",
		StartedAt:        now.Add(-5 * time.Minute),
	}
	rt2 := model.Runtime{
		RuntimeID:        "rt-ev-2",
		TargetID:         "t1",
		PaneID:           "%2",
		TmuxServerBootID: "boot-b",
		PaneEpoch:        1,
		AgentType:        "claude",
		StartedAt:        now.Add(-4 * time.Minute),
	}
	if err := store.InsertRuntime(ctx, rt1); err != nil {
		t.Fatalf("insert rt1: %v", err)
	}
	if err := store.InsertRuntime(ctx, rt2); err != nil {
		t.Fatalf("insert rt2: %v", err)
	}

	insert := func(eventID, runtimeID, eventType string, source model.EventSource, at time.Time, payload string) {
		if err := store.InsertEvent(ctx, model.EventEnvelope{
			EventID:    eventID,
			RuntimeID:  runtimeID,
			EventType:  eventType,
			Source:     source,
			DedupeKey:  eventID,
			EventTime:  at,
			IngestedAt: at,
			RawPayload: payload,
		}, payload); err != nil {
			t.Fatalf("insert event %s: %v", eventID, err)
		}
	}

	insert("ev-a-old", rt1.RuntimeID, "input-requested", model.SourceNotify, now.Add(-3*time.Minute), `{"message":"old input"}`)
	insert("ev-a-new", rt1.RuntimeID, "agent-turn-complete", model.SourceNotify, now.Add(-2*time.Minute), `{"message":"new response"}`)
	insert("ev-a-poller", rt1.RuntimeID, "running", model.SourcePoller, now.Add(-1*time.Minute), "")
	insert("ev-b-only", rt2.RuntimeID, "approval-requested", model.SourceHook, now.Add(-90*time.Second), `{"summary":"needs approval"}`)

	got, err := store.ListLatestRuntimeEvents(ctx, []string{rt1.RuntimeID, rt2.RuntimeID})
	if err != nil {
		t.Fatalf("list latest runtime events: %v", err)
	}
	if len(got) != 2 {
		t.Fatalf("expected 2 runtime events, got %d", len(got))
	}

	byRuntime := map[string]RuntimeLatestEvent{}
	for _, item := range got {
		byRuntime[item.RuntimeID] = item
	}

	if byRuntime[rt1.RuntimeID].EventType != "agent-turn-complete" {
		t.Fatalf("expected latest non-poller event for rt1, got %+v", byRuntime[rt1.RuntimeID])
	}
	if byRuntime[rt2.RuntimeID].EventType != "approval-requested" {
		t.Fatalf("expected hook event for rt2, got %+v", byRuntime[rt2.RuntimeID])
	}
}

func TestUpsertStateRoundTripWithProvenance(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC()
	seedTargetPaneForAction(t, store, ctx, "t1", "%1", now)
	rt := model.Runtime{
		RuntimeID:        "rt-state-1",
		TargetID:         "t1",
		PaneID:           "%1",
		TmuxServerBootID: "boot",
		PaneEpoch:        1,
		AgentType:        "codex",
		StartedAt:        now,
	}
	if err := store.InsertRuntime(ctx, rt); err != nil {
		t.Fatalf("seed runtime: %v", err)
	}

	seq := int64(12)
	eventAt := now.Add(-1 * time.Second)
	row := model.StateRow{
		TargetID:      "t1",
		PaneID:        "%1",
		RuntimeID:     rt.RuntimeID,
		State:         model.StateWaitingInput,
		ReasonCode:    "input_required",
		Confidence:    "high",
		StateVersion:  2,
		StateSource:   model.SourceNotify,
		LastEventType: "input-requested",
		LastEventAt:   &eventAt,
		LastSourceSeq: &seq,
		LastSeenAt:    now,
		UpdatedAt:     now,
	}
	if err := store.UpsertState(ctx, row); err != nil {
		t.Fatalf("upsert state: %v", err)
	}

	got, err := store.GetState(ctx, "t1", "%1")
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if got.StateSource != model.SourceNotify || got.LastEventType != "input-requested" {
		t.Fatalf("unexpected provenance fields: %+v", got)
	}
	if got.LastEventAt == nil || !got.LastEventAt.Equal(eventAt) {
		t.Fatalf("expected last_event_at=%s, got %+v", eventAt, got.LastEventAt)
	}

	list, err := store.ListStates(ctx)
	if err != nil {
		t.Fatalf("list states: %v", err)
	}
	if len(list) != 1 {
		t.Fatalf("expected one state row, got %d", len(list))
	}
	if list[0].StateSource != model.SourceNotify || list[0].LastEventType != "input-requested" {
		t.Fatalf("unexpected listed provenance fields: %+v", list[0])
	}
}

func TestInsertActionAndGetActionByTypeRequestRef(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC()
	seedTargetPaneForAction(t, store, ctx, "t1", "%1", now)
	completedAt := now.Add(2 * time.Second)
	errCode := "NONE"
	meta := `{"k":"v"}`
	action := model.Action{
		ActionID:     "a1",
		ActionType:   model.ActionTypeAttach,
		RequestRef:   "req-1",
		TargetID:     "t1",
		PaneID:       "%1",
		RequestedAt:  now,
		CompletedAt:  &completedAt,
		ResultCode:   "completed",
		ErrorCode:    &errCode,
		MetadataJSON: &meta,
	}
	if err := store.InsertAction(ctx, action); err != nil {
		t.Fatalf("insert action: %v", err)
	}

	got, err := store.GetActionByTypeRequestRef(ctx, model.ActionTypeAttach, "req-1")
	if err != nil {
		t.Fatalf("get action by type/request_ref: %v", err)
	}
	if got.ActionID != action.ActionID || got.TargetID != action.TargetID || got.PaneID != action.PaneID {
		t.Fatalf("unexpected action row: %+v", got)
	}
	if got.ActionType != model.ActionTypeAttach || got.ResultCode != "completed" {
		t.Fatalf("unexpected action fields: %+v", got)
	}
	if got.CompletedAt == nil || !got.CompletedAt.Equal(completedAt) {
		t.Fatalf("expected completed_at=%s, got %+v", completedAt, got.CompletedAt)
	}
	if got.ErrorCode == nil || *got.ErrorCode != errCode {
		t.Fatalf("expected error_code=%s, got %+v", errCode, got.ErrorCode)
	}
	if got.MetadataJSON == nil || *got.MetadataJSON != meta {
		t.Fatalf("expected metadata_json=%s, got %+v", meta, got.MetadataJSON)
	}
}

func TestInsertActionAutoRequestedAtAndNullableRoundTrip(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC()
	seedTargetPaneForAction(t, store, ctx, "t1", "%1", now)
	action := model.Action{
		ActionID:    "a-nulls",
		ActionType:  model.ActionTypeAttach,
		RequestRef:  "req-nulls",
		TargetID:    "t1",
		PaneID:      "%1",
		ResultCode:  "completed",
		RequestedAt: time.Time{},
	}
	if err := store.InsertAction(ctx, action); err != nil {
		t.Fatalf("insert action: %v", err)
	}
	got, err := store.GetActionByTypeRequestRef(ctx, model.ActionTypeAttach, "req-nulls")
	if err != nil {
		t.Fatalf("get action: %v", err)
	}
	if got.RequestedAt.IsZero() {
		t.Fatalf("expected requested_at to be auto-populated, got %+v", got)
	}
	if got.CompletedAt != nil || got.ErrorCode != nil || got.MetadataJSON != nil || got.RuntimeID != nil {
		t.Fatalf("expected nullable fields to stay nil, got %+v", got)
	}
}

func TestInsertActionDuplicateAndForeignKey(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC()
	seedTargetPaneForAction(t, store, ctx, "t1", "%1", now)
	first := model.Action{
		ActionID:    "a1",
		ActionType:  model.ActionTypeAttach,
		RequestRef:  "req-dup",
		TargetID:    "t1",
		PaneID:      "%1",
		RequestedAt: now,
		ResultCode:  "completed",
	}
	if err := store.InsertAction(ctx, first); err != nil {
		t.Fatalf("insert first action: %v", err)
	}
	second := first
	second.ActionID = "a2"
	if err := store.InsertAction(ctx, second); err != ErrDuplicate {
		t.Fatalf("expected ErrDuplicate, got %v", err)
	}

	missingPane := model.Action{
		ActionID:    "a3",
		ActionType:  model.ActionTypeAttach,
		RequestRef:  "req-missing",
		TargetID:    "t1",
		PaneID:      "%999",
		RequestedAt: now,
		ResultCode:  "completed",
	}
	if err := store.InsertAction(ctx, missingPane); err != ErrNotFound {
		t.Fatalf("expected ErrNotFound on missing pane FK, got %v", err)
	}

	otherType := model.Action{
		ActionID:    "a4",
		ActionType:  model.ActionTypeSend,
		RequestRef:  "req-dup",
		TargetID:    "t1",
		PaneID:      "%1",
		RequestedAt: now,
		ResultCode:  "completed",
	}
	if err := store.InsertAction(ctx, otherType); err != nil {
		t.Fatalf("expected same request_ref with different action_type to be allowed, got %v", err)
	}
}

func TestGetActionByTypeRequestRefNotFound(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	_, err = store.GetActionByTypeRequestRef(ctx, model.ActionTypeAttach, "missing")
	if err != ErrNotFound {
		t.Fatalf("expected ErrNotFound, got %v", err)
	}
}

func TestInsertActionRoundTripWithRuntimeIDAndActionTypes(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC()
	seedTargetPaneForAction(t, store, ctx, "t1", "%1", now)
	pid := int64(1234)
	rt := model.Runtime{
		RuntimeID:        "rt-action-test",
		TargetID:         "t1",
		PaneID:           "%1",
		TmuxServerBootID: "boot",
		PaneEpoch:        1,
		AgentType:        "codex",
		PID:              &pid,
		StartedAt:        now,
	}
	if err := store.InsertRuntime(ctx, rt); err != nil {
		t.Fatalf("insert runtime: %v", err)
	}
	runtimeID := rt.RuntimeID
	cases := []struct {
		actionType model.ActionType
		requestRef string
		actionID   string
	}{
		{actionType: model.ActionTypeAttach, requestRef: "req-attach-rt", actionID: "a-attach"},
		{actionType: model.ActionTypeSend, requestRef: "req-send-rt", actionID: "a-send"},
		{actionType: model.ActionTypeViewOutput, requestRef: "req-view-rt", actionID: "a-view"},
		{actionType: model.ActionTypeKill, requestRef: "req-kill-rt", actionID: "a-kill"},
	}
	for _, tc := range cases {
		err := store.InsertAction(ctx, model.Action{
			ActionID:    tc.actionID,
			ActionType:  tc.actionType,
			RequestRef:  tc.requestRef,
			TargetID:    "t1",
			PaneID:      "%1",
			RuntimeID:   &runtimeID,
			RequestedAt: now,
			ResultCode:  "completed",
		})
		if err != nil {
			t.Fatalf("insert action %s: %v", tc.actionType, err)
		}
		got, err := store.GetActionByTypeRequestRef(ctx, tc.actionType, tc.requestRef)
		if err != nil {
			t.Fatalf("get action %s: %v", tc.actionType, err)
		}
		if got.ActionID != tc.actionID || got.ActionType != tc.actionType {
			t.Fatalf("unexpected action roundtrip for %s: %+v", tc.actionType, got)
		}
		if got.RuntimeID == nil || *got.RuntimeID != runtimeID {
			t.Fatalf("expected runtime_id=%s, got %+v", runtimeID, got.RuntimeID)
		}
	}
}

func TestGetActionByIDAndListEventsByActionID(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC()
	seedTargetPaneForAction(t, store, ctx, "t1", "%1", now)
	pid := int64(2222)
	rt := model.Runtime{
		RuntimeID:        "rt-corr-1",
		TargetID:         "t1",
		PaneID:           "%1",
		TmuxServerBootID: "boot-corr",
		PaneEpoch:        1,
		AgentType:        "codex",
		PID:              &pid,
		StartedAt:        now,
	}
	if err := store.InsertRuntime(ctx, rt); err != nil {
		t.Fatalf("insert runtime: %v", err)
	}
	runtimeID := rt.RuntimeID
	action := model.Action{
		ActionID:    "a-corr-1",
		ActionType:  model.ActionTypeSend,
		RequestRef:  "req-corr-1",
		TargetID:    "t1",
		PaneID:      "%1",
		RuntimeID:   &runtimeID,
		RequestedAt: now,
		ResultCode:  "completed",
	}
	if err := store.InsertAction(ctx, action); err != nil {
		t.Fatalf("insert action: %v", err)
	}
	gotAction, err := store.GetActionByID(ctx, action.ActionID)
	if err != nil {
		t.Fatalf("get action by id: %v", err)
	}
	if gotAction.ActionID != action.ActionID || gotAction.ActionType != action.ActionType {
		t.Fatalf("unexpected action by id: %+v", gotAction)
	}

	event := model.EventEnvelope{
		EventID:    "ev-corr-1",
		RuntimeID:  rt.RuntimeID,
		EventType:  "action.send",
		Source:     model.SourceWrapper,
		DedupeKey:  "action:" + action.ActionID,
		EventTime:  now,
		IngestedAt: now,
		ActionID:   &action.ActionID,
		RawPayload: "",
	}
	if err := store.InsertEvent(ctx, event, ""); err != nil {
		t.Fatalf("insert event: %v", err)
	}
	events, err := store.ListEventsByActionID(ctx, action.ActionID)
	if err != nil {
		t.Fatalf("list events by action_id: %v", err)
	}
	if len(events) != 1 {
		t.Fatalf("expected 1 event, got %d", len(events))
	}
	if events[0].EventID != event.EventID || events[0].ActionID != action.ActionID {
		t.Fatalf("unexpected correlated event: %+v", events[0])
	}
	if events[0].RuntimeID != rt.RuntimeID || events[0].EventType != event.EventType {
		t.Fatalf("unexpected correlated event fields: %+v", events[0])
	}
}

func TestGetActionByIDNotFound(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}
	if _, err := store.GetActionByID(ctx, "missing-action-id"); err != ErrNotFound {
		t.Fatalf("expected ErrNotFound, got %v", err)
	}
}

func TestInsertActionSnapshotRoundTripAndDuplicateActionID(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC()
	seedTargetPaneForAction(t, store, ctx, "t1", "%1", now)
	pid := int64(7777)
	rt := model.Runtime{
		RuntimeID:        "rt-snap-1",
		TargetID:         "t1",
		PaneID:           "%1",
		TmuxServerBootID: "boot-snap",
		PaneEpoch:        1,
		AgentType:        "codex",
		PID:              &pid,
		StartedAt:        now,
	}
	if err := store.InsertRuntime(ctx, rt); err != nil {
		t.Fatalf("insert runtime: %v", err)
	}
	runtimeID := rt.RuntimeID
	action := model.Action{
		ActionID:    "a-snap-1",
		ActionType:  model.ActionTypeSend,
		RequestRef:  "req-snap-1",
		TargetID:    "t1",
		PaneID:      "%1",
		RuntimeID:   &runtimeID,
		RequestedAt: now,
		ResultCode:  "completed",
	}
	if err := store.InsertAction(ctx, action); err != nil {
		t.Fatalf("insert action: %v", err)
	}

	first := model.ActionSnapshot{
		SnapshotID:   "snap-1",
		ActionID:     action.ActionID,
		TargetID:     "t1",
		PaneID:       "%1",
		RuntimeID:    runtimeID,
		StateVersion: 11,
		ObservedAt:   now,
		ExpiresAt:    now.Add(30 * time.Second),
		Nonce:        "nonce-1",
	}
	if err := store.InsertActionSnapshot(ctx, first); err != nil {
		t.Fatalf("insert snapshot: %v", err)
	}
	got, err := store.GetActionSnapshotByActionID(ctx, action.ActionID)
	if err != nil {
		t.Fatalf("get action snapshot: %v", err)
	}
	if got.ActionID != action.ActionID || got.RuntimeID != runtimeID || got.StateVersion != 11 {
		t.Fatalf("unexpected action snapshot: %+v", got)
	}

	second := model.ActionSnapshot{
		SnapshotID:   "snap-2",
		ActionID:     action.ActionID,
		TargetID:     "t1",
		PaneID:       "%1",
		RuntimeID:    runtimeID,
		StateVersion: 12,
		ObservedAt:   now.Add(1 * time.Second),
		ExpiresAt:    now.Add(31 * time.Second),
		Nonce:        "nonce-2",
	}
	if err := store.InsertActionSnapshot(ctx, second); err != ErrDuplicate {
		t.Fatalf("expected ErrDuplicate for duplicate action_id snapshot, got %v", err)
	}
}

func TestListEventsByActionIDIsolationAndOrdering(t *testing.T) {
	ctx := context.Background()
	store, err := Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	defer store.Close() //nolint:errcheck
	if err := ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC()
	seedTargetPaneForAction(t, store, ctx, "t1", "%1", now)
	pid := int64(5555)
	rt := model.Runtime{
		RuntimeID:        "rt-corr-iso-1",
		TargetID:         "t1",
		PaneID:           "%1",
		TmuxServerBootID: "boot-iso",
		PaneEpoch:        1,
		AgentType:        "codex",
		PID:              &pid,
		StartedAt:        now,
	}
	if err := store.InsertRuntime(ctx, rt); err != nil {
		t.Fatalf("insert runtime: %v", err)
	}
	rtID := rt.RuntimeID
	actionA := model.Action{
		ActionID:    "a-corr-iso-a",
		ActionType:  model.ActionTypeSend,
		RequestRef:  "req-corr-iso-a",
		TargetID:    "t1",
		PaneID:      "%1",
		RuntimeID:   &rtID,
		RequestedAt: now,
		ResultCode:  "completed",
	}
	actionB := model.Action{
		ActionID:    "a-corr-iso-b",
		ActionType:  model.ActionTypeKill,
		RequestRef:  "req-corr-iso-b",
		TargetID:    "t1",
		PaneID:      "%1",
		RuntimeID:   &rtID,
		RequestedAt: now,
		ResultCode:  "completed",
	}
	if err := store.InsertAction(ctx, actionA); err != nil {
		t.Fatalf("insert action a: %v", err)
	}
	if err := store.InsertAction(ctx, actionB); err != nil {
		t.Fatalf("insert action b: %v", err)
	}

	actionAID := actionA.ActionID
	actionBID := actionB.ActionID
	eventA1 := model.EventEnvelope{
		EventID:    "ev-iso-a1",
		RuntimeID:  rt.RuntimeID,
		EventType:  "action.send",
		Source:     model.SourceWrapper,
		DedupeKey:  "action:" + actionA.ActionID + ":1",
		EventTime:  now.Add(1 * time.Second),
		IngestedAt: now.Add(1 * time.Second),
		ActionID:   &actionAID,
	}
	eventA2 := model.EventEnvelope{
		EventID:    "ev-iso-a2",
		RuntimeID:  rt.RuntimeID,
		EventType:  "action.send",
		Source:     model.SourceWrapper,
		DedupeKey:  "action:" + actionA.ActionID + ":2",
		EventTime:  now.Add(2 * time.Second),
		IngestedAt: now.Add(2 * time.Second),
		ActionID:   &actionAID,
	}
	eventB1 := model.EventEnvelope{
		EventID:    "ev-iso-b1",
		RuntimeID:  rt.RuntimeID,
		EventType:  "action.kill",
		Source:     model.SourceWrapper,
		DedupeKey:  "action:" + actionB.ActionID + ":1",
		EventTime:  now.Add(3 * time.Second),
		IngestedAt: now.Add(3 * time.Second),
		ActionID:   &actionBID,
	}
	if err := store.InsertEvent(ctx, eventA1, ""); err != nil {
		t.Fatalf("insert event a1: %v", err)
	}
	if err := store.InsertEvent(ctx, eventA2, ""); err != nil {
		t.Fatalf("insert event a2: %v", err)
	}
	if err := store.InsertEvent(ctx, eventB1, ""); err != nil {
		t.Fatalf("insert event b1: %v", err)
	}

	eventsA, err := store.ListEventsByActionID(ctx, actionA.ActionID)
	if err != nil {
		t.Fatalf("list events for action a: %v", err)
	}
	if len(eventsA) != 2 {
		t.Fatalf("expected 2 events for action a, got %+v", eventsA)
	}
	if eventsA[0].EventID != "ev-iso-a1" || eventsA[1].EventID != "ev-iso-a2" {
		t.Fatalf("unexpected event ordering/filtering for action a: %+v", eventsA)
	}
	for _, ev := range eventsA {
		if ev.ActionID != actionA.ActionID {
			t.Fatalf("expected action_id=%s, got %+v", actionA.ActionID, ev)
		}
	}
	eventsB, err := store.ListEventsByActionID(ctx, actionB.ActionID)
	if err != nil {
		t.Fatalf("list events for action b: %v", err)
	}
	if len(eventsB) != 1 || eventsB[0].EventID != "ev-iso-b1" || eventsB[0].ActionID != actionB.ActionID {
		t.Fatalf("unexpected event filtering for action b: %+v", eventsB)
	}
}
