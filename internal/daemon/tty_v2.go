package daemon

import (
	"bufio"
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"strings"
	"sync"
	"time"

	"golang.org/x/sys/unix"

	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/target"
	"github.com/g960059/agtmux/internal/ttyv2"
)

const ttyV2UpgradeToken = "agtmux-tty-v2"
const ttyV2OutputPollInterval = 75 * time.Millisecond
const ttyV2BackgroundCaptureInterval = 250 * time.Millisecond
const ttyV2SSHBackgroundCaptureInterval = 450 * time.Millisecond
const ttyV2BackgroundDispatchInterval = 350 * time.Millisecond
const ttyV2PendingDropWatermark = 4
const ttyV2CaptureBackoffLocalForegroundBase = 100 * time.Millisecond
const ttyV2CaptureBackoffLocalBackgroundBase = 250 * time.Millisecond
const ttyV2CaptureBackoffSSHForegroundBase = 280 * time.Millisecond
const ttyV2CaptureBackoffSSHBackgroundBase = 650 * time.Millisecond
const ttyV2CaptureBackoffLocalMax = 2 * time.Second
const ttyV2CaptureBackoffSSHMax = 8 * time.Second
const ttyV2CaptureErrorThrottleForeground = 1200 * time.Millisecond
const ttyV2CaptureErrorThrottleBackground = 3 * time.Second
const ttyV2BridgeEventBuffer = 512

const (
	ttyV2ResyncReasonManual       = "manual"
	ttyV2ResyncReasonLayoutChange = "layout_change"
	ttyV2ResyncReasonUnknown      = "unknown"
)

type ttyV2AttachedPane struct {
	key               string
	alias             string
	ref               ttyv2.PaneRef
	targetID          string
	lastContent       string
	lastSource        string
	lastCursorX       *int
	lastCursorY       *int
	lastPaneCols      *int
	lastPaneRows      *int
	outputSeq         uint64
	lastOutputAt      time.Time
	lastCaptureAt     time.Time
	nextCaptureAt     time.Time
	captureFailures   int
	lastErrorAt       time.Time
	pendingRaw        string
	pendingSource     string
	pendingCursorX    *int
	pendingCursorY    *int
	pendingPaneCols   *int
	pendingPaneRows   *int
	pendingSeq        uint64
	pendingFrom       uint64
	pendingDrops      int
	lastBridgeAt      time.Time
	forceResync       bool
	forceResyncReason string
}

type ttyV2ResolvedPane struct {
	target model.Target
	pane   model.Pane
	found  bool
}

type ttyV2Session struct {
	srv            *Server
	conn           net.Conn
	rw             *bufio.ReadWriter
	sendMu         sync.Mutex
	stateMu        sync.RWMutex
	closeMu        sync.Mutex
	closed         bool
	done           chan struct{}
	attached       map[string]*ttyV2AttachedPane
	focusKey       string
	nextAlias      int
	nextSeq        uint64
	bridge         *tmuxControlBridgeHandle
	paneTap        *paneTapHandle
	layoutByWindow map[string]tmuxLayoutGeometry
	telemetry      ttyV2SessionTelemetry
}

type ttyV2SessionTelemetry struct {
	HotpathCaptureSelected    int
	HotpathCaptureNonSelected int
	OutputBridge              int
	OutputPaneTap             int
	OutputSnapshot            int
	ResyncQueuedByReason      map[string]int
	ResyncAppliedByReason     map[string]int
}

type ttyV2SessionTelemetrySnapshot struct {
	HotpathCaptureSelected    int
	HotpathCaptureNonSelected int
	OutputBridge              int
	OutputPaneTap             int
	OutputSnapshot            int
	ResyncQueuedByReason      map[string]int
	ResyncAppliedByReason     map[string]int
}

type tmuxControlBridgeHandle struct {
	targetName  string
	sessionName string
	ctx         context.Context
	cancel      context.CancelFunc
	cmd         *exec.Cmd
	stdin       io.WriteCloser
	stdinMu     sync.Mutex
	events      chan tmuxControlEvent
	errs        chan error
}

func (h *tmuxControlBridgeHandle) sendCommands(commands ...string) error {
	if h == nil || h.stdin == nil {
		return errors.New("bridge stdin unavailable")
	}
	h.stdinMu.Lock()
	defer h.stdinMu.Unlock()
	for _, command := range commands {
		line := strings.TrimSpace(command)
		if line == "" {
			continue
		}
		if _, err := io.WriteString(h.stdin, line+"\n"); err != nil {
			return err
		}
	}
	return nil
}

func (s *Server) ttyV2SessionHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		s.methodNotAllowed(w, http.MethodGet)
		return
	}
	if !strings.EqualFold(strings.TrimSpace(r.Header.Get("Upgrade")), ttyV2UpgradeToken) {
		s.writeError(w, http.StatusUpgradeRequired, model.ErrRefInvalid, "upgrade header is required")
		return
	}
	if !strings.Contains(strings.ToLower(strings.TrimSpace(r.Header.Get("Connection"))), "upgrade") {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "connection upgrade header is required")
		return
	}

	hj, ok := w.(http.Hijacker)
	if !ok {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "hijack not supported")
		return
	}

	conn, rw, err := hj.Hijack()
	if err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to hijack tty session")
		return
	}

	if err := s.verifyTTYV2PeerConn(conn); err != nil {
		_, _ = rw.WriteString("HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n")
		_ = rw.Flush()
		_ = conn.Close()
		return
	}

	if _, err := rw.WriteString("HTTP/1.1 101 Switching Protocols\r\nUpgrade: " + ttyV2UpgradeToken + "\r\nConnection: Upgrade\r\n\r\n"); err != nil {
		_ = conn.Close()
		return
	}
	if err := rw.Flush(); err != nil {
		_ = conn.Close()
		return
	}

	session := &ttyV2Session{
		srv:            s,
		conn:           conn,
		rw:             rw,
		done:           make(chan struct{}),
		attached:       map[string]*ttyV2AttachedPane{},
		nextAlias:      1,
		nextSeq:        1,
		layoutByWindow: map[string]tmuxLayoutGeometry{},
		telemetry: ttyV2SessionTelemetry{
			ResyncQueuedByReason:  map[string]int{},
			ResyncAppliedByReason: map[string]int{},
		},
	}
	go session.outputLoop()
	session.readLoop()
}

func (s *Server) verifyTTYV2PeerConn(conn net.Conn) error {
	uc, ok := conn.(*net.UnixConn)
	if !ok {
		return fmt.Errorf("ttyv2 requires unix domain socket")
	}
	raw, err := uc.SyscallConn()
	if err != nil {
		return fmt.Errorf("peer syscall conn: %w", err)
	}
	var peerUID uint32
	var controlErr error
	if err := raw.Control(func(fd uintptr) {
		creds, credErr := unix.GetsockoptXucred(int(fd), unix.SOL_LOCAL, unix.LOCAL_PEERCRED)
		if credErr != nil {
			controlErr = credErr
			return
		}
		peerUID = creds.Uid
	}); err != nil {
		return fmt.Errorf("peer control: %w", err)
	}
	if controlErr != nil {
		return fmt.Errorf("peer credentials: %w", controlErr)
	}
	expectedUID := uint32(os.Getuid())
	if peerUID != expectedUID {
		return fmt.Errorf("peer uid mismatch")
	}
	return nil
}

func (ss *ttyV2Session) close() {
	ss.closeMu.Lock()
	if ss.closed {
		ss.closeMu.Unlock()
		return
	}
	ss.closed = true
	close(ss.done)
	ss.stopControlBridgeLocked()
	ss.stopPaneTapLocked()
	ss.closeMu.Unlock()
	_ = ss.conn.Close()
}

