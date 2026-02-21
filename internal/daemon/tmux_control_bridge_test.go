package daemon

import (
	"bytes"
	"errors"
	"io"
	"strings"
	"testing"
)

type testNopWriteCloser struct {
	io.Writer
}

func (testNopWriteCloser) Close() error { return nil }

type testErrWriteCloser struct{}

func (testErrWriteCloser) Write(_ []byte) (int, error) { return 0, errors.New("write failed") }
func (testErrWriteCloser) Close() error                 { return nil }

func TestParseTmuxControlEventLineParsesOutputEvents(t *testing.T) {
	line := `%output %43 \033[1mhello\033[0m\015\012`
	event, ok := parseTmuxControlEventLine(line)
	if !ok {
		t.Fatalf("expected parse success")
	}
	if event.Type != tmuxControlEventOutput {
		t.Fatalf("expected output event, got %s", event.Type)
	}
	if event.PaneID != "%43" {
		t.Fatalf("unexpected pane id: %q", event.PaneID)
	}
	expected := "\x1b[1mhello\x1b[0m\r\n"
	if string(event.Bytes) != expected {
		t.Fatalf("unexpected payload: %q", string(event.Bytes))
	}
}

func TestTmuxControlBridgeHandleSendCommandsWritesLines(t *testing.T) {
	var buf bytes.Buffer
	handle := &tmuxControlBridgeHandle{
		stdin: testNopWriteCloser{Writer: &buf},
	}
	if err := handle.sendCommands(
		"select-window -t @3",
		"  ",
		"select-pane -t %10",
	); err != nil {
		t.Fatalf("send commands: %v", err)
	}
	got := buf.String()
	want := "select-window -t @3\nselect-pane -t %10\n"
	if got != want {
		t.Fatalf("unexpected stdin payload: got=%q want=%q", got, want)
	}
}

func TestTTYV2SessionSendControlBridgeCommandsStopsBridgeOnWriteError(t *testing.T) {
	ss := &ttyV2Session{
		bridge: &tmuxControlBridgeHandle{
			stdin: testErrWriteCloser{},
		},
		layoutByWindow: map[string]tmuxLayoutGeometry{},
	}
	if ss.sendControlBridgeCommands("select-pane -t %1") {
		t.Fatalf("expected command write failure")
	}
	if ss.currentControlBridge() != nil {
		t.Fatalf("expected control bridge to be stopped after write failure")
	}
}

func TestFilteredExecEnvDropsTmuxVariables(t *testing.T) {
	base := []string{
		"PATH=/usr/bin",
		"TMUX=/tmp/tmux-1000/default,1234,0",
		"TMUX_PANE=%6",
		"HOME=/Users/tester",
	}
	got := filteredExecEnv(base, "TMUX", "TMUX_PANE")
	if containsEnvKey(got, "TMUX") {
		t.Fatalf("expected TMUX to be removed, got=%v", got)
	}
	if containsEnvKey(got, "TMUX_PANE") {
		t.Fatalf("expected TMUX_PANE to be removed, got=%v", got)
	}
	if !containsEnvKey(got, "PATH") || !containsEnvKey(got, "HOME") {
		t.Fatalf("expected non-tmux vars to remain, got=%v", got)
	}
}

func containsEnvKey(values []string, key string) bool {
	prefix := key + "="
	for _, value := range values {
		if value == key || strings.HasPrefix(value, prefix) {
			return true
		}
	}
	return false
}

func TestParseTmuxControlEventLineParsesControlStateEvents(t *testing.T) {
	layout, ok := parseTmuxControlEventLine("%layout-change @3 8cee,302x85,0,0,0")
	if !ok || layout.Type != tmuxControlEventLayoutChange || layout.WindowID != "@3" {
		t.Fatalf("unexpected layout event: %+v parsed=%v", layout, ok)
	}
	if !layout.LayoutKnown || layout.LayoutCols != 302 || layout.LayoutRows != 85 {
		t.Fatalf("unexpected layout geometry: %+v", layout)
	}
	session, ok := parseTmuxControlEventLine("%session-changed $3 vm agtmux exp-go-codex-implementation-poc")
	if !ok || session.Type != tmuxControlEventSessionChanged || session.SessionID != "$3" {
		t.Fatalf("unexpected session event: %+v parsed=%v", session, ok)
	}
	if session.SessionName != "vm agtmux exp-go-codex-implementation-poc" {
		t.Fatalf("unexpected session name: %q", session.SessionName)
	}
	windowAdd, ok := parseTmuxControlEventLine("%window-add @13")
	if !ok || windowAdd.Type != tmuxControlEventWindowAdd || windowAdd.WindowID != "@13" {
		t.Fatalf("unexpected window-add event: %+v parsed=%v", windowAdd, ok)
	}
	exitEvent, ok := parseTmuxControlEventLine("%exit")
	if !ok || exitEvent.Type != tmuxControlEventExit {
		t.Fatalf("unexpected exit event: %+v parsed=%v", exitEvent, ok)
	}
}

