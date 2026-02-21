package daemon

import (
	"bufio"
	"context"
	"fmt"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/db"
	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/target"
	"github.com/g960059/agtmux/internal/ttyv2"
)

func TestTTYV2SessionHelloAttachAndPing(t *testing.T) {
	tmp := t.TempDir()
	socketPath := shortSocketPath(t, "agtmuxd-ttyv2-attach")
	dbPath := filepath.Join(tmp, "state.db")

	ctx := context.Background()
	store, err := db.Open(ctx, dbPath)
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	t.Cleanup(func() { _ = store.Close() })
	if err := db.ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	now := time.Now().UTC()
	if err := store.UpsertTarget(ctx, model.Target{
		TargetID:   "t-local",
		TargetName: "local",
		Kind:       model.TargetKindLocal,
		Health:     model.TargetHealthOK,
		UpdatedAt:  now,
	}); err != nil {
		t.Fatalf("upsert target: %v", err)
	}
	if err := store.UpsertPane(ctx, model.Pane{
		TargetID:     "t-local",
		PaneID:       "%10",
		SessionName:  "exp-go-codex-implementation-poc",
		WindowID:     "@3",
		WindowName:   "main",
		CurrentCmd:   "zsh",
		CurrentPath:  "/tmp",
		PaneTitle:    "zsh",
		HistoryBytes: 10,
		UpdatedAt:    now,
	}); err != nil {
		t.Fatalf("upsert pane: %v", err)
	}

	runner := &stubRunner{}
	cfg := config.DefaultConfig()
	cfg.SocketPath = socketPath
	cfg.CommandTimeout = 1 * time.Second
	executor := target.NewExecutorWithRunner(cfg, runner)
	srv := NewServerWithDeps(cfg, store, executor)

	startCtx, cancel := context.WithCancel(context.Background())
	defer cancel()
	errCh := make(chan error, 1)
	go func() {
		errCh <- srv.Start(startCtx)
	}()
	waitForSocket(t, socketPath, errCh)

	conn, err := net.Dial("unix", socketPath)
	if err != nil {
		t.Fatalf("dial unix: %v", err)
	}
	defer conn.Close() //nolint:errcheck

	br := bufio.NewReader(conn)
	bw := bufio.NewWriter(conn)
	if _, err := bw.WriteString("GET /v2/tty/session HTTP/1.1\r\nHost: unix\r\nConnection: Upgrade\r\nUpgrade: agtmux-tty-v2\r\n\r\n"); err != nil {
		t.Fatalf("write upgrade request: %v", err)
	}
	if err := bw.Flush(); err != nil {
		t.Fatalf("flush upgrade request: %v", err)
	}

	statusLine, err := br.ReadString('\n')
	if err != nil {
		t.Fatalf("read status line: %v", err)
	}
	if !strings.Contains(statusLine, "101") {
		t.Fatalf("expected 101 switching protocols, got %q", statusLine)
	}
	for {
		line, readErr := br.ReadString('\n')
		if readErr != nil {
			t.Fatalf("read header line: %v", readErr)
		}
		if line == "\r\n" {
			break
		}
	}

	sendFrame := func(frameType, requestID string, payload any) {
		env, newErr := ttyv2.NewEnvelope(frameType, 1, requestID, payload)
		if newErr != nil {
			t.Fatalf("new envelope(%s): %v", frameType, newErr)
		}
		if writeErr := ttyv2.WriteFrame(bw, env); writeErr != nil {
			t.Fatalf("write frame(%s): %v", frameType, writeErr)
		}
		if flushErr := bw.Flush(); flushErr != nil {
			t.Fatalf("flush frame(%s): %v", frameType, flushErr)
		}
	}

	sendFrame("hello", "req-hello", ttyv2.HelloPayload{
		ClientID:         "agtmux-desktop",
		ProtocolVersions: []string{ttyv2.SchemaVersion},
		Capabilities:     []string{"raw_output", "resync"},
	})
	helloAck, err := ttyv2.ReadFrame(br, ttyv2.DefaultMaxFrame)
	if err != nil {
		t.Fatalf("read hello_ack: %v", err)
	}
	if helloAck.Type != "hello_ack" {
		t.Fatalf("expected hello_ack, got %s", helloAck.Type)
	}

	pref := ttyv2.PaneRef{Target: "local", SessionName: "exp-go-codex-implementation-poc", WindowID: "@3", PaneID: "%10"}
	cols := 120
	rows := 42
	wantSnapshot := false
	sendFrame("attach", "req-attach", ttyv2.AttachPayload{
		PaneRef:             pref,
		AttachMode:          "live",
		WantInitialSnapshot: &wantSnapshot,
		Cols:                intPtr(cols),
		Rows:                intPtr(rows),
	})
	attached, err := ttyv2.ReadFrame(br, ttyv2.DefaultMaxFrame)
	if err != nil {
		t.Fatalf("read attached: %v", err)
	}
	if attached.Type != "attached" {
		t.Fatalf("expected attached, got %s", attached.Type)
	}
	var attachedPayload ttyv2.AttachedPayload
	if err := attached.DecodePayload(&attachedPayload); err != nil {
		t.Fatalf("decode attached payload: %v", err)
	}
	if attachedPayload.PaneAlias == "" {
		t.Fatalf("expected pane alias in attached payload")
	}
	if attachedPayload.SnapshotMode != "stream" {
		t.Fatalf("expected snapshot_mode=stream, got %q", attachedPayload.SnapshotMode)
	}
	if attachedPayload.InitialSnapshotANSIB64 != "" {
		t.Fatalf("expected no initial snapshot in stream-only attach, got non-empty payload")
	}
	if len(runner.calls) == 0 {
		t.Fatalf("expected resize-pane call during attach")
	}
	foundResize := false
	for _, call := range runner.calls {
		joined := strings.Join(call.args, " ")
		if strings.Contains(joined, "resize-pane") &&
			strings.Contains(joined, "-t %10") &&
			strings.Contains(joined, "-x 120") &&
			strings.Contains(joined, "-y 42") {
			foundResize = true
			break
		}
	}
	if !foundResize {
		t.Fatalf("expected attach SIGWINCH resize command, calls=%+v", runner.calls)
	}

	stateFrame, err := ttyv2.ReadFrame(br, ttyv2.DefaultMaxFrame)
	if err != nil {
		t.Fatalf("read state frame: %v", err)
	}
	if stateFrame.Type != "state" {
		t.Fatalf("expected state frame, got %s", stateFrame.Type)
	}

	sendFrame("ping", "req-ping", ttyv2.PingPayload{TS: "2026-02-20T12:35:00.000Z"})
	pong, err := ttyv2.ReadFrame(br, ttyv2.DefaultMaxFrame)
	if err != nil {
		t.Fatalf("read pong: %v", err)
	}
	if pong.Type != "pong" {
		t.Fatalf("expected pong, got %s", pong.Type)
	}

	cancel()
	select {
	case startErr := <-errCh:
		if startErr != nil && startErr != context.Canceled {
			t.Fatalf("server shutdown error: %v", startErr)
		}
	case <-time.After(3 * time.Second):
		t.Fatalf("timeout waiting for daemon shutdown")
	}

	if _, statErr := os.Stat(socketPath); statErr == nil {
		t.Fatalf("expected socket to be removed on shutdown")
	}
}