func (ss *ttyV2Session) send(frameType string, requestID string, payload any) error {
	env, err := ttyv2.NewEnvelope(frameType, ss.nextFrameSeq(), requestID, payload)
	if err != nil {
		return err
	}
	ss.sendMu.Lock()
	defer ss.sendMu.Unlock()
	if err := ttyv2.WriteFrame(ss.rw, env); err != nil {
		return err
	}
	if err := ss.rw.Flush(); err != nil {
		return err
	}
	return nil
}

func (ss *ttyV2Session) nextFrameSeq() uint64 {
	ss.stateMu.Lock()
	defer ss.stateMu.Unlock()
	seq := ss.nextSeq
	ss.nextSeq++
	return seq
}

func (ss *ttyV2Session) sendError(requestID, code, message string, recoverable bool, pref *ttyv2.PaneRef) {
	_ = ss.send("error", requestID, ttyv2.ErrorPayload{
		Code:        strings.TrimSpace(code),
		Message:     strings.TrimSpace(message),
		Recoverable: recoverable,
		PaneRef:     pref,
	})
}

func (ss *ttyV2Session) readLoop() {
	defer ss.close()
	for {
		env, err := ttyv2.ReadFrame(ss.rw, ttyv2.DefaultMaxFrame)
		if err != nil {
			if errors.Is(err, io.EOF) || errors.Is(err, net.ErrClosed) {
				return
			}
			ss.sendError("", "e_protocol_invalid_frame", "invalid tty v2 frame", false, nil)
			return
		}
		if err := ss.handleFrame(env); err != nil {
			if errors.Is(err, io.EOF) || errors.Is(err, net.ErrClosed) {
				return
			}
			ss.sendError(env.RequestID, "e_internal", err.Error(), true, nil)
		}
	}
}

func (ss *ttyV2Session) handleFrame(env ttyv2.Envelope) error {
	switch env.Type {
	case "hello":
		return ss.handleHello(env)
	case "attach":
		return ss.handleAttach(env)
	case "write":
		return ss.handleWrite(env)
	case "resize":
		return ss.handleResize(env)
	case "focus":
		return ss.handleFocus(env)
	case "detach":
		return ss.handleDetach(env)
	case "resync":
		return ss.handleResync(env)
	case "ping":
		return ss.handlePing(env)
	default:
		ss.sendError(env.RequestID, "e_protocol_invalid_frame", "unknown frame type", true, nil)
		return nil
	}
}

func (ss *ttyV2Session) handleHello(env ttyv2.Envelope) error {
	var req ttyv2.HelloPayload
	if err := env.DecodePayload(&req); err != nil {
		ss.sendError(env.RequestID, "e_protocol_invalid_frame", "invalid hello payload", false, nil)
		return nil
	}
	ok := false
	for _, ver := range req.ProtocolVersions {
		if strings.TrimSpace(ver) == ttyv2.SchemaVersion {
			ok = true
			break
		}
	}
	if !ok {
		ss.sendError(env.RequestID, "e_protocol_unsupported_version", "tty.v2.0 is required", false, nil)
		return nil
	}
	return ss.send("hello_ack", env.RequestID, ttyv2.HelloAckPayload{
		ServerID:        "agtmuxd",
		ProtocolVersion: ttyv2.SchemaVersion,
		Features: []string{
			"raw_output",
			"resync",
			"peer_cred_auth",
			"resize_conflict_ack",
			"pane_alias",
			"coalescing_latest_wins",
		},
	})
}

func (ss *ttyV2Session) handleAttach(env ttyv2.Envelope) error {
	var req ttyv2.AttachPayload
	if err := env.DecodePayload(&req); err != nil {
		ss.sendError(env.RequestID, "e_protocol_invalid_frame", "invalid attach payload", true, nil)
		return nil
	}
	if !req.PaneRef.IsValid() {
		ss.sendError(env.RequestID, "e_ref_invalid", "pane_ref is required", true, nil)
		return nil
	}
	tg, pane, err := ss.resolveTargetAndPane(req.PaneRef)
	if err != nil {
		ss.sendError(env.RequestID, "e_ref_not_found", "target/pane not found", true, &req.PaneRef)
		return nil
	}

	wantInitialSnapshot := req.WantInitialSnapshot != nil && *req.WantInitialSnapshot
	var (
		content  string
		cursorX  *int
		cursorY  *int
		paneCols *int
		paneRows *int
	)
	snapshotMode := "stream"
	if wantInitialSnapshot {
		lines := defaultTerminalStreamLines
		captured, cx, cy, cols, rows, runErr := ss.srv.capturePaneSnapshotWithCursor(
			context.Background(),
			tg,
			req.PaneRef.PaneID,
			lines,
		)
		if runErr != nil {
			ss.sendError(env.RequestID, "e_tmux_bridge_down", "failed to read pane snapshot", true, &req.PaneRef)
			return nil
		}
		content = trimSnapshotToVisibleRows(captured, rows)
		cursorX = cloneIntPtr(cx)
		cursorY = cloneIntPtr(cy)
		paneCols = cloneIntPtr(cols)
		paneRows = cloneIntPtr(rows)
		snapshotMode = "initial"
	}

	key := req.PaneRef.CanonicalKey()
	ss.stateMu.Lock()
	attached := ss.attached[key]
	if attached == nil {
		attached = &ttyV2AttachedPane{
			key:      key,
			alias:    fmt.Sprintf("p%d", ss.nextAlias),
			ref:      req.PaneRef,
			targetID: tg.TargetID,
		}
		ss.nextAlias++
		ss.attached[key] = attached
	}
	attached.ref = req.PaneRef
	attached.targetID = tg.TargetID
	if wantInitialSnapshot {
		attached.lastContent = clipTerminalStateContent(content)
		attached.lastSource = "snapshot"
	} else {
		attached.lastContent = ""
		attached.lastSource = "bridge"
	}
	attached.lastCursorX = cloneIntPtr(cursorX)
	attached.lastCursorY = cloneIntPtr(cursorY)
	attached.lastPaneCols = cloneIntPtr(paneCols)
	attached.lastPaneRows = cloneIntPtr(paneRows)
	attached.outputSeq++
	attached.lastCaptureAt = time.Now().UTC()
	attached.nextCaptureAt = time.Time{}
	attached.captureFailures = 0
	attached.lastErrorAt = time.Time{}
	attached.pendingRaw = ""
	attached.pendingSource = ""
	attached.pendingCursorX = nil
	attached.pendingCursorY = nil
	attached.pendingPaneCols = nil
	attached.pendingPaneRows = nil
	attached.pendingSeq = 0
	attached.pendingFrom = 0
	attached.pendingDrops = 0
	seq := attached.outputSeq
	alias := attached.alias
	ss.focusKey = key
	ss.stateMu.Unlock()

	// Bring up control-mode stream as soon as pane gets attached.
	ss.ensureControlBridgeForFocus(req.PaneRef)
	ss.alignTmuxControlBridgeFocus(req.PaneRef)
	ss.ensurePaneTapForFocus(req.PaneRef)

	state := ttyv2.TTYState{
		ActivityState:       "idle",
		AttentionState:      "none",
		SessionLastActiveAt: pane.UpdatedAt.UTC().Format(time.RFC3339Nano),
	}
	attachedPayload := ttyv2.AttachedPayload{
		PaneRef:      req.PaneRef,
		PaneAlias:    alias,
		OutputSeq:    seq,
		SnapshotMode: snapshotMode,
		CursorX:      cloneIntPtr(cursorX),
		CursorY:      cloneIntPtr(cursorY),
		PaneCols:     cloneIntPtr(paneCols),
		PaneRows:     cloneIntPtr(paneRows),
		State:        state,
	}
	if content != "" {
		attachedPayload.InitialSnapshotANSIB64 = base64.StdEncoding.EncodeToString([]byte(content))
	}
	if err := ss.send("attached", env.RequestID, ttyv2.AttachedPayload{
		PaneRef:                attachedPayload.PaneRef,
		PaneAlias:              attachedPayload.PaneAlias,
		OutputSeq:              attachedPayload.OutputSeq,
		InitialSnapshotANSIB64: attachedPayload.InitialSnapshotANSIB64,
		SnapshotMode:           attachedPayload.SnapshotMode,
		CursorX:                attachedPayload.CursorX,
		CursorY:                attachedPayload.CursorY,
		PaneCols:               attachedPayload.PaneCols,
		PaneRows:               attachedPayload.PaneRows,
		State:                  attachedPayload.State,
	}); err != nil {
		return err
	}
	ss.triggerSIGWINCHOnAttach(tg, req.PaneRef, req.Cols, req.Rows)
	return ss.send("state", "", ttyv2.StatePayload{PaneRef: req.PaneRef, State: state})
}