func TestParseTmuxControlOutputLineParsesOutputLine(t *testing.T) {
	line := `%output %43 \033[1mhello\033[0m\015\012`
	got, ok := parseTmuxControlOutputLine(line)
	if !ok {
		t.Fatalf("expected parse success")
	}
	if got.PaneID != "%43" {
		t.Fatalf("unexpected pane id: %q", got.PaneID)
	}
	expected := "\x1b[1mhello\x1b[0m\r\n"
	if string(got.Bytes) != expected {
		t.Fatalf("unexpected payload: %q", string(got.Bytes))
	}
}

func TestParseTmuxControlOutputLineParsesExtendedOutputLine(t *testing.T) {
	line := `%extended-output %6 0 \033[32mok\033[0m`
	got, ok := parseTmuxControlOutputLine(line)
	if !ok {
		t.Fatalf("expected parse success")
	}
	if got.PaneID != "%6" {
		t.Fatalf("unexpected pane id: %q", got.PaneID)
	}
	expected := "\x1b[32mok\x1b[0m"
	if string(got.Bytes) != expected {
		t.Fatalf("unexpected payload: %q", string(got.Bytes))
	}
}

func TestParseTmuxControlOutputLinePreservesCJKUTF8Bytes(t *testing.T) {
	line := "%output %7 テストです"
	got, ok := parseTmuxControlOutputLine(line)
	if !ok {
		t.Fatalf("expected parse success")
	}
	if got.PaneID != "%7" {
		t.Fatalf("unexpected pane id: %q", got.PaneID)
	}
	if string(got.Bytes) != "テストです" {
		t.Fatalf("expected cjk payload to be preserved, got %q", string(got.Bytes))
	}
}

func TestParseTmuxControlOutputLineRejectsInvalidInput(t *testing.T) {
	cases := []string{
		"",
		"%output",
		"%output %6 \\0",
		"%extended-output %6",
		"%foo %6 abc",
	}
	for _, tc := range cases {
		if _, ok := parseTmuxControlOutputLine(tc); ok {
			t.Fatalf("expected parse failure for %q", tc)
		}
	}
}

func TestParseTmuxControlEventLineRejectsUnsupportedEvents(t *testing.T) {
	cases := []string{
		"",
		"%begin 1 2 1",
		"%end 1 2 1",
		"%error 1 2 1",
		"%sessions-changed",
	}
	for _, tc := range cases {
		if _, ok := parseTmuxControlEventLine(tc); ok {
			t.Fatalf("expected parse failure for %q", tc)
		}
	}
}

func TestParseTmuxLayoutGeometry(t *testing.T) {
	cols, rows, ok := parseTmuxLayoutGeometry("8cee,302x85,0,0,0")
	if !ok || cols != 302 || rows != 85 {
		t.Fatalf("unexpected parsed geometry: cols=%d rows=%d ok=%v", cols, rows, ok)
	}
	if _, _, ok := parseTmuxLayoutGeometry("invalid"); ok {
		t.Fatalf("expected parse failure for invalid layout")
	}
}

func TestShouldHandleLayoutGeometryChange(t *testing.T) {
	if !shouldHandleLayoutGeometryChange(tmuxLayoutGeometry{}, tmuxLayoutGeometry{Cols: 120, Rows: 40}) {
		t.Fatalf("expected first known geometry to trigger handling")
	}
	if shouldHandleLayoutGeometryChange(tmuxLayoutGeometry{Cols: 120, Rows: 40}, tmuxLayoutGeometry{Cols: 120, Rows: 40}) {
		t.Fatalf("expected identical geometry to skip handling")
	}
	if !shouldHandleLayoutGeometryChange(tmuxLayoutGeometry{Cols: 120, Rows: 40}, tmuxLayoutGeometry{Cols: 121, Rows: 40}) {
		t.Fatalf("expected width change to trigger handling")
	}
}

func TestParsePaneSizeAndChange(t *testing.T) {
	cols, rows, ok := parsePaneSize("120,40")
	if !ok || cols != 120 || rows != 40 {
		t.Fatalf("unexpected pane size parse: cols=%d rows=%d ok=%v", cols, rows, ok)
	}
	prevCols := 120
	prevRows := 40
	if paneSizeChanged(&prevCols, &prevRows, 120, 40) {
		t.Fatalf("expected unchanged pane size to skip")
	}
	if !paneSizeChanged(&prevCols, &prevRows, 121, 40) {
		t.Fatalf("expected changed pane size to trigger")
	}
	if !paneSizeChanged(nil, nil, 120, 40) {
		t.Fatalf("expected missing previous size to trigger")
	}
}