func TestTTYV2SessionRejectsMissingUpgradeHeader(t *testing.T) {
	srv, _ := newAPITestServer(t, &stubRunner{})
	rec := doJSONRequest(t, srv.httpSrv.Handler, http.MethodGet, "/v2/tty/session", nil)
	if rec.Code != http.StatusUpgradeRequired {
		t.Fatalf("expected 426, got %d body=%s", rec.Code, rec.Body.String())
	}
	if !strings.Contains(rec.Body.String(), model.ErrRefInvalid) {
		t.Fatalf("expected error code %s, got body=%s", model.ErrRefInvalid, rec.Body.String())
	}
}

func TestTTYV2SessionUnknownFrameReturnsError(t *testing.T) {
	tmp := t.TempDir()
	socketPath := shortSocketPath(t, "agtmuxd-ttyv2-unknown")
	dbPath := filepath.Join(tmp, "state.db")

	ctx := context.Background()
	store, err := db.Open(ctx, dbPath)
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	t.Cleanup(func() { _ = store.Close() })
	if err := db.ApplyMigrations(ctx, store.DB()); err != nil {
		t.Fatalf("apply migrations: %v", err)
	}

	cfg := config.DefaultConfig()
	cfg.SocketPath = socketPath
	srv := NewServerWithDeps(cfg, store, target.NewExecutorWithRunner(cfg, &stubRunner{}))

	startCtx, cancel := context.WithCancel(context.Background())
	defer cancel()
	errCh := make(chan error, 1)
	go func() { errCh <- srv.Start(startCtx) }()
	waitForSocket(t, socketPath, errCh)

	conn, err := net.Dial("unix", socketPath)
	if err != nil {
		t.Fatalf("dial unix: %v", err)
	}
	defer conn.Close() //nolint:errcheck

	br := bufio.NewReader(conn)
	bw := bufio.NewWriter(conn)
	_, _ = bw.WriteString("GET /v2/tty/session HTTP/1.1\r\nHost: unix\r\nConnection: Upgrade\r\nUpgrade: agtmux-tty-v2\r\n\r\n")
	_ = bw.Flush()
	_, _ = br.ReadString('\n')
	for {
		line, readErr := br.ReadString('\n')
		if readErr != nil {
			t.Fatalf("read headers: %v", readErr)
		}
		if line == "\r\n" {
			break
		}
	}
	env, newErr := ttyv2.NewEnvelope("unknown", 1, "req-x", map[string]any{"x": 1})
	if newErr != nil {
		t.Fatalf("new envelope: %v", newErr)
	}
	if writeErr := ttyv2.WriteFrame(bw, env); writeErr != nil {
		t.Fatalf("write frame: %v", writeErr)
	}
	if flushErr := bw.Flush(); flushErr != nil {
		t.Fatalf("flush frame: %v", flushErr)
	}

	errFrame, readErr := ttyv2.ReadFrame(br, ttyv2.DefaultMaxFrame)
	if readErr != nil {
		t.Fatalf("read error frame: %v", readErr)
	}
	if errFrame.Type != "error" {
		t.Fatalf("expected error frame, got %s", errFrame.Type)
	}
	var payload ttyv2.ErrorPayload
	if decodeErr := errFrame.DecodePayload(&payload); decodeErr != nil {
		t.Fatalf("decode error payload: %v", decodeErr)
	}
	if payload.Code != "e_protocol_invalid_frame" {
		t.Fatalf("unexpected error payload: %+v", payload)
	}

	cancel()
	select {
	case startErr := <-errCh:
		if startErr != nil && startErr != context.Canceled {
			t.Fatalf("server shutdown error: %v", startErr)
		}
	case <-time.After(3 * time.Second):
		t.Fatalf("timeout waiting for daemon shutdown")
	}
}

func shortSocketPath(t *testing.T, prefix string) string {
	t.Helper()
	path := filepath.Join(os.TempDir(), fmt.Sprintf("%s-%d.sock", prefix, time.Now().UnixNano()))
	t.Cleanup(func() {
		_ = os.Remove(path)
	})
	return path
}
