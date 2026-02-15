package appclient

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/api"
)

func TestWatchOnceParsesJSONLAndCursor(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/watch", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet {
			t.Fatalf("expected GET, got %s", r.Method)
		}
		if r.URL.Query().Get("scope") != "panes" {
			t.Fatalf("expected scope panes, got %q", r.URL.Query().Get("scope"))
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","type":"snapshot","scope":"panes","sequence":1,"cursor":"stream:1","summary":{"by_state":{"running":1}}}`+"\n")
		_, _ = io.WriteString(w, `{"schema_version":"v1","type":"delta","scope":"panes","sequence":2,"cursor":"stream:2","summary":{"by_state":{"running":1}}}`+"\n")
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	lines, cursor, err := client.WatchOnce(context.Background(), WatchOptions{Scope: "panes"})
	if err != nil {
		t.Fatalf("watch once: %v", err)
	}
	if len(lines) != 2 {
		t.Fatalf("expected 2 lines, got %d", len(lines))
	}
	if cursor != "stream:2" {
		t.Fatalf("expected cursor stream:2, got %q", cursor)
	}
	if lines[0].Type != "snapshot" || lines[1].Type != "delta" {
		t.Fatalf("unexpected line types: %+v", lines)
	}
}

func TestWatchLoopRetriesAndResumes(t *testing.T) {
	var calls atomic.Int32
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/watch", func(w http.ResponseWriter, r *http.Request) {
		n := calls.Add(1)
		if n == 1 {
			w.WriteHeader(http.StatusBadGateway)
			_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","error":{"code":"E_TARGET_UNREACHABLE","message":"boom"}}`)
			return
		}
		if n == 2 && r.URL.Query().Get("cursor") != "" {
			t.Fatalf("first successful request should not pass cursor, got %q", r.URL.Query().Get("cursor"))
		}
		if n == 3 && r.URL.Query().Get("cursor") != "stream:1" {
			t.Fatalf("expected resume cursor stream:1, got %q", r.URL.Query().Get("cursor"))
		}
		sequence := int64(1)
		cursor := "stream:1"
		if n >= 3 {
			sequence = 2
			cursor = "stream:2"
		}
		line := map[string]any{
			"schema_version": "v1",
			"type":           "snapshot",
			"scope":          "panes",
			"sequence":       sequence,
			"cursor":         cursor,
			"summary": map[string]any{
				"by_state": map[string]int{"running": int(sequence)},
			},
		}
		buf, _ := json.Marshal(line)
		_, _ = io.WriteString(w, string(buf)+"\n")
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	received := make([]int64, 0)
	err := client.WatchLoop(ctx, WatchLoopOptions{
		Scope:           "panes",
		PollInterval:    20 * time.Millisecond,
		RetryMinBackoff: 20 * time.Millisecond,
		RetryMaxBackoff: 40 * time.Millisecond,
	}, func(line api.WatchLine) error {
		received = append(received, line.Sequence)
		if len(received) >= 2 {
			return context.Canceled
		}
		return nil
	})
	if err == nil || !strings.Contains(err.Error(), "context canceled") {
		t.Fatalf("expected context canceled sentinel, got %v", err)
	}
	if len(received) < 2 || received[0] != 1 || received[1] != 2 {
		t.Fatalf("unexpected received sequences: %+v", received)
	}
}

func TestWatchLoopStopsOnNonRetryableError(t *testing.T) {
	var calls atomic.Int32
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/watch", func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		w.WriteHeader(http.StatusBadRequest)
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","error":{"code":"E_CURSOR_INVALID","message":"bad cursor"}}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
	defer cancel()
	err := client.WatchLoop(ctx, WatchLoopOptions{
		Scope:           "panes",
		PollInterval:    10 * time.Millisecond,
		RetryMinBackoff: 10 * time.Millisecond,
		RetryMaxBackoff: 20 * time.Millisecond,
	}, nil)
	if err == nil || !strings.Contains(err.Error(), "E_CURSOR_INVALID") {
		t.Fatalf("expected non-retryable cursor error, got %v", err)
	}
	if calls.Load() != 1 {
		t.Fatalf("expected single watch call for non-retryable error, got %d", calls.Load())
	}
}

