package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestTargetListJSONCallsAPI(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/targets", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet {
			t.Fatalf("expected GET, got %s", r.Method)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","targets":[{"target_id":"t1","target_name":"t1","kind":"local","connection_ref":"","is_default":true,"health":"ok","updated_at":"2026-02-13T00:00:00Z"}]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	code := r.Run(context.Background(), []string{"target", "list", "--json"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"targets"`) {
		t.Fatalf("expected targets JSON output, got: %s", out.String())
	}
}

func TestAdapterListCallsAPI(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/adapters", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet {
			t.Fatalf("expected GET, got %s", r.Method)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","adapters":[{"adapter_name":"claude-hook","agent_type":"claude","version":"v1","compatible":true,"capabilities":["event_driven"],"enabled":true,"updated_at":"2026-02-13T00:00:00Z"}]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	if code := r.Run(context.Background(), []string{"adapter", "list", "--json"}); code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"adapters"`) {
		t.Fatalf("expected adapters JSON output, got: %s", out.String())
	}

	out.Reset()
	if code := r.Run(context.Background(), []string{"adapter", "list"}); code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), "claude-hook\tclaude\tv1\tenabled\tcompatible") {
		t.Fatalf("expected tabular adapter output, got: %s", out.String())
	}
}

func TestAdapterEnableDisableCallsAPI(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/adapters/claude-hook/enable", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Fatalf("expected POST, got %s", r.Method)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","adapters":[{"adapter_name":"claude-hook","agent_type":"claude","version":"v1","compatible":true,"capabilities":["event_driven"],"enabled":true,"updated_at":"2026-02-13T00:00:00Z"}]}`)
	})
	mux.HandleFunc("/v1/adapters/claude-hook/disable", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Fatalf("expected POST, got %s", r.Method)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","adapters":[{"adapter_name":"claude-hook","agent_type":"claude","version":"v1","compatible":true,"capabilities":["event_driven"],"enabled":false,"updated_at":"2026-02-13T00:00:00Z"}]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	if code := r.Run(context.Background(), []string{"adapter", "enable", "claude-hook", "--json"}); code != 0 {
		t.Fatalf("expected enable exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"enabled":true`) {
		t.Fatalf("unexpected enable output: %s", out.String())
	}
	out.Reset()
	if code := r.Run(context.Background(), []string{"adapter", "disable", "claude-hook"}); code != 0 {
		t.Fatalf("expected disable exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), "disable adapter claude-hook (disabled)") {
		t.Fatalf("unexpected disable output: %s", out.String())
	}
}

