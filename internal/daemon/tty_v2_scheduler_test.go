package daemon

import (
	"encoding/base64"
	"strconv"
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/ttyv2"
)

func TestTTYV2RecordObservedOutputForegroundSendsImmediately(t *testing.T) {
	ref := ttyv2.PaneRef{Target: "local", SessionName: "s", WindowID: "@1", PaneID: "%1"}
	key := ref.CanonicalKey()
	ss := &ttyV2Session{
		attached: map[string]*ttyV2AttachedPane{
			key: {
				key:         key,
				alias:       "p1",
				ref:         ref,
				lastContent: "old",
			},
		},
	}
	now := time.Now().UTC()
	cursorX := 5
	cursorY := 9
	paneCols := 120
	paneRows := 32

	payload, ok := ss.recordObservedOutput(
		key,
		"new",
		"new",
		"snapshot",
		&cursorX,
		&cursorY,
		&paneCols,
		&paneRows,
		now,
		true,
	)
	if !ok {
		t.Fatalf("expected foreground output payload")
	}
	if payload.OutputSeq != 1 {
		t.Fatalf("expected output_seq=1, got %d", payload.OutputSeq)
	}
	if payload.Coalesced {
		t.Fatalf("expected non-coalesced foreground payload")
	}
	if payload.Source != "snapshot" {
		t.Fatalf("expected source=snapshot, got %q", payload.Source)
	}
	if payload.CursorX == nil || *payload.CursorX != cursorX {
		t.Fatalf("expected cursor_x=%d, got %+v", cursorX, payload.CursorX)
	}
	if payload.CursorY == nil || *payload.CursorY != cursorY {
		t.Fatalf("expected cursor_y=%d, got %+v", cursorY, payload.CursorY)
	}
	if payload.PaneCols == nil || *payload.PaneCols != paneCols {
		t.Fatalf("expected pane_cols=%d, got %+v", paneCols, payload.PaneCols)
	}
	if payload.PaneRows == nil || *payload.PaneRows != paneRows {
		t.Fatalf("expected pane_rows=%d, got %+v", paneRows, payload.PaneRows)
	}
	decoded, err := base64.StdEncoding.DecodeString(payload.BytesBase64)
	if err != nil {
		t.Fatalf("decode bytes_base64: %v", err)
	}
	if string(decoded) != "new" {
		t.Fatalf("unexpected output payload: %q", string(decoded))
	}
}

func TestTTYV2BackgroundCoalescingFlushesLatestWins(t *testing.T) {
	ref := ttyv2.PaneRef{Target: "local", SessionName: "s", WindowID: "@1", PaneID: "%2"}
	key := ref.CanonicalKey()
	start := time.Now().UTC()
	cursorX1 := 1
	cursorY1 := 2
	cursorX2 := 7
	cursorY2 := 8
	paneCols := 100
	paneRows := 40
	ss := &ttyV2Session{
		attached: map[string]*ttyV2AttachedPane{
			key: {
				key:          key,
				alias:        "p2",
				ref:          ref,
				lastContent:  "a",
				lastOutputAt: start,
			},
		},
	}

	if _, ok := ss.recordObservedOutput(
		key,
		"b",
		"b",
		"snapshot",
		&cursorX1,
		&cursorY1,
		&paneCols,
		&paneRows,
		start.Add(10*time.Millisecond),
		false,
	); ok {
		t.Fatalf("expected first background update to queue (not send)")
	}
	if _, ok := ss.recordObservedOutput(
		key,
		"c",
		"c",
		"snapshot",
		&cursorX2,
		&cursorY2,
		&paneCols,
		&paneRows,
		start.Add(20*time.Millisecond),
		false,
	); ok {
		t.Fatalf("expected second background update to queue (not send)")
	}

	payload, ok := ss.flushPendingOutputIfReady(key, start.Add(500*time.Millisecond), false)
	if !ok {
		t.Fatalf("expected pending background payload to flush")
	}
	if payload.OutputSeq != 2 {
		t.Fatalf("expected output_seq=2, got %d", payload.OutputSeq)
	}
	if !payload.Coalesced {
		t.Fatalf("expected coalesced payload")
	}
	if payload.CoalescedFromSeq != 1 {
		t.Fatalf("expected coalesced_from_seq=1, got %d", payload.CoalescedFromSeq)
	}
	if payload.DroppedChunks != 1 {
		t.Fatalf("expected dropped_chunks=1, got %d", payload.DroppedChunks)
	}
	if payload.Source != "snapshot" {
		t.Fatalf("expected source=snapshot, got %q", payload.Source)
	}
	if payload.CursorX == nil || *payload.CursorX != cursorX2 {
		t.Fatalf("expected latest cursor_x=%d, got %+v", cursorX2, payload.CursorX)
	}
	if payload.CursorY == nil || *payload.CursorY != cursorY2 {
		t.Fatalf("expected latest cursor_y=%d, got %+v", cursorY2, payload.CursorY)
	}
	decoded, err := base64.StdEncoding.DecodeString(payload.BytesBase64)
	if err != nil {
		t.Fatalf("decode bytes_base64: %v", err)
	}
	if string(decoded) != "c" {
		t.Fatalf("expected latest payload content 'c', got %q", string(decoded))
	}
}

