package daemon

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/api"
	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/db"
	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/target"
)

func TestHealthEndpointOverUDS(t *testing.T) {
	tmp := t.TempDir()
	socketPath := filepath.Join(tmp, "agtmuxd.sock")
	cfg := config.DefaultConfig()
	cfg.SocketPath = socketPath

	srv := NewServer(cfg)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	errCh := make(chan error, 1)
	go func() {
		errCh <- srv.Start(ctx)
	}()

	waitForSocket(t, socketPath, errCh)

	client := &http.Client{Transport: &http.Transport{
		DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
			var d net.Dialer
			return d.DialContext(ctx, "unix", socketPath)
		},
	}}
	resp, err := client.Get("http://unix/v1/health")
	if err != nil {
		t.Fatalf("get health over uds: %v", err)
	}
	defer resp.Body.Close() //nolint:errcheck
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("expected 200, got %d", resp.StatusCode)
	}
	var payload api.HealthResponse
	if err := json.NewDecoder(resp.Body).Decode(&payload); err != nil {
		t.Fatalf("decode health response: %v", err)
	}
	if payload.SchemaVersion != "v1" || payload.Status != "ok" {
		t.Fatalf("unexpected payload: %+v", payload)
	}

	cancel()
	select {
	case err := <-errCh:
		if err != nil && err != context.Canceled {
			t.Fatalf("server error: %v", err)
		}
	case <-time.After(3 * time.Second):
		t.Fatalf("timeout waiting for server shutdown")
	}
}

func TestStartFailsWhenSocketPathIsRegularFile(t *testing.T) {
	tmp := t.TempDir()
	socketPath := filepath.Join(tmp, "agtmuxd.sock")
	if err := os.WriteFile(socketPath, []byte("not-a-socket"), 0o600); err != nil {
		t.Fatalf("write regular file: %v", err)
	}

	cfg := config.DefaultConfig()
	cfg.SocketPath = socketPath
	srv := NewServer(cfg)

	err := srv.Start(context.Background())
	if err == nil {
		t.Fatalf("expected start to fail for non-socket file")
	}
	if err := os.Remove(socketPath); err != nil {
		t.Fatalf("regular file should remain for caller cleanup, got remove error: %v", err)
	}
}

func TestSingleInstanceLock(t *testing.T) {
	tmp := t.TempDir()
	socketPath := filepath.Join(tmp, "agtmuxd.sock")
	cfg := config.DefaultConfig()
	cfg.SocketPath = socketPath

	srv1 := NewServer(cfg)
	ctx1, cancel1 := context.WithCancel(context.Background())
	defer cancel1()

	errCh1 := make(chan error, 1)
	go func() {
		errCh1 <- srv1.Start(ctx1)
	}()
	waitForSocket(t, socketPath, errCh1)

	srv2 := NewServer(cfg)
	err := srv2.Start(context.Background())
	if err == nil {
		t.Fatalf("expected second server start to fail while first lock is held")
	}
	if !strings.Contains(err.Error(), "daemon already running") {
		t.Fatalf("expected lock contention error, got: %v", err)
	}

	cancel1()
	select {
	case err := <-errCh1:
		if err != nil && err != context.Canceled {
			t.Fatalf("server1 shutdown error: %v", err)
		}
	case <-time.After(3 * time.Second):
		t.Fatalf("timeout waiting for server1 shutdown")
	}

	srv3 := NewServer(cfg)
	ctx3, cancel3 := context.WithCancel(context.Background())
	defer cancel3()
	errCh3 := make(chan error, 1)
	go func() {
		errCh3 <- srv3.Start(ctx3)
	}()
	waitForSocket(t, socketPath, errCh3)
	cancel3()
	select {
	case err := <-errCh3:
		if err != nil && err != context.Canceled {
			t.Fatalf("server3 shutdown error: %v", err)
		}
	case <-time.After(3 * time.Second):
		t.Fatalf("timeout waiting for server3 shutdown")
	}
}

type stubRunner struct {
	calls []runnerCall
	out   []byte
	err   error
}

type runnerCall struct {
	name string
	args []string
}

func (r *stubRunner) Run(_ context.Context, name string, args ...string) ([]byte, error) {
	callArgs := make([]string, len(args))
	copy(callArgs, args)
	r.calls = append(r.calls, runnerCall{name: name, args: callArgs})
	if r.err != nil {
		return nil, r.err
	}
	if len(r.out) == 0 {
		return []byte("ok"), nil
	}
	return r.out, nil
}

type blockingFirstCallRunner struct {
	mu               sync.Mutex
	calls            []runnerCall
	firstCallStarted chan struct{}
	releaseFirstCall chan struct{}
}

func newBlockingFirstCallRunner() *blockingFirstCallRunner {
	return &blockingFirstCallRunner{
		firstCallStarted: make(chan struct{}),
		releaseFirstCall: make(chan struct{}),
	}
}

func (r *blockingFirstCallRunner) Run(_ context.Context, name string, args ...string) ([]byte, error) {
	callArgs := make([]string, len(args))
	copy(callArgs, args)

	r.mu.Lock()
	r.calls = append(r.calls, runnerCall{name: name, args: callArgs})
	callNum := len(r.calls)
	r.mu.Unlock()

	if callNum == 1 {
		close(r.firstCallStarted)
		<-r.releaseFirstCall
	}
	return []byte("ok"), nil
}

func (r *blockingFirstCallRunner) CallCount() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return len(r.calls)
}

func newAPITestServer(t *testing.T, runner target.Runner) (*Server, *db.Store) {
	t.Helper()
	ctx := context.Background()
	store, err := db.Open(ctx, filepath.Join(t.TempDir(), "state.db"))
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	t.Cleanup(func() { _ = store.Close() })
	if err := db.ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}
	cfg := config.DefaultConfig()
	cfg.CommandTimeout = 1 * time.Second
	executor := target.NewExecutorWithRunner(cfg, runner)
	return NewServerWithDeps(cfg, store, executor), store
}

func doJSONRequest(t *testing.T, handler http.Handler, method, path string, body any) *httptest.ResponseRecorder {
	t.Helper()
	var reader io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			t.Fatalf("marshal request body: %v", err)
		}
		reader = bytes.NewReader(b)
	}
	req := httptest.NewRequest(method, path, reader)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	rec := httptest.NewRecorder()
	handler.ServeHTTP(rec, req)
	return rec
}

func decodeJSON[T any](t *testing.T, rec *httptest.ResponseRecorder) T {
	t.Helper()
	var out T
	if err := json.NewDecoder(rec.Body).Decode(&out); err != nil {
		t.Fatalf("decode response: %v body=%q", err, rec.Body.String())
	}
	return out
}

func seedTarget(t *testing.T, store *db.Store, targetID, targetName string) {
	t.Helper()
	now := time.Now().UTC()
	if err := store.UpsertTarget(context.Background(), model.Target{
		TargetID:      targetID,
		TargetName:    targetName,
		Kind:          model.TargetKindLocal,
		ConnectionRef: "",
		Health:        model.TargetHealthOK,
		UpdatedAt:     now,
	}); err != nil {
		t.Fatalf("seed target: %v", err)
	}
}

func seedPaneRuntimeState(t *testing.T, store *db.Store, pane model.Pane, runtime model.Runtime, state model.StateRow) {
	t.Helper()
	ctx := context.Background()
	if err := store.UpsertPane(ctx, pane); err != nil {
		t.Fatalf("seed pane: %v", err)
	}
	if err := store.InsertRuntime(ctx, runtime); err != nil {
		t.Fatalf("seed runtime: %v", err)
	}
	if err := store.UpsertState(ctx, state); err != nil {
		t.Fatalf("seed state: %v", err)
	}
}

func decodeWatchLines(t *testing.T, body string) []api.WatchLine {
	t.Helper()
	lines := strings.Split(strings.TrimSpace(body), "\n")
	out := make([]api.WatchLine, 0, len(lines))
	for _, line := range lines {
		if strings.TrimSpace(line) == "" {
			continue
		}
		var wl api.WatchLine
		if err := json.Unmarshal([]byte(line), &wl); err != nil {
			t.Fatalf("unmarshal watch line: %v line=%q", err, line)
		}
		out = append(out, wl)
	}
	return out
}

func TestMethodNotAllowedReturnsStructuredErrorEnvelope(t *testing.T) {
	runner := &stubRunner{}
	srv, _ := newAPITestServer(t, runner)

	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPatch, "/v1/targets", map[string]any{"name": "x"})
	if rec.Code != http.StatusMethodNotAllowed {
		t.Fatalf("expected 405, got %d", rec.Code)
	}
	if allow := rec.Header().Get("Allow"); allow != "GET, POST" {
		t.Fatalf("expected allow header 'GET, POST', got %q", allow)
	}
	payload := decodeJSON[api.ErrorResponse](t, rec)
	if payload.SchemaVersion != "v1" {
		t.Fatalf("expected schema_version=v1, got %+v", payload)
	}
	if payload.Error.Code != model.ErrRefInvalid {
		t.Fatalf("expected error code %s, got %+v", model.ErrRefInvalid, payload)
	}
}

func TestTargetsCreateConnectDeleteContract(t *testing.T) {
	runner := &stubRunner{out: []byte("session\n")}
	srv, _ := newAPITestServer(t, runner)

	createRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/targets", map[string]any{
		"name":           "vm1",
		"kind":           "ssh",
		"connection_ref": "vm1",
		"is_default":     true,
	})
	if createRec.Code != http.StatusCreated {
		t.Fatalf("expected 201, got %d body=%s", createRec.Code, createRec.Body.String())
	}
	created := decodeJSON[api.TargetsEnvelope](t, createRec)
	if len(created.Targets) != 1 {
		t.Fatalf("expected one target, got %+v", created)
	}
	if created.Targets[0].TargetName != "vm1" || !created.Targets[0].IsDefault {
		t.Fatalf("unexpected create payload: %+v", created.Targets[0])
	}
	if created.Targets[0].Kind != "ssh" || created.Targets[0].ConnectionRef != "vm1" {
		t.Fatalf("expected kind=ssh and connection_ref=vm1, got %+v", created.Targets[0])
	}

	connectRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/targets/vm1/connect", nil)
	if connectRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", connectRec.Code, connectRec.Body.String())
	}
	connected := decodeJSON[api.TargetsEnvelope](t, connectRec)
	if len(connected.Targets) != 1 {
		t.Fatalf("expected one target, got %+v", connected)
	}
	if connected.Targets[0].Health != string(model.TargetHealthOK) {
		t.Fatalf("expected health ok, got %+v", connected.Targets[0])
	}
	if connected.Targets[0].LastSeenAt == nil {
		t.Fatalf("expected last_seen_at to be set, got %+v", connected.Targets[0])
	}

	if len(runner.calls) == 0 {
		t.Fatalf("expected runner to be called")
	}
	lastCall := runner.calls[len(runner.calls)-1]
	if lastCall.name != "ssh" {
		t.Fatalf("expected ssh command, got %+v", lastCall)
	}
	argsJoined := strings.Join(lastCall.args, " ")
	if !strings.Contains(argsJoined, "tmux") || !strings.Contains(argsJoined, "list-sessions") {
		t.Fatalf("expected tmux list-sessions in args, got %q", argsJoined)
	}
	if !strings.Contains(argsJoined, "vm1") {
		t.Fatalf("expected connection_ref vm1 in args, got %q", argsJoined)
	}

	deleteRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodDelete, "/v1/targets/vm1", nil)
	if deleteRec.Code != http.StatusNoContent {
		t.Fatalf("expected 204, got %d body=%s", deleteRec.Code, deleteRec.Body.String())
	}

	listRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/targets", nil)
	if listRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", listRec.Code, listRec.Body.String())
	}
	listed := decodeJSON[api.TargetsEnvelope](t, listRec)
	if len(listed.Targets) != 0 {
		t.Fatalf("expected target list to be empty, got %+v", listed.Targets)
	}
}

func TestAdaptersListContract(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()

	if err := store.UpsertAdapter(context.Background(), model.AdapterRecord{
		AdapterName:  "codex-notify-wrapper",
		AgentType:    "codex",
		Version:      "v1",
		Capabilities: []string{"event_driven", "supports_completed"},
		Enabled:      true,
		UpdatedAt:    now,
	}); err != nil {
		t.Fatalf("seed codex adapter: %v", err)
	}
	if err := store.UpsertAdapter(context.Background(), model.AdapterRecord{
		AdapterName:  "claude-hook",
		AgentType:    "claude",
		Version:      "v1",
		Capabilities: []string{"event_driven"},
		Enabled:      false,
		UpdatedAt:    now.Add(1 * time.Second),
	}); err != nil {
		t.Fatalf("seed claude adapter: %v", err)
	}

	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/adapters", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", rec.Code, rec.Body.String())
	}
	resp := decodeJSON[api.AdaptersEnvelope](t, rec)
	if len(resp.Adapters) != 2 {
		t.Fatalf("expected 2 adapters, got %+v", resp.Adapters)
	}
	if resp.Adapters[0].AdapterName != "claude-hook" || resp.Adapters[1].AdapterName != "codex-notify-wrapper" {
		t.Fatalf("expected deterministic adapter order, got %+v", resp.Adapters)
	}
	if resp.Adapters[0].Enabled {
		t.Fatalf("expected claude-hook enabled=false, got %+v", resp.Adapters[0])
	}
	if !resp.Adapters[1].Enabled {
		t.Fatalf("expected codex-notify-wrapper enabled=true, got %+v", resp.Adapters[1])
	}
	if !resp.Adapters[0].Compatible || !resp.Adapters[1].Compatible {
		t.Fatalf("expected built-in adapters to be compatible, got %+v", resp.Adapters)
	}

	enabledRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/adapters?enabled=true", nil)
	if enabledRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", enabledRec.Code, enabledRec.Body.String())
	}
	enabledResp := decodeJSON[api.AdaptersEnvelope](t, enabledRec)
	if len(enabledResp.Adapters) != 1 || enabledResp.Adapters[0].AdapterName != "codex-notify-wrapper" {
		t.Fatalf("unexpected enabled=true filter response: %+v", enabledResp.Adapters)
	}

	disabledRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/adapters?enabled=false", nil)
	if disabledRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", disabledRec.Code, disabledRec.Body.String())
	}
	disabledResp := decodeJSON[api.AdaptersEnvelope](t, disabledRec)
	if len(disabledResp.Adapters) != 1 || disabledResp.Adapters[0].AdapterName != "claude-hook" {
		t.Fatalf("unexpected enabled=false filter response: %+v", disabledResp.Adapters)
	}

	invalidFilter := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/adapters?enabled=maybe", nil)
	if invalidFilter.Code != http.StatusBadRequest {
		t.Fatalf("expected 400 for invalid filter, got %d body=%s", invalidFilter.Code, invalidFilter.Body.String())
	}
}