func TestWatchLoopOnceReturnsFirstErrorWithoutRetry(t *testing.T) {
	var calls atomic.Int32
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/watch", func(w http.ResponseWriter, _ *http.Request) {
		calls.Add(1)
		w.WriteHeader(http.StatusBadGateway)
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","error":{"code":"E_TARGET_UNREACHABLE","message":"boom"}}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	err := client.WatchLoop(context.Background(), WatchLoopOptions{
		Scope:           "panes",
		PollInterval:    10 * time.Millisecond,
		RetryMinBackoff: 10 * time.Millisecond,
		RetryMaxBackoff: 20 * time.Millisecond,
		Once:            true,
	}, nil)
	if err == nil || !strings.Contains(err.Error(), "E_TARGET_UNREACHABLE") {
		t.Fatalf("expected first error to be returned in once mode, got %v", err)
	}
	if calls.Load() != 1 {
		t.Fatalf("expected single watch call in once mode, got %d", calls.Load())
	}
}

func TestWatchLoopStopsOnInvalidPayload(t *testing.T) {
	var calls atomic.Int32
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/watch", func(w http.ResponseWriter, _ *http.Request) {
		calls.Add(1)
		_, _ = io.WriteString(w, `{"schema_version":"v1","type":"snapshot"`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	err := client.WatchLoop(context.Background(), WatchLoopOptions{
		Scope:           "panes",
		PollInterval:    10 * time.Millisecond,
		RetryMinBackoff: 10 * time.Millisecond,
		RetryMaxBackoff: 20 * time.Millisecond,
	}, nil)
	if err == nil || !errors.Is(err, ErrWatchPayloadInvalid) {
		t.Fatalf("expected payload invalid error, got %v", err)
	}
	if calls.Load() != 1 {
		t.Fatalf("expected single call for invalid payload, got %d", calls.Load())
	}
}

func TestActionAndAdapterEndpoints(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/send", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Fatalf("expected POST, got %s", r.Method)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a-send","result_code":"completed","completed_at":"2026-02-13T00:00:00Z"}`)
	})
	mux.HandleFunc("/v1/adapters", func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","adapters":[{"adapter_name":"claude-hook","agent_type":"claude","version":"v1","compatible":true,"capabilities":["event_driven"],"enabled":true,"updated_at":"2026-02-13T00:00:00Z"}]}`)
	})
	mux.HandleFunc("/v1/adapters/claude-hook/disable", func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","adapters":[{"adapter_name":"claude-hook","agent_type":"claude","version":"v1","compatible":true,"capabilities":["event_driven"],"enabled":false,"updated_at":"2026-02-13T00:00:00Z"}]}`)
	})
	mux.HandleFunc("/v1/panes", func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Query().Get("target") != "t1" {
			t.Fatalf("expected target=t1 query, got %q", r.URL.Query().Get("target"))
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","filters":{"target":"t1"},"summary":{"by_state":{"running":1},"by_agent":{"codex":1},"by_target":{"t1":1}},"partial":false,"requested_targets":["t1"],"responded_targets":["t1"],"items":[{"identity":{"target":"t1","session_name":"s1","window_id":"@1","pane_id":"%1"},"state":"running","reason_code":"active","confidence":"high","runtime_id":"rt-1","agent_type":"codex","updated_at":"2026-02-13T00:00:00Z"}]}`)
	})
	mux.HandleFunc("/v1/windows", func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","filters":{},"summary":{"by_state":{"running":1},"by_agent":{"codex":1},"by_target":{"t1":1}},"partial":false,"requested_targets":["t1"],"responded_targets":["t1"],"items":[{"identity":{"target":"t1","session_name":"s1","window_id":"@1"},"top_state":"running","waiting_count":0,"running_count":1,"total_panes":1}]}`)
	})
	mux.HandleFunc("/v1/sessions", func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","filters":{},"summary":{"by_state":{"running":1},"by_agent":{"codex":1},"by_target":{"t1":1}},"partial":false,"requested_targets":["t1"],"responded_targets":["t1"],"items":[{"identity":{"target":"t1","session_name":"s1"},"total_panes":1,"by_state":{"running":1},"by_agent":{"codex":1}}]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	sendResp, err := client.Send(context.Background(), SendRequest{
		RequestRef: "req-1",
		Target:     "t1",
		PaneID:     "%1",
		Text:       "hello",
	})
	if err != nil {
		t.Fatalf("send action: %v", err)
	}
	if sendResp.ActionID != "a-send" || sendResp.ResultCode != "completed" {
		t.Fatalf("unexpected send response: %+v", sendResp)
	}

	listResp, err := client.ListAdapters(context.Background(), nil)
	if err != nil {
		t.Fatalf("list adapters: %v", err)
	}
	if len(listResp.Adapters) != 1 || listResp.Adapters[0].AdapterName != "claude-hook" {
		t.Fatalf("unexpected adapters list: %+v", listResp.Adapters)
	}

	disableResp, err := client.SetAdapterEnabled(context.Background(), "claude-hook", false)
	if err != nil {
		t.Fatalf("disable adapter: %v", err)
	}
	if len(disableResp.Adapters) != 1 || disableResp.Adapters[0].Enabled {
		t.Fatalf("unexpected disable response: %+v", disableResp.Adapters)
	}

	panesResp, err := client.ListPanes(context.Background(), ListOptions{Target: "t1"})
	if err != nil {
		t.Fatalf("list panes: %v", err)
	}
	if len(panesResp.Items) != 1 || panesResp.Items[0].Identity.PaneID != "%1" {
		t.Fatalf("unexpected panes response: %+v", panesResp.Items)
	}

	windowsResp, err := client.ListWindows(context.Background(), ListOptions{})
	if err != nil {
		t.Fatalf("list windows: %v", err)
	}
	if len(windowsResp.Items) != 1 || windowsResp.Items[0].Identity.WindowID != "@1" {
		t.Fatalf("unexpected windows response: %+v", windowsResp.Items)
	}

	sessionsResp, err := client.ListSessions(context.Background(), ListOptions{})
	if err != nil {
		t.Fatalf("list sessions: %v", err)
	}
	if len(sessionsResp.Items) != 1 || sessionsResp.Items[0].Identity.SessionName != "s1" {
		t.Fatalf("unexpected sessions response: %+v", sessionsResp.Items)
	}
}