func TestTargetAddCallsAPI(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/targets", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Fatalf("expected POST, got %s", r.Method)
		}
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["name"] != "vm1" || req["kind"] != "ssh" || req["connection_ref"] != "vm1" {
			t.Fatalf("unexpected request: %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","targets":[{"target_id":"vm1","target_name":"vm1","kind":"ssh","connection_ref":"vm1","is_default":false,"health":"ok","updated_at":"2026-02-13T00:00:00Z"}]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	code := r.Run(context.Background(), []string{"target", "add", "vm1", "--kind", "ssh", "--connection-ref", "vm1", "--json"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"target_name":"vm1"`) {
		t.Fatalf("unexpected output: %s", out.String())
	}
}

func TestTargetConnectAndRemoveCallAPI(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/targets/vm1/connect", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Fatalf("expected POST, got %s", r.Method)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","targets":[{"target_id":"vm1","target_name":"vm1","kind":"ssh","connection_ref":"vm1","is_default":false,"health":"ok","updated_at":"2026-02-13T00:00:00Z"}]}`)
	})
	mux.HandleFunc("/v1/targets/vm1", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodDelete {
			t.Fatalf("expected DELETE, got %s", r.Method)
		}
		w.WriteHeader(http.StatusNoContent)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	if code := r.Run(context.Background(), []string{"target", "connect", "vm1", "--json"}); code != 0 {
		t.Fatalf("connect expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if code := r.Run(context.Background(), []string{"target", "remove", "vm1"}); code != 0 {
		t.Fatalf("remove expected exit 0, got %d stderr=%s", code, errOut.String())
	}
}

func TestListAndWatchCallAPI(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/panes", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet {
			t.Fatalf("expected GET, got %s", r.Method)
		}
		if r.URL.Query().Get("target") != "t1" {
			t.Fatalf("expected target query t1, got %q", r.URL.Query().Get("target"))
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","filters":{"target":"t1"},"summary":{"by_state":{"running":1}},"partial":false,"requested_targets":["t1"],"responded_targets":["t1"],"items":[{"identity":{"target":"t1","session_name":"s1","window_id":"@1","pane_id":"%1"},"state":"running","updated_at":"2026-02-13T00:00:00Z"}]}`)
	})
	mux.HandleFunc("/v1/watch", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet {
			t.Fatalf("expected GET, got %s", r.Method)
		}
		if r.URL.Query().Get("scope") != "windows" {
			t.Fatalf("expected scope windows, got %q", r.URL.Query().Get("scope"))
		}
		if r.URL.Query().Get("cursor") != "stream:1" {
			t.Fatalf("expected cursor stream:1, got %q", r.URL.Query().Get("cursor"))
		}
		if r.URL.Query().Get("target") != "t1" {
			t.Fatalf("expected target query t1, got %q", r.URL.Query().Get("target"))
		}
		w.Header().Set("Content-Type", "application/x-ndjson")
		_, _ = io.WriteString(w, `{"schema_version":"v1","type":"snapshot","scope":"windows","sequence":2}`+"\n")
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	if code := r.Run(context.Background(), []string{"list", "panes", "--target", "t1", "--json"}); code != 0 {
		t.Fatalf("list expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"items"`) {
		t.Fatalf("expected items in list output: %s", out.String())
	}
	out.Reset()
	if code := r.Run(context.Background(), []string{"watch", "--scope", "windows", "--cursor", "stream:1", "--target", "t1", "--json", "--once"}); code != 0 {
		t.Fatalf("watch expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"scope":"windows"`) {
		t.Fatalf("expected watch jsonl output: %s", out.String())
	}
}

func TestAPIErrorsAreSurfaced(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/targets", func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusBadRequest)
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","error":{"code":"E_REF_INVALID","message":"invalid"}}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	code := r.Run(context.Background(), []string{"target", "list", "--json"})
	if code == 0 {
		t.Fatalf("expected non-zero exit for API error")
	}
	if !strings.Contains(errOut.String(), "E_REF_INVALID") {
		t.Fatalf("expected error code in stderr, got %s", errOut.String())
	}
}

func TestSendViewOutputKillCallAPI(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/send", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Fatalf("expected POST, got %s", r.Method)
		}
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["request_ref"] != "req-send" || req["target"] != "t1" || req["pane_id"] != "%1" || req["text"] != "hello" {
			t.Fatalf("unexpected send request: %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a1","result_code":"completed","completed_at":"2026-02-13T00:00:00Z"}`)
	})
	mux.HandleFunc("/v1/actions/view-output", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Fatalf("expected POST, got %s", r.Method)
		}
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["request_ref"] != "req-view" || req["target"] != "t1" || req["pane_id"] != "%1" {
			t.Fatalf("unexpected view-output request: %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a2","result_code":"completed","completed_at":"2026-02-13T00:00:00Z","output":"line1\nline2\n"}`)
	})
	mux.HandleFunc("/v1/actions/kill", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Fatalf("expected POST, got %s", r.Method)
		}
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["request_ref"] != "req-kill" || req["target"] != "t1" || req["pane_id"] != "%1" || req["mode"] != "key" {
			t.Fatalf("unexpected kill request: %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a3","result_code":"completed","completed_at":"2026-02-13T00:00:00Z"}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	if code := r.Run(context.Background(), []string{"send", "--request-ref", "req-send", "--target", "t1", "--pane", "%1", "--text", "hello", "--enter", "--json"}); code != 0 {
		t.Fatalf("send expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"action_id":"a1"`) {
		t.Fatalf("unexpected send output: %s", out.String())
	}
	out.Reset()
	if code := r.Run(context.Background(), []string{"view-output", "--request-ref", "req-view", "--target", "t1", "--pane", "%1", "--lines", "20", "--json"}); code != 0 {
		t.Fatalf("view-output expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"action_id":"a2"`) {
		t.Fatalf("unexpected view-output output: %s", out.String())
	}
	out.Reset()
	if code := r.Run(context.Background(), []string{"kill", "--request-ref", "req-kill", "--target", "t1", "--pane", "%1", "--mode", "key", "--signal", "INT", "--json"}); code != 0 {
		t.Fatalf("kill expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"action_id":"a3"`) {
		t.Fatalf("unexpected kill output: %s", out.String())
	}
}

func TestRunnerSendKeyPayload(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/send", func(w http.ResponseWriter, r *http.Request) {
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["request_ref"] != "req-send-key" || req["target"] != "t1" || req["pane_id"] != "%1" {
			t.Fatalf("unexpected request: %+v", req)
		}
		if req["key"] != "C-c" || req["text"] != "" {
			t.Fatalf("expected key payload with empty text, got %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a-key","result_code":"completed","completed_at":"2026-02-13T00:00:00Z"}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	if code := r.Run(context.Background(), []string{"send", "--request-ref", "req-send-key", "--target", "t1", "--pane", "%1", "--key", "C-c", "--json"}); code != 0 {
		t.Fatalf("send --key expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"action_id":"a-key"`) {
		t.Fatalf("unexpected output: %s", out.String())
	}
}

func TestRunnerSendPastePayload(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/send", func(w http.ResponseWriter, r *http.Request) {
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["request_ref"] != "req-send-paste" || req["target"] != "t1" || req["pane_id"] != "%1" {
			t.Fatalf("unexpected request: %+v", req)
		}
		if req["text"] != "hello world" || req["paste"] != true {
			t.Fatalf("expected paste payload, got %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a-paste","result_code":"completed","completed_at":"2026-02-13T00:00:00Z"}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	if code := r.Run(context.Background(), []string{"send", "--request-ref", "req-send-paste", "--target", "t1", "--pane", "%1", "--text", "hello world", "--paste", "--json"}); code != 0 {
		t.Fatalf("send --paste expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"action_id":"a-paste"`) {
		t.Fatalf("unexpected output: %s", out.String())
	}
}

func TestRunnerSendPreservesWhitespaceTextPayload(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/send", func(w http.ResponseWriter, r *http.Request) {
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["request_ref"] != "req-send-ws" || req["target"] != "t1" || req["pane_id"] != "%1" {
			t.Fatalf("unexpected request: %+v", req)
		}
		if req["text"] != "  hi  " {
			t.Fatalf("expected whitespace text payload, got %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a-send-ws","result_code":"completed","completed_at":"2026-02-13T00:00:00Z"}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	if code := r.Run(context.Background(), []string{"send", "--request-ref", "req-send-ws", "--target", "t1", "--pane", "%1", "--text", "  hi  ", "--json"}); code != 0 {
		t.Fatalf("send whitespace expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"action_id":"a-send-ws"`) {
		t.Fatalf("unexpected output: %s", out.String())
	}
}

func TestRunnerSendStdinPayload(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/send", func(w http.ResponseWriter, r *http.Request) {
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["request_ref"] != "req-send-stdin" || req["target"] != "t1" || req["pane_id"] != "%1" {
			t.Fatalf("unexpected request: %+v", req)
		}
		if req["text"] != "  hello\nworld  \n" || req["key"] != "" {
			t.Fatalf("expected stdin payload with empty key, got %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a-send-stdin","result_code":"completed","completed_at":"2026-02-13T00:00:00Z"}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	reader, writer, err := os.Pipe()
	if err != nil {
		t.Fatalf("pipe: %v", err)
	}
	if _, err := writer.WriteString("  hello\nworld  \n"); err != nil {
		t.Fatalf("write stdin payload: %v", err)
	}
	if err := writer.Close(); err != nil {
		t.Fatalf("close stdin writer: %v", err)
	}
	origStdin := os.Stdin
	os.Stdin = reader
	t.Cleanup(func() {
		os.Stdin = origStdin
		_ = reader.Close()
	})

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	if code := r.Run(context.Background(), []string{"send", "--request-ref", "req-send-stdin", "--target", "t1", "--pane", "%1", "--stdin", "--json"}); code != 0 {
		t.Fatalf("send stdin expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"action_id":"a-send-stdin"`) {
		t.Fatalf("unexpected output: %s", out.String())
	}
}

func TestRunnerSendStdinRejectsEmptyInput(t *testing.T) {
	reader, writer, err := os.Pipe()
	if err != nil {
		t.Fatalf("pipe: %v", err)
	}
	if err := writer.Close(); err != nil {
		t.Fatalf("close stdin writer: %v", err)
	}
	origStdin := os.Stdin
	os.Stdin = reader
	t.Cleanup(func() {
		os.Stdin = origStdin
		_ = reader.Close()
	})

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://example.invalid", &http.Client{}, out, errOut)
	code := r.Run(context.Background(), []string{"send", "--request-ref", "req-send-stdin-empty", "--target", "t1", "--pane", "%1", "--stdin"})
	if code != 2 {
		t.Fatalf("expected validation exit 2, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(errOut.String(), "non-empty payload") {
		t.Fatalf("expected empty stdin error, got %s", errOut.String())
	}
}

func TestRunnerSendStdinReadError(t *testing.T) {
	f, err := os.CreateTemp(t.TempDir(), "stdin-broken")
	if err != nil {
		t.Fatalf("create temp stdin file: %v", err)
	}
	name := f.Name()
	if err := f.Close(); err != nil {
		t.Fatalf("close temp stdin file: %v", err)
	}
	broken, err := os.Open(name)
	if err != nil {
		t.Fatalf("open temp stdin file: %v", err)
	}
	if err := broken.Close(); err != nil {
		t.Fatalf("close broken stdin handle: %v", err)
	}
	origStdin := os.Stdin
	os.Stdin = broken
	t.Cleanup(func() {
		os.Stdin = origStdin
	})

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://example.invalid", &http.Client{}, out, errOut)
	code := r.Run(context.Background(), []string{"send", "--request-ref", "req-send-stdin-err", "--target", "t1", "--pane", "%1", "--stdin"})
	if code != 1 {
		t.Fatalf("expected exit 1, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(errOut.String(), "read stdin") {
		t.Fatalf("expected read stdin error, got %s", errOut.String())
	}
}

func TestRunnerSendStdinRejectsPayloadTooLarge(t *testing.T) {
	reader, writer, err := os.Pipe()
	if err != nil {
		t.Fatalf("pipe: %v", err)
	}
	payload := strings.Repeat("a", int(maxSendStdinBytes)+1)
	writeDone := make(chan error, 1)
	go func() {
		if _, err := writer.WriteString(payload); err != nil {
			writeDone <- err
			_ = writer.Close()
			return
		}
		writeDone <- writer.Close()
	}()
	origStdin := os.Stdin
	os.Stdin = reader
	t.Cleanup(func() {
		os.Stdin = origStdin
		_ = reader.Close()
	})

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://example.invalid", &http.Client{}, out, errOut)
	code := r.Run(context.Background(), []string{"send", "--request-ref", "req-send-stdin-big", "--target", "t1", "--pane", "%1", "--stdin"})
	if code != 2 {
		t.Fatalf("expected validation exit 2, got %d stderr=%s", code, errOut.String())
	}
	if err := <-writeDone; err != nil {
		t.Fatalf("write/close stdin payload: %v", err)
	}
	if !strings.Contains(errOut.String(), "payload exceeds") {
		t.Fatalf("expected payload exceeds error, got %s", errOut.String())
	}
}

func TestRunnerSendRejectsTextAndStdin(t *testing.T) {
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://example.invalid", &http.Client{}, out, errOut)
	code := r.Run(context.Background(), []string{"send", "--request-ref", "req-send-x", "--target", "t1", "--pane", "%1", "--text", "hello", "--stdin"})
	if code != 2 {
		t.Fatalf("expected validation exit 2, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(errOut.String(), "exactly one of --text, --key, or --stdin is required") {
		t.Fatalf("expected exclusivity error, got %s", errOut.String())
	}
}

func TestRunnerSendRejectsKeyAndStdin(t *testing.T) {
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://example.invalid", &http.Client{}, out, errOut)
	code := r.Run(context.Background(), []string{"send", "--request-ref", "req-send-x", "--target", "t1", "--pane", "%1", "--key", "C-c", "--stdin"})
	if code != 2 {
		t.Fatalf("expected validation exit 2, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(errOut.String(), "exactly one of --text, --key, or --stdin is required") {
		t.Fatalf("expected exclusivity error, got %s", errOut.String())
	}
}

func TestSendValidation(t *testing.T) {
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://example.invalid", &http.Client{}, out, errOut)
	code := r.Run(context.Background(), []string{"send", "--target", "t1", "--pane", "%1", "--text", "hello"})
	if code != 2 {
		t.Fatalf("expected validation exit 2, got %d", code)
	}
	if !strings.Contains(errOut.String(), "request-ref") {
		t.Fatalf("expected validation message, got %s", errOut.String())
	}
}

func TestViewOutputValidation(t *testing.T) {
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://example.invalid", &http.Client{}, out, errOut)
	code := r.Run(context.Background(), []string{"view-output", "--target", "t1", "--pane", "%1"})
	if code != 2 {
		t.Fatalf("expected validation exit 2, got %d", code)
	}
	if !strings.Contains(errOut.String(), "request-ref") {
		t.Fatalf("expected validation message, got %s", errOut.String())
	}
}

func TestKillValidation(t *testing.T) {
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://example.invalid", &http.Client{}, out, errOut)
	code := r.Run(context.Background(), []string{"kill", "--target", "t1", "--pane", "%1"})
	if code != 2 {
		t.Fatalf("expected validation exit 2, got %d", code)
	}
	if !strings.Contains(errOut.String(), "request-ref") {
		t.Fatalf("expected validation message, got %s", errOut.String())
	}
}

func TestRunnerKillDefaultsPayload(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/kill", func(w http.ResponseWriter, r *http.Request) {
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["mode"] != "key" || req["signal"] != "INT" {
			t.Fatalf("expected default mode/signal, got %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a1","result_code":"completed","completed_at":"2026-02-13T00:00:00Z"}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	if code := r.Run(context.Background(), []string{"kill", "--request-ref", "req-kill-default", "--target", "t1", "--pane", "%1", "--json"}); code != 0 {
		t.Fatalf("kill expected exit 0, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunnerKillSignalPayload(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/kill", func(w http.ResponseWriter, r *http.Request) {
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["mode"] != "signal" || req["signal"] != "TERM" {
			t.Fatalf("expected signal mode payload, got %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a-kill-signal","result_code":"completed","completed_at":"2026-02-13T00:00:00Z"}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	if code := r.Run(context.Background(), []string{"kill", "--request-ref", "req-kill-signal", "--target", "t1", "--pane", "%1", "--mode", "signal", "--signal", "TERM", "--json"}); code != 0 {
		t.Fatalf("kill signal expected exit 0, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunnerViewOutputDefaultLinesPayload(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/view-output", func(w http.ResponseWriter, r *http.Request) {
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["lines"] != float64(200) {
			t.Fatalf("expected default lines=200, got %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a1","result_code":"completed","completed_at":"2026-02-13T00:00:00Z"}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	if code := r.Run(context.Background(), []string{"view-output", "--request-ref", "req-view-default", "--target", "t1", "--pane", "%1", "--json"}); code != 0 {
		t.Fatalf("view-output expected exit 0, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunnerViewOutputExplicitLinesPayload(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/view-output", func(w http.ResponseWriter, r *http.Request) {
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["lines"] != float64(20) {
			t.Fatalf("expected explicit lines=20, got %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a-view-lines","result_code":"completed","completed_at":"2026-02-13T00:00:00Z"}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	if code := r.Run(context.Background(), []string{"view-output", "--request-ref", "req-view-lines", "--target", "t1", "--pane", "%1", "--lines", "20", "--json"}); code != 0 {
		t.Fatalf("view-output expected exit 0, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunnerSendEnterPayload(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/send", func(w http.ResponseWriter, r *http.Request) {
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["enter"] != true || req["text"] != "hello" {
			t.Fatalf("expected enter payload, got %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a-send-enter","result_code":"completed","completed_at":"2026-02-13T00:00:00Z"}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	if code := r.Run(context.Background(), []string{"send", "--request-ref", "req-send-enter", "--target", "t1", "--pane", "%1", "--text", "hello", "--enter", "--json"}); code != 0 {
		t.Fatalf("send --enter expected exit 0, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunnerViewOutputPlainOutputFormatting(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/view-output", func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a-view","result_code":"completed","completed_at":"2026-02-13T00:00:00Z","output":"line1\nline2\n"}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	if code := r.Run(context.Background(), []string{"view-output", "--request-ref", "req-view-plain", "--target", "t1", "--pane", "%1"}); code != 0 {
		t.Fatalf("view-output plain expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if out.String() != "line1\nline2\n" {
		t.Fatalf("unexpected plain output formatting: %q", out.String())
	}
}

func TestRunnerSendIncludesGuardFields(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/send", func(w http.ResponseWriter, r *http.Request) {
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["if_runtime"] != "rt-1" || req["if_state"] != "running" || req["if_updated_within"] != "30s" || req["force_stale"] != true {
			t.Fatalf("expected guard fields in send request, got %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a-send-guard","result_code":"completed","completed_at":"2026-02-13T00:00:00Z"}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	code := r.Run(context.Background(), []string{
		"send",
		"--request-ref", "req-send-guard",
		"--target", "t1",
		"--pane", "%1",
		"--text", "hello",
		"--if-runtime", "rt-1",
		"--if-state", "running",
		"--if-updated-within", "30s",
		"--force-stale",
		"--json",
	})
	if code != 0 {
		t.Fatalf("send with guard fields expected exit 0, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunnerViewOutputIncludesGuardFields(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/view-output", func(w http.ResponseWriter, r *http.Request) {
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["if_runtime"] != "rt-2" || req["if_state"] != "waiting" || req["if_updated_within"] != "45s" || req["force_stale"] != true {
			t.Fatalf("expected guard fields in view-output request, got %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a-view-guard","result_code":"completed","completed_at":"2026-02-13T00:00:00Z","output":"ok\n"}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	code := r.Run(context.Background(), []string{
		"view-output",
		"--request-ref", "req-view-guard",
		"--target", "t1",
		"--pane", "%1",
		"--lines", "10",
		"--if-runtime", "rt-2",
		"--if-state", "waiting",
		"--if-updated-within", "45s",
		"--force-stale",
		"--json",
	})
	if code != 0 {
		t.Fatalf("view-output with guard fields expected exit 0, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunnerKillIncludesGuardFields(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/kill", func(w http.ResponseWriter, r *http.Request) {
		var req map[string]any
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Fatalf("decode body: %v", err)
		}
		if req["if_runtime"] != "rt-3" || req["if_state"] != "running" || req["if_updated_within"] != "15s" || req["force_stale"] != true {
			t.Fatalf("expected guard fields in kill request, got %+v", req)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a-kill-guard","result_code":"completed","completed_at":"2026-02-13T00:00:00Z"}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	code := r.Run(context.Background(), []string{
		"kill",
		"--request-ref", "req-kill-guard",
		"--target", "t1",
		"--pane", "%1",
		"--mode", "key",
		"--if-runtime", "rt-3",
		"--if-state", "running",
		"--if-updated-within", "15s",
		"--force-stale",
		"--json",
	})
	if code != 0 {
		t.Fatalf("kill with guard fields expected exit 0, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunnerActionEventsJSON(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/a-send/events", func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet {
			t.Fatalf("expected GET, got %s", r.Method)
		}
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a-send","events":[{"event_id":"ev-1","action_id":"a-send","runtime_id":"rt-1","event_type":"action.send","source":"daemon","event_time":"2026-02-13T00:00:00Z","ingested_at":"2026-02-13T00:00:00Z","dedupe_key":"k1"}]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	code := r.Run(context.Background(), []string{"events", "--action-id", "a-send", "--json"})
	if code != 0 {
		t.Fatalf("events --json expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), `"event_id":"ev-1"`) {
		t.Fatalf("unexpected events json output: %s", out.String())
	}
}

func TestRunnerActionEventsPlain(t *testing.T) {
	mux := http.NewServeMux()
	mux.HandleFunc("/v1/actions/a-send/events", func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, `{"schema_version":"v1","generated_at":"2026-02-13T00:00:00Z","action_id":"a-send","events":[{"event_id":"ev-1","action_id":"a-send","runtime_id":"rt-1","event_type":"action.send","source":"daemon","event_time":"2026-02-13T00:00:00Z","ingested_at":"2026-02-13T00:00:00Z","dedupe_key":"k1"}]}`)
	})
	srv := httptest.NewServer(mux)
	defer srv.Close()

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient(srv.URL, srv.Client(), out, errOut)
	code := r.Run(context.Background(), []string{"events", "--action-id", "a-send"})
	if code != 0 {
		t.Fatalf("events expected exit 0, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(out.String(), "action_id=a-send events=1") {
		t.Fatalf("unexpected events plain output: %s", out.String())
	}
	if !strings.Contains(out.String(), "event=ev-1 type=action.send source=daemon runtime=rt-1") {
		t.Fatalf("unexpected events plain detail output: %s", out.String())
	}
}

func TestRunnerActionEventsValidation(t *testing.T) {
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://example.invalid", &http.Client{}, out, errOut)
	code := r.Run(context.Background(), []string{"events"})
	if code != 2 {
		t.Fatalf("expected validation exit 2, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(errOut.String(), "--action-id") {
		t.Fatalf("expected validation message, got %s", errOut.String())
	}
}

func TestRunnerAppCommandNotFound(t *testing.T) {
	t.Setenv("AGTMUX_APP_BIN", filepath.Join(t.TempDir(), "does-not-exist"))
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://example.invalid", &http.Client{}, out, errOut)
	code := r.Run(context.Background(), []string{"app", "run", "--once"})
	if code != 1 {
		t.Fatalf("expected exit 1 for missing app binary, got %d stderr=%s", code, errOut.String())
	}
	if !strings.Contains(errOut.String(), "agtmux-app binary not found") {
		t.Fatalf("expected not found message, got %s", errOut.String())
	}
}

func TestRunnerAppCommandPassesSocketAndArgs(t *testing.T) {
	tmpDir := t.TempDir()
	argsFile := filepath.Join(tmpDir, "args.txt")
	script := filepath.Join(tmpDir, "fake-app.sh")
	scriptBody := "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"" + argsFile + "\"\nexit 0\n"
	if err := os.WriteFile(script, []byte(scriptBody), 0o755); err != nil {
		t.Fatalf("write fake app script: %v", err)
	}
	t.Setenv("AGTMUX_APP_BIN", script)

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://example.invalid", &http.Client{}, out, errOut)
	code := r.Run(context.Background(), []string{"--socket", "/tmp/custom.sock", "app", "run", "--once"})
	if code != 0 {
		t.Fatalf("expected exit 0, got %d stderr=%s", code, errOut.String())
	}

	argsRaw, err := os.ReadFile(argsFile)
	if err != nil {
		t.Fatalf("read args file: %v", err)
	}
	lines := strings.Split(strings.TrimSpace(string(argsRaw)), "\n")
	expected := []string{"--socket", "/tmp/custom.sock", "run", "--once"}
	if len(lines) != len(expected) {
		t.Fatalf("expected args %v, got %v", expected, lines)
	}
	for i := range expected {
		if lines[i] != expected[i] {
			t.Fatalf("expected args %v, got %v", expected, lines)
		}
	}
}

func TestRunnerAppCommandPropagatesExitCode(t *testing.T) {
	tmpDir := t.TempDir()
	script := filepath.Join(tmpDir, "fake-app-fail.sh")
	if err := os.WriteFile(script, []byte("#!/bin/sh\nexit 7\n"), 0o755); err != nil {
		t.Fatalf("write fake app fail script: %v", err)
	}
	t.Setenv("AGTMUX_APP_BIN", script)

	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	r := NewRunnerWithClient("http://example.invalid", &http.Client{}, out, errOut)
	code := r.Run(context.Background(), []string{"app", "run"})
	if code != 7 {
		t.Fatalf("expected exit 7, got %d stderr=%s", code, errOut.String())
	}
}