func (ss *ttyV2Session) handleWrite(env ttyv2.Envelope) error {
	var req ttyv2.WritePayload
	if err := env.DecodePayload(&req); err != nil {
		ss.sendError(env.RequestID, "e_protocol_invalid_frame", "invalid write payload", true, nil)
		return nil
	}
	if !req.PaneRef.IsValid() {
		ss.sendError(env.RequestID, "e_ref_invalid", "pane_ref is required", true, nil)
		return nil
	}
	decoded, err := base64.StdEncoding.DecodeString(strings.TrimSpace(req.BytesBase64))
	if err != nil || len(decoded) == 0 {
		ss.sendError(env.RequestID, "e_protocol_invalid_frame", "bytes_base64 must be non-empty base64", true, &req.PaneRef)
		return nil
	}
	tg, resolveErr := ss.srv.store.GetTargetByName(context.Background(), req.PaneRef.Target)
	if resolveErr != nil {
		ss.sendError(env.RequestID, "e_ref_not_found", "target not found", true, &req.PaneRef)
		return nil
	}

	cmd := []string{"send-keys", "-t", req.PaneRef.PaneID}
	if literalText, ok := decodeLiteralSendKeysText(decoded); ok {
		cmd = append(cmd, "-l", literalText)
	} else {
		cmd = append(cmd, "-H")
		for _, b := range decoded {
			cmd = append(cmd, fmt.Sprintf("%02x", b))
		}
	}
	resultCode := "ok"
	if _, runErr := ss.srv.executor.Run(context.Background(), tg, target.BuildTmuxCommand(cmd...)); runErr != nil {
		resultCode = "stale_runtime"
	}

	ss.stateMu.Lock()
	if attached := ss.attached[req.PaneRef.CanonicalKey()]; attached != nil {
		attached.lastContent = ""
		attached.nextCaptureAt = time.Time{}
		attached.captureFailures = 0
		attached.lastErrorAt = time.Time{}
		attached.pendingRaw = ""
		attached.pendingSource = ""
		attached.pendingCursorX = nil
		attached.pendingCursorY = nil
		attached.pendingPaneCols = nil
		attached.pendingPaneRows = nil
		attached.pendingSeq = 0
		attached.pendingFrom = 0
		attached.pendingDrops = 0
	}
	ss.stateMu.Unlock()

	return ss.send("ack", env.RequestID, ttyv2.AckPayload{
		PaneRef:    &req.PaneRef,
		AckKind:    "write",
		InputSeq:   req.InputSeq,
		ResultCode: resultCode,
	})
}

func (ss *ttyV2Session) handleResize(env ttyv2.Envelope) error {
	var req ttyv2.ResizePayload
	if err := env.DecodePayload(&req); err != nil {
		ss.sendError(env.RequestID, "e_protocol_invalid_frame", "invalid resize payload", true, nil)
		return nil
	}
	if !req.PaneRef.IsValid() || req.Cols < 20 || req.Rows < 5 || req.Cols > 500 || req.Rows > 300 {
		ss.sendError(env.RequestID, "e_ref_invalid", "invalid resize payload", true, &req.PaneRef)
		return nil
	}
	resultCode := "ok"
	key := req.PaneRef.CanonicalKey()

	ss.stateMu.RLock()
	focusKey := ss.focusKey
	ss.stateMu.RUnlock()
	if focusKey != "" && focusKey != key {
		resultCode = "skipped_conflict"
	} else {
		tg, _, resolveErr := ss.resolveTargetAndPane(req.PaneRef)
		if resolveErr != nil {
			resultCode = "not_attached"
		} else {
			if _, runErr := ss.srv.executor.Run(
				context.Background(),
				tg,
				target.BuildTmuxCommand("resize-pane", "-t", req.PaneRef.PaneID, "-x", fmt.Sprintf("%d", req.Cols), "-y", fmt.Sprintf("%d", req.Rows)),
			); runErr != nil {
				resultCode = "stale_runtime"
			}
		}
	}

	return ss.send("ack", env.RequestID, ttyv2.AckPayload{
		PaneRef:    &req.PaneRef,
		AckKind:    "resize",
		ResizeSeq:  req.ResizeSeq,
		ResultCode: resultCode,
	})
}

func (ss *ttyV2Session) handleFocus(env ttyv2.Envelope) error {
	var req ttyv2.FocusPayload
	if err := env.DecodePayload(&req); err != nil {
		ss.sendError(env.RequestID, "e_protocol_invalid_frame", "invalid focus payload", true, nil)
		return nil
	}
	if !req.PaneRef.IsValid() {
		ss.sendError(env.RequestID, "e_ref_invalid", "pane_ref is required", true, nil)
		return nil
	}
	ss.stateMu.Lock()
	ss.focusKey = req.PaneRef.CanonicalKey()
	ss.stateMu.Unlock()
	ss.ensureControlBridgeForFocus(req.PaneRef)
	ss.alignTmuxControlBridgeFocus(req.PaneRef)
	ss.ensurePaneTapForFocus(req.PaneRef)
	return ss.send("ack", env.RequestID, ttyv2.AckPayload{PaneRef: &req.PaneRef, AckKind: "focus", ResultCode: "ok"})
}

func (ss *ttyV2Session) handleDetach(env ttyv2.Envelope) error {
	var req ttyv2.DetachPayload
	if err := env.DecodePayload(&req); err != nil {
		ss.sendError(env.RequestID, "e_protocol_invalid_frame", "invalid detach payload", true, nil)
		return nil
	}
	if !req.PaneRef.IsValid() {
		ss.sendError(env.RequestID, "e_ref_invalid", "pane_ref is required", true, nil)
		return nil
	}
	key := req.PaneRef.CanonicalKey()
	removed := false
	ss.stateMu.Lock()
	if _, ok := ss.attached[key]; ok {
		delete(ss.attached, key)
		removed = true
	}
	if ss.focusKey == key {
		ss.focusKey = ""
	}
	focusKey := ss.focusKey
	paneTap := ss.paneTap
	ss.stateMu.Unlock()
	if paneTap != nil && paneTap.matches(req.PaneRef) {
		ss.stopPaneTap()
	}
	if focusKey == "" {
		ss.stopControlBridge()
		ss.stopPaneTap()
	}
	resultCode := "ok"
	if !removed {
		resultCode = "not_attached"
	}
	if err := ss.send("ack", env.RequestID, ttyv2.AckPayload{PaneRef: &req.PaneRef, AckKind: "detach", ResultCode: resultCode}); err != nil {
		return err
	}
	return ss.send("detached", "", map[string]any{
		"pane_ref": req.PaneRef,
		"reason":   "client_detach",
	})
}