func TestListActionEventsEndpoint(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/a-send/events", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet {
			t.Fatalf("expected GET, got %s", r.Method)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a-send","events":[{"event_id":"ev-1","action_id":"a-send","runtime_id":"rt-1","event_type":"action.send","source":"daemon","event_time":"2026-02-13T00:00:00Z","ingested_at":"2026-02-13T00:00:00Z","dedupe_key":"dk-1"}]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	resp, err := client.ListActionEvents(context.Background(), "a-send")
	if err != nil {
		t.Fatalf("list action events: %v", err)
	}
	if resp.ActionID != "a-send" {
		t.Fatalf("unexpected action id: %+v", resp)
	}
	if len(resp.Events) != 1 || resp.Events[0].EventID != "ev-1" {
		t.Fatalf("unexpected events payload: %+v", resp.Events)
	}
}

func TestListActionEventsRejectsBlankID(t *testing.T) {
	client := NewWithClient("http://example.invalid", &http.Client{})
	if _, err := client.ListActionEvents(context.Background(), "   "); err == nil {
		t.Fatalf("expected blank action id error")
	}
}

func TestListActionEventsEscapesActionID(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet {
			t.Fatalf("expected GET, got %s", r.Method)
		}
		if r.URL.EscapedPath() != "/v1/actions/a%2Fb%3Fc/events" {
			t.Fatalf("unexpected escaped path: %s", r.URL.EscapedPath())
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a/b?c","events":[]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	resp, err := client.ListActionEvents(context.Background(), "a/b?c")
	if err != nil {
		t.Fatalf("list action events with escaped id: %v", err)
	}
	if resp.ActionID != "a/b?c" {
		t.Fatalf("unexpected action id: %+v", resp)
	}
}

func TestListActionEventsReturnsRequestErrorOnHTTPFailure(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/missing/events", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusNotFound)
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","error":{"code":"E_REF_NOT_FOUND","message":"action not found"}}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	_, err := client.ListActionEvents(context.Background(), "missing")
	if err == nil {
		t.Fatalf("expected request error")
	}
	var reqErr *RequestError
	if !errors.As(err, &reqErr) {
		t.Fatalf("expected RequestError, got %T (%v)", err, err)
	}
	if reqErr.StatusCode != http.StatusNotFound || reqErr.Code != "E_REF_NOT_FOUND" {
		t.Fatalf("unexpected request error: %+v", reqErr)
	}
}

func TestListActionEventsDecodeErrorOnInvalidJSON(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/a-send/events", func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, `{"schema_version":"v1","action_id":"a-send","events":[`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	_, err := client.ListActionEvents(context.Background(), "a-send")
	if err == nil {
		t.Fatalf("expected decode error")
	}
	if !strings.Contains(err.Error(), "decode action events envelope") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestListTargetsEndpoint(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/targets", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet {
			t.Fatalf("expected GET, got %s", r.Method)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","targets":[{"target_id":"tgt-1","target_name":"local","kind":"local","connection_ref":"","is_default":true,"health":"ok","updated_at":"2026-02-13T00:00:00Z"}]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	resp, err := client.ListTargets(context.Background())
	if err != nil {
		t.Fatalf("list targets: %v", err)
	}
	if len(resp.Targets) != 1 || resp.Targets[0].TargetName != "local" {
		t.Fatalf("unexpected targets payload: %+v", resp.Targets)
	}
}

func TestListTargetsRequestError(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/targets", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","error":{"code":"E_PRECONDITION_FAILED","message":"store unavailable"}}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	_, err := client.ListTargets(context.Background())
	if err == nil {
		t.Fatalf("expected request error")
	}
	var reqErr *RequestError
	if !errors.As(err, &reqErr) {
		t.Fatalf("expected RequestError, got %T (%v)", err, err)
	}
	if reqErr.StatusCode != http.StatusInternalServerError || reqErr.Code != "E_PRECONDITION_FAILED" {
		t.Fatalf("unexpected request error: %+v", reqErr)
	}
}

func TestListTargetsDecodeError(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/targets", func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, `{"schema_version":"v1","targets":[`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	_, err := client.ListTargets(context.Background())
	if err == nil {
		t.Fatalf("expected decode error")
	}
	if !strings.Contains(err.Error(), "decode targets envelope") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestCreateTargetEndpoint(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/targets", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Fatalf("expected POST, got %s", r.Method)
		}
		var req CreateTargetRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req.Name != "vm1" || req.Kind != "ssh" || req.ConnectionRef != "ssh://vm1" || !req.IsDefault {
			t.Fatalf("unexpected create target request: %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","targets":[{"target_id":"vm1","target_name":"vm1","kind":"ssh","connection_ref":"ssh://vm1","is_default":true,"health":"ok","updated_at":"2026-02-13T00:00:00Z"}]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	resp, err := client.CreateTarget(context.Background(), CreateTargetRequest{
		Name:          "vm1",
		Kind:          "ssh",
		ConnectionRef: "ssh://vm1",
		IsDefault:     true,
	})
	if err != nil {
		t.Fatalf("create target: %v", err)
	}
	if len(resp.Targets) != 1 || resp.Targets[0].TargetName != "vm1" || !resp.Targets[0].IsDefault {
		t.Fatalf("unexpected create target response: %+v", resp.Targets)
	}
}

func TestConnectTargetEndpointEscapesName(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Fatalf("expected POST, got %s", r.Method)
		}
		if r.URL.EscapedPath() != "/v1/targets/vm%2F1/connect" {
			t.Fatalf("unexpected escaped path: %s", r.URL.EscapedPath())
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","targets":[{"target_id":"vm/1","target_name":"vm/1","kind":"ssh","connection_ref":"ssh://vm1","is_default":false,"health":"ok","updated_at":"2026-02-13T00:00:00Z"}]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	resp, err := client.ConnectTarget(context.Background(), "vm/1")
	if err != nil {
		t.Fatalf("connect target: %v", err)
	}
	if len(resp.Targets) != 1 || resp.Targets[0].TargetName != "vm/1" {
		t.Fatalf("unexpected connect target response: %+v", resp.Targets)
	}
}

func TestDeleteTargetEndpoint(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/targets/vm1", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodDelete {
			t.Fatalf("expected DELETE, got %s", r.Method)
		}
		w.WriteHeader(http.StatusNoContent)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	if err := client.DeleteTarget(context.Background(), "vm1"); err != nil {
		t.Fatalf("delete target: %v", err)
	}
}

func TestDeleteTargetRejectsBlankName(t *testing.T) {
	client := NewWithClient("http://example.invalid", &http.Client{})
	if err := client.DeleteTarget(context.Background(), "   "); err == nil {
		t.Fatalf("expected delete target blank name error")
	}
}

func TestDecodeWatchLinesLargeLine(t *testing.T) {
	large := strings.Repeat("a", 70*1024)
	line := fmt.Sprintf(`{"schema_version":"v1","type":"snapshot","scope":"panes","sequence":1,"cursor":"stream:1","stream_id":"%s","summary":{"by_state":{"running":1}}}`+"\n", large)
	lines, cursor, err := decodeWatchLines([]byte(line))
	if err != nil {
		t.Fatalf("decode large watch line: %v", err)
	}
	if len(lines) != 1 || cursor != "stream:1" {
		t.Fatalf("unexpected decode result: len=%d cursor=%q", len(lines), cursor)
	}
}

func TestUnaryRequestUsesTimeout(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/panes", func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(150 * time.Millisecond)
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","filters":{},"summary":{"by_state":{},"by_agent":{},"by_target":{}},"partial":false,"requested_targets":[],"responded_targets":[],"items":[]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client()).WithUnaryTimeout(20 * time.Millisecond)
	start := time.Now()
	_, err := client.ListPanes(context.Background(), ListOptions{})
	if err == nil {
		t.Fatalf("expected timeout error")
	}
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("expected context deadline exceeded, got %v", err)
	}
	if time.Since(start) > 120*time.Millisecond {
		t.Fatalf("timeout should happen quickly, elapsed=%s", time.Since(start))
	}
}

func TestWatchOnceNotAffectedByUnaryTimeout(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/watch", func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(80 * time.Millisecond)
		_, _ = io.WriteString(w, `{"schema_version":"v1","type":"snapshot","scope":"panes","sequence":1,"cursor":"stream:1","summary":{"by_state":{"running":1}}}`+"\n")
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client()).WithUnaryTimeout(20 * time.Millisecond)
	lines, cursor, err := client.WatchOnce(context.Background(), WatchOptions{Scope: "panes"})
	if err != nil {
		t.Fatalf("watch once should not use unary timeout: %v", err)
	}
	if len(lines) != 1 || cursor != "stream:1" {
		t.Fatalf("unexpected watch result: len=%d cursor=%q", len(lines), cursor)
	}
}

