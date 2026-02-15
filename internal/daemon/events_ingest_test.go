package daemon

import (
	"context"
	"net/http"
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/model"
)

func TestEventsIngestPendingBindAccepted(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	seedTarget(t, store, "local", "local")

	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/events", map[string]any{
		"target":     "local",
		"pane_id":    "%9",
		"agent_type": "codex",
		"source":     "notify",
		"event_type": "agent-turn-complete",
		"dedupe_key": "dk-pending-1",
	})
	if rec.Code != http.StatusAccepted {
		t.Fatalf("expected 202, got %d body=%s", rec.Code, rec.Body.String())
	}
	resp := decodeJSON[map[string]any](t, rec)
	if got, _ := resp["status"].(string); got != "pending_bind" {
		t.Fatalf("expected status pending_bind, got %+v", resp)
	}

	inbox, err := store.ListPendingInbox(context.Background())
	if err != nil {
		t.Fatalf("list pending inbox: %v", err)
	}
	if len(inbox) != 1 {
		t.Fatalf("expected 1 pending inbox event, got %d", len(inbox))
	}
	if inbox[0].TargetID != "local" || inbox[0].PaneID != "%9" {
		t.Fatalf("unexpected inbox binding target/pane: %+v", inbox[0])
	}
}

func TestEventsIngestBoundWhenSingleRuntimeMatchesHints(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	seedTarget(t, store, "local", "local")

	now := time.Now().UTC()
	pid := int64(4242)
	pane := model.Pane{
		TargetID:    "local",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}
	if err := store.UpsertPane(context.Background(), pane); err != nil {
		t.Fatalf("seed pane: %v", err)
	}
	rt := model.Runtime{
		RuntimeID:        "rt-event-bound-1",
		TargetID:         "local",
		PaneID:           "%1",
		TmuxServerBootID: "boot",
		PaneEpoch:        1,
		AgentType:        "codex",
		PID:              &pid,
		StartedAt:        now,
	}
	if err := store.InsertRuntime(context.Background(), rt); err != nil {
		t.Fatalf("seed runtime: %v", err)
	}

	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/events", map[string]any{
		"target":     "local",
		"pane_id":    "%1",
		"agent_type": "codex",
		"source":     "notify",
		"event_type": "agent-turn-complete",
		"dedupe_key": "dk-bound-1",
		"pid":        pid,
		"start_hint": now.Format(time.RFC3339Nano),
	})
	if rec.Code != http.StatusAccepted {
		t.Fatalf("expected 202, got %d body=%s", rec.Code, rec.Body.String())
	}
	resp := decodeJSON[map[string]any](t, rec)
	if got, _ := resp["status"].(string); got != "bound" {
		t.Fatalf("expected status bound, got %+v", resp)
	}
	if got, _ := resp["runtime_id"].(string); got != "rt-event-bound-1" {
		t.Fatalf("expected runtime_id rt-event-bound-1, got %+v", resp)
	}

	st, err := store.GetState(context.Background(), "local", "%1")
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.RuntimeID != "rt-event-bound-1" || st.State != model.StateCompleted {
		t.Fatalf("unexpected state after bound event: %+v", st)
	}
}

func TestEventsIngestRejectsInvalidSource(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	seedTarget(t, store, "local", "local")

	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/events", map[string]any{
		"target":     "local",
		"pane_id":    "%3",
		"agent_type": "codex",
		"source":     "invalid-source",
		"event_type": "agent-turn-complete",
		"dedupe_key": "dk-invalid-source",
	})
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d body=%s", rec.Code, rec.Body.String())
	}
	errResp := decodeJSON[map[string]any](t, rec)
	rawErr, ok := errResp["error"].(map[string]any)
	if !ok {
		t.Fatalf("expected error payload, got %+v", errResp)
	}
	if code, _ := rawErr["code"].(string); code != model.ErrRefInvalid {
		t.Fatalf("expected code %s, got %+v", model.ErrRefInvalid, errResp)
	}
}

func TestEventsIngestDuplicateRetryWithoutEventIDIsIdempotent(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	seedTarget(t, store, "local", "local")

	now := time.Now().UTC()
	pid := int64(5252)
	pane := model.Pane{
		TargetID:    "local",
		PaneID:      "%5",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}
	if err := store.UpsertPane(context.Background(), pane); err != nil {
		t.Fatalf("seed pane: %v", err)
	}
	rt := model.Runtime{
		RuntimeID:        "rt-event-dup-1",
		TargetID:         "local",
		PaneID:           "%5",
		TmuxServerBootID: "boot",
		PaneEpoch:        1,
		AgentType:        "codex",
		PID:              &pid,
		StartedAt:        now,
	}
	if err := store.InsertRuntime(context.Background(), rt); err != nil {
		t.Fatalf("seed runtime: %v", err)
	}

	body := map[string]any{
		"target":     "local",
		"pane_id":    "%5",
		"agent_type": "codex",
		"source":     "notify",
		"event_type": "agent-turn-complete",
		"dedupe_key": "dk-bound-dup-1",
		"pid":        pid,
	}

	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/events", body)
	if first.Code != http.StatusAccepted {
		t.Fatalf("first expected 202, got %d body=%s", first.Code, first.Body.String())
	}
	firstResp := decodeJSON[map[string]any](t, first)
	if got, _ := firstResp["status"].(string); got != "bound" {
		t.Fatalf("first expected bound status, got %+v", firstResp)
	}

	second := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/events", body)
	if second.Code != http.StatusAccepted {
		t.Fatalf("second expected 202, got %d body=%s", second.Code, second.Body.String())
	}
	secondResp := decodeJSON[map[string]any](t, second)
	if got, _ := secondResp["status"].(string); got != "bound" {
		t.Fatalf("second expected bound status, got %+v", secondResp)
	}

	st, err := store.GetState(context.Background(), "local", "%5")
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.State != model.StateCompleted {
		t.Fatalf("expected completed state after idempotent retry, got %+v", st)
	}
}