func TestAdaptersEnableDisableRoutes(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()

	if err := store.UpsertAdapter(context.Background(), model.AdapterRecord{
		AdapterName:  "gemini-wrapper-parser",
		AgentType:    "gemini",
		Version:      "v1",
		Capabilities: []string{"event_driven"},
		Enabled:      true,
		UpdatedAt:    now,
	}); err != nil {
		t.Fatalf("seed adapter: %v", err)
	}
	if err := store.UpsertAdapter(context.Background(), model.AdapterRecord{
		AdapterName:  "future-adapter",
		AgentType:    "future",
		Version:      "v2",
		Capabilities: []string{"event_driven"},
		Enabled:      false,
		UpdatedAt:    now,
	}); err != nil {
		t.Fatalf("seed future adapter: %v", err)
	}

	disableRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/adapters/gemini-wrapper-parser/disable", nil)
	if disableRec.Code != http.StatusOK {
		t.Fatalf("expected 200 disable, got %d body=%s", disableRec.Code, disableRec.Body.String())
	}
	disableResp := decodeJSON[api.AdaptersEnvelope](t, disableRec)
	if len(disableResp.Adapters) != 1 || disableResp.Adapters[0].Enabled {
		t.Fatalf("expected disabled adapter response, got %+v", disableResp.Adapters)
	}

	enableRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/adapters/gemini-wrapper-parser/enable", nil)
	if enableRec.Code != http.StatusOK {
		t.Fatalf("expected 200 enable, got %d body=%s", enableRec.Code, enableRec.Body.String())
	}
	enableResp := decodeJSON[api.AdaptersEnvelope](t, enableRec)
	if len(enableResp.Adapters) != 1 || !enableResp.Adapters[0].Enabled {
		t.Fatalf("expected enabled adapter response, got %+v", enableResp.Adapters)
	}

	incompatibleEnable := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/adapters/future-adapter/enable", nil)
	if incompatibleEnable.Code != http.StatusPreconditionFailed {
		t.Fatalf("expected 412 for incompatible enable, got %d body=%s", incompatibleEnable.Code, incompatibleEnable.Body.String())
	}

	missing := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/adapters/missing/enable", nil)
	if missing.Code != http.StatusNotFound {
		t.Fatalf("expected 404 for missing adapter, got %d body=%s", missing.Code, missing.Body.String())
	}
}

func TestAdaptersMethodNotAllowed(t *testing.T) {
	runner := &stubRunner{}
	srv, _ := newAPITestServer(t, runner)

	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/adapters", nil)
	if rec.Code != http.StatusMethodNotAllowed {
		t.Fatalf("expected 405, got %d body=%s", rec.Code, rec.Body.String())
	}
	if allow := rec.Header().Get("Allow"); allow != "GET" {
		t.Fatalf("expected Allow=GET, got %q", allow)
	}
	errResp := decodeJSON[api.ErrorResponse](t, rec)
	if errResp.Error.Code != model.ErrRefInvalid {
		t.Fatalf("unexpected error response: %+v", errResp)
	}

	routeRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/adapters/gemini-wrapper-parser/enable", nil)
	if routeRec.Code != http.StatusMethodNotAllowed {
		t.Fatalf("expected 405, got %d body=%s", routeRec.Code, routeRec.Body.String())
	}
	if allow := routeRec.Header().Get("Allow"); allow != "POST" {
		t.Fatalf("expected Allow=POST, got %q", allow)
	}
}

func TestListEndpointsFilterSummaryAndAggregation(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	waitingEventAt := now.Add(-1 * time.Second)
	seedTarget(t, store, "t1", "t1")
	seedTarget(t, store, "t2", "t2")

	pid1 := int64(101)
	seedPaneRuntimeState(t, store,
		model.Pane{TargetID: "t1", PaneID: "%1", SessionName: "s-main", WindowID: "@2", WindowName: "w2", UpdatedAt: now},
		model.Runtime{RuntimeID: "rt-1", TargetID: "t1", PaneID: "%1", TmuxServerBootID: "boot", PaneEpoch: 1, AgentType: "codex", PID: &pid1, StartedAt: now},
		model.StateRow{
			TargetID:      "t1",
			PaneID:        "%1",
			RuntimeID:     "rt-1",
			State:         model.StateWaitingInput,
			ReasonCode:    "test",
			Confidence:    "high",
			StateVersion:  1,
			StateSource:   model.SourceNotify,
			LastEventType: "input-requested",
			LastEventAt:   &waitingEventAt,
			LastSeenAt:    now,
			UpdatedAt:     now,
		},
	)
	pid2 := int64(102)
	seedPaneRuntimeState(t, store,
		model.Pane{TargetID: "t1", PaneID: "%2", SessionName: "s-main", WindowID: "@1", WindowName: "w1", UpdatedAt: now},
		model.Runtime{RuntimeID: "rt-2", TargetID: "t1", PaneID: "%2", TmuxServerBootID: "boot", PaneEpoch: 1, AgentType: "codex", PID: &pid2, StartedAt: now.Add(1 * time.Second)},
		model.StateRow{TargetID: "t1", PaneID: "%2", RuntimeID: "rt-2", State: model.StateRunning, ReasonCode: "test", Confidence: "high", StateVersion: 1, LastSeenAt: now, UpdatedAt: now},
	)
	pid3 := int64(103)
	seedPaneRuntimeState(t, store,
		model.Pane{TargetID: "t2", PaneID: "%3", SessionName: "s-other", WindowID: "@9", WindowName: "w9", UpdatedAt: now},
		model.Runtime{RuntimeID: "rt-3", TargetID: "t2", PaneID: "%3", TmuxServerBootID: "boot", PaneEpoch: 1, AgentType: "claude", PID: &pid3, StartedAt: now},
		model.StateRow{TargetID: "t2", PaneID: "%3", RuntimeID: "rt-3", State: model.StateIdle, ReasonCode: "test", Confidence: "high", StateVersion: 1, LastSeenAt: now, UpdatedAt: now},
	)

	panesRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/panes?target=t1", nil)
	if panesRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", panesRec.Code, panesRec.Body.String())
	}
	panes := decodeJSON[api.ListEnvelope[api.PaneItem]](t, panesRec)
	if len(panes.Items) != 2 {
		t.Fatalf("expected 2 panes, got %+v", panes.Items)
	}
	for _, item := range panes.Items {
		if item.AgentPresence != "managed" {
			t.Fatalf("expected managed agent_presence, got %+v", item)
		}
		if item.DisplayCategory == "" {
			t.Fatalf("expected display_category, got %+v", item)
		}
	}
	waitingPane := findPaneItem(t, panes.Items, "t1", "s-main", "%1")
	if waitingPane.StateSource != string(model.SourceNotify) || waitingPane.LastEventType != "input-requested" {
		t.Fatalf("expected provenance on waiting pane, got %+v", waitingPane)
	}
	if waitingPane.AwaitingKind != "input" {
		t.Fatalf("expected awaiting_response_kind=input, got %+v", waitingPane)
	}
	if waitingPane.LastEventAt == nil {
		t.Fatalf("expected last_event_at set, got %+v", waitingPane)
	}
	if panes.Summary.ByTarget["t1"] != 2 {
		t.Fatalf("unexpected pane summary: %+v", panes.Summary)
	}
	if panes.Summary.ByCategory["attention"] != 1 || panes.Summary.ByCategory["running"] != 1 {
		t.Fatalf("unexpected pane summary by_category: %+v", panes.Summary.ByCategory)
	}
	if _, ok := panes.Summary.ByTarget["t2"]; ok {
		t.Fatalf("unexpected t2 summary for filtered query: %+v", panes.Summary.ByTarget)
	}
	if len(panes.RespondedTargets) != 1 || panes.RespondedTargets[0] != "t1" {
		t.Fatalf("unexpected responded targets: %+v", panes.RespondedTargets)
	}

	windowsRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/windows?target=t1", nil)
	if windowsRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", windowsRec.Code, windowsRec.Body.String())
	}
	windows := decodeJSON[api.ListEnvelope[api.WindowItem]](t, windowsRec)
	if len(windows.Items) != 2 {
		t.Fatalf("expected 2 windows, got %+v", windows.Items)
	}
	if windows.Items[0].Identity.WindowID != "@1" || windows.Items[1].Identity.WindowID != "@2" {
		t.Fatalf("expected deterministic window order, got %+v", windows.Items)
	}
	if windows.Summary.ByState[string(model.StateRunning)] != 1 || windows.Summary.ByState[string(model.StateWaitingInput)] != 1 {
		t.Fatalf("unexpected windows summary by_state: %+v", windows.Summary)
	}
	if windows.Summary.ByAgent["codex"] != 2 || windows.Summary.ByTarget["t1"] != 2 {
		t.Fatalf("unexpected windows summary by_agent/by_target: %+v", windows.Summary)
	}
	if windows.Items[0].TopCategory == "" || windows.Items[1].TopCategory == "" {
		t.Fatalf("expected top_category in windows: %+v", windows.Items)
	}
	if len(windows.RequestedTargets) != 1 || windows.RequestedTargets[0] != "t1" {
		t.Fatalf("unexpected windows requested targets: %+v", windows.RequestedTargets)
	}
	if len(windows.RespondedTargets) != 1 || windows.RespondedTargets[0] != "t1" {
		t.Fatalf("unexpected windows responded targets: %+v", windows.RespondedTargets)
	}

	sessionsRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/sessions?target=t1", nil)
	if sessionsRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", sessionsRec.Code, sessionsRec.Body.String())
	}
	sessions := decodeJSON[api.ListEnvelope[api.SessionItem]](t, sessionsRec)
	if len(sessions.Items) != 1 {
		t.Fatalf("expected one session item, got %+v", sessions.Items)
	}
	if sessions.Items[0].ByState[string(model.StateWaitingInput)] != 1 || sessions.Items[0].ByState[string(model.StateRunning)] != 1 {
		t.Fatalf("unexpected session by_state: %+v", sessions.Items[0].ByState)
	}
	if sessions.Summary.ByState[string(model.StateRunning)] != 1 || sessions.Summary.ByState[string(model.StateWaitingInput)] != 1 {
		t.Fatalf("unexpected sessions summary by_state: %+v", sessions.Summary)
	}
	if sessions.Summary.ByAgent["codex"] != 2 || sessions.Summary.ByTarget["t1"] != 2 {
		t.Fatalf("unexpected sessions summary by_agent/by_target: %+v", sessions.Summary)
	}
	if sessions.Items[0].TopCategory == "" {
		t.Fatalf("expected top_category in session item: %+v", sessions.Items[0])
	}
	if len(sessions.RequestedTargets) != 1 || sessions.RequestedTargets[0] != "t1" {
		t.Fatalf("unexpected sessions requested targets: %+v", sessions.RequestedTargets)
	}
	if len(sessions.RespondedTargets) != 1 || sessions.RespondedTargets[0] != "t1" {
		t.Fatalf("unexpected sessions responded targets: %+v", sessions.RespondedTargets)
	}
}

func TestWindowsAggregationCountsAndTopState(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")

	for i, st := range []model.CanonicalState{model.StateRunning, model.StateWaitingInput, model.StateIdle} {
		paneID := fmt.Sprintf("%%%d", i+1)
		runtimeID := fmt.Sprintf("rt-win-%d", i+1)
		pid := int64(600 + i)
		seedPaneRuntimeState(t, store,
			model.Pane{TargetID: "t1", PaneID: paneID, SessionName: "s1", WindowID: "@1", WindowName: "w1", UpdatedAt: now},
			model.Runtime{RuntimeID: runtimeID, TargetID: "t1", PaneID: paneID, TmuxServerBootID: "boot", PaneEpoch: 1, AgentType: "codex", PID: &pid, StartedAt: now.Add(time.Duration(i) * time.Second)},
			model.StateRow{TargetID: "t1", PaneID: paneID, RuntimeID: runtimeID, State: st, ReasonCode: "test", Confidence: "high", StateVersion: 1, LastSeenAt: now, UpdatedAt: now},
		)
	}

	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/windows?target=t1", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", rec.Code, rec.Body.String())
	}
	resp := decodeJSON[api.ListEnvelope[api.WindowItem]](t, rec)
	if len(resp.Items) != 1 {
		t.Fatalf("expected one window, got %+v", resp.Items)
	}
	item := resp.Items[0]
	if item.TotalPanes != 3 || item.RunningCount != 1 || item.WaitingCount != 1 {
		t.Fatalf("unexpected window counts: %+v", item)
	}
	if item.TopState != string(model.StateWaitingInput) {
		t.Fatalf("expected top_state=waiting_input, got %+v", item)
	}
}