func (ss *ttyV2Session) handleResync(env ttyv2.Envelope) error {
	var req ttyv2.ResyncPayload
	if err := env.DecodePayload(&req); err != nil {
		ss.sendError(env.RequestID, "e_protocol_invalid_frame", "invalid resync payload", true, nil)
		return nil
	}
	if !req.PaneRef.IsValid() {
		ss.sendError(env.RequestID, "e_ref_invalid", "pane_ref is required", true, nil)
		return nil
	}
	key := req.PaneRef.CanonicalKey()
	resyncReason := normalizeResyncReason(req.Reason)
	if resyncReason == ttyV2ResyncReasonUnknown {
		resyncReason = ttyV2ResyncReasonManual
	}
	ss.stateMu.RLock()
	attached := ss.attached[key]
	ss.stateMu.RUnlock()
	if attached == nil {
		return ss.send("ack", env.RequestID, ttyv2.AckPayload{PaneRef: &req.PaneRef, AckKind: "resync", ResultCode: "not_attached"})
	}
	_, pane, err := ss.resolveTargetAndPane(req.PaneRef)
	if err != nil {
		ss.sendError(env.RequestID, "e_ref_not_found", "target/pane not found", true, &req.PaneRef)
		return nil
	}

	ss.stateMu.Lock()
	attached.lastContent = ""
	attached.lastSource = "bridge"
	attached.outputSeq++
	now := time.Now().UTC()
	attached.lastCaptureAt = now
	attached.nextCaptureAt = time.Time{}
	attached.captureFailures = 0
	attached.lastErrorAt = time.Time{}
	attached.pendingRaw = ""
	attached.pendingSource = ""
	attached.pendingCursorX = nil
	attached.pendingCursorY = nil
	attached.pendingPaneCols = nil
	attached.pendingPaneRows = nil
	attached.pendingSeq = 0
	attached.pendingFrom = 0
	attached.pendingDrops = 0
	attached.forceResync = false
	attached.forceResyncReason = ""
	seq := attached.outputSeq
	alias := attached.alias
	cursorX := cloneIntPtr(attached.lastCursorX)
	cursorY := cloneIntPtr(attached.lastCursorY)
	paneCols := cloneIntPtr(attached.lastPaneCols)
	paneRows := cloneIntPtr(attached.lastPaneRows)
	ss.recordResyncAppliedLocked(resyncReason)
	focusKey := ss.focusKey
	ss.stateMu.Unlock()

	if focusKey == key {
		ss.ensureControlBridgeForFocus(req.PaneRef)
		ss.alignTmuxControlBridgeFocus(req.PaneRef)
	}

	if err := ss.send("ack", env.RequestID, ttyv2.AckPayload{PaneRef: &req.PaneRef, AckKind: "resync", ResultCode: "ok"}); err != nil {
		return err
	}
	return ss.send("attached", "", ttyv2.AttachedPayload{
		PaneRef:      req.PaneRef,
		PaneAlias:    alias,
		OutputSeq:    seq,
		SnapshotMode: "stream_resync",
		CursorX:      cursorX,
		CursorY:      cursorY,
		PaneCols:     paneCols,
		PaneRows:     paneRows,
		State: ttyv2.TTYState{
			ActivityState:       "idle",
			AttentionState:      "none",
			SessionLastActiveAt: pane.UpdatedAt.UTC().Format(time.RFC3339Nano),
		},
	})
}

func (ss *ttyV2Session) handlePing(env ttyv2.Envelope) error {
	var req ttyv2.PingPayload
	if err := env.DecodePayload(&req); err != nil {
		req.TS = time.Now().UTC().Format(time.RFC3339Nano)
	}
	return ss.send("pong", env.RequestID, ttyv2.PongPayload{TS: req.TS})
}

func (ss *ttyV2Session) triggerSIGWINCHOnAttach(
	tg model.Target,
	pref ttyv2.PaneRef,
	cols *int,
	rows *int,
) {
	if cols == nil || rows == nil {
		return
	}
	c := *cols
	r := *rows
	// keep the same bounds as resize validation
	if c < 20 || r < 5 || c > 500 || r > 300 {
		return
	}
	_, _ = ss.srv.executor.Run(
		context.Background(),
		tg,
		target.BuildTmuxCommand(
			"resize-pane",
			"-t",
			pref.PaneID,
			"-x",
			fmt.Sprintf("%d", c),
			"-y",
			fmt.Sprintf("%d", r),
		),
	)
}

func (ss *ttyV2Session) outputLoop() {
	ticker := time.NewTicker(ttyV2OutputPollInterval)
	defer ticker.Stop()
	for {
		select {
		case <-ss.done:
			return
		case <-ticker.C:
			ss.pushOutputs()
		}
	}
}

func (ss *ttyV2Session) pushOutputs() {
	now := time.Now().UTC()
	attached, focusKey := ss.snapshotAttached()
	if focusKey != "" {
		var focused *ttyV2AttachedPane
		for i := range attached {
			if attached[i].key == focusKey {
				focused = &attached[i]
				break
			}
		}
		if focused != nil {
			if err := ss.processPaneTapEvents(now, *focused); err != nil {
				ss.close()
				return
			}
			allowBridgeOutput := !ss.isPaneTapActiveFor(focused.ref)
			if err := ss.processControlBridgeEvents(now, *focused, allowBridgeOutput); err != nil {
				ss.close()
				return
			}
		}
	}
	resolvedMap, resolveErr := ss.resolveAttachedRefs(attached)
	for _, item := range attached {
		isForeground := focusKey != "" && focusKey == item.key
		if isForeground {
			if payload, ok := ss.flushPendingOutputIfReady(item.key, now, true); ok {
				ss.recordOutputSource(payload.Source)
				if err := ss.send("output", "", payload); err != nil {
					ss.close()
					return
				}
				continue
			}
		}
		if !isForeground {
			if payload, ok := ss.flushPendingOutputIfReady(item.key, now, false); ok {
				ss.recordOutputSource(payload.Source)
				if err := ss.send("output", "", payload); err != nil {
					ss.close()
					return
				}
				continue
			}
		}
		var (
			tg   model.Target
			pane model.Pane
			err  error
		)
		if resolveErr == nil {
			resolved := resolvedMap[item.key]
			if !resolved.found {
				err = errors.New("pane not found")
			} else {
				tg = resolved.target
				pane = resolved.pane
			}
		} else {
			tg, pane, err = ss.resolveTargetAndPane(item.ref)
		}
		if err != nil {
			pref := item.ref
			ss.sendError("", "e_ref_not_found", "attached pane no longer exists", true, &pref)
			ss.dropAttached(item.key)
			_ = ss.send("detached", "", map[string]any{"pane_ref": item.ref, "reason": "pane_killed"})
			continue
		}
		if !ss.shouldCaptureOutput(item, tg.Kind, isForeground, now) {
			continue
		}
		ss.recordHotpathCapture(isForeground)
		content, cursorX, cursorY, paneCols, paneRows, runErr := ss.srv.capturePaneSnapshotWithCursor(context.Background(), tg, item.ref.PaneID, defaultTerminalStreamLines)
		if runErr != nil {
			if !ss.recordCaptureFailure(item.key, now, tg.Kind, isForeground) {
				continue
			}
			pref := paneRefForError(pane, item.ref)
			ss.sendError("", "e_tmux_bridge_down", "failed to stream pane output", true, &pref)
			continue
		}
		content = trimSnapshotToVisibleRows(content, paneRows)
		resyncApplied, resyncReason := ss.clearForceResync(item.key)
		if resyncApplied {
			ss.recordResyncApplied(resyncReason)
		}
		clipped := clipTerminalStateContent(content)
		if payload, ok := ss.recordObservedOutput(
			item.key,
			clipped,
			content,
			"snapshot",
			cursorX,
			cursorY,
			paneCols,
			paneRows,
			now,
			isForeground,
		); ok {
			ss.recordOutputSource(payload.Source)
			if err := ss.send("output", "", payload); err != nil {
				ss.close()
				return
			}
		}
	}
}