func TestTTYV2BackgroundWatermarkForcesImmediateFlush(t *testing.T) {
	ref := ttyv2.PaneRef{Target: "local", SessionName: "s", WindowID: "@1", PaneID: "%3"}
	key := ref.CanonicalKey()
	start := time.Now().UTC()
	ss := &ttyV2Session{
		attached: map[string]*ttyV2AttachedPane{
			key: {
				key:          key,
				alias:        "p3",
				ref:          ref,
				lastContent:  "seed",
				lastOutputAt: start,
			},
		},
	}

	var final ttyv2.OutputPayload
	var sent bool
	for i := 1; i <= ttyV2PendingDropWatermark+1; i++ {
		next := start.Add(time.Duration(i*10) * time.Millisecond)
		payload, ok := ss.recordObservedOutput(key, "v"+strconv.Itoa(i), "v"+strconv.Itoa(i), "snapshot", nil, nil, nil, nil, next, false)
		if i <= ttyV2PendingDropWatermark {
			if ok {
				t.Fatalf("did not expect send before watermark at i=%d", i)
			}
			continue
		}
		if !ok {
			t.Fatalf("expected send at watermark crossing")
		}
		final = payload
		sent = true
	}
	if !sent {
		t.Fatalf("expected watermark-triggered payload")
	}
	if !final.Coalesced {
		t.Fatalf("expected watermark payload to be coalesced")
	}
	if final.DroppedChunks != ttyV2PendingDropWatermark {
		t.Fatalf("unexpected dropped_chunks: %d", final.DroppedChunks)
	}
	decoded, err := base64.StdEncoding.DecodeString(final.BytesBase64)
	if err != nil {
		t.Fatalf("decode bytes_base64: %v", err)
	}
	if string(decoded) != "v5" {
		t.Fatalf("expected latest payload content v5, got %q", string(decoded))
	}
}

func TestTTYV2ShouldCaptureOutputForegroundVsBackground(t *testing.T) {
	now := time.Now().UTC()
	item := ttyV2AttachedPane{lastCaptureAt: now}
	ss := &ttyV2Session{}

	if ss.shouldCaptureOutput(item, model.TargetKindLocal, true, now.Add(10*time.Millisecond)) {
		t.Fatalf("foreground should stay stream-only and skip capture")
	}
	if ss.shouldCaptureOutput(item, model.TargetKindLocal, false, now.Add(100*time.Millisecond)) {
		t.Fatalf("background should not capture before interval")
	}
	if !ss.shouldCaptureOutput(item, model.TargetKindLocal, false, now.Add(ttyV2BackgroundCaptureInterval+5*time.Millisecond)) {
		t.Fatalf("background should capture after interval")
	}
}

func TestTTYV2ShouldCaptureOutputSSHUsesLongerBackgroundInterval(t *testing.T) {
	now := time.Now().UTC()
	item := ttyV2AttachedPane{lastCaptureAt: now}
	ss := &ttyV2Session{}

	if ss.shouldCaptureOutput(item, model.TargetKindSSH, false, now.Add(ttyV2BackgroundCaptureInterval+10*time.Millisecond)) {
		t.Fatalf("ssh background should wait longer than local interval")
	}
	if !ss.shouldCaptureOutput(item, model.TargetKindSSH, false, now.Add(ttyV2SSHBackgroundCaptureInterval+10*time.Millisecond)) {
		t.Fatalf("ssh background should capture after ssh interval")
	}
}