func TestPanesUsesRuntimeFromStateWhenRuntimeEnded(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")

	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}
	rt := model.Runtime{
		RuntimeID:        "rt-ended",
		TargetID:         "t1",
		PaneID:           "%1",
		TmuxServerBootID: "boot",
		PaneEpoch:        1,
		AgentType:        "claude",
		StartedAt:        now,
	}
	if err := store.InsertRuntime(context.Background(), rt); err != nil {
		t.Fatalf("seed runtime: %v", err)
	}
	if err := store.EndRuntime(context.Background(), rt.RuntimeID, now.Add(1*time.Second)); err != nil {
		t.Fatalf("end runtime: %v", err)
	}
	if err := store.UpsertState(context.Background(), model.StateRow{
		TargetID:     "t1",
		PaneID:       "%1",
		RuntimeID:    rt.RuntimeID,
		State:        model.StateIdle,
		ReasonCode:   "test",
		Confidence:   "high",
		StateVersion: 1,
		LastSeenAt:   now,
		UpdatedAt:    now,
	}); err != nil {
		t.Fatalf("seed state: %v", err)
	}

	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/panes?target=t1", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", rec.Code, rec.Body.String())
	}
	resp := decodeJSON[api.ListEnvelope[api.PaneItem]](t, rec)
	if len(resp.Items) != 1 {
		t.Fatalf("expected one pane, got %+v", resp.Items)
	}
	if resp.Items[0].AgentType != "claude" {
		t.Fatalf("expected ended runtime agent_type fallback, got %+v", resp.Items[0])
	}
}

func TestPanesDerivesUnmanagedDisplayCategory(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")

	pid := int64(321)
	seedPaneRuntimeState(t, store,
		model.Pane{TargetID: "t1", PaneID: "%1", SessionName: "s1", WindowID: "@1", WindowName: "w1", UpdatedAt: now},
		model.Runtime{RuntimeID: "rt-none", TargetID: "t1", PaneID: "%1", TmuxServerBootID: "boot", PaneEpoch: 1, AgentType: "none", PID: &pid, StartedAt: now},
		model.StateRow{TargetID: "t1", PaneID: "%1", RuntimeID: "rt-none", State: model.StateUnknown, ReasonCode: "no_agent", Confidence: "high", StateVersion: 1, LastSeenAt: now, UpdatedAt: now},
	)

	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/panes?target=t1", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", rec.Code, rec.Body.String())
	}
	resp := decodeJSON[api.ListEnvelope[api.PaneItem]](t, rec)
	if len(resp.Items) != 1 {
		t.Fatalf("expected one pane, got %+v", resp.Items)
	}
	item := resp.Items[0]
	if item.AgentPresence != "none" || item.DisplayCategory != "unmanaged" {
		t.Fatalf("expected unmanaged pane classification, got %+v", item)
	}
	if resp.Summary.ByCategory["unmanaged"] != 1 {
		t.Fatalf("expected unmanaged summary count, got %+v", resp.Summary.ByCategory)
	}
}

func TestWatchSnapshotAndCursorResetContract(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	pid := int64(501)
	seedPaneRuntimeState(t, store,
		model.Pane{TargetID: "t1", PaneID: "%1", SessionName: "s1", WindowID: "@1", WindowName: "w1", UpdatedAt: now},
		model.Runtime{RuntimeID: "rt-watch", TargetID: "t1", PaneID: "%1", TmuxServerBootID: "boot", PaneEpoch: 1, AgentType: "codex", PID: &pid, StartedAt: now},
		model.StateRow{TargetID: "t1", PaneID: "%1", RuntimeID: "rt-watch", State: model.StateRunning, ReasonCode: "test", Confidence: "high", StateVersion: 1, LastSeenAt: now, UpdatedAt: now},
	)

	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/watch?scope=panes", nil)
	if first.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", first.Code, first.Body.String())
	}
	if ct := first.Header().Get("Content-Type"); ct != "application/x-ndjson" {
		t.Fatalf("unexpected content-type: %s", ct)
	}
	firstLines := decodeWatchLines(t, first.Body.String())
	if len(firstLines) != 1 || firstLines[0].Type != "snapshot" {
		t.Fatalf("expected one snapshot line, got %+v", firstLines)
	}
	streamID := firstLines[0].StreamID
	if streamID == "" {
		t.Fatalf("expected stream_id in watch line: %+v", firstLines[0])
	}
	if firstLines[0].Scope != "panes" {
		t.Fatalf("expected panes scope, got %+v", firstLines[0])
	}
	if !strings.HasPrefix(firstLines[0].Cursor, streamID+":") {
		t.Fatalf("expected cursor with stream prefix, got %+v", firstLines[0])
	}

	currentSeq := firstLines[0].Sequence
	currentCursor := fmt.Sprintf("%s:%d", streamID, currentSeq)
	currentRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/watch?scope=panes&cursor="+url.QueryEscape(currentCursor), nil)
	if currentRec.Code != http.StatusOK {
		t.Fatalf("expected 200 for current cursor, got %d body=%s", currentRec.Code, currentRec.Body.String())
	}
	currentLines := decodeWatchLines(t, currentRec.Body.String())
	if len(currentLines) != 1 || currentLines[0].Type != "snapshot" {
		t.Fatalf("expected single snapshot for current cursor, got %+v", currentLines)
	}

	invalidCursor := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/watch?scope=panes&cursor=%3A1", nil)
	if invalidCursor.Code != http.StatusBadRequest {
		t.Fatalf("expected 400 for invalid cursor, got %d body=%s", invalidCursor.Code, invalidCursor.Body.String())
	}
	invalidPayload := decodeJSON[api.ErrorResponse](t, invalidCursor)
	if invalidPayload.Error.Code != model.ErrCursorInvalid {
		t.Fatalf("expected %s, got %+v", model.ErrCursorInvalid, invalidPayload)
	}

	oldCursor := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, fmt.Sprintf("/v1/watch?scope=panes&cursor=%s:0", streamID), nil)
	if oldCursor.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", oldCursor.Code, oldCursor.Body.String())
	}
	oldCursorLines := decodeWatchLines(t, oldCursor.Body.String())
	if len(oldCursorLines) != 2 {
		t.Fatalf("expected reset+snapshot, got %+v", oldCursorLines)
	}
	if oldCursorLines[0].Type != "reset" || oldCursorLines[1].Type != "snapshot" {
		t.Fatalf("unexpected watch line types: %+v", oldCursorLines)
	}
	if oldCursorLines[0].Scope != "panes" || oldCursorLines[1].Scope != "panes" {
		t.Fatalf("unexpected watch scopes: %+v", oldCursorLines)
	}
	if oldCursorLines[0].Sequence >= oldCursorLines[1].Sequence {
		t.Fatalf("expected monotonic sequence, got %+v", oldCursorLines)
	}

	mismatchCursor := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/watch?scope=panes&cursor=other-stream:1", nil)
	if mismatchCursor.Code != http.StatusOK {
		t.Fatalf("expected 200 for stream mismatch cursor, got %d body=%s", mismatchCursor.Code, mismatchCursor.Body.String())
	}
	mismatchLines := decodeWatchLines(t, mismatchCursor.Body.String())
	if len(mismatchLines) != 2 {
		t.Fatalf("expected reset+snapshot for stream mismatch, got %+v", mismatchLines)
	}
	if mismatchLines[0].Type != "reset" || mismatchLines[1].Type != "snapshot" {
		t.Fatalf("unexpected stream mismatch watch types: %+v", mismatchLines)
	}
}

func TestWatchWindowsAndSessionsScopesContract(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	pid := int64(701)
	seedPaneRuntimeState(t, store,
		model.Pane{TargetID: "t1", PaneID: "%1", SessionName: "s1", WindowID: "@1", WindowName: "w1", UpdatedAt: now},
		model.Runtime{RuntimeID: "rt-scope-1", TargetID: "t1", PaneID: "%1", TmuxServerBootID: "boot", PaneEpoch: 1, AgentType: "codex", PID: &pid, StartedAt: now},
		model.StateRow{TargetID: "t1", PaneID: "%1", RuntimeID: "rt-scope-1", State: model.StateRunning, ReasonCode: "test", Confidence: "high", StateVersion: 1, LastSeenAt: now, UpdatedAt: now},
	)

	for _, scope := range []string{"windows", "sessions"} {
		first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/watch?scope="+scope, nil)
		if first.Code != http.StatusOK {
			t.Fatalf("scope=%s expected 200, got %d body=%s", scope, first.Code, first.Body.String())
		}
		lines := decodeWatchLines(t, first.Body.String())
		if len(lines) != 1 || lines[0].Type != "snapshot" || lines[0].Scope != scope {
			t.Fatalf("scope=%s expected one snapshot line, got %+v", scope, lines)
		}
		items, ok := lines[0].Items.([]any)
		if !ok || len(items) == 0 {
			t.Fatalf("scope=%s expected non-empty items array, got %+v", scope, lines[0].Items)
		}
		firstItem, ok := items[0].(map[string]any)
		if !ok {
			t.Fatalf("scope=%s expected map item, got %+v", scope, items[0])
		}
		switch scope {
		case "windows":
			if _, ok := firstItem["top_state"]; !ok {
				t.Fatalf("scope=%s expected top_state field: %+v", scope, firstItem)
			}
			if _, ok := firstItem["running_count"]; !ok {
				t.Fatalf("scope=%s expected running_count field: %+v", scope, firstItem)
			}
			if _, ok := firstItem["waiting_count"]; !ok {
				t.Fatalf("scope=%s expected waiting_count field: %+v", scope, firstItem)
			}
			if _, ok := firstItem["total_panes"]; !ok {
				t.Fatalf("scope=%s expected total_panes field: %+v", scope, firstItem)
			}
			if _, ok := firstItem["state"]; ok {
				t.Fatalf("scope=%s should not expose pane state field: %+v", scope, firstItem)
			}
		case "sessions":
			if _, ok := firstItem["by_state"]; !ok {
				t.Fatalf("scope=%s expected by_state field: %+v", scope, firstItem)
			}
			if _, ok := firstItem["by_agent"]; !ok {
				t.Fatalf("scope=%s expected by_agent field: %+v", scope, firstItem)
			}
			if _, ok := firstItem["total_panes"]; !ok {
				t.Fatalf("scope=%s expected total_panes field: %+v", scope, firstItem)
			}
			if _, ok := firstItem["state"]; ok {
				t.Fatalf("scope=%s should not expose pane state field: %+v", scope, firstItem)
			}
		}

		mismatch := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/watch?scope="+scope+"&cursor=other-stream:1", nil)
		if mismatch.Code != http.StatusOK {
			t.Fatalf("scope=%s mismatch cursor expected 200, got %d body=%s", scope, mismatch.Code, mismatch.Body.String())
		}
		mismatchLines := decodeWatchLines(t, mismatch.Body.String())
		if len(mismatchLines) != 2 {
			t.Fatalf("scope=%s expected reset+snapshot, got %+v", scope, mismatchLines)
		}
		if mismatchLines[0].Type != "reset" || mismatchLines[1].Type != "snapshot" {
			t.Fatalf("scope=%s unexpected line types: %+v", scope, mismatchLines)
		}
		if mismatchLines[0].Scope != scope || mismatchLines[1].Scope != scope {
			t.Fatalf("scope=%s unexpected scope values: %+v", scope, mismatchLines)
		}
		if mismatchLines[0].Sequence >= mismatchLines[1].Sequence {
			t.Fatalf("scope=%s expected monotonic sequence: %+v", scope, mismatchLines)
		}
	}
}

func TestTargetFilterDBErrorReturns500(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	seedTarget(t, store, "t1", "t1")
	if err := store.Close(); err != nil {
		t.Fatalf("close store: %v", err)
	}

	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/panes?target=t1", nil)
	if rec.Code != http.StatusInternalServerError {
		t.Fatalf("expected 500, got %d body=%s", rec.Code, rec.Body.String())
	}
	payload := decodeJSON[api.ErrorResponse](t, rec)
	if payload.Error.Code != model.ErrPreconditionFailed {
		t.Fatalf("expected %s, got %+v", model.ErrPreconditionFailed, payload)
	}
}

func TestWatchDefaultScopeAndInvalidScope(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	pid := int64(801)
	seedPaneRuntimeState(t, store,
		model.Pane{TargetID: "t1", PaneID: "%1", SessionName: "s1", WindowID: "@1", WindowName: "w1", UpdatedAt: now},
		model.Runtime{RuntimeID: "rt-default-scope", TargetID: "t1", PaneID: "%1", TmuxServerBootID: "boot", PaneEpoch: 1, AgentType: "codex", PID: &pid, StartedAt: now},
		model.StateRow{TargetID: "t1", PaneID: "%1", RuntimeID: "rt-default-scope", State: model.StateRunning, ReasonCode: "test", Confidence: "high", StateVersion: 1, LastSeenAt: now, UpdatedAt: now},
	)

	defaultRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/watch", nil)
	if defaultRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", defaultRec.Code, defaultRec.Body.String())
	}
	defaultLines := decodeWatchLines(t, defaultRec.Body.String())
	if len(defaultLines) != 1 || defaultLines[0].Scope != "panes" {
		t.Fatalf("expected default panes scope, got %+v", defaultLines)
	}

	invalidScopeRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/watch?scope=invalid", nil)
	if invalidScopeRec.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d body=%s", invalidScopeRec.Code, invalidScopeRec.Body.String())
	}
	invalidScopePayload := decodeJSON[api.ErrorResponse](t, invalidScopeRec)
	if invalidScopePayload.Error.Code != model.ErrRefInvalid {
		t.Fatalf("expected %s, got %+v", model.ErrRefInvalid, invalidScopePayload)
	}

	invalidCursorRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/watch?cursor=stream%3A-1", nil)
	if invalidCursorRec.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d body=%s", invalidCursorRec.Code, invalidCursorRec.Body.String())
	}
	invalidCursorPayload := decodeJSON[api.ErrorResponse](t, invalidCursorRec)
	if invalidCursorPayload.Error.Code != model.ErrCursorInvalid {
		t.Fatalf("expected %s, got %+v", model.ErrCursorInvalid, invalidCursorPayload)
	}
}