func TestWatchLoopNotAffectedByUnaryTimeout(t *testing.T) {
	var calls atomic.Int32
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/watch", func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		time.Sleep(80 * time.Millisecond)
		_, _ = io.WriteString(w, `{"schema_version":"v1","type":"snapshot","scope":"panes","sequence":1,"cursor":"stream:1","summary":{"by_state":{"running":1}}}`+"\n")
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client()).WithUnaryTimeout(20 * time.Millisecond)
	err := client.WatchLoop(context.Background(), WatchLoopOptions{
		Scope:           "panes",
		PollInterval:    10 * time.Millisecond,
		RetryMinBackoff: 10 * time.Millisecond,
		RetryMaxBackoff: 20 * time.Millisecond,
	}, func(api.WatchLine) error {
		return context.Canceled
	})
	if err == nil || !errors.Is(err, context.Canceled) {
		t.Fatalf("expected context canceled from callback, got %v", err)
	}
	if calls.Load() != 1 {
		t.Fatalf("expected one watch request, got %d", calls.Load())
	}
}

func TestListAdaptersIncludesEnabledQuery(t *testing.T) {
	queries := make([]string, 0, 3)
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/adapters", func(w http.ResponseWriter, r *http.Request) {
		queries = append(queries, r.URL.Query().Get("enabled"))
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","adapters":[]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	client := NewWithClient(srv.URL, srv.Client())
	enabledTrue := true
	if _, err := client.ListAdapters(context.Background(), &enabledTrue); err != nil {
		t.Fatalf("list adapters enabled=true: %v", err)
	}
	enabledFalse := false
	if _, err := client.ListAdapters(context.Background(), &enabledFalse); err != nil {
		t.Fatalf("list adapters enabled=false: %v", err)
	}
	if _, err := client.ListAdapters(context.Background(), nil); err != nil {
		t.Fatalf("list adapters enabled=nil: %v", err)
	}
	if len(queries) != 3 {
		t.Fatalf("expected 3 adapter requests, got %d", len(queries))
	}
	if queries[0] != "true" || queries[1] != "false" || queries[2] != "" {
		t.Fatalf("unexpected enabled query sequence: %+v", queries)
	}
}