func TestEventsAPI_IdempotentRetryWithDifferentEventIDAndEventTime(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	seedTarget(t, store, "local", "local")

	now := time.Now().UTC()
	pid := int64(5353)
	pane := model.Pane{
		TargetID:    "local",
		PaneID:      "%7",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}
	if err := store.UpsertPane(context.Background(), pane); err != nil {
		t.Fatalf("seed pane: %v", err)
	}
	rt := model.Runtime{
		RuntimeID:        "rt-event-dup-payload-1",
		TargetID:         "local",
		PaneID:           "%7",
		TmuxServerBootID: "boot",
		PaneEpoch:        1,
		AgentType:        "codex",
		PID:              &pid,
		StartedAt:        now,
	}
	if err := store.InsertRuntime(context.Background(), rt); err != nil {
		t.Fatalf("seed runtime: %v", err)
	}

	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/events", map[string]any{
		"runtime_id": rt.RuntimeID,
		"source":     "notify",
		"event_type": "agent-turn-complete",
		"dedupe_key": "dk-bound-dup-payload-1",
		"event_id":   "ev-dup-payload-first",
		"event_time": now.Add(-2 * time.Second).Format(time.RFC3339Nano),
		"agent_type": "codex",
	})
	if first.Code != http.StatusAccepted {
		t.Fatalf("first expected 202, got %d body=%s", first.Code, first.Body.String())
	}

	second := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/events", map[string]any{
		"runtime_id":  rt.RuntimeID,
		"source":      "notify",
		"event_type":  "agent-turn-complete",
		"dedupe_key":  "dk-bound-dup-payload-1",
		"event_id":    "ev-dup-payload-retry",
		"event_time":  now.Add(4 * time.Second).Format(time.RFC3339Nano),
		"agent_type":  "codex",
		"raw_payload": `{"type":"input-requested"}`,
	})
	if second.Code != http.StatusAccepted {
		t.Fatalf("second expected 202, got %d body=%s", second.Code, second.Body.String())
	}
	secondResp := decodeJSON[map[string]any](t, second)
	if got, _ := secondResp["status"].(string); got != "bound" {
		t.Fatalf("second expected bound status, got %+v", secondResp)
	}

	st, err := store.GetState(context.Background(), "local", "%7")
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.State != model.StateWaitingInput || st.ReasonCode != "input_required" {
		t.Fatalf("expected payload-derived waiting_input on idempotent retry, got %+v", st)
	}
}

func TestEventsIngestFutureEventTimeIsClamped(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	seedTarget(t, store, "local", "local")

	now := time.Now().UTC()
	pid := int64(6161)
	pane := model.Pane{
		TargetID:    "local",
		PaneID:      "%6",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}
	if err := store.UpsertPane(context.Background(), pane); err != nil {
		t.Fatalf("seed pane: %v", err)
	}
	rt := model.Runtime{
		RuntimeID:        "rt-event-future-1",
		TargetID:         "local",
		PaneID:           "%6",
		TmuxServerBootID: "boot",
		PaneEpoch:        1,
		AgentType:        "codex",
		PID:              &pid,
		StartedAt:        now,
	}
	if err := store.InsertRuntime(context.Background(), rt); err != nil {
		t.Fatalf("seed runtime: %v", err)
	}

	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/events", map[string]any{
		"runtime_id":  rt.RuntimeID,
		"source":      "notify",
		"event_type":  "input-requested",
		"dedupe_key":  "dk-future-1",
		"event_time":  now.Add(24 * time.Hour).Format(time.RFC3339Nano),
		"agent_type":  "codex",
		"raw_payload": `{"type":"input-requested"}`,
	})
	if rec.Code != http.StatusAccepted {
		t.Fatalf("expected 202, got %d body=%s", rec.Code, rec.Body.String())
	}

	st, err := store.GetState(context.Background(), "local", "%6")
	if err != nil {
		t.Fatalf("get state: %v", err)
	}
	if st.LastEventAt == nil {
		t.Fatalf("expected last_event_at set, got %+v", st)
	}
	if st.LastEventAt.After(time.Now().UTC().Add(5 * time.Second)) {
		t.Fatalf("expected clamped last_event_at near now, got %s", st.LastEventAt.Format(time.RFC3339Nano))
	}
}