func (ss *ttyV2Session) snapshotAttached() ([]ttyV2AttachedPane, string) {
	ss.stateMu.RLock()
	defer ss.stateMu.RUnlock()
	out := make([]ttyV2AttachedPane, 0, len(ss.attached))
	for _, item := range ss.attached {
		out = append(out, *item)
	}
	return out, ss.focusKey
}

func (ss *ttyV2Session) processPaneTapEvents(now time.Time, focused ttyV2AttachedPane) error {
	tap := ss.currentPaneTap()
	if tap == nil || !tap.matches(focused.ref) {
		return nil
	}
	const maxDrainPerTick = 256
	for i := 0; i < maxDrainPerTick; i++ {
		select {
		case err := <-tap.errs:
			ss.stopPaneTap()
			ss.sendError("", "e_tmux_pipe_down", fmt.Sprintf("pane tap stopped: %v", err), true, &focused.ref)
			return nil
		case event := <-tap.events:
			if event.PaneID != focused.ref.PaneID || len(event.Bytes) == 0 {
				continue
			}
			if payload, ok := ss.recordPaneTapOutput(focused.key, event.Bytes, now); ok {
				ss.recordOutputSource(payload.Source)
				if err := ss.send("output", "", payload); err != nil {
					return err
				}
			}
		default:
			return nil
		}
	}
	return nil
}

func (ss *ttyV2Session) processControlBridgeEvents(now time.Time, focused ttyV2AttachedPane, allowOutput bool) error {
	bridge := ss.currentControlBridge()
	if bridge == nil {
		return nil
	}
	const maxDrainPerTick = 256
	for i := 0; i < maxDrainPerTick; i++ {
		select {
		case err := <-bridge.errs:
			ss.stopControlBridge()
			return err
		case event := <-bridge.events:
			if event.Type == tmuxControlEventOutput || event.Type == tmuxControlEventExtendedOutput {
				if !allowOutput || event.PaneID != focused.ref.PaneID || len(event.Bytes) == 0 {
					continue
				}
				if payload, ok := ss.recordBridgeOutput(focused.key, string(event.Bytes), now); ok {
					ss.recordOutputSource(payload.Source)
					if err := ss.send("output", "", payload); err != nil {
						return err
					}
				}
				continue
			}
			if event.Type == tmuxControlEventLayoutChange {
				if event.WindowID != focused.ref.WindowID || !event.LayoutKnown {
					continue
				}
				ss.markResyncOnLayoutGeometryDiff(focused.key, focused.ref.WindowID, event.LayoutCols, event.LayoutRows)
			}
		default:
			return nil
		}
	}
	return nil
}

func (ss *ttyV2Session) recordBridgeOutput(key string, raw string, now time.Time) (ttyv2.OutputPayload, bool) {
	ss.stateMu.Lock()
	defer ss.stateMu.Unlock()
	item := ss.attached[key]
	if item == nil || raw == "" {
		return ttyv2.OutputPayload{}, false
	}
	item.outputSeq++
	item.lastSource = "bridge"
	item.lastBridgeAt = now
	item.lastCaptureAt = now
	item.captureFailures = 0
	item.nextCaptureAt = time.Time{}
	item.pendingRaw = ""
	item.pendingSource = ""
	item.pendingCursorX = nil
	item.pendingCursorY = nil
	item.pendingPaneCols = nil
	item.pendingPaneRows = nil
	item.pendingSeq = 0
	item.pendingFrom = 0
	item.pendingDrops = 0
	item.forceResync = false
	return ttyv2.OutputPayload{
		PaneAlias:   item.alias,
		PaneRef:     &item.ref,
		OutputSeq:   item.outputSeq,
		BytesBase64: base64.StdEncoding.EncodeToString([]byte(raw)),
		Source:      "bridge",
	}, true
}

func (ss *ttyV2Session) recordPaneTapOutput(key string, raw []byte, now time.Time) (ttyv2.OutputPayload, bool) {
	ss.stateMu.Lock()
	defer ss.stateMu.Unlock()
	item := ss.attached[key]
	if item == nil || len(raw) == 0 {
		return ttyv2.OutputPayload{}, false
	}
	item.outputSeq++
	item.lastSource = "pane_tap"
	item.lastCaptureAt = now
	item.captureFailures = 0
	item.nextCaptureAt = time.Time{}
	item.pendingRaw = ""
	item.pendingSource = ""
	item.pendingCursorX = nil
	item.pendingCursorY = nil
	item.pendingPaneCols = nil
	item.pendingPaneRows = nil
	item.pendingSeq = 0
	item.pendingFrom = 0
	item.pendingDrops = 0
	return ttyv2.OutputPayload{
		PaneAlias:   item.alias,
		PaneRef:     &item.ref,
		OutputSeq:   item.outputSeq,
		BytesBase64: base64.StdEncoding.EncodeToString(raw),
		Source:      "pane_tap",
	}, true
}

func (ss *ttyV2Session) markResyncOnLayoutGeometryDiff(key, windowID string, cols, rows int) {
	if cols <= 0 || rows <= 0 {
		return
	}
	ss.stateMu.Lock()
	defer ss.stateMu.Unlock()
	next := tmuxLayoutGeometry{Cols: cols, Rows: rows}
	prev := ss.layoutByWindow[windowID]
	if !shouldHandleLayoutGeometryChange(prev, next) {
		return
	}
	ss.layoutByWindow[windowID] = next
	if item := ss.attached[key]; item != nil {
		if paneSizeChanged(item.lastPaneCols, item.lastPaneRows, cols, rows) {
			item.forceResync = true
			item.forceResyncReason = ttyV2ResyncReasonLayoutChange
			ss.recordResyncQueuedLocked(ttyV2ResyncReasonLayoutChange)
		}
	}
}

func (ss *ttyV2Session) clearForceResync(key string) (bool, string) {
	ss.stateMu.Lock()
	defer ss.stateMu.Unlock()
	if item := ss.attached[key]; item != nil {
		wasForced := item.forceResync
		reason := normalizeResyncReason(item.forceResyncReason)
		item.forceResync = false
		item.forceResyncReason = ""
		return wasForced, reason
	}
	return false, ""
}

func (ss *ttyV2Session) recordHotpathCapture(selected bool) {
	ss.stateMu.Lock()
	defer ss.stateMu.Unlock()
	if selected {
		ss.telemetry.HotpathCaptureSelected++
		return
	}
	ss.telemetry.HotpathCaptureNonSelected++
}

func (ss *ttyV2Session) recordOutputSource(source string) {
	ss.stateMu.Lock()
	defer ss.stateMu.Unlock()
	switch strings.TrimSpace(strings.ToLower(source)) {
	case "bridge":
		ss.telemetry.OutputBridge++
	case "pane_tap":
		ss.telemetry.OutputPaneTap++
	default:
		ss.telemetry.OutputSnapshot++
	}
}