func TestTargetsValidationAndNotFoundErrors(t *testing.T) {
	runner := &stubRunner{err: fmt.Errorf("unreachable")}
	srv, _ := newAPITestServer(t, runner)

	cases := []struct {
		name string
		body string
	}{
		{name: "missing name", body: `{"kind":"local"}`},
		{name: "invalid kind", body: `{"name":"x","kind":"bad"}`},
		{name: "unknown field", body: `{"name":"x","unknown":1}`},
		{name: "invalid json", body: `{`},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			req := httptest.NewRequest(http.MethodPost, "/v1/targets", strings.NewReader(tc.body))
			req.Header.Set("Content-Type", "application/json")
			rec := httptest.NewRecorder()
			srv.httpSrv.Handler.ServeHTTP(rec, req)
			if rec.Code != http.StatusBadRequest {
				t.Fatalf("expected 400, got %d body=%s", rec.Code, rec.Body.String())
			}
			payload := decodeJSON[api.ErrorResponse](t, rec)
			if payload.Error.Code != model.ErrRefInvalid {
				t.Fatalf("expected %s, got %+v", model.ErrRefInvalid, payload)
			}
		})
	}

	connectNotFound := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/targets/missing/connect", nil)
	if connectNotFound.Code != http.StatusNotFound {
		t.Fatalf("expected 404, got %d body=%s", connectNotFound.Code, connectNotFound.Body.String())
	}
	connectNotFoundPayload := decodeJSON[api.ErrorResponse](t, connectNotFound)
	if connectNotFoundPayload.Error.Code != model.ErrRefNotFound {
		t.Fatalf("expected %s, got %+v", model.ErrRefNotFound, connectNotFoundPayload)
	}

	deleteNotFound := doJSONRequest(t, srv.httpSrv.Handler, http.MethodDelete, "/v1/targets/missing", nil)
	if deleteNotFound.Code != http.StatusNotFound {
		t.Fatalf("expected 404, got %d body=%s", deleteNotFound.Code, deleteNotFound.Body.String())
	}
	deleteNotFoundPayload := decodeJSON[api.ErrorResponse](t, deleteNotFound)
	if deleteNotFoundPayload.Error.Code != model.ErrRefNotFound {
		t.Fatalf("expected %s, got %+v", model.ErrRefNotFound, deleteNotFoundPayload)
	}

	invalidReq := httptest.NewRequest(http.MethodPost, "/v1/targets/x/connect", nil)
	invalidReq.URL.Path = "/v1/targets/%zz/connect"
	invalidEncoding := httptest.NewRecorder()
	srv.httpSrv.Handler.ServeHTTP(invalidEncoding, invalidReq)
	if invalidEncoding.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d body=%s", invalidEncoding.Code, invalidEncoding.Body.String())
	}
	invalidEncodingPayload := decodeJSON[api.ErrorResponse](t, invalidEncoding)
	if invalidEncodingPayload.Error.Code != model.ErrRefInvalidEncoding {
		t.Fatalf("expected %s, got %+v", model.ErrRefInvalidEncoding, invalidEncodingPayload)
	}
}

func TestPanesUnknownDefaultsAndNotFoundTarget(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%9",
		SessionName: "s9",
		WindowID:    "@9",
		WindowName:  "w9",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}

	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/panes?target=t1", nil)
	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", rec.Code, rec.Body.String())
	}
	resp := decodeJSON[api.ListEnvelope[api.PaneItem]](t, rec)
	if len(resp.Items) != 1 {
		t.Fatalf("expected one pane, got %+v", resp.Items)
	}
	if resp.Items[0].State != string(model.StateUnknown) || resp.Items[0].ReasonCode != "unsupported_signal" || resp.Items[0].Confidence != "low" {
		t.Fatalf("unexpected default state fields: %+v", resp.Items[0])
	}
	if resp.Items[0].AgentType != "unknown" {
		t.Fatalf("expected default agent_type=unknown, got %+v", resp.Items[0])
	}
	if resp.Summary.ByState[string(model.StateUnknown)] != 1 || resp.Summary.ByAgent["unknown"] != 1 {
		t.Fatalf("unexpected summary defaults: %+v", resp.Summary)
	}

	notFound := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/panes?target=missing", nil)
	if notFound.Code != http.StatusNotFound {
		t.Fatalf("expected 404, got %d body=%s", notFound.Code, notFound.Body.String())
	}
	notFoundPayload := decodeJSON[api.ErrorResponse](t, notFound)
	if notFoundPayload.Error.Code != model.ErrRefNotFound {
		t.Fatalf("expected %s, got %+v", model.ErrRefNotFound, notFoundPayload)
	}
}

func TestAttachActionIdempotent(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}

	body := map[string]any{
		"request_ref": "req-attach-1",
		"target":      "t1",
		"pane_id":     "%1",
	}
	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/attach", body)
	if first.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", first.Code, first.Body.String())
	}
	firstResp := decodeJSON[api.ActionResponse](t, first)
	if firstResp.ActionID == "" || firstResp.ResultCode == "" {
		t.Fatalf("unexpected attach response: %+v", firstResp)
	}

	second := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/attach", body)
	if second.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", second.Code, second.Body.String())
	}
	secondResp := decodeJSON[api.ActionResponse](t, second)
	if secondResp.ActionID != firstResp.ActionID {
		t.Fatalf("expected idempotent action_id, first=%+v second=%+v", firstResp, secondResp)
	}
	if secondResp.ResultCode != firstResp.ResultCode {
		t.Fatalf("expected stable result_code on replay, first=%+v second=%+v", firstResp, secondResp)
	}
	if firstResp.CompletedAt == nil || secondResp.CompletedAt == nil || *firstResp.CompletedAt != *secondResp.CompletedAt {
		t.Fatalf("expected stable completed_at on replay, first=%+v second=%+v", firstResp, secondResp)
	}
}

func TestAttachActionValidationAndNotFound(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	seedTarget(t, store, "t1", "t1")

	missingRef := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/attach", map[string]any{
		"target":  "t1",
		"pane_id": "%1",
	})
	if missingRef.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d body=%s", missingRef.Code, missingRef.Body.String())
	}
	missingRefPayload := decodeJSON[api.ErrorResponse](t, missingRef)
	if missingRefPayload.Error.Code != model.ErrRefInvalid {
		t.Fatalf("expected %s, got %+v", model.ErrRefInvalid, missingRefPayload)
	}

	missingTarget := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/attach", map[string]any{
		"request_ref": "req-attach-2",
		"target":      "missing",
		"pane_id":     "%1",
	})
	if missingTarget.Code != http.StatusNotFound {
		t.Fatalf("expected 404, got %d body=%s", missingTarget.Code, missingTarget.Body.String())
	}
	missingTargetPayload := decodeJSON[api.ErrorResponse](t, missingTarget)
	if missingTargetPayload.Error.Code != model.ErrRefNotFound {
		t.Fatalf("expected %s, got %+v", model.ErrRefNotFound, missingTargetPayload)
	}

	missingPane := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/attach", map[string]any{
		"request_ref": "req-attach-3",
		"target":      "t1",
		"pane_id":     "%404",
	})
	if missingPane.Code != http.StatusNotFound {
		t.Fatalf("expected 404, got %d body=%s", missingPane.Code, missingPane.Body.String())
	}
	missingPanePayload := decodeJSON[api.ErrorResponse](t, missingPane)
	if missingPanePayload.Error.Code != model.ErrRefNotFound {
		t.Fatalf("expected %s, got %+v", model.ErrRefNotFound, missingPanePayload)
	}

	unknownFieldReq := httptest.NewRequest(http.MethodPost, "/v1/actions/attach", strings.NewReader(`{"request_ref":"r","target":"t1","pane_id":"%1","unknown":1}`))
	unknownFieldReq.Header.Set("Content-Type", "application/json")
	unknownFieldRec := httptest.NewRecorder()
	srv.httpSrv.Handler.ServeHTTP(unknownFieldRec, unknownFieldReq)
	if unknownFieldRec.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d body=%s", unknownFieldRec.Code, unknownFieldRec.Body.String())
	}
	unknownFieldPayload := decodeJSON[api.ErrorResponse](t, unknownFieldRec)
	if unknownFieldPayload.Error.Code != model.ErrRefInvalid {
		t.Fatalf("expected %s, got %+v", model.ErrRefInvalid, unknownFieldPayload)
	}
}

func TestAttachActionIdempotencyPayloadMismatch(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane 1: %v", err)
	}
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%2",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane 2: %v", err)
	}

	base := map[string]any{
		"request_ref": "req-attach-conflict",
		"target":      "t1",
		"pane_id":     "%1",
	}
	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/attach", base)
	if first.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", first.Code, first.Body.String())
	}

	conflict := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/attach", map[string]any{
		"request_ref": "req-attach-conflict",
		"target":      "t1",
		"pane_id":     "%2",
	})
	if conflict.Code != http.StatusConflict {
		t.Fatalf("expected 409, got %d body=%s", conflict.Code, conflict.Body.String())
	}
	conflictPayload := decodeJSON[api.ErrorResponse](t, conflict)
	if conflictPayload.Error.Code != model.ErrIdempotencyConflict {
		t.Fatalf("expected %s, got %+v", model.ErrIdempotencyConflict, conflictPayload)
	}
}

func TestAttachActionFailClosedRuntimeGuard(t *testing.T) {
	runner := &stubRunner{}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	pid := int64(8888)
	seedPaneRuntimeState(t, store,
		model.Pane{
			TargetID:    "t1",
			PaneID:      "%1",
			SessionName: "s1",
			WindowID:    "@1",
			WindowName:  "w1",
			UpdatedAt:   now,
		},
		model.Runtime{
			RuntimeID:        "rt-guard-attach-1",
			TargetID:         "t1",
			PaneID:           "%1",
			TmuxServerBootID: "boot-1",
			PaneEpoch:        1,
			AgentType:        "codex",
			PID:              &pid,
			StartedAt:        now.Add(-1 * time.Minute),
		},
		model.StateRow{
			TargetID:     "t1",
			PaneID:       "%1",
			RuntimeID:    "rt-guard-attach-1",
			State:        model.StateRunning,
			ReasonCode:   "heartbeat",
			Confidence:   "high",
			StateVersion: 10,
			LastSeenAt:   now,
			UpdatedAt:    now,
		},
	)

	staleRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/attach", map[string]any{
		"request_ref": "req-attach-guard-stale",
		"target":      "t1",
		"pane_id":     "%1",
		"if_runtime":  "rt-guard-attach-old",
	})
	if staleRec.Code != http.StatusConflict {
		t.Fatalf("expected 409, got %d body=%s", staleRec.Code, staleRec.Body.String())
	}
	staleResp := decodeJSON[api.ErrorResponse](t, staleRec)
	if staleResp.Error.Code != model.ErrRuntimeStale {
		t.Fatalf("expected %s, got %+v", model.ErrRuntimeStale, staleResp)
	}

	forceRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/attach", map[string]any{
		"request_ref": "req-attach-guard-force",
		"target":      "t1",
		"pane_id":     "%1",
		"if_runtime":  "rt-guard-attach-old",
		"force_stale": true,
	})
	if forceRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", forceRec.Code, forceRec.Body.String())
	}
	forceResp := decodeJSON[api.ActionResponse](t, forceRec)
	if forceResp.ActionID == "" {
		t.Fatalf("expected action response, got %+v", forceResp)
	}
}

func TestSendActionFailClosedStateAndFreshnessGuards(t *testing.T) {
	runner := &stubRunner{out: []byte("ok")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	pid := int64(8989)
	staleUpdatedAt := now.Add(-2 * time.Minute)
	seedPaneRuntimeState(t, store,
		model.Pane{
			TargetID:    "t1",
			PaneID:      "%1",
			SessionName: "s1",
			WindowID:    "@1",
			WindowName:  "w1",
			UpdatedAt:   now,
		},
		model.Runtime{
			RuntimeID:        "rt-guard-send-1",
			TargetID:         "t1",
			PaneID:           "%1",
			TmuxServerBootID: "boot-1",
			PaneEpoch:        1,
			AgentType:        "codex",
			PID:              &pid,
			StartedAt:        now.Add(-3 * time.Minute),
		},
		model.StateRow{
			TargetID:     "t1",
			PaneID:       "%1",
			RuntimeID:    "rt-guard-send-1",
			State:        model.StateWaitingInput,
			ReasonCode:   "waiting_input",
			Confidence:   "high",
			StateVersion: 11,
			LastSeenAt:   staleUpdatedAt,
			UpdatedAt:    staleUpdatedAt,
		},
	)

	stateMismatch := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", map[string]any{
		"request_ref": "req-send-guard-state",
		"target":      "t1",
		"pane_id":     "%1",
		"text":        "hello",
		"if_state":    "running",
	})
	if stateMismatch.Code != http.StatusConflict {
		t.Fatalf("expected 409, got %d body=%s", stateMismatch.Code, stateMismatch.Body.String())
	}
	stateMismatchResp := decodeJSON[api.ErrorResponse](t, stateMismatch)
	if stateMismatchResp.Error.Code != model.ErrPreconditionFailed {
		t.Fatalf("expected %s, got %+v", model.ErrPreconditionFailed, stateMismatchResp)
	}

	staleFreshness := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", map[string]any{
		"request_ref":       "req-send-guard-freshness",
		"target":            "t1",
		"pane_id":           "%1",
		"text":              "hello",
		"if_updated_within": "30s",
	})
	if staleFreshness.Code != http.StatusConflict {
		t.Fatalf("expected 409, got %d body=%s", staleFreshness.Code, staleFreshness.Body.String())
	}
	staleFreshnessResp := decodeJSON[api.ErrorResponse](t, staleFreshness)
	if staleFreshnessResp.Error.Code != model.ErrPreconditionFailed {
		t.Fatalf("expected %s, got %+v", model.ErrPreconditionFailed, staleFreshnessResp)
	}

	invalidDuration := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", map[string]any{
		"request_ref":       "req-send-guard-duration",
		"target":            "t1",
		"pane_id":           "%1",
		"text":              "hello",
		"if_updated_within": "bad-duration",
	})
	if invalidDuration.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d body=%s", invalidDuration.Code, invalidDuration.Body.String())
	}
	invalidDurationResp := decodeJSON[api.ErrorResponse](t, invalidDuration)
	if invalidDurationResp.Error.Code != model.ErrRefInvalid {
		t.Fatalf("expected %s, got %+v", model.ErrRefInvalid, invalidDurationResp)
	}

	forceRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", map[string]any{
		"request_ref":       "req-send-guard-force",
		"target":            "t1",
		"pane_id":           "%1",
		"text":              "hello",
		"if_state":          "running",
		"if_updated_within": "30s",
		"force_stale":       true,
	})
	if forceRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", forceRec.Code, forceRec.Body.String())
	}
	if len(runner.calls) != 1 {
		t.Fatalf("expected one executor call for force_stale path, got %d", len(runner.calls))
	}
}