func TestSetAdapterEnabledRejectsEmptyName(t *testing.T) {
	client := NewWithClient("http://example.invalid", &http.Client{})
	_, err := client.SetAdapterEnabled(context.Background(), "   ", true)
	if err == nil {
		t.Fatalf("expected error for empty adapter name")
	}
	if !strings.Contains(err.Error(), "adapter name is required") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestRequestErrorStringIncludesCodeWithoutMessage(t *testing.T) {
	err := (&RequestError{StatusCode: http.StatusBadRequest, Code: "E_CURSOR_INVALID"}).Error()
	if !strings.Contains(err, "E_CURSOR_INVALID") {
		t.Fatalf("expected error string to include code, got %q", err)
	}
	if !strings.Contains(err, "400") {
		t.Fatalf("expected error string to include status code, got %q", err)
	}
}

func TestRequestErrorStringIncludesCodeWithoutStatus(t *testing.T) {
	err := (&RequestError{Code: "E_CURSOR_INVALID"}).Error()
	if err != "E_CURSOR_INVALID" {
		t.Fatalf("expected code-only error string, got %q", err)
	}
}

func TestWithUnaryTimeoutReturnsClonedClient(t *testing.T) {
	base := NewWithClient("http://example.invalid", &http.Client{})
	updated := base.WithUnaryTimeout(25 * time.Millisecond)
	if updated == base {
		t.Fatalf("expected cloned client instance")
	}
	if base.unaryTimeout != defaultUnaryTimeout {
		t.Fatalf("expected original timeout unchanged, got %s", base.unaryTimeout)
	}
	if updated.unaryTimeout != 25*time.Millisecond {
		t.Fatalf("expected updated timeout, got %s", updated.unaryTimeout)
	}
}