func (ss *ttyV2Session) recordResyncQueued(reason string) {
	ss.stateMu.Lock()
	defer ss.stateMu.Unlock()
	ss.recordResyncQueuedLocked(reason)
}

func (ss *ttyV2Session) recordResyncQueuedLocked(reason string) {
	reason = normalizeResyncReason(reason)
	if ss.telemetry.ResyncQueuedByReason == nil {
		ss.telemetry.ResyncQueuedByReason = map[string]int{}
	}
	ss.telemetry.ResyncQueuedByReason[reason]++
}

func (ss *ttyV2Session) recordResyncApplied(reason string) {
	ss.stateMu.Lock()
	defer ss.stateMu.Unlock()
	ss.recordResyncAppliedLocked(reason)
}

func (ss *ttyV2Session) recordResyncAppliedLocked(reason string) {
	reason = normalizeResyncReason(reason)
	if ss.telemetry.ResyncAppliedByReason == nil {
		ss.telemetry.ResyncAppliedByReason = map[string]int{}
	}
	ss.telemetry.ResyncAppliedByReason[reason]++
}

func (ss *ttyV2Session) telemetrySnapshot() ttyV2SessionTelemetrySnapshot {
	ss.stateMu.RLock()
	defer ss.stateMu.RUnlock()
	return ttyV2SessionTelemetrySnapshot{
		HotpathCaptureSelected:    ss.telemetry.HotpathCaptureSelected,
		HotpathCaptureNonSelected: ss.telemetry.HotpathCaptureNonSelected,
		OutputBridge:              ss.telemetry.OutputBridge,
		OutputPaneTap:             ss.telemetry.OutputPaneTap,
		OutputSnapshot:            ss.telemetry.OutputSnapshot,
		ResyncQueuedByReason:      cloneStringIntMap(ss.telemetry.ResyncQueuedByReason),
		ResyncAppliedByReason:     cloneStringIntMap(ss.telemetry.ResyncAppliedByReason),
	}
}

func cloneStringIntMap(in map[string]int) map[string]int {
	if len(in) == 0 {
		return map[string]int{}
	}
	out := make(map[string]int, len(in))
	for key, value := range in {
		out[key] = value
	}
	return out
}

func normalizeResyncReason(reason string) string {
	token := strings.TrimSpace(strings.ToLower(reason))
	if token == "" {
		return ttyV2ResyncReasonUnknown
	}
	return token
}

func (ss *ttyV2Session) shouldCaptureOutput(item ttyV2AttachedPane, targetKind model.TargetKind, isForeground bool, now time.Time) bool {
	if !item.nextCaptureAt.IsZero() && now.Before(item.nextCaptureAt) {
		return false
	}
	if isForeground {
		// stream-only selected pane path:
		// never mix capture-pane snapshots with live bridge bytes.
		return false
	}
	if item.lastCaptureAt.IsZero() {
		return true
	}
	requiredInterval := ttyV2BackgroundCaptureInterval
	if targetKind == model.TargetKindSSH {
		requiredInterval = ttyV2SSHBackgroundCaptureInterval
	}
	return now.Sub(item.lastCaptureAt) >= requiredInterval
}

func (ss *ttyV2Session) flushPendingOutputIfReady(key string, now time.Time, force bool) (ttyv2.OutputPayload, bool) {
	ss.stateMu.Lock()
	defer ss.stateMu.Unlock()
	item := ss.attached[key]
	if item == nil {
		return ttyv2.OutputPayload{}, false
	}
	if item.pendingSeq == 0 || item.pendingRaw == "" {
		return ttyv2.OutputPayload{}, false
	}
	if !force && !item.lastOutputAt.IsZero() && now.Sub(item.lastOutputAt) < ttyV2BackgroundDispatchInterval {
		return ttyv2.OutputPayload{}, false
	}
	coalesced := item.pendingDrops > 0 || (item.pendingFrom > 0 && item.pendingFrom < item.pendingSeq)
	coalescedFrom := item.pendingFrom
	if coalescedFrom == 0 {
		coalescedFrom = item.pendingSeq
	}
	payload := ttyv2.OutputPayload{
		PaneAlias:   item.alias,
		PaneRef:     &item.ref,
		OutputSeq:   item.pendingSeq,
		BytesBase64: base64.StdEncoding.EncodeToString([]byte(item.pendingRaw)),
		Source:      item.pendingSource,
		CursorX:     cloneIntPtr(item.pendingCursorX),
		CursorY:     cloneIntPtr(item.pendingCursorY),
		PaneCols:    cloneIntPtr(item.pendingPaneCols),
		PaneRows:    cloneIntPtr(item.pendingPaneRows),
		Coalesced:   coalesced,
	}
	if coalesced {
		payload.CoalescedFromSeq = coalescedFrom
		payload.DroppedChunks = item.pendingDrops
	}
	item.lastOutputAt = now
	item.pendingRaw = ""
	item.pendingSource = ""
	item.pendingCursorX = nil
	item.pendingCursorY = nil
	item.pendingPaneCols = nil
	item.pendingPaneRows = nil
	item.pendingSeq = 0
	item.pendingFrom = 0
	item.pendingDrops = 0
	return payload, true
}

func (ss *ttyV2Session) recordObservedOutput(
	key string,
	clipped string,
	raw string,
	source string,
	cursorX *int,
	cursorY *int,
	paneCols *int,
	paneRows *int,
	now time.Time,
	isForeground bool,
) (ttyv2.OutputPayload, bool) {
	ss.stateMu.Lock()
	defer ss.stateMu.Unlock()
	item := ss.attached[key]
	if item == nil {
		return ttyv2.OutputPayload{}, false
	}
	item.lastCaptureAt = now
	item.captureFailures = 0
	item.nextCaptureAt = time.Time{}
	if clipped == item.lastContent {
		return ttyv2.OutputPayload{}, false
	}
	item.outputSeq++
	seq := item.outputSeq
	item.lastContent = clipped
	item.lastSource = strings.TrimSpace(source)
	item.lastCursorX = cloneIntPtr(cursorX)
	item.lastCursorY = cloneIntPtr(cursorY)
	item.lastPaneCols = cloneIntPtr(paneCols)
	item.lastPaneRows = cloneIntPtr(paneRows)

	shouldSendImmediately := isForeground || item.lastOutputAt.IsZero() || now.Sub(item.lastOutputAt) >= ttyV2BackgroundDispatchInterval
	if !shouldSendImmediately {
		if item.pendingFrom == 0 {
			item.pendingFrom = seq
			item.pendingDrops = 0
		} else {
			item.pendingDrops++
		}
		item.pendingSeq = seq
		item.pendingRaw = raw
		item.pendingSource = item.lastSource
		item.pendingCursorX = cloneIntPtr(item.lastCursorX)
		item.pendingCursorY = cloneIntPtr(item.lastCursorY)
		item.pendingPaneCols = cloneIntPtr(item.lastPaneCols)
		item.pendingPaneRows = cloneIntPtr(item.lastPaneRows)
		if item.pendingDrops < ttyV2PendingDropWatermark {
			return ttyv2.OutputPayload{}, false
		}
	}

	coalesced := item.pendingDrops > 0 || item.pendingFrom > 0
	coalescedFrom := item.pendingFrom
	if coalescedFrom == 0 {
		coalescedFrom = seq
	}
	payload := ttyv2.OutputPayload{
		PaneAlias:   item.alias,
		PaneRef:     &item.ref,
		OutputSeq:   seq,
		BytesBase64: base64.StdEncoding.EncodeToString([]byte(raw)),
		Source:      item.lastSource,
		CursorX:     cloneIntPtr(item.lastCursorX),
		CursorY:     cloneIntPtr(item.lastCursorY),
		PaneCols:    cloneIntPtr(item.lastPaneCols),
		PaneRows:    cloneIntPtr(item.lastPaneRows),
		Coalesced:   coalesced,
	}
	if coalesced {
		payload.CoalescedFromSeq = coalescedFrom
		payload.DroppedChunks = item.pendingDrops
	}
	item.lastOutputAt = now
	item.pendingRaw = ""
	item.pendingSource = ""
	item.pendingCursorX = nil
	item.pendingCursorY = nil
	item.pendingPaneCols = nil
	item.pendingPaneRows = nil
	item.pendingSeq = 0
	item.pendingFrom = 0
	item.pendingDrops = 0
	return payload, true
}