func TestSendActionFailClosedSnapshotExpired(t *testing.T) {
	runner := &stubRunner{out: []byte("ok")}
	srv, store := newAPITestServer(t, runner)
	srv.snapshotTTL = -1 * time.Second

	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	pid := int64(9090)
	seedPaneRuntimeState(t, store,
		model.Pane{
			TargetID:    "t1",
			PaneID:      "%1",
			SessionName: "s1",
			WindowID:    "@1",
			WindowName:  "w1",
			UpdatedAt:   now,
		},
		model.Runtime{
			RuntimeID:        "rt-guard-expired-1",
			TargetID:         "t1",
			PaneID:           "%1",
			TmuxServerBootID: "boot-1",
			PaneEpoch:        1,
			AgentType:        "codex",
			PID:              &pid,
			StartedAt:        now.Add(-1 * time.Minute),
		},
		model.StateRow{
			TargetID:     "t1",
			PaneID:       "%1",
			RuntimeID:    "rt-guard-expired-1",
			State:        model.StateRunning,
			ReasonCode:   "heartbeat",
			Confidence:   "high",
			StateVersion: 12,
			LastSeenAt:   now,
			UpdatedAt:    now,
		},
	)

	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", map[string]any{
		"request_ref": "req-send-guard-expired",
		"target":      "t1",
		"pane_id":     "%1",
		"text":        "hello",
	})
	if rec.Code != http.StatusConflict {
		t.Fatalf("expected 409, got %d body=%s", rec.Code, rec.Body.String())
	}
	payload := decodeJSON[api.ErrorResponse](t, rec)
	if payload.Error.Code != model.ErrSnapshotExpired {
		t.Fatalf("expected %s, got %+v", model.ErrSnapshotExpired, payload)
	}
	if len(runner.calls) != 0 {
		t.Fatalf("expected no executor call on expired snapshot, got %d", len(runner.calls))
	}
}

func TestSendActionPersistsActionSnapshot(t *testing.T) {
	runner := &stubRunner{out: []byte("ok")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	pid := int64(9191)
	seedPaneRuntimeState(t, store,
		model.Pane{
			TargetID:    "t1",
			PaneID:      "%1",
			SessionName: "s1",
			WindowID:    "@1",
			WindowName:  "w1",
			UpdatedAt:   now,
		},
		model.Runtime{
			RuntimeID:        "rt-guard-snapshot-1",
			TargetID:         "t1",
			PaneID:           "%1",
			TmuxServerBootID: "boot-1",
			PaneEpoch:        1,
			AgentType:        "codex",
			PID:              &pid,
			StartedAt:        now.Add(-1 * time.Minute),
		},
		model.StateRow{
			TargetID:     "t1",
			PaneID:       "%1",
			RuntimeID:    "rt-guard-snapshot-1",
			State:        model.StateRunning,
			ReasonCode:   "heartbeat",
			Confidence:   "high",
			StateVersion: 21,
			LastSeenAt:   now,
			UpdatedAt:    now,
		},
	)

	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", map[string]any{
		"request_ref": "req-send-snapshot-persist",
		"target":      "t1",
		"pane_id":     "%1",
		"text":        "echo snapshot",
	})
	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", rec.Code, rec.Body.String())
	}
	resp := decodeJSON[api.ActionResponse](t, rec)
	if resp.ActionID == "" {
		t.Fatalf("expected action_id, got %+v", resp)
	}

	action, err := store.GetActionByTypeRequestRef(context.Background(), model.ActionTypeSend, "req-send-snapshot-persist")
	if err != nil {
		t.Fatalf("get action by request_ref: %v", err)
	}
	snapshot, err := store.GetActionSnapshotByActionID(context.Background(), action.ActionID)
	if err != nil {
		t.Fatalf("get action snapshot: %v", err)
	}
	if snapshot.ActionID != action.ActionID || snapshot.RuntimeID != "rt-guard-snapshot-1" {
		t.Fatalf("unexpected snapshot identity: %+v action=%+v", snapshot, action)
	}
	if snapshot.StateVersion != 21 {
		t.Fatalf("expected state_version=21, got %+v", snapshot)
	}
	if !snapshot.ExpiresAt.After(snapshot.ObservedAt) {
		t.Fatalf("expected expires_at > observed_at, got %+v", snapshot)
	}
}

func TestSendActionReplaySucceedsAfterStateChange(t *testing.T) {
	runner := &stubRunner{out: []byte("ok")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	pid1 := int64(1001)
	seedPaneRuntimeState(t, store,
		model.Pane{
			TargetID:    "t1",
			PaneID:      "%1",
			SessionName: "s1",
			WindowID:    "@1",
			WindowName:  "w1",
			UpdatedAt:   now,
		},
		model.Runtime{
			RuntimeID:        "rt-replay-1",
			TargetID:         "t1",
			PaneID:           "%1",
			TmuxServerBootID: "boot-replay",
			PaneEpoch:        1,
			AgentType:        "codex",
			PID:              &pid1,
			StartedAt:        now.Add(-2 * time.Minute),
		},
		model.StateRow{
			TargetID:     "t1",
			PaneID:       "%1",
			RuntimeID:    "rt-replay-1",
			State:        model.StateRunning,
			ReasonCode:   "heartbeat",
			Confidence:   "high",
			StateVersion: 1,
			LastSeenAt:   now,
			UpdatedAt:    now,
		},
	)

	req := map[string]any{
		"request_ref": "req-send-replay-pane-removed",
		"target":      "t1",
		"pane_id":     "%1",
		"text":        "echo replay",
		"if_runtime":  "rt-replay-1",
	}
	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", req)
	if first.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", first.Code, first.Body.String())
	}
	firstResp := decodeJSON[api.ActionResponse](t, first)
	callCount := len(runner.calls)

	pid2 := int64(1002)
	if err := store.EndRuntime(context.Background(), "rt-replay-1", now.Add(-90*time.Second)); err != nil {
		t.Fatalf("end old runtime: %v", err)
	}
	if err := store.InsertRuntime(context.Background(), model.Runtime{
		RuntimeID:        "rt-replay-2",
		TargetID:         "t1",
		PaneID:           "%1",
		TmuxServerBootID: "boot-replay",
		PaneEpoch:        2,
		AgentType:        "codex",
		PID:              &pid2,
		StartedAt:        now.Add(-1 * time.Minute),
	}); err != nil {
		t.Fatalf("insert replacement runtime: %v", err)
	}
	if err := store.UpsertState(context.Background(), model.StateRow{
		TargetID:     "t1",
		PaneID:       "%1",
		RuntimeID:    "rt-replay-2",
		State:        model.StateRunning,
		ReasonCode:   "heartbeat",
		Confidence:   "high",
		StateVersion: 2,
		LastSeenAt:   now,
		UpdatedAt:    now,
	}); err != nil {
		t.Fatalf("update state runtime: %v", err)
	}

	replay := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", req)
	if replay.Code != http.StatusOK {
		t.Fatalf("expected replay 200, got %d body=%s", replay.Code, replay.Body.String())
	}
	replayResp := decodeJSON[api.ActionResponse](t, replay)
	if replayResp.ActionID != firstResp.ActionID {
		t.Fatalf("expected replay action_id=%s, got %+v", firstResp.ActionID, replayResp)
	}
	if len(runner.calls) != callCount {
		t.Fatalf("expected replay without executor re-run, calls before=%d after=%d", callCount, len(runner.calls))
	}
}

func TestSendActionReplayDoesNotBackfillSnapshotWithoutState(t *testing.T) {
	runner := &stubRunner{out: []byte("ok")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}

	req := map[string]any{
		"request_ref": "req-send-replay-no-snapshot",
		"target":      "t1",
		"pane_id":     "%1",
		"text":        "echo replay",
	}
	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", req)
	if first.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", first.Code, first.Body.String())
	}
	action, err := store.GetActionByTypeRequestRef(context.Background(), model.ActionTypeSend, "req-send-replay-no-snapshot")
	if err != nil {
		t.Fatalf("get action: %v", err)
	}
	if _, err := store.GetActionSnapshotByActionID(context.Background(), action.ActionID); err != db.ErrNotFound {
		t.Fatalf("expected no snapshot before replay, got err=%v", err)
	}

	replay := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", req)
	if replay.Code != http.StatusOK {
		t.Fatalf("expected replay 200, got %d body=%s", replay.Code, replay.Body.String())
	}
	if _, err := store.GetActionSnapshotByActionID(context.Background(), action.ActionID); err != db.ErrNotFound {
		t.Fatalf("expected replay not to backfill snapshot, got err=%v", err)
	}
}

func TestSendActionIdempotentAndConflict(t *testing.T) {
	runner := &stubRunner{out: []byte("ok")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}

	req := map[string]any{
		"request_ref": "req-send-1",
		"target":      "t1",
		"pane_id":     "%1",
		"text":        "hello",
		"enter":       true,
	}
	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", req)
	if first.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", first.Code, first.Body.String())
	}
	firstResp := decodeJSON[api.ActionResponse](t, first)
	if firstResp.ActionID == "" || firstResp.ResultCode != "completed" {
		t.Fatalf("unexpected send response: %+v", firstResp)
	}
	if len(runner.calls) == 0 {
		t.Fatalf("expected executor call")
	}
	lastCall := runner.calls[len(runner.calls)-1]
	if lastCall.name != "tmux" {
		t.Fatalf("expected tmux command, got %+v", lastCall)
	}
	if strings.Join(lastCall.args, " ") != "send-keys -t %1 hello Enter" {
		t.Fatalf("unexpected send args: %+v", lastCall.args)
	}

	replay := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", req)
	if replay.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", replay.Code, replay.Body.String())
	}
	replayResp := decodeJSON[api.ActionResponse](t, replay)
	if replayResp.ActionID != firstResp.ActionID {
		t.Fatalf("expected same action_id on replay, first=%+v replay=%+v", firstResp, replayResp)
	}

	conflict := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", map[string]any{
		"request_ref": "req-send-1",
		"target":      "t1",
		"pane_id":     "%1",
		"text":        "different",
	})
	if conflict.Code != http.StatusConflict {
		t.Fatalf("expected 409, got %d body=%s", conflict.Code, conflict.Body.String())
	}
	conflictPayload := decodeJSON[api.ErrorResponse](t, conflict)
	if conflictPayload.Error.Code != model.ErrIdempotencyConflict {
		t.Fatalf("expected %s, got %+v", model.ErrIdempotencyConflict, conflictPayload)
	}
}

func TestSendActionValidationErrors(t *testing.T) {
	runner := &stubRunner{out: []byte("ok")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}

	cases := []struct {
		name    string
		body    map[string]any
		message string
	}{
		{
			name: "missing request_ref",
			body: map[string]any{
				"target":  "t1",
				"pane_id": "%1",
				"text":    "hello",
			},
			message: "request_ref, target, pane_id are required",
		},
		{
			name: "missing text and key",
			body: map[string]any{
				"request_ref": "req-send-validation-empty",
				"target":      "t1",
				"pane_id":     "%1",
			},
			message: "either text or key is required",
		},
		{
			name: "text and key together",
			body: map[string]any{
				"request_ref": "req-send-validation-both",
				"target":      "t1",
				"pane_id":     "%1",
				"text":        "hello",
				"key":         "C-c",
			},
			message: "text and key are mutually exclusive",
		},
		{
			name: "whitespace-only key",
			body: map[string]any{
				"request_ref": "req-send-validation-key-space",
				"target":      "t1",
				"pane_id":     "%1",
				"key":         "   ",
			},
			message: "either text or key is required",
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", tc.body)
			if rec.Code != http.StatusBadRequest {
				t.Fatalf("expected 400, got %d body=%s", rec.Code, rec.Body.String())
			}
			payload := decodeJSON[api.ErrorResponse](t, rec)
			if payload.Error.Code != model.ErrRefInvalid {
				t.Fatalf("expected %s, got %+v", model.ErrRefInvalid, payload)
			}
			if !strings.Contains(payload.Error.Message, tc.message) {
				t.Fatalf("expected message %q, got %+v", tc.message, payload.Error)
			}
		})
	}
}