func TestTTYV2ShouldCaptureOutputRespectsBackoff(t *testing.T) {
	now := time.Now().UTC()
	item := ttyV2AttachedPane{
		lastCaptureAt: now,
		nextCaptureAt: now.Add(500 * time.Millisecond),
	}
	ss := &ttyV2Session{}

	if ss.shouldCaptureOutput(item, model.TargetKindLocal, true, now.Add(100*time.Millisecond)) {
		t.Fatalf("foreground should never capture in stream-only mode")
	}
	if ss.shouldCaptureOutput(item, model.TargetKindLocal, true, now.Add(600*time.Millisecond)) {
		t.Fatalf("foreground should never capture after backoff either")
	}
}

func TestTTYV2ShouldCaptureOutputForegroundSkipsWhenBridgeActive(t *testing.T) {
	ref := ttyv2.PaneRef{Target: "local", SessionName: "s", WindowID: "@1", PaneID: "%31"}
	key := ref.CanonicalKey()
	ss := &ttyV2Session{
		attached: map[string]*ttyV2AttachedPane{
			key: {key: key, ref: ref},
		},
		bridge: &tmuxControlBridgeHandle{
			targetName:  "local",
			sessionName: "s",
		},
	}
	now := time.Now().UTC()
	if ss.shouldCaptureOutput(*ss.attached[key], model.TargetKindLocal, true, now) {
		t.Fatalf("expected foreground to skip snapshot capture")
	}
	ss.attached[key].lastBridgeAt = now
	if ss.shouldCaptureOutput(*ss.attached[key], model.TargetKindLocal, true, now.Add(50*time.Millisecond)) {
		t.Fatalf("expected foreground to keep skipping snapshot capture")
	}
	if ss.shouldCaptureOutput(*ss.attached[key], model.TargetKindLocal, true, now.Add(5*time.Second)) {
		t.Fatalf("expected foreground to keep skipping snapshot capture even when idle")
	}
}

func TestTTYV2MarkResyncOnLayoutGeometryDiff(t *testing.T) {
	ref := ttyv2.PaneRef{Target: "local", SessionName: "s", WindowID: "@2", PaneID: "%44"}
	key := ref.CanonicalKey()
	cols := 120
	rows := 40
	ss := &ttyV2Session{
		attached: map[string]*ttyV2AttachedPane{
			key: {
				key:          key,
				ref:          ref,
				lastPaneCols: &cols,
				lastPaneRows: &rows,
			},
		},
		layoutByWindow: map[string]tmuxLayoutGeometry{},
	}

	ss.markResyncOnLayoutGeometryDiff(key, "@2", 120, 40)
	if ss.attached[key].forceResync {
		t.Fatalf("expected same geometry not to trigger resync")
	}
	ss.markResyncOnLayoutGeometryDiff(key, "@2", 121, 40)
	if !ss.attached[key].forceResync {
		t.Fatalf("expected changed geometry to trigger resync")
	}
}

func TestTTYV2RecordCaptureFailureBackoffAndThrottle(t *testing.T) {
	ref := ttyv2.PaneRef{Target: "local", SessionName: "s", WindowID: "@1", PaneID: "%9"}
	key := ref.CanonicalKey()
	ss := &ttyV2Session{
		attached: map[string]*ttyV2AttachedPane{
			key: {
				key:   key,
				alias: "p9",
				ref:   ref,
			},
		},
	}
	now := time.Now().UTC()

	if !ss.recordCaptureFailure(key, now, model.TargetKindLocal, false) {
		t.Fatalf("expected first capture failure to emit error")
	}
	item := ss.attached[key]
	if item.captureFailures != 1 {
		t.Fatalf("expected failure count=1, got %d", item.captureFailures)
	}
	if item.nextCaptureAt.IsZero() || !item.nextCaptureAt.After(now) {
		t.Fatalf("expected nextCaptureAt after now")
	}

	if ss.recordCaptureFailure(key, now.Add(200*time.Millisecond), model.TargetKindLocal, false) {
		t.Fatalf("expected throttled error emission within background throttle window")
	}
}