func cloneIntPtr(v *int) *int {
	if v == nil {
		return nil
	}
	return intPtr(*v)
}

func (ss *ttyV2Session) recordCaptureFailure(key string, now time.Time, targetKind model.TargetKind, isForeground bool) bool {
	ss.stateMu.Lock()
	defer ss.stateMu.Unlock()
	item := ss.attached[key]
	if item == nil {
		return false
	}
	item.captureFailures++
	item.nextCaptureAt = now.Add(captureBackoffDuration(item.captureFailures, targetKind, isForeground))
	throttle := ttyV2CaptureErrorThrottleBackground
	if isForeground {
		throttle = ttyV2CaptureErrorThrottleForeground
	}
	if item.lastErrorAt.IsZero() || now.Sub(item.lastErrorAt) >= throttle {
		item.lastErrorAt = now
		return true
	}
	return false
}

func (ss *ttyV2Session) dropAttached(key string) {
	ss.stateMu.Lock()
	var ref ttyv2.PaneRef
	if item := ss.attached[key]; item != nil {
		ref = item.ref
	}
	delete(ss.attached, key)
	if ss.focusKey == key {
		ss.focusKey = ""
	}
	focusKey := ss.focusKey
	paneTap := ss.paneTap
	ss.stateMu.Unlock()
	if paneTap != nil && paneTap.matches(ref) {
		ss.stopPaneTap()
	}
	if focusKey == "" {
		ss.stopControlBridge()
		ss.stopPaneTap()
	}
}

func captureBackoffDuration(failures int, targetKind model.TargetKind, isForeground bool) time.Duration {
	base := ttyV2CaptureBackoffLocalBackgroundBase
	maxDelay := ttyV2CaptureBackoffLocalMax
	if isForeground {
		base = ttyV2CaptureBackoffLocalForegroundBase
	}
	if targetKind == model.TargetKindSSH {
		base = ttyV2CaptureBackoffSSHBackgroundBase
		maxDelay = ttyV2CaptureBackoffSSHMax
		if isForeground {
			base = ttyV2CaptureBackoffSSHForegroundBase
		}
	}
	if failures <= 1 {
		return base
	}
	delay := base
	for i := 1; i < failures && delay < maxDelay; i++ {
		delay *= 2
		if delay > maxDelay {
			delay = maxDelay
			break
		}
	}
	return delay
}

func paneRefForError(pane model.Pane, fallback ttyv2.PaneRef) ttyv2.PaneRef {
	if pane.TargetID == "" || pane.PaneID == "" || pane.SessionName == "" || pane.WindowID == "" {
		return fallback
	}
	return ttyv2.PaneRef{
		Target:      fallback.Target,
		SessionName: pane.SessionName,
		WindowID:    pane.WindowID,
		PaneID:      pane.PaneID,
	}
}

func (ss *ttyV2Session) resolveAttachedRefs(attached []ttyV2AttachedPane) (map[string]ttyV2ResolvedPane, error) {
	out := make(map[string]ttyV2ResolvedPane, len(attached))
	if len(attached) == 0 {
		return out, nil
	}
	targets, err := ss.srv.store.ListTargets(context.Background())
	if err != nil {
		return nil, err
	}
	targetByName := make(map[string]model.Target, len(targets))
	for _, tg := range targets {
		targetByName[tg.TargetName] = tg
	}
	panes, err := ss.srv.store.ListPanes(context.Background())
	if err != nil {
		return nil, err
	}
	paneByRef := make(map[string]model.Pane, len(panes))
	for _, pane := range panes {
		key := pane.TargetID + "\x1f" + pane.SessionName + "\x1f" + pane.WindowID + "\x1f" + pane.PaneID
		paneByRef[key] = pane
	}
	for _, item := range attached {
		tg, ok := targetByName[item.ref.Target]
		if !ok {
			out[item.key] = ttyV2ResolvedPane{found: false}
			continue
		}
		paneKey := tg.TargetID + "\x1f" + item.ref.SessionName + "\x1f" + item.ref.WindowID + "\x1f" + item.ref.PaneID
		pane, ok := paneByRef[paneKey]
		if !ok {
			out[item.key] = ttyV2ResolvedPane{found: false}
			continue
		}
		out[item.key] = ttyV2ResolvedPane{target: tg, pane: pane, found: true}
	}
	return out, nil
}

func (ss *ttyV2Session) resolveTargetAndPane(pref ttyv2.PaneRef) (model.Target, model.Pane, error) {
	tg, err := ss.srv.store.GetTargetByName(context.Background(), pref.Target)
	if err != nil {
		return model.Target{}, model.Pane{}, err
	}
	panes, err := ss.srv.store.ListPanes(context.Background())
	if err != nil {
		return model.Target{}, model.Pane{}, err
	}
	for _, pane := range panes {
		if pane.TargetID != tg.TargetID {
			continue
		}
		if pane.PaneID != pref.PaneID {
			continue
		}
		if pane.SessionName != pref.SessionName {
			continue
		}
		if pane.WindowID != pref.WindowID {
			continue
		}
		return tg, pane, nil
	}
	return model.Target{}, model.Pane{}, errors.New("pane not found")
}

func (ss *ttyV2Session) currentControlBridge() *tmuxControlBridgeHandle {
	ss.stateMu.RLock()
	defer ss.stateMu.RUnlock()
	return ss.bridge
}

func (ss *ttyV2Session) currentPaneTap() *paneTapHandle {
	ss.stateMu.RLock()
	defer ss.stateMu.RUnlock()
	return ss.paneTap
}

func (ss *ttyV2Session) isControlBridgeActiveFor(targetName, sessionName string) bool {
	ss.stateMu.RLock()
	defer ss.stateMu.RUnlock()
	if ss.bridge == nil {
		return false
	}
	return ss.bridge.targetName == strings.TrimSpace(targetName) &&
		ss.bridge.sessionName == strings.TrimSpace(sessionName)
}

func (ss *ttyV2Session) isPaneTapActiveFor(pref ttyv2.PaneRef) bool {
	ss.stateMu.RLock()
	defer ss.stateMu.RUnlock()
	return ss.paneTap != nil && ss.paneTap.matches(pref)
}

func (ss *ttyV2Session) stopControlBridge() {
	ss.closeMu.Lock()
	defer ss.closeMu.Unlock()
	ss.stopControlBridgeLocked()
}

func (ss *ttyV2Session) stopPaneTap() {
	ss.closeMu.Lock()
	defer ss.closeMu.Unlock()
	ss.stopPaneTapLocked()
}