func TestSendActionPreservesWhitespacePayload(t *testing.T) {
	runner := &stubRunner{out: []byte("ok")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}

	payload := "  hello\nworld  \n"
	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", map[string]any{
		"request_ref": "req-send-ws",
		"target":      "t1",
		"pane_id":     "%1",
		"text":        payload,
		"paste":       true,
	})
	if rec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", rec.Code, rec.Body.String())
	}
	if len(runner.calls) == 0 {
		t.Fatalf("expected executor call")
	}
	lastCall := runner.calls[len(runner.calls)-1]
	if lastCall.name != "tmux" {
		t.Fatalf("expected tmux command, got %+v", lastCall)
	}
	if len(lastCall.args) != 5 {
		t.Fatalf("unexpected send arg length: %+v", lastCall.args)
	}
	if lastCall.args[0] != "send-keys" || lastCall.args[1] != "-t" || lastCall.args[2] != "%1" || lastCall.args[3] != "-l" {
		t.Fatalf("unexpected send args: %+v", lastCall.args)
	}
	if lastCall.args[4] != payload {
		t.Fatalf("expected payload to preserve whitespace, got %q", lastCall.args[4])
	}
}

func TestSendActionConcurrentIdempotency(t *testing.T) {
	runner := newBlockingFirstCallRunner()
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}

	var sendReqCount int32
	secondStarted := make(chan struct{})
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodPost && r.URL.Path == "/v1/actions/send" {
			if atomic.AddInt32(&sendReqCount, 1) == 2 {
				close(secondStarted)
			}
		}
		srv.httpSrv.Handler.ServeHTTP(w, r)
	})

	reqBody := map[string]any{
		"request_ref": "req-send-concurrent",
		"target":      "t1",
		"pane_id":     "%1",
		"text":        "hello",
	}
	firstCh := make(chan *httptest.ResponseRecorder, 1)
	secondCh := make(chan *httptest.ResponseRecorder, 1)
	go func() {
		firstCh <- doJSONRequest(t, handler, http.MethodPost, "/v1/actions/send", reqBody)
	}()

	select {
	case <-runner.firstCallStarted:
	case <-time.After(2 * time.Second):
		t.Fatalf("timeout waiting for first executor call")
	}

	go func() {
		secondCh <- doJSONRequest(t, handler, http.MethodPost, "/v1/actions/send", reqBody)
	}()

	select {
	case <-secondStarted:
	case <-time.After(2 * time.Second):
		t.Fatalf("timeout waiting for second request to start")
	}
	close(runner.releaseFirstCall)

	var firstRec *httptest.ResponseRecorder
	var secondRec *httptest.ResponseRecorder
	select {
	case firstRec = <-firstCh:
	case <-time.After(2 * time.Second):
		t.Fatalf("timeout waiting for first response")
	}
	select {
	case secondRec = <-secondCh:
	case <-time.After(2 * time.Second):
		t.Fatalf("timeout waiting for second response")
	}
	if firstRec.Code != http.StatusOK || secondRec.Code != http.StatusOK {
		t.Fatalf("expected both 200, first=%d second=%d", firstRec.Code, secondRec.Code)
	}

	firstResp := decodeJSON[api.ActionResponse](t, firstRec)
	secondResp := decodeJSON[api.ActionResponse](t, secondRec)
	if firstResp.ActionID == "" || secondResp.ActionID == "" || firstResp.ActionID != secondResp.ActionID {
		t.Fatalf("expected same non-empty action_id, first=%+v second=%+v", firstResp, secondResp)
	}
	if runner.CallCount() != 1 {
		t.Fatalf("expected single executor call for concurrent idempotent requests, got %d", runner.CallCount())
	}
}

func TestViewOutputActionConcurrentIdempotency(t *testing.T) {
	runner := newBlockingFirstCallRunner()
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}

	var reqCount int32
	secondStarted := make(chan struct{})
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodPost && r.URL.Path == "/v1/actions/view-output" {
			if atomic.AddInt32(&reqCount, 1) == 2 {
				close(secondStarted)
			}
		}
		srv.httpSrv.Handler.ServeHTTP(w, r)
	})

	reqBody := map[string]any{
		"request_ref": "req-view-concurrent",
		"target":      "t1",
		"pane_id":     "%1",
		"lines":       20,
	}
	firstCh := make(chan *httptest.ResponseRecorder, 1)
	secondCh := make(chan *httptest.ResponseRecorder, 1)
	go func() {
		firstCh <- doJSONRequest(t, handler, http.MethodPost, "/v1/actions/view-output", reqBody)
	}()
	select {
	case <-runner.firstCallStarted:
	case <-time.After(2 * time.Second):
		t.Fatalf("timeout waiting for first executor call")
	}
	go func() {
		secondCh <- doJSONRequest(t, handler, http.MethodPost, "/v1/actions/view-output", reqBody)
	}()
	select {
	case <-secondStarted:
	case <-time.After(2 * time.Second):
		t.Fatalf("timeout waiting for second request to start")
	}
	close(runner.releaseFirstCall)

	var firstRec *httptest.ResponseRecorder
	var secondRec *httptest.ResponseRecorder
	select {
	case firstRec = <-firstCh:
	case <-time.After(2 * time.Second):
		t.Fatalf("timeout waiting for first response")
	}
	select {
	case secondRec = <-secondCh:
	case <-time.After(2 * time.Second):
		t.Fatalf("timeout waiting for second response")
	}
	if firstRec.Code != http.StatusOK || secondRec.Code != http.StatusOK {
		t.Fatalf("expected both 200, first=%d second=%d", firstRec.Code, secondRec.Code)
	}
	firstResp := decodeJSON[api.ActionResponse](t, firstRec)
	secondResp := decodeJSON[api.ActionResponse](t, secondRec)
	if firstResp.ActionID == "" || secondResp.ActionID == "" || firstResp.ActionID != secondResp.ActionID {
		t.Fatalf("expected same non-empty action_id, first=%+v second=%+v", firstResp, secondResp)
	}
	if runner.CallCount() != 1 {
		t.Fatalf("expected single executor call for concurrent idempotent requests, got %d", runner.CallCount())
	}
}

func TestViewOutputAction(t *testing.T) {
	runner := &stubRunner{out: []byte("line1\nline2\n")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}
	respRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/view-output", map[string]any{
		"request_ref": "req-view-1",
		"target":      "t1",
		"pane_id":     "%1",
		"lines":       20,
	})
	if respRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", respRec.Code, respRec.Body.String())
	}
	resp := decodeJSON[api.ActionResponse](t, respRec)
	if resp.ActionID == "" || resp.ResultCode != "completed" {
		t.Fatalf("unexpected view-output response: %+v", resp)
	}
	lastCall := runner.calls[len(runner.calls)-1]
	if lastCall.name != "tmux" {
		t.Fatalf("expected tmux command, got %+v", lastCall)
	}
	if strings.Join(lastCall.args, " ") != "capture-pane -t %1 -p -S -20" {
		t.Fatalf("unexpected capture args: %+v", lastCall.args)
	}
}

func TestKillActionKeyAndSignalMode(t *testing.T) {
	runner := &stubRunner{out: []byte("ok")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}

	keyRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/kill", map[string]any{
		"request_ref": "req-kill-key",
		"target":      "t1",
		"pane_id":     "%1",
		"mode":        "key",
	})
	if keyRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", keyRec.Code, keyRec.Body.String())
	}
	keyCall := runner.calls[len(runner.calls)-1]
	if keyCall.name != "tmux" || strings.Join(keyCall.args, " ") != "send-keys -t %1 C-c" {
		t.Fatalf("unexpected key kill command: %+v", keyCall)
	}

	noPID := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/kill", map[string]any{
		"request_ref": "req-kill-signal-no-pid",
		"target":      "t1",
		"pane_id":     "%1",
		"mode":        "signal",
		"signal":      "TERM",
	})
	if noPID.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d body=%s", noPID.Code, noPID.Body.String())
	}
	noPIDPayload := decodeJSON[api.ErrorResponse](t, noPID)
	if noPIDPayload.Error.Code != model.ErrPIDUnavailable {
		t.Fatalf("expected %s, got %+v", model.ErrPIDUnavailable, noPIDPayload)
	}
}

func TestViewOutputActionIdempotentAndConflict(t *testing.T) {
	runner := &stubRunner{out: []byte("output")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}

	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/view-output", map[string]any{
		"request_ref": "req-view-idem",
		"target":      "t1",
		"pane_id":     "%1",
	})
	if first.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", first.Code, first.Body.String())
	}
	firstResp := decodeJSON[api.ActionResponse](t, first)
	if firstResp.ActionID == "" {
		t.Fatalf("unexpected response: %+v", firstResp)
	}
	callCount := len(runner.calls)

	replay := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/view-output", map[string]any{
		"request_ref": "req-view-idem",
		"target":      "t1",
		"pane_id":     "%1",
		"lines":       200,
	})
	if replay.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", replay.Code, replay.Body.String())
	}
	replayResp := decodeJSON[api.ActionResponse](t, replay)
	if replayResp.ActionID != firstResp.ActionID {
		t.Fatalf("expected same action_id on replay, first=%+v replay=%+v", firstResp, replayResp)
	}
	if len(runner.calls) != callCount {
		t.Fatalf("expected replay without executor re-run, calls before=%d after=%d", callCount, len(runner.calls))
	}

	conflict := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/view-output", map[string]any{
		"request_ref": "req-view-idem",
		"target":      "t1",
		"pane_id":     "%1",
		"lines":       300,
	})
	if conflict.Code != http.StatusConflict {
		t.Fatalf("expected 409, got %d body=%s", conflict.Code, conflict.Body.String())
	}
	conflictPayload := decodeJSON[api.ErrorResponse](t, conflict)
	if conflictPayload.Error.Code != model.ErrIdempotencyConflict {
		t.Fatalf("expected %s, got %+v", model.ErrIdempotencyConflict, conflictPayload)
	}
}

func TestViewOutputActionReplayOutputContract(t *testing.T) {
	runner := &stubRunner{out: []byte("line1\nline2\n")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}

	req := map[string]any{
		"request_ref": "req-view-output-contract",
		"target":      "t1",
		"pane_id":     "%1",
		"lines":       20,
	}
	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/view-output", req)
	if first.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", first.Code, first.Body.String())
	}
	firstResp := decodeJSON[api.ActionResponse](t, first)
	if firstResp.Output == nil || *firstResp.Output != "line1\nline2\n" {
		t.Fatalf("expected output on first execution, got %+v", firstResp)
	}
	callCount := len(runner.calls)

	replay := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/view-output", req)
	if replay.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", replay.Code, replay.Body.String())
	}
	replayResp := decodeJSON[api.ActionResponse](t, replay)
	if replayResp.ActionID != firstResp.ActionID {
		t.Fatalf("expected same action_id on replay, first=%+v replay=%+v", firstResp, replayResp)
	}
	if replayResp.Output != nil {
		t.Fatalf("expected replay without output payload, got %+v", replayResp)
	}
	if len(runner.calls) != callCount {
		t.Fatalf("expected replay without executor re-run, calls before=%d after=%d", callCount, len(runner.calls))
	}
}

func TestKillActionIdempotentAndConflict(t *testing.T) {
	runner := &stubRunner{out: []byte("ok")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}

	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/kill", map[string]any{
		"request_ref": "req-kill-idem",
		"target":      "t1",
		"pane_id":     "%1",
	})
	if first.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", first.Code, first.Body.String())
	}
	firstResp := decodeJSON[api.ActionResponse](t, first)
	callCount := len(runner.calls)

	replay := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/kill", map[string]any{
		"request_ref": "req-kill-idem",
		"target":      "t1",
		"pane_id":     "%1",
		"mode":        "key",
		"signal":      "INT",
	})
	if replay.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", replay.Code, replay.Body.String())
	}
	replayResp := decodeJSON[api.ActionResponse](t, replay)
	if replayResp.ActionID != firstResp.ActionID {
		t.Fatalf("expected same action_id on replay, first=%+v replay=%+v", firstResp, replayResp)
	}
	if len(runner.calls) != callCount {
		t.Fatalf("expected replay without executor re-run, calls before=%d after=%d", callCount, len(runner.calls))
	}

	conflict := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/kill", map[string]any{
		"request_ref": "req-kill-idem",
		"target":      "t1",
		"pane_id":     "%1",
		"mode":        "signal",
		"signal":      "INT",
	})
	if conflict.Code != http.StatusConflict {
		t.Fatalf("expected 409, got %d body=%s", conflict.Code, conflict.Body.String())
	}
	conflictPayload := decodeJSON[api.ErrorResponse](t, conflict)
	if conflictPayload.Error.Code != model.ErrIdempotencyConflict {
		t.Fatalf("expected %s, got %+v", model.ErrIdempotencyConflict, conflictPayload)
	}
}