func TestTTYV2CaptureSuccessClearsFailureBackoff(t *testing.T) {
	ref := ttyv2.PaneRef{Target: "local", SessionName: "s", WindowID: "@1", PaneID: "%11"}
	key := ref.CanonicalKey()
	now := time.Now().UTC()
	ss := &ttyV2Session{
		attached: map[string]*ttyV2AttachedPane{
			key: {
				key:             key,
				alias:           "p11",
				ref:             ref,
				lastContent:     "old",
				captureFailures: 2,
				nextCaptureAt:   now.Add(5 * time.Second),
			},
		},
	}

	if _, ok := ss.recordObservedOutput(key, "new", "new", "snapshot", nil, nil, nil, nil, now.Add(100*time.Millisecond), true); !ok {
		t.Fatalf("expected output payload on capture success")
	}
	item := ss.attached[key]
	if item.captureFailures != 0 {
		t.Fatalf("expected failure count reset, got %d", item.captureFailures)
	}
	if !item.nextCaptureAt.IsZero() {
		t.Fatalf("expected nextCaptureAt reset after success")
	}
}

func TestTTYV2TelemetryTracksHotpathAndOutputSources(t *testing.T) {
	ss := &ttyV2Session{
		telemetry: ttyV2SessionTelemetry{
			ResyncQueuedByReason:  map[string]int{},
			ResyncAppliedByReason: map[string]int{},
		},
	}
	ss.recordHotpathCapture(true)
	ss.recordHotpathCapture(true)
	ss.recordHotpathCapture(false)
	ss.recordOutputSource("bridge")
	ss.recordOutputSource("pane_tap")
	ss.recordOutputSource("snapshot")
	ss.recordOutputSource("")

	metrics := ss.telemetrySnapshot()
	if metrics.HotpathCaptureSelected != 2 {
		t.Fatalf("expected selected capture count=2, got %d", metrics.HotpathCaptureSelected)
	}
	if metrics.HotpathCaptureNonSelected != 1 {
		t.Fatalf("expected non-selected capture count=1, got %d", metrics.HotpathCaptureNonSelected)
	}
	if metrics.OutputBridge != 1 {
		t.Fatalf("expected bridge output count=1, got %d", metrics.OutputBridge)
	}
	if metrics.OutputPaneTap != 1 {
		t.Fatalf("expected pane_tap output count=1, got %d", metrics.OutputPaneTap)
	}
	if metrics.OutputSnapshot != 2 {
		t.Fatalf("expected snapshot output count=2, got %d", metrics.OutputSnapshot)
	}
}

func TestTTYV2TelemetryTracksResyncQueuedAndApplied(t *testing.T) {
	ref := ttyv2.PaneRef{Target: "local", SessionName: "s", WindowID: "@2", PaneID: "%44"}
	key := ref.CanonicalKey()
	cols := 120
	rows := 40
	ss := &ttyV2Session{
		attached: map[string]*ttyV2AttachedPane{
			key: {
				key:          key,
				ref:          ref,
				lastPaneCols: &cols,
				lastPaneRows: &rows,
			},
		},
		layoutByWindow: map[string]tmuxLayoutGeometry{},
		telemetry: ttyV2SessionTelemetry{
			ResyncQueuedByReason:  map[string]int{},
			ResyncAppliedByReason: map[string]int{},
		},
	}

	ss.markResyncOnLayoutGeometryDiff(key, "@2", 121, 40)
	wasForced, reason := ss.clearForceResync(key)
	if !wasForced {
		t.Fatalf("expected forceResync to be set")
	}
	if reason != ttyV2ResyncReasonLayoutChange {
		t.Fatalf("expected layout-change reason, got %q", reason)
	}
	ss.recordResyncApplied(reason)
	metrics := ss.telemetrySnapshot()
	if metrics.ResyncQueuedByReason[ttyV2ResyncReasonLayoutChange] != 1 {
		t.Fatalf("expected queued layout-change=1, got %d", metrics.ResyncQueuedByReason[ttyV2ResyncReasonLayoutChange])
	}
	if metrics.ResyncAppliedByReason[ttyV2ResyncReasonLayoutChange] != 1 {
		t.Fatalf("expected applied layout-change=1, got %d", metrics.ResyncAppliedByReason[ttyV2ResyncReasonLayoutChange])
	}
}