func (ss *ttyV2Session) stopControlBridgeLocked() {
	ss.stateMu.Lock()
	bridge := ss.bridge
	ss.bridge = nil
	ss.layoutByWindow = map[string]tmuxLayoutGeometry{}
	ss.stateMu.Unlock()
	if bridge != nil && bridge.stdin != nil {
		_ = bridge.stdin.Close()
	}
	if bridge != nil && bridge.cancel != nil {
		bridge.cancel()
	}
}

func (ss *ttyV2Session) stopPaneTapLocked() {
	ss.stateMu.Lock()
	tap := ss.paneTap
	ss.paneTap = nil
	ss.stateMu.Unlock()
	ss.stopPaneTapHandle(tap)
}

func (ss *ttyV2Session) stopPaneTapHandle(tap *paneTapHandle) {
	if tap == nil {
		return
	}
	if ss.srv == nil || ss.srv.store == nil || ss.srv.executor == nil {
		tap.cancel()
		return
	}
	tg, err := ss.srv.store.GetTargetByName(context.Background(), tap.targetName)
	if err != nil {
		tap.cancel()
		return
	}
	tap.stop(ss.srv.executor, tg)
}

func (ss *ttyV2Session) ensureControlBridgeForFocus(pref ttyv2.PaneRef) {
	tg, _, err := ss.resolveTargetAndPane(pref)
	if err != nil || tg.Kind != model.TargetKindLocal {
		ss.stopControlBridge()
		return
	}
	targetName := strings.TrimSpace(pref.Target)
	sessionName := strings.TrimSpace(pref.SessionName)

	ss.stateMu.RLock()
	existing := ss.bridge
	ss.stateMu.RUnlock()
	if existing != nil && existing.targetName == targetName && existing.sessionName == sessionName {
		return
	}
	handle, startErr := startTmuxControlBridge(targetName, sessionName)
	if startErr != nil {
		ss.stopControlBridge()
		ss.sendError(
			"",
			"e_tmux_bridge_down",
			fmt.Sprintf("failed to start tmux control bridge for session %q: %v", sessionName, startErr),
			true,
			&pref,
		)
		return
	}
	ss.stateMu.Lock()
	prev := ss.bridge
	ss.bridge = handle
	ss.layoutByWindow = map[string]tmuxLayoutGeometry{}
	ss.stateMu.Unlock()
	if prev != nil && prev.cancel != nil {
		prev.cancel()
	}
}

func (ss *ttyV2Session) ensurePaneTapForFocus(pref ttyv2.PaneRef) {
	if !ss.srv.cfg.EnableTTYV2PaneTap {
		return
	}
	tg, _, err := ss.resolveTargetAndPane(pref)
	if err != nil || tg.Kind != model.TargetKindLocal {
		ss.stopPaneTap()
		return
	}

	ss.stateMu.RLock()
	existing := ss.paneTap
	ss.stateMu.RUnlock()
	if existing != nil && existing.matches(pref) {
		return
	}
	handle, startErr := startPaneTapForPane(tg, pref, ss.srv.executor)
	if startErr != nil {
		ss.stopPaneTap()
		ss.sendError(
			"",
			"e_tmux_pipe_down",
			fmt.Sprintf("failed to start pane tap for pane %q: %v", pref.PaneID, startErr),
			true,
			&pref,
		)
		return
	}
	ss.stateMu.Lock()
	prev := ss.paneTap
	ss.paneTap = handle
	ss.stateMu.Unlock()
	if prev != nil {
		ss.stopPaneTapHandle(prev)
	}
}

func (ss *ttyV2Session) alignTmuxControlBridgeFocus(pref ttyv2.PaneRef) {
	tg, _, err := ss.resolveTargetAndPane(pref)
	if err != nil {
		return
	}
	targetName := strings.TrimSpace(pref.Target)
	sessionName := strings.TrimSpace(pref.SessionName)
	if ss.isControlBridgeActiveFor(targetName, sessionName) {
		if ss.sendControlBridgeCommands(
			fmt.Sprintf("select-window -t %s", pref.WindowID),
			fmt.Sprintf("select-pane -t %s", pref.PaneID),
		) {
			return
		}
	}
	// Best-effort alignment: keep tmux control bridge focused on the selected pane
	// so %output events correspond to the pane the app is displaying.
	_, _ = ss.srv.executor.Run(
		context.Background(),
		tg,
		target.BuildTmuxCommand("select-window", "-t", pref.WindowID),
	)
	_, _ = ss.srv.executor.Run(
		context.Background(),
		tg,
		target.BuildTmuxCommand("select-pane", "-t", pref.PaneID),
	)
}

func startTmuxControlBridge(targetName, sessionName string) (*tmuxControlBridgeHandle, error) {
	ctx, cancel := context.WithCancel(context.Background())
	cmd := exec.CommandContext(ctx, "tmux", "-C", "attach-session", "-t", sessionName)
	cmd.Env = filteredExecEnv(os.Environ(), "TMUX", "TMUX_PANE")
	stdin, err := cmd.StdinPipe()
	if err != nil {
		cancel()
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		cancel()
		return nil, err
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		cancel()
		return nil, err
	}
	if err := cmd.Start(); err != nil {
		cancel()
		return nil, err
	}
	handle := &tmuxControlBridgeHandle{
		targetName:  targetName,
		sessionName: sessionName,
		ctx:         ctx,
		cancel:      cancel,
		cmd:         cmd,
		stdin:       stdin,
		events:      make(chan tmuxControlEvent, ttyV2BridgeEventBuffer),
		errs:        make(chan error, 1),
	}
	go readTmuxControlBridgeStream(ctx, stdout, handle.events)
	go readTmuxControlBridgeStream(ctx, stderr, handle.events)
	go func() {
		waitErr := cmd.Wait()
		if waitErr != nil && !errors.Is(waitErr, context.Canceled) {
			select {
			case handle.errs <- waitErr:
			default:
			}
		}
	}()
	return handle, nil
}

func (ss *ttyV2Session) sendControlBridgeCommands(commands ...string) bool {
	ss.stateMu.RLock()
	bridge := ss.bridge
	ss.stateMu.RUnlock()
	if bridge == nil {
		return false
	}
	if err := bridge.sendCommands(commands...); err != nil {
		ss.stopControlBridge()
		return false
	}
	return true
}

func filteredExecEnv(base []string, removeKeys ...string) []string {
	if len(base) == 0 {
		return []string{}
	}
	if len(removeKeys) == 0 {
		out := make([]string, len(base))
		copy(out, base)
		return out
	}
	removeSet := make(map[string]struct{}, len(removeKeys))
	for _, key := range removeKeys {
		trimmed := strings.TrimSpace(key)
		if trimmed == "" {
			continue
		}
		removeSet[trimmed] = struct{}{}
	}
	if len(removeSet) == 0 {
		out := make([]string, len(base))
		copy(out, base)
		return out
	}
	out := make([]string, 0, len(base))
	for _, entry := range base {
		if entry == "" {
			continue
		}
		key := entry
		if idx := strings.IndexByte(entry, '='); idx >= 0 {
			key = entry[:idx]
		}
		if _, drop := removeSet[key]; drop {
			continue
		}
		out = append(out, entry)
	}
	return out
}

func readTmuxControlBridgeStream(ctx context.Context, r io.Reader, events chan<- tmuxControlEvent) {
	reader := bufio.NewReader(r)
	for {
		select {
		case <-ctx.Done():
			return
		default:
		}
		line, err := reader.ReadString('\n')
		if line != "" {
			if event, ok := parseTmuxControlEventLine(line); ok {
				select {
				case events <- event:
				default:
				}
			}
		}
		if err != nil {
			return
		}
	}
}