func TestKillActionSignalModeSuccessAndReplay(t *testing.T) {
	runner := &stubRunner{out: []byte("ok")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	pid := int64(4242)
	seedPaneRuntimeState(t, store,
		model.Pane{
			TargetID:    "t1",
			PaneID:      "%1",
			SessionName: "s1",
			WindowID:    "@1",
			WindowName:  "w1",
			UpdatedAt:   now,
		},
		model.Runtime{
			RuntimeID:        "rt-signal-1",
			TargetID:         "t1",
			PaneID:           "%1",
			TmuxServerBootID: "boot-1",
			PaneEpoch:        1,
			AgentType:        "codex",
			PID:              &pid,
			StartedAt:        now.Add(-1 * time.Minute),
		},
		model.StateRow{
			TargetID:     "t1",
			PaneID:       "%1",
			RuntimeID:    "rt-signal-1",
			State:        model.StateRunning,
			ReasonCode:   "heartbeat",
			Confidence:   "high",
			StateVersion: 1,
			LastSeenAt:   now,
			UpdatedAt:    now,
		},
	)

	req := map[string]any{
		"request_ref": "req-kill-signal-ok",
		"target":      "t1",
		"pane_id":     "%1",
		"mode":        "signal",
		"signal":      "term",
	}
	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/kill", req)
	if first.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", first.Code, first.Body.String())
	}
	firstResp := decodeJSON[api.ActionResponse](t, first)
	if firstResp.ActionID == "" || firstResp.ResultCode != "completed" {
		t.Fatalf("unexpected first response: %+v", firstResp)
	}
	if len(runner.calls) == 0 {
		t.Fatalf("expected runner call")
	}
	lastCall := runner.calls[len(runner.calls)-1]
	if lastCall.name != "kill" || strings.Join(lastCall.args, " ") != "-TERM 4242" {
		t.Fatalf("unexpected signal command: %+v", lastCall)
	}
	callCount := len(runner.calls)

	replay := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/kill", req)
	if replay.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", replay.Code, replay.Body.String())
	}
	replayResp := decodeJSON[api.ActionResponse](t, replay)
	if replayResp.ActionID != firstResp.ActionID {
		t.Fatalf("expected same action_id on replay, first=%+v replay=%+v", firstResp, replayResp)
	}
	if len(runner.calls) != callCount {
		t.Fatalf("expected replay without executor re-run, calls before=%d after=%d", callCount, len(runner.calls))
	}

	conflict := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/kill", map[string]any{
		"request_ref": "req-kill-signal-ok",
		"target":      "t1",
		"pane_id":     "%1",
		"mode":        "signal",
		"signal":      "KILL",
	})
	if conflict.Code != http.StatusConflict {
		t.Fatalf("expected 409, got %d body=%s", conflict.Code, conflict.Body.String())
	}
	conflictPayload := decodeJSON[api.ErrorResponse](t, conflict)
	if conflictPayload.Error.Code != model.ErrIdempotencyConflict {
		t.Fatalf("expected %s, got %+v", model.ErrIdempotencyConflict, conflictPayload)
	}
}

func TestKillActionConcurrentIdempotencySignalMode(t *testing.T) {
	runner := newBlockingFirstCallRunner()
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	pid := int64(4343)
	seedPaneRuntimeState(t, store,
		model.Pane{
			TargetID:    "t1",
			PaneID:      "%1",
			SessionName: "s1",
			WindowID:    "@1",
			WindowName:  "w1",
			UpdatedAt:   now,
		},
		model.Runtime{
			RuntimeID:        "rt-signal-concurrent-1",
			TargetID:         "t1",
			PaneID:           "%1",
			TmuxServerBootID: "boot-1",
			PaneEpoch:        1,
			AgentType:        "codex",
			PID:              &pid,
			StartedAt:        now.Add(-1 * time.Minute),
		},
		model.StateRow{
			TargetID:     "t1",
			PaneID:       "%1",
			RuntimeID:    "rt-signal-concurrent-1",
			State:        model.StateRunning,
			ReasonCode:   "heartbeat",
			Confidence:   "high",
			StateVersion: 1,
			LastSeenAt:   now,
			UpdatedAt:    now,
		},
	)

	var reqCount int32
	secondStarted := make(chan struct{})
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodPost && r.URL.Path == "/v1/actions/kill" {
			if atomic.AddInt32(&reqCount, 1) == 2 {
				close(secondStarted)
			}
		}
		srv.httpSrv.Handler.ServeHTTP(w, r)
	})
	reqBody := map[string]any{
		"request_ref": "req-kill-signal-concurrent",
		"target":      "t1",
		"pane_id":     "%1",
		"mode":        "signal",
		"signal":      "TERM",
	}
	firstCh := make(chan *httptest.ResponseRecorder, 1)
	secondCh := make(chan *httptest.ResponseRecorder, 1)
	go func() {
		firstCh <- doJSONRequest(t, handler, http.MethodPost, "/v1/actions/kill", reqBody)
	}()
	select {
	case <-runner.firstCallStarted:
	case <-time.After(2 * time.Second):
		t.Fatalf("timeout waiting for first executor call")
	}
	go func() {
		secondCh <- doJSONRequest(t, handler, http.MethodPost, "/v1/actions/kill", reqBody)
	}()
	select {
	case <-secondStarted:
	case <-time.After(2 * time.Second):
		t.Fatalf("timeout waiting for second request to start")
	}
	close(runner.releaseFirstCall)

	var firstRec *httptest.ResponseRecorder
	var secondRec *httptest.ResponseRecorder
	select {
	case firstRec = <-firstCh:
	case <-time.After(2 * time.Second):
		t.Fatalf("timeout waiting for first response")
	}
	select {
	case secondRec = <-secondCh:
	case <-time.After(2 * time.Second):
		t.Fatalf("timeout waiting for second response")
	}
	if firstRec.Code != http.StatusOK || secondRec.Code != http.StatusOK {
		t.Fatalf("expected both 200, first=%d second=%d", firstRec.Code, secondRec.Code)
	}
	firstResp := decodeJSON[api.ActionResponse](t, firstRec)
	secondResp := decodeJSON[api.ActionResponse](t, secondRec)
	if firstResp.ActionID == "" || secondResp.ActionID == "" || firstResp.ActionID != secondResp.ActionID {
		t.Fatalf("expected same non-empty action_id, first=%+v second=%+v", firstResp, secondResp)
	}
	if runner.CallCount() != 1 {
		t.Fatalf("expected single executor call for concurrent idempotent requests, got %d", runner.CallCount())
	}
}

func TestSendActionExecutorFailureReplay(t *testing.T) {
	runner := &stubRunner{err: fmt.Errorf("boom")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}
	req := map[string]any{
		"request_ref": "req-send-fail",
		"target":      "t1",
		"pane_id":     "%1",
		"text":        "hello",
	}
	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", req)
	if first.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", first.Code, first.Body.String())
	}
	firstResp := decodeJSON[api.ActionResponse](t, first)
	if firstResp.ResultCode != "failed" || firstResp.ErrorCode == nil || *firstResp.ErrorCode != model.ErrTargetUnreachable {
		t.Fatalf("expected failed/E_TARGET_UNREACHABLE, got %+v", firstResp)
	}
	callCount := len(runner.calls)

	replay := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", req)
	if replay.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", replay.Code, replay.Body.String())
	}
	replayResp := decodeJSON[api.ActionResponse](t, replay)
	if replayResp.ActionID != firstResp.ActionID || replayResp.ResultCode != "failed" {
		t.Fatalf("expected replay failed result, first=%+v replay=%+v", firstResp, replayResp)
	}
	if len(runner.calls) != callCount {
		t.Fatalf("expected replay without executor re-run, calls before=%d after=%d", callCount, len(runner.calls))
	}
}

func TestViewOutputActionExecutorFailureReplay(t *testing.T) {
	runner := &stubRunner{err: fmt.Errorf("boom")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}
	req := map[string]any{
		"request_ref": "req-view-fail",
		"target":      "t1",
		"pane_id":     "%1",
		"lines":       50,
	}
	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/view-output", req)
	if first.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", first.Code, first.Body.String())
	}
	firstResp := decodeJSON[api.ActionResponse](t, first)
	if firstResp.ResultCode != "failed" || firstResp.ErrorCode == nil || *firstResp.ErrorCode != model.ErrTargetUnreachable {
		t.Fatalf("expected failed/E_TARGET_UNREACHABLE, got %+v", firstResp)
	}
	callCount := len(runner.calls)

	replay := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/view-output", req)
	if replay.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", replay.Code, replay.Body.String())
	}
	replayResp := decodeJSON[api.ActionResponse](t, replay)
	if replayResp.ActionID != firstResp.ActionID || replayResp.ResultCode != "failed" {
		t.Fatalf("expected replay failed result, first=%+v replay=%+v", firstResp, replayResp)
	}
	if len(runner.calls) != callCount {
		t.Fatalf("expected replay without executor re-run, calls before=%d after=%d", callCount, len(runner.calls))
	}
}

func TestKillActionExecutorFailureReplay(t *testing.T) {
	runner := &stubRunner{err: fmt.Errorf("boom")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}
	req := map[string]any{
		"request_ref": "req-kill-fail",
		"target":      "t1",
		"pane_id":     "%1",
		"mode":        "key",
	}
	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/kill", req)
	if first.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", first.Code, first.Body.String())
	}
	firstResp := decodeJSON[api.ActionResponse](t, first)
	if firstResp.ResultCode != "failed" || firstResp.ErrorCode == nil || *firstResp.ErrorCode != model.ErrTargetUnreachable {
		t.Fatalf("expected failed/E_TARGET_UNREACHABLE, got %+v", firstResp)
	}
	callCount := len(runner.calls)

	replay := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/kill", req)
	if replay.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", replay.Code, replay.Body.String())
	}
	replayResp := decodeJSON[api.ActionResponse](t, replay)
	if replayResp.ActionID != firstResp.ActionID || replayResp.ResultCode != "failed" {
		t.Fatalf("expected replay failed result, first=%+v replay=%+v", firstResp, replayResp)
	}
	if len(runner.calls) != callCount {
		t.Fatalf("expected replay without executor re-run, calls before=%d after=%d", callCount, len(runner.calls))
	}
}

func TestActionEventCorrelation(t *testing.T) {
	runner := &stubRunner{out: []byte("ok")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	pid := int64(3333)
	seedPaneRuntimeState(t, store,
		model.Pane{
			TargetID:    "t1",
			PaneID:      "%1",
			SessionName: "s1",
			WindowID:    "@1",
			WindowName:  "w1",
			UpdatedAt:   now,
		},
		model.Runtime{
			RuntimeID:        "rt-corr-api-1",
			TargetID:         "t1",
			PaneID:           "%1",
			TmuxServerBootID: "boot-1",
			PaneEpoch:        1,
			AgentType:        "codex",
			PID:              &pid,
			StartedAt:        now.Add(-1 * time.Minute),
		},
		model.StateRow{
			TargetID:     "t1",
			PaneID:       "%1",
			RuntimeID:    "rt-corr-api-1",
			State:        model.StateRunning,
			ReasonCode:   "heartbeat",
			Confidence:   "high",
			StateVersion: 1,
			LastSeenAt:   now,
			UpdatedAt:    now,
		},
	)

	actionRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", map[string]any{
		"request_ref": "req-corr-api-1",
		"target":      "t1",
		"pane_id":     "%1",
		"text":        "echo hello",
	})
	if actionRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", actionRec.Code, actionRec.Body.String())
	}
	actionResp := decodeJSON[api.ActionResponse](t, actionRec)
	if actionResp.ActionID == "" {
		t.Fatalf("expected action_id, got %+v", actionResp)
	}

	eventsRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/actions/"+url.PathEscape(actionResp.ActionID)+"/events", nil)
	if eventsRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", eventsRec.Code, eventsRec.Body.String())
	}
	eventsResp := decodeJSON[api.ActionEventsEnvelope](t, eventsRec)
	if eventsResp.ActionID != actionResp.ActionID {
		t.Fatalf("unexpected action events response: %+v", eventsResp)
	}
	if len(eventsResp.Events) != 1 {
		t.Fatalf("expected one correlated event, got %+v", eventsResp.Events)
	}
	event := eventsResp.Events[0]
	if event.ActionID != actionResp.ActionID {
		t.Fatalf("expected event action_id=%s, got %+v", actionResp.ActionID, event)
	}
	if event.RuntimeID != "rt-corr-api-1" || event.Source != string(model.SourceWrapper) {
		t.Fatalf("unexpected correlated event fields: %+v", event)
	}
	if event.EventType != "action.send" {
		t.Fatalf("expected event_type=action.send, got %+v", event)
	}
	if event.DedupeKey != "action:"+actionResp.ActionID {
		t.Fatalf("unexpected dedupe key: %+v", event)
	}
}

func TestActionEventCorrelationByActionTypeAndIsolation(t *testing.T) {
	runner := &stubRunner{out: []byte("line1\n")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	pid := int64(3334)
	seedPaneRuntimeState(t, store,
		model.Pane{
			TargetID:    "t1",
			PaneID:      "%1",
			SessionName: "s1",
			WindowID:    "@1",
			WindowName:  "w1",
			UpdatedAt:   now,
		},
		model.Runtime{
			RuntimeID:        "rt-corr-types-1",
			TargetID:         "t1",
			PaneID:           "%1",
			TmuxServerBootID: "boot-1",
			PaneEpoch:        1,
			AgentType:        "codex",
			PID:              &pid,
			StartedAt:        now.Add(-1 * time.Minute),
		},
		model.StateRow{
			TargetID:     "t1",
			PaneID:       "%1",
			RuntimeID:    "rt-corr-types-1",
			State:        model.StateRunning,
			ReasonCode:   "heartbeat",
			Confidence:   "high",
			StateVersion: 1,
			LastSeenAt:   now,
			UpdatedAt:    now,
		},
	)

	cases := []struct {
		path      string
		body      map[string]any
		eventType string
	}{
		{
			path: "/v1/actions/attach",
			body: map[string]any{
				"request_ref": "req-corr-attach",
				"target":      "t1",
				"pane_id":     "%1",
			},
			eventType: "action.attach",
		},
		{
			path: "/v1/actions/send",
			body: map[string]any{
				"request_ref": "req-corr-send",
				"target":      "t1",
				"pane_id":     "%1",
				"text":        "echo hi",
			},
			eventType: "action.send",
		},
		{
			path: "/v1/actions/view-output",
			body: map[string]any{
				"request_ref": "req-corr-view",
				"target":      "t1",
				"pane_id":     "%1",
				"lines":       20,
			},
			eventType: "action.view-output",
		},
		{
			path: "/v1/actions/kill",
			body: map[string]any{
				"request_ref": "req-corr-kill",
				"target":      "t1",
				"pane_id":     "%1",
				"mode":        "key",
			},
			eventType: "action.kill",
		},
	}

	seenEvents := map[string]struct{}{}
	for _, tc := range cases {
		actionRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, tc.path, tc.body)
		if actionRec.Code != http.StatusOK {
			t.Fatalf("%s expected 200, got %d body=%s", tc.path, actionRec.Code, actionRec.Body.String())
		}
		actionResp := decodeJSON[api.ActionResponse](t, actionRec)
		if actionResp.ActionID == "" {
			t.Fatalf("%s expected action_id, got %+v", tc.path, actionResp)
		}
		eventsRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/actions/"+url.PathEscape(actionResp.ActionID)+"/events", nil)
		if eventsRec.Code != http.StatusOK {
			t.Fatalf("%s events expected 200, got %d body=%s", tc.path, eventsRec.Code, eventsRec.Body.String())
		}
		eventsResp := decodeJSON[api.ActionEventsEnvelope](t, eventsRec)
		if len(eventsResp.Events) != 1 {
			t.Fatalf("%s expected one correlated event, got %+v", tc.path, eventsResp.Events)
		}
		ev := eventsResp.Events[0]
		if ev.ActionID != actionResp.ActionID || ev.EventType != tc.eventType {
			t.Fatalf("%s unexpected correlated event: %+v", tc.path, ev)
		}
		if _, ok := seenEvents[ev.EventID]; ok {
			t.Fatalf("expected unique event ids per action, duplicated id=%s", ev.EventID)
		}
		seenEvents[ev.EventID] = struct{}{}
	}
}

func TestActionEventCorrelationActionExistsButNoEvents(t *testing.T) {
	runner := &stubRunner{out: []byte("ok")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	if err := store.UpsertPane(context.Background(), model.Pane{
		TargetID:    "t1",
		PaneID:      "%1",
		SessionName: "s1",
		WindowID:    "@1",
		WindowName:  "w1",
		UpdatedAt:   now,
	}); err != nil {
		t.Fatalf("seed pane: %v", err)
	}

	actionRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/attach", map[string]any{
		"request_ref": "req-corr-no-events",
		"target":      "t1",
		"pane_id":     "%1",
	})
	if actionRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", actionRec.Code, actionRec.Body.String())
	}
	actionResp := decodeJSON[api.ActionResponse](t, actionRec)
	eventsRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/actions/"+url.PathEscape(actionResp.ActionID)+"/events", nil)
	if eventsRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", eventsRec.Code, eventsRec.Body.String())
	}
	eventsResp := decodeJSON[api.ActionEventsEnvelope](t, eventsRec)
	if len(eventsResp.Events) != 0 {
		t.Fatalf("expected zero events, got %+v", eventsResp.Events)
	}
}

func TestActionEventCorrelationActionNotFound(t *testing.T) {
	runner := &stubRunner{}
	srv, _ := newAPITestServer(t, runner)
	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/actions/missing-action/events", nil)
	if rec.Code != http.StatusNotFound {
		t.Fatalf("expected 404, got %d body=%s", rec.Code, rec.Body.String())
	}
	payload := decodeJSON[api.ErrorResponse](t, rec)
	if payload.Error.Code != model.ErrRefNotFound {
		t.Fatalf("expected %s, got %+v", model.ErrRefNotFound, payload)
	}
}

func TestActionEventCorrelationRouteValidation(t *testing.T) {
	runner := &stubRunner{}
	srv, _ := newAPITestServer(t, runner)

	emptyID := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/actions/%20/events", nil)
	if emptyID.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d body=%s", emptyID.Code, emptyID.Body.String())
	}
	emptyPayload := decodeJSON[api.ErrorResponse](t, emptyID)
	if emptyPayload.Error.Code != model.ErrRefInvalid {
		t.Fatalf("expected %s, got %+v", model.ErrRefInvalid, emptyPayload)
	}

	invalidReq := httptest.NewRequest(http.MethodGet, "/v1/actions/a/events", nil)
	invalidReq.URL.Path = "/v1/actions/%ZZ/events"
	invalidEncoding := httptest.NewRecorder()
	srv.httpSrv.Handler.ServeHTTP(invalidEncoding, invalidReq)
	if invalidEncoding.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d body=%s", invalidEncoding.Code, invalidEncoding.Body.String())
	}
	invalidEncodingPayload := decodeJSON[api.ErrorResponse](t, invalidEncoding)
	if invalidEncodingPayload.Error.Code != model.ErrRefInvalidEncoding {
		t.Fatalf("expected %s, got %+v", model.ErrRefInvalidEncoding, invalidEncodingPayload)
	}

	methodRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/a1/events", nil)
	if methodRec.Code != http.StatusMethodNotAllowed {
		t.Fatalf("expected 405, got %d body=%s", methodRec.Code, methodRec.Body.String())
	}
	if allow := methodRec.Header().Get("Allow"); allow != "GET" {
		t.Fatalf("expected allow header GET, got %q", allow)
	}

	encodedSlash := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/actions/a%2Fb/events", nil)
	if encodedSlash.Code != http.StatusNotFound {
		t.Fatalf("expected 404, got %d body=%s", encodedSlash.Code, encodedSlash.Body.String())
	}

	trailingSlash := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/actions/a1/events/", nil)
	if trailingSlash.Code != http.StatusNotFound {
		t.Fatalf("expected 404, got %d body=%s", trailingSlash.Code, trailingSlash.Body.String())
	}
}

func TestActionEventCorrelationReplayBackfillsAuditEvent(t *testing.T) {
	runner := &stubRunner{out: []byte("ok")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	pid := int64(4444)
	seedPaneRuntimeState(t, store,
		model.Pane{
			TargetID:    "t1",
			PaneID:      "%1",
			SessionName: "s1",
			WindowID:    "@1",
			WindowName:  "w1",
			UpdatedAt:   now,
		},
		model.Runtime{
			RuntimeID:        "rt-corr-backfill-1",
			TargetID:         "t1",
			PaneID:           "%1",
			TmuxServerBootID: "boot-1",
			PaneEpoch:        1,
			AgentType:        "codex",
			PID:              &pid,
			StartedAt:        now.Add(-1 * time.Minute),
		},
		model.StateRow{
			TargetID:     "t1",
			PaneID:       "%1",
			RuntimeID:    "rt-corr-backfill-1",
			State:        model.StateRunning,
			ReasonCode:   "heartbeat",
			Confidence:   "high",
			StateVersion: 1,
			LastSeenAt:   now,
			UpdatedAt:    now,
		},
	)

	failOnce := true
	srv.auditEventHook = func(_ model.Action, _ string) error {
		if failOnce {
			failOnce = false
			return errors.New("injected audit failure")
		}
		return nil
	}

	req := map[string]any{
		"request_ref": "req-corr-backfill",
		"target":      "t1",
		"pane_id":     "%1",
		"text":        "echo retry",
	}
	first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", req)
	if first.Code != http.StatusInternalServerError {
		t.Fatalf("expected 500, got %d body=%s", first.Code, first.Body.String())
	}
	firstErr := decodeJSON[api.ErrorResponse](t, first)
	if firstErr.Error.Code != model.ErrPreconditionFailed {
		t.Fatalf("expected %s, got %+v", model.ErrPreconditionFailed, firstErr)
	}

	second := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, "/v1/actions/send", req)
	if second.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", second.Code, second.Body.String())
	}
	secondResp := decodeJSON[api.ActionResponse](t, second)
	if secondResp.ActionID == "" {
		t.Fatalf("expected replayed action_id, got %+v", secondResp)
	}
	if len(runner.calls) != 1 {
		t.Fatalf("expected replay without executor re-run, calls=%d", len(runner.calls))
	}

	eventsRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/actions/"+url.PathEscape(secondResp.ActionID)+"/events", nil)
	if eventsRec.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d body=%s", eventsRec.Code, eventsRec.Body.String())
	}
	eventsResp := decodeJSON[api.ActionEventsEnvelope](t, eventsRec)
	if len(eventsResp.Events) != 1 {
		t.Fatalf("expected backfilled single event, got %+v", eventsResp.Events)
	}
	if eventsResp.Events[0].ActionID != secondResp.ActionID || eventsResp.Events[0].EventType != "action.send" {
		t.Fatalf("unexpected backfilled event: %+v", eventsResp.Events[0])
	}
}

func TestActionEventCorrelationReplayDoesNotDuplicateEvents(t *testing.T) {
	runner := &stubRunner{out: []byte("line1\n")}
	srv, store := newAPITestServer(t, runner)
	now := time.Now().UTC()
	seedTarget(t, store, "t1", "t1")
	pid := int64(5556)
	seedPaneRuntimeState(t, store,
		model.Pane{
			TargetID:    "t1",
			PaneID:      "%1",
			SessionName: "s1",
			WindowID:    "@1",
			WindowName:  "w1",
			UpdatedAt:   now,
		},
		model.Runtime{
			RuntimeID:        "rt-corr-replay-1",
			TargetID:         "t1",
			PaneID:           "%1",
			TmuxServerBootID: "boot-1",
			PaneEpoch:        1,
			AgentType:        "codex",
			PID:              &pid,
			StartedAt:        now.Add(-1 * time.Minute),
		},
		model.StateRow{
			TargetID:     "t1",
			PaneID:       "%1",
			RuntimeID:    "rt-corr-replay-1",
			State:        model.StateRunning,
			ReasonCode:   "heartbeat",
			Confidence:   "high",
			StateVersion: 1,
			LastSeenAt:   now,
			UpdatedAt:    now,
		},
	)

	cases := []struct {
		path string
		body map[string]any
	}{
		{
			path: "/v1/actions/attach",
			body: map[string]any{
				"request_ref": "req-corr-replay-attach",
				"target":      "t1",
				"pane_id":     "%1",
			},
		},
		{
			path: "/v1/actions/send",
			body: map[string]any{
				"request_ref": "req-corr-replay-send",
				"target":      "t1",
				"pane_id":     "%1",
				"text":        "echo hi",
			},
		},
		{
			path: "/v1/actions/view-output",
			body: map[string]any{
				"request_ref": "req-corr-replay-view",
				"target":      "t1",
				"pane_id":     "%1",
				"lines":       10,
			},
		},
		{
			path: "/v1/actions/kill",
			body: map[string]any{
				"request_ref": "req-corr-replay-kill",
				"target":      "t1",
				"pane_id":     "%1",
				"mode":        "key",
			},
		},
	}

	for _, tc := range cases {
		first := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, tc.path, tc.body)
		if first.Code != http.StatusOK {
			t.Fatalf("%s first expected 200, got %d body=%s", tc.path, first.Code, first.Body.String())
		}
		firstResp := decodeJSON[api.ActionResponse](t, first)
		if firstResp.ActionID == "" {
			t.Fatalf("%s missing action id on first response: %+v", tc.path, firstResp)
		}

		second := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, tc.path, tc.body)
		if second.Code != http.StatusOK {
			t.Fatalf("%s second expected 200, got %d body=%s", tc.path, second.Code, second.Body.String())
		}
		secondResp := decodeJSON[api.ActionResponse](t, second)
		if secondResp.ActionID != firstResp.ActionID {
			t.Fatalf("%s expected same action_id on replay, first=%+v second=%+v", tc.path, firstResp, secondResp)
		}

		third := doJSONRequest(t, srv.httpSrv.Handler, http.MethodPost, tc.path, tc.body)
		if third.Code != http.StatusOK {
			t.Fatalf("%s third expected 200, got %d body=%s", tc.path, third.Code, third.Body.String())
		}
		thirdResp := decodeJSON[api.ActionResponse](t, third)
		if thirdResp.ActionID != firstResp.ActionID {
			t.Fatalf("%s expected stable action_id, first=%+v third=%+v", tc.path, firstResp, thirdResp)
		}

		eventsRec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v1/actions/"+url.PathEscape(firstResp.ActionID)+"/events", nil)
		if eventsRec.Code != http.StatusOK {
			t.Fatalf("%s events expected 200, got %d body=%s", tc.path, eventsRec.Code, eventsRec.Body.String())
		}
		eventsResp := decodeJSON[api.ActionEventsEnvelope](t, eventsRec)
		if len(eventsResp.Events) != 1 {
			t.Fatalf("%s expected exactly one audit event after replays, got %+v", tc.path, eventsResp.Events)
		}
	}
}

func findPaneItem(t *testing.T, items []api.PaneItem, target, sessionName, paneID string) api.PaneItem {
	t.Helper()
	for _, item := range items {
		if item.Identity.Target == target &&
			item.Identity.SessionName == sessionName &&
			item.Identity.PaneID == paneID {
			return item
		}
	}
	t.Fatalf("pane not found target=%s session=%s pane=%s in %+v", target, sessionName, paneID, items)
	return api.PaneItem{}
}

func waitForSocket(t *testing.T, path string, errCh <-chan error) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		select {
		case err := <-errCh:
			if err == nil || err == context.Canceled {
				t.Fatalf("server exited before socket creation: %v", err)
			}
			if isUDSUnsupported(err) {
				t.Skipf("unix domain sockets unavailable in this environment: %v", err)
			}
			t.Fatalf("server start failed before socket creation: %v", err)
		default:
		}
		if st, err := os.Stat(path); err == nil {
			if st.Mode()&os.ModeSocket != 0 {
				return
			}
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatalf("socket was not created: %s", fmt.Sprintf("%s", path))
}

func isUDSUnsupported(err error) bool {
	if err == nil {
		return false
	}
	msg := strings.ToLower(err.Error())
	return strings.Contains(msg, "operation not permitted") ||
		strings.Contains(msg, "permission denied") ||
		strings.Contains(msg, "not supported") ||
		strings.Contains(msg, "address family not supported")
}
