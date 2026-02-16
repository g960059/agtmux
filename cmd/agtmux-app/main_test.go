package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/api"
	"github.com/g960059/agtmux/internal/appclient"
)

type fakeService struct {
	watchLines []api.WatchLine
	watchOpts  appclient.WatchLoopOptions
	watchCalls int
	watchErr   error

	sendReq   appclient.SendRequest
	attachErr error

	actionEventsID  string
	actionEvents    api.ActionEventsEnvelope
	actionEventsErr error
	terminalReadReq appclient.TerminalReadRequest
	terminalResize  appclient.TerminalResizeRequest

	listPanesOpts     appclient.ListOptions
	listWindowsOpts   appclient.ListOptions
	listSessionsOpts  appclient.ListOptions
	listPanesCalls    int
	listWindowsCalls  int
	listSessionsCalls int
	listTargetsCalls  int
	listPanesErr      error
	listWindowsErr    error
	listSessionsErr   error
	listTargetsErr    error
	createTargetReq   appclient.CreateTargetRequest
	connectTargetName string
	deleteTargetName  string
	panesResp         api.ListEnvelope[api.PaneItem]
	windowsResp       api.ListEnvelope[api.WindowItem]
	sessionsResp      api.ListEnvelope[api.SessionItem]
	targetsResp       api.TargetsEnvelope
	createTargetResp  api.TargetsEnvelope
	connectTargetResp api.TargetsEnvelope
	createTargetErr   error
	connectTargetErr  error
	deleteTargetErr   error

	setAdapterName string
	setAdapterFlag bool
}

type failWriter struct {
	err error
}

func (w failWriter) Write(_ []byte) (int, error) {
	if w.err != nil {
		return 0, w.err
	}
	return 0, errors.New("write failed")
}

func (f *fakeService) WatchLoop(_ context.Context, opts appclient.WatchLoopOptions, onLine func(api.WatchLine) error) error {
	f.watchOpts = opts
	f.watchCalls++
	for _, line := range f.watchLines {
		if err := onLine(line); err != nil {
			return err
		}
	}
	if f.watchErr != nil {
		return f.watchErr
	}
	return nil
}

func (f *fakeService) Attach(_ context.Context, _ appclient.AttachRequest) (api.ActionResponse, error) {
	if f.attachErr != nil {
		return api.ActionResponse{}, f.attachErr
	}
	return api.ActionResponse{ActionID: "a-attach", ResultCode: "completed"}, nil
}

func (f *fakeService) Send(_ context.Context, req appclient.SendRequest) (api.ActionResponse, error) {
	f.sendReq = req
	return api.ActionResponse{ActionID: "a-send", ResultCode: "completed"}, nil
}

func (f *fakeService) ViewOutput(_ context.Context, _ appclient.ViewOutputRequest) (api.ActionResponse, error) {
	out := "line1\nline2\n"
	return api.ActionResponse{ActionID: "a-view", ResultCode: "completed", Output: &out}, nil
}

func (f *fakeService) Kill(_ context.Context, _ appclient.KillRequest) (api.ActionResponse, error) {
	return api.ActionResponse{ActionID: "a-kill", ResultCode: "completed"}, nil
}

func (f *fakeService) ListActionEvents(_ context.Context, actionID string) (api.ActionEventsEnvelope, error) {
	f.actionEventsID = actionID
	if f.actionEventsErr != nil {
		return api.ActionEventsEnvelope{}, f.actionEventsErr
	}
	if f.actionEvents.ActionID == "" {
		f.actionEvents = api.ActionEventsEnvelope{
			SchemaVersion: "v1",
			ActionID:      actionID,
			Events: []api.ActionEventItem{
				{
					EventID:    "ev-1",
					ActionID:   actionID,
					RuntimeID:  "rt-1",
					EventType:  "action.send",
					Source:     "daemon",
					EventTime:  "2026-02-13T00:00:00Z",
					IngestedAt: "2026-02-13T00:00:00Z",
					DedupeKey:  "dk-1",
				},
			},
		}
	}
	return f.actionEvents, nil
}

func (f *fakeService) ListCapabilities(_ context.Context) (api.CapabilitiesEnvelope, error) {
	return api.CapabilitiesEnvelope{
		SchemaVersion: "v1",
		Capabilities: api.CapabilityFlags{
			EmbeddedTerminal:       true,
			TerminalRead:           true,
			TerminalResize:         true,
			TerminalWriteViaAction: true,
			TerminalFrameProtocol:  "snapshot-delta-reset",
		},
	}, nil
}

func (f *fakeService) TerminalRead(_ context.Context, req appclient.TerminalReadRequest) (api.TerminalReadEnvelope, error) {
	f.terminalReadReq = req
	return api.TerminalReadEnvelope{
		SchemaVersion: "v1",
		Frame: api.TerminalFrameItem{
			FrameType: "snapshot",
			StreamID:  "stream-1",
			Cursor:    "stream-1:2",
			Target:    req.Target,
			PaneID:    req.PaneID,
			Lines:     req.Lines,
			Content:   "hello",
		},
	}, nil
}

func (f *fakeService) TerminalResize(_ context.Context, req appclient.TerminalResizeRequest) (api.TerminalResizeResponse, error) {
	f.terminalResize = req
	return api.TerminalResizeResponse{
		SchemaVersion: "v1",
		Target:        req.Target,
		PaneID:        req.PaneID,
		Cols:          req.Cols,
		Rows:          req.Rows,
		ResultCode:    "completed",
	}, nil
}

func (f *fakeService) ListPanes(_ context.Context, opts appclient.ListOptions) (api.ListEnvelope[api.PaneItem], error) {
	if f.listPanesErr != nil {
		return api.ListEnvelope[api.PaneItem]{}, f.listPanesErr
	}
	f.listPanesOpts = opts
	f.listPanesCalls++
	if len(f.panesResp.Items) == 0 {
		f.panesResp = api.ListEnvelope[api.PaneItem]{
			SchemaVersion: "v1",
			Summary: api.ListSummary{
				ByState:  map[string]int{"running": 1},
				ByAgent:  map[string]int{"codex": 1},
				ByTarget: map[string]int{"t1": 1},
			},
			Items: []api.PaneItem{
				{
					Identity: api.PaneIdentity{
						Target:      "t1",
						SessionName: "s1",
						WindowID:    "@1",
						PaneID:      "%1",
					},
					State:      "running",
					ReasonCode: "active",
					AgentType:  "codex",
					RuntimeID:  "rt-1",
					UpdatedAt:  "2026-02-13T00:00:00Z",
				},
			},
		}
	}
	return f.panesResp, nil
}

func (f *fakeService) ListWindows(_ context.Context, opts appclient.ListOptions) (api.ListEnvelope[api.WindowItem], error) {
	if f.listWindowsErr != nil {
		return api.ListEnvelope[api.WindowItem]{}, f.listWindowsErr
	}
	f.listWindowsOpts = opts
	f.listWindowsCalls++
	if len(f.windowsResp.Items) == 0 {
		f.windowsResp = api.ListEnvelope[api.WindowItem]{
			SchemaVersion: "v1",
			Items: []api.WindowItem{
				{
					Identity: api.WindowIdentity{
						Target:      "t1",
						SessionName: "s1",
						WindowID:    "@1",
					},
					TopState:     "running",
					WaitingCount: 0,
					RunningCount: 1,
					TotalPanes:   1,
				},
			},
		}
	}
	return f.windowsResp, nil
}

func (f *fakeService) ListSessions(_ context.Context, opts appclient.ListOptions) (api.ListEnvelope[api.SessionItem], error) {
	if f.listSessionsErr != nil {
		return api.ListEnvelope[api.SessionItem]{}, f.listSessionsErr
	}
	f.listSessionsOpts = opts
	f.listSessionsCalls++
	if len(f.sessionsResp.Items) == 0 {
		f.sessionsResp = api.ListEnvelope[api.SessionItem]{
			SchemaVersion: "v1",
			Items: []api.SessionItem{
				{
					Identity: api.SessionIdentity{
						Target:      "t1",
						SessionName: "s1",
					},
					TotalPanes: 1,
					ByState:    map[string]int{"running": 1},
					ByAgent:    map[string]int{"codex": 1},
				},
			},
		}
	}
	return f.sessionsResp, nil
}

func (f *fakeService) ListTargets(_ context.Context) (api.TargetsEnvelope, error) {
	if f.listTargetsErr != nil {
		return api.TargetsEnvelope{}, f.listTargetsErr
	}
	f.listTargetsCalls++
	if f.targetsResp.SchemaVersion == "" && f.targetsResp.Targets == nil {
		f.targetsResp = api.TargetsEnvelope{
			SchemaVersion: "v1",
			Targets: []api.TargetResponse{
				{
					TargetID:      "tgt-1",
					TargetName:    "local",
					Kind:          "local",
					ConnectionRef: "",
					IsDefault:     true,
					Health:        "ok",
					UpdatedAt:     "2026-02-13T00:00:00Z",
				},
			},
		}
	}
	return f.targetsResp, nil
}

func (f *fakeService) CreateTarget(_ context.Context, req appclient.CreateTargetRequest) (api.TargetsEnvelope, error) {
	f.createTargetReq = req
	if f.createTargetErr != nil {
		return api.TargetsEnvelope{}, f.createTargetErr
	}
	if f.createTargetResp.SchemaVersion == "" && f.createTargetResp.Targets == nil {
		f.createTargetResp = api.TargetsEnvelope{
			SchemaVersion: "v1",
			Targets: []api.TargetResponse{
				{
					TargetID:      req.Name,
					TargetName:    req.Name,
					Kind:          req.Kind,
					ConnectionRef: req.ConnectionRef,
					IsDefault:     req.IsDefault,
					Health:        "ok",
					UpdatedAt:     "2026-02-13T00:00:00Z",
				},
			},
		}
	}
	return f.createTargetResp, nil
}

func (f *fakeService) ConnectTarget(_ context.Context, targetName string) (api.TargetsEnvelope, error) {
	f.connectTargetName = targetName
	if f.connectTargetErr != nil {
		return api.TargetsEnvelope{}, f.connectTargetErr
	}
	if f.connectTargetResp.SchemaVersion == "" && f.connectTargetResp.Targets == nil {
		f.connectTargetResp = api.TargetsEnvelope{
			SchemaVersion: "v1",
			Targets: []api.TargetResponse{
				{
					TargetID:      targetName,
					TargetName:    targetName,
					Kind:          "ssh",
					ConnectionRef: "ssh://" + targetName,
					IsDefault:     false,
					Health:        "ok",
					UpdatedAt:     "2026-02-13T00:00:00Z",
				},
			},
		}
	}
	return f.connectTargetResp, nil
}

func (f *fakeService) DeleteTarget(_ context.Context, targetName string) error {
	f.deleteTargetName = targetName
	if f.deleteTargetErr != nil {
		return f.deleteTargetErr
	}
	return nil
}

func (f *fakeService) ListAdapters(_ context.Context, _ *bool) (api.AdaptersEnvelope, error) {
	return api.AdaptersEnvelope{
		SchemaVersion: "v1",
		Adapters: []api.AdapterResponse{
			{
				AdapterName: "claude-hook",
				AgentType:   "claude",
				Version:     "v1",
				Compatible:  true,
				Enabled:     true,
			},
		},
	}, nil
}

func (f *fakeService) SetAdapterEnabled(_ context.Context, adapterName string, enabled bool) (api.AdaptersEnvelope, error) {
	f.setAdapterName = adapterName
	f.setAdapterFlag = enabled
	return api.AdaptersEnvelope{
		SchemaVersion: "v1",
		Adapters: []api.AdapterResponse{
			{
				AdapterName: adapterName,
				AgentType:   "claude",
				Version:     "v1",
				Compatible:  true,
				Enabled:     enabled,
			},
		},
	}, nil
}

func TestRunResidentOnceJSON(t *testing.T) {
	svc := &fakeService{
		watchLines: []api.WatchLine{
			{SchemaVersion: "v1", Scope: "panes", Type: "snapshot", Sequence: 1, Cursor: "stream:1"},
		},
	}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"run", "--once", "--json"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.watchOpts.Once != true {
		t.Fatalf("expected once option true, got %+v", svc.watchOpts)
	}
	if !bytes.Contains(out.Bytes(), []byte(`"type":"snapshot"`)) {
		t.Fatalf("expected snapshot json output, got %s", out.String())
	}
}

func TestRunResidentOnceJSONWriteError(t *testing.T) {
	svc := &fakeService{
		watchLines: []api.WatchLine{
			{SchemaVersion: "v1", Scope: "panes", Type: "snapshot", Sequence: 1, Cursor: "stream:1"},
		},
	}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"run", "--once", "--json"}, failWriter{err: errors.New("broken pipe")}, errOut, svc)
	if code != 1 {
		t.Fatalf("expected code 1, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("broken pipe")) {
		t.Fatalf("expected write error in stderr, got %s", errOut.String())
	}
}

func TestRunActionSendJSON(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "send",
		"--request-ref", "req-1",
		"--target", "t1",
		"--pane", "%1",
		"--text", "hello",
		"--json",
	}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.sendReq.RequestRef != "req-1" || svc.sendReq.Target != "t1" || svc.sendReq.PaneID != "%1" || svc.sendReq.Text != "hello" {
		t.Fatalf("unexpected send request: %+v", svc.sendReq)
	}
	var resp api.ActionResponse
	if err := json.Unmarshal(out.Bytes(), &resp); err != nil {
		t.Fatalf("decode output json: %v output=%s", err, out.String())
	}
	if resp.ActionID != "a-send" || resp.ResultCode != "completed" {
		t.Fatalf("unexpected action response: %+v", resp)
	}
}

func TestRunTerminalCapabilitiesJSON(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"terminal", "capabilities", "--json"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	var resp api.CapabilitiesEnvelope
	if err := json.Unmarshal(out.Bytes(), &resp); err != nil {
		t.Fatalf("decode output json: %v output=%s", err, out.String())
	}
	if !resp.Capabilities.EmbeddedTerminal || !resp.Capabilities.TerminalRead || !resp.Capabilities.TerminalResize {
		t.Fatalf("unexpected capabilities response: %+v", resp.Capabilities)
	}
}

func TestRunTerminalReadJSON(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"terminal", "read",
		"--target", "t1",
		"--pane", "%1",
		"--cursor", "stream-1:1",
		"--lines", "120",
		"--json",
	}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.terminalReadReq.Target != "t1" || svc.terminalReadReq.PaneID != "%1" || svc.terminalReadReq.Cursor != "stream-1:1" || svc.terminalReadReq.Lines != 120 {
		t.Fatalf("unexpected terminal read request: %+v", svc.terminalReadReq)
	}
	var resp api.TerminalReadEnvelope
	if err := json.Unmarshal(out.Bytes(), &resp); err != nil {
		t.Fatalf("decode output json: %v output=%s", err, out.String())
	}
	if resp.Frame.PaneID != "%1" || resp.Frame.Target != "t1" {
		t.Fatalf("unexpected terminal read response: %+v", resp.Frame)
	}
}

func TestRunTerminalResizeUsageError(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"terminal", "resize",
		"--target", "t1",
		"--pane", "%1",
		"--cols", "0",
		"--rows", "40",
	}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("usage: agtmux-app terminal resize")) {
		t.Fatalf("expected usage output, got %s", errOut.String())
	}
}

func TestRunActionSendPreservesWhitespace(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "send",
		"--request-ref", "req-ws",
		"--target", "t1",
		"--pane", "%1",
		"--text", "  hi  ",
	}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.sendReq.Text != "  hi  " {
		t.Fatalf("expected whitespace-preserved text, got %q", svc.sendReq.Text)
	}
}

func TestRunActionSendStdin(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}

	reader, writer, err := os.Pipe()
	if err != nil {
		t.Fatalf("pipe: %v", err)
	}
	payload := "  hello\nworld  \n"
	if _, err := writer.WriteString(payload); err != nil {
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

	code := run(context.Background(), []string{
		"action", "send",
		"--request-ref", "req-stdin",
		"--target", "t1",
		"--pane", "%1",
		"--stdin",
	}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.sendReq.Text != payload {
		t.Fatalf("expected stdin payload preserved, got %q", svc.sendReq.Text)
	}
	if svc.sendReq.Key != "" {
		t.Fatalf("expected key to be empty, got %q", svc.sendReq.Key)
	}
}

func TestRunActionSendStdinRejectsEmptyInput(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}

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

	code := run(context.Background(), []string{
		"action", "send",
		"--request-ref", "req-stdin-empty",
		"--target", "t1",
		"--pane", "%1",
		"--stdin",
	}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("non-empty payload")) {
		t.Fatalf("expected non-empty payload error, got %s", errOut.String())
	}
	if svc.sendReq.RequestRef != "" {
		t.Fatalf("expected send action not called, got %+v", svc.sendReq)
	}
}

func TestRunActionSendStdinReadError(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}

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

	code := run(context.Background(), []string{
		"action", "send",
		"--request-ref", "req-stdin-err",
		"--target", "t1",
		"--pane", "%1",
		"--stdin",
	}, out, errOut, svc)
	if code != 1 {
		t.Fatalf("expected code 1, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("read stdin")) {
		t.Fatalf("expected read stdin error, got %s", errOut.String())
	}
	if svc.sendReq.RequestRef != "" {
		t.Fatalf("expected send action not called, got %+v", svc.sendReq)
	}
}

func TestRunActionSendStdinRejectsPayloadTooLarge(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}

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

	code := run(context.Background(), []string{
		"action", "send",
		"--request-ref", "req-stdin-big",
		"--target", "t1",
		"--pane", "%1",
		"--stdin",
	}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
	if err := <-writeDone; err != nil {
		t.Fatalf("write/close stdin payload: %v", err)
	}
	if !bytes.Contains(errOut.Bytes(), []byte("payload exceeds")) {
		t.Fatalf("expected payload exceeds error, got %s", errOut.String())
	}
	if svc.sendReq.RequestRef != "" {
		t.Fatalf("expected send action not called, got %+v", svc.sendReq)
	}
}

func TestRunActionSendRejectsTextAndStdin(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "send",
		"--request-ref", "req-x",
		"--target", "t1",
		"--pane", "%1",
		"--text", "hello",
		"--stdin",
	}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
	if svc.sendReq.RequestRef != "" {
		t.Fatalf("expected send action not called, got %+v", svc.sendReq)
	}
}

func TestRunActionSendRejectsKeyAndStdin(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "send",
		"--request-ref", "req-x",
		"--target", "t1",
		"--pane", "%1",
		"--key", "C-c",
		"--stdin",
	}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
	if svc.sendReq.RequestRef != "" {
		t.Fatalf("expected send action not called, got %+v", svc.sendReq)
	}
}

func TestRunActionSendRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "send",
		"--request-ref", "req-x",
		"--target", "t1",
		"--pane", "%1",
		"--text", "hello",
		"extra",
	}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
	if svc.sendReq.RequestRef != "" {
		t.Fatalf("expected send action not called, got %+v", svc.sendReq)
	}
}

func TestRunActionAttachStaleRuntimeError(t *testing.T) {
	svc := &fakeService{attachErr: errors.New("E_RUNTIME_STALE: stale runtime")}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "attach",
		"--request-ref", "req-stale-1",
		"--target", "t1",
		"--pane", "%1",
	}, out, errOut, svc)
	if code != 1 {
		t.Fatalf("expected code 1, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("E_RUNTIME_STALE")) {
		t.Fatalf("expected stale runtime error in stderr, got %s", errOut.String())
	}
}

func TestRunActionAttachRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "attach",
		"--request-ref", "req-1",
		"--target", "t1",
		"--pane", "%1",
		"extra",
	}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunActionEventsJSON(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "events",
		"--action-id", "a-send",
		"--json",
	}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.actionEventsID != "a-send" {
		t.Fatalf("expected action id forwarded, got %q", svc.actionEventsID)
	}
	var env api.ActionEventsEnvelope
	if err := json.Unmarshal(out.Bytes(), &env); err != nil {
		t.Fatalf("decode action events json: %v output=%s", err, out.String())
	}
	if env.ActionID != "a-send" || len(env.Events) != 1 || env.Events[0].EventID != "ev-1" {
		t.Fatalf("unexpected action events payload: %+v", env)
	}
}

func TestRunActionEventsHumanReadable(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "events",
		"--action-id", "a-send",
	}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(out.Bytes(), []byte("action_id=a-send events=1")) {
		t.Fatalf("expected summary line, got %s", out.String())
	}
	if !bytes.Contains(out.Bytes(), []byte("event=ev-1 type=action.send source=daemon runtime=rt-1")) {
		t.Fatalf("expected event line, got %s", out.String())
	}
}

func TestRunActionEventsRequiresActionID(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "events",
	}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunActionEventsRejectsBlankActionID(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "events",
		"--action-id", "   ",
	}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunActionEventsRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "events",
		"--action-id", "a-send",
		"extra",
	}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunActionEventsServiceError(t *testing.T) {
	svc := &fakeService{actionEventsErr: errors.New("E_REF_NOT_FOUND: action not found")}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "events",
		"--action-id", "missing",
	}, out, errOut, svc)
	if code != 1 {
		t.Fatalf("expected code 1, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("E_REF_NOT_FOUND")) {
		t.Fatalf("expected service error in stderr, got %s", errOut.String())
	}
}

func TestRunActionEventsHumanWriteError(t *testing.T) {
	svc := &fakeService{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "events",
		"--action-id", "a-send",
	}, failWriter{err: errors.New("broken pipe")}, errOut, svc)
	if code != 1 {
		t.Fatalf("expected code 1, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("broken pipe")) {
		t.Fatalf("expected write error in stderr, got %s", errOut.String())
	}
}

func TestRunTargetListJSON(t *testing.T) {
	svc := &fakeService{
		targetsResp: api.TargetsEnvelope{
			SchemaVersion: "v1",
			Targets: []api.TargetResponse{
				{
					TargetID:      "tgt-1",
					TargetName:    "local",
					Kind:          "local",
					ConnectionRef: "",
					IsDefault:     true,
					Health:        "ok",
					UpdatedAt:     "2026-02-13T00:00:00Z",
				},
			},
		},
	}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"target", "list", "--json"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.listTargetsCalls != 1 {
		t.Fatalf("expected list targets call once, got %d", svc.listTargetsCalls)
	}
	var env api.TargetsEnvelope
	if err := json.Unmarshal(out.Bytes(), &env); err != nil {
		t.Fatalf("decode target list json: %v output=%s", err, out.String())
	}
	if len(env.Targets) != 1 || env.Targets[0].TargetName != "local" {
		t.Fatalf("unexpected target list payload: %+v", env.Targets)
	}
}

func TestRunTargetListRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"target", "list", "extra"}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
	if svc.listTargetsCalls != 0 {
		t.Fatalf("expected no list targets call on invalid args")
	}
}

func TestRunTargetListJSONWriteError(t *testing.T) {
	svc := &fakeService{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"target", "list", "--json"}, failWriter{err: errors.New("broken pipe")}, errOut, svc)
	if code != 1 {
		t.Fatalf("expected code 1, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("broken pipe")) {
		t.Fatalf("expected write error in stderr, got %s", errOut.String())
	}
}

func TestRunTargetAddJSON(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"target", "add", "vm1",
		"--kind", "ssh",
		"--connection-ref", "ssh://vm1",
		"--default",
		"--json",
	}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.createTargetReq.Name != "vm1" || svc.createTargetReq.Kind != "ssh" || svc.createTargetReq.ConnectionRef != "ssh://vm1" || !svc.createTargetReq.IsDefault {
		t.Fatalf("unexpected create target request: %+v", svc.createTargetReq)
	}
	var env api.TargetsEnvelope
	if err := json.Unmarshal(out.Bytes(), &env); err != nil {
		t.Fatalf("decode target add json: %v output=%s", err, out.String())
	}
	if len(env.Targets) != 1 || env.Targets[0].TargetName != "vm1" {
		t.Fatalf("unexpected target add payload: %+v", env.Targets)
	}
}

func TestRunTargetAddRequiresName(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"target", "add", "--kind", "ssh"}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
	if svc.createTargetReq.Name != "" {
		t.Fatalf("expected no create target call, got %+v", svc.createTargetReq)
	}
}

func TestRunTargetAddRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"target", "add", "vm1", "extra"}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
	if svc.createTargetReq.Name != "" {
		t.Fatalf("expected no create target call, got %+v", svc.createTargetReq)
	}
}

func TestRunTargetConnectJSON(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"target", "connect", "vm1", "--json"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.connectTargetName != "vm1" {
		t.Fatalf("expected connect target vm1, got %q", svc.connectTargetName)
	}
	var env api.TargetsEnvelope
	if err := json.Unmarshal(out.Bytes(), &env); err != nil {
		t.Fatalf("decode target connect json: %v output=%s", err, out.String())
	}
	if len(env.Targets) != 1 || env.Targets[0].TargetName != "vm1" {
		t.Fatalf("unexpected target connect payload: %+v", env.Targets)
	}
}

func TestRunTargetConnectRequiresName(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"target", "connect"}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
	if svc.connectTargetName != "" {
		t.Fatalf("expected no connect target call, got %q", svc.connectTargetName)
	}
}

func TestRunTargetConnectRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"target", "connect", "vm1", "extra"}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
	if svc.connectTargetName != "" {
		t.Fatalf("expected no connect target call, got %q", svc.connectTargetName)
	}
}

func TestRunTargetRemoveHuman(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"target", "remove", "vm1"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.deleteTargetName != "vm1" {
		t.Fatalf("expected delete target vm1, got %q", svc.deleteTargetName)
	}
	if !bytes.Contains(out.Bytes(), []byte("removed target vm1")) {
		t.Fatalf("unexpected remove output: %s", out.String())
	}
}

func TestRunTargetRemoveRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"target", "remove", "vm1", "extra"}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
	if svc.deleteTargetName != "" {
		t.Fatalf("expected no delete target call, got %q", svc.deleteTargetName)
	}
}

func TestRunAdapterEnable(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"adapter", "enable", "claude-hook"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.setAdapterName != "claude-hook" || svc.setAdapterFlag != true {
		t.Fatalf("unexpected set adapter call: name=%s enabled=%v", svc.setAdapterName, svc.setAdapterFlag)
	}
	if !bytes.Contains(out.Bytes(), []byte("enable adapter claude-hook (enabled)")) {
		t.Fatalf("unexpected output: %s", out.String())
	}
}

func TestRunAdapterEnableRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"adapter", "enable", "claude-hook", "extra"}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
	if svc.setAdapterName != "" {
		t.Fatalf("expected no adapter call on invalid args, got name=%s", svc.setAdapterName)
	}
}

func TestRunAdapterDisableRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"adapter", "disable", "claude-hook", "extra"}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
	if svc.setAdapterName != "" {
		t.Fatalf("expected no adapter call on invalid args, got name=%s", svc.setAdapterName)
	}
}

func TestRunAdapterListRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"adapter", "list", "extra"}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
}

func TestParseGlobalArgs(t *testing.T) {
	socket, rest, err := parseGlobalArgs([]string{"--socket", "/tmp/app.sock", "run", "--once"}, "/default.sock")
	if err != nil {
		t.Fatalf("parse global args: %v", err)
	}
	if socket != "/tmp/app.sock" {
		t.Fatalf("expected overridden socket, got %q", socket)
	}
	if len(rest) != 2 || rest[0] != "run" || rest[1] != "--once" {
		t.Fatalf("unexpected rest args: %+v", rest)
	}

	if _, _, err := parseGlobalArgs([]string{"--socket"}, "/default.sock"); err == nil {
		t.Fatalf("expected parse error for missing socket value")
	}
}

func TestParseGlobalArgsDoesNotConsumeSubcommandSocketToken(t *testing.T) {
	socket, rest, err := parseGlobalArgs([]string{
		"action", "send",
		"--request-ref", "req-1",
		"--target", "t1",
		"--pane", "%1",
		"--text", "--socket",
	}, "/default.sock")
	if err != nil {
		t.Fatalf("parse global args: %v", err)
	}
	if socket != "/default.sock" {
		t.Fatalf("expected default socket, got %q", socket)
	}
	if len(rest) != 10 || rest[len(rest)-1] != "--socket" {
		t.Fatalf("unexpected rest args: %+v", rest)
	}
}

func TestParseGlobalArgsRejectsEmptySocketValue(t *testing.T) {
	if _, _, err := parseGlobalArgs([]string{"--socket", "", "run"}, "/default.sock"); err == nil {
		t.Fatalf("expected parse error for empty socket value")
	}
	if _, _, err := parseGlobalArgs([]string{"--socket=", "run"}, "/default.sock"); err == nil {
		t.Fatalf("expected parse error for --socket= empty value")
	}
	if _, _, err := parseGlobalArgs([]string{"--socket", "   ", "run"}, "/default.sock"); err == nil {
		t.Fatalf("expected parse error for whitespace-only socket value")
	}
}

func TestParseGlobalArgsWithRequestTimeout(t *testing.T) {
	socket, timeout, rest, err := parseGlobalArgsWithTimeout([]string{
		"--socket", "/tmp/app.sock",
		"--request-timeout", "3s",
		"run", "--once",
	}, "/default.sock")
	if err != nil {
		t.Fatalf("parse global args with timeout: %v", err)
	}
	if socket != "/tmp/app.sock" {
		t.Fatalf("expected overridden socket, got %q", socket)
	}
	if timeout == nil || *timeout != 3*time.Second {
		t.Fatalf("expected timeout 3s, got %v", timeout)
	}
	if len(rest) != 2 || rest[0] != "run" || rest[1] != "--once" {
		t.Fatalf("unexpected rest args: %+v", rest)
	}

	_, timeoutEq, restEq, err := parseGlobalArgsWithTimeout([]string{
		"--request-timeout=250ms",
		"action", "send",
	}, "/default.sock")
	if err != nil {
		t.Fatalf("parse global args with timeout equals: %v", err)
	}
	if timeoutEq == nil || *timeoutEq != 250*time.Millisecond {
		t.Fatalf("expected timeout 250ms, got %v", timeoutEq)
	}
	if len(restEq) != 2 || restEq[0] != "action" || restEq[1] != "send" {
		t.Fatalf("unexpected rest args for equals: %+v", restEq)
	}
}

func TestParseGlobalArgsWithRequestTimeoutRejectsInvalidValues(t *testing.T) {
	if _, _, _, err := parseGlobalArgsWithTimeout([]string{"--request-timeout"}, "/default.sock"); err == nil {
		t.Fatalf("expected parse error for missing request-timeout value")
	}
	if _, _, _, err := parseGlobalArgsWithTimeout([]string{"--request-timeout="}, "/default.sock"); err == nil {
		t.Fatalf("expected parse error for empty request-timeout value")
	}
	if _, _, _, err := parseGlobalArgsWithTimeout([]string{"--request-timeout", "   "}, "/default.sock"); err == nil {
		t.Fatalf("expected parse error for whitespace request-timeout value")
	}
	if _, _, _, err := parseGlobalArgsWithTimeout([]string{"--request-timeout", "abc"}, "/default.sock"); err == nil {
		t.Fatalf("expected parse error for invalid request-timeout value")
	}
	if _, _, _, err := parseGlobalArgsWithTimeout([]string{"--request-timeout", "-1s"}, "/default.sock"); err == nil {
		t.Fatalf("expected parse error for negative request-timeout value")
	}
}

func TestParseGlobalArgsWithRequestTimeoutDoesNotConsumeSubcommandToken(t *testing.T) {
	socket, timeout, rest, err := parseGlobalArgsWithTimeout([]string{
		"action", "send",
		"--request-ref", "req-1",
		"--target", "t1",
		"--pane", "%1",
		"--text", "--request-timeout",
	}, "/default.sock")
	if err != nil {
		t.Fatalf("parse global args with timeout token: %v", err)
	}
	if socket != "/default.sock" {
		t.Fatalf("expected default socket, got %q", socket)
	}
	if timeout != nil {
		t.Fatalf("expected nil timeout when flag appears in subcommand payload, got %v", timeout)
	}
	if len(rest) != 10 || rest[len(rest)-1] != "--request-timeout" {
		t.Fatalf("unexpected rest args: %+v", rest)
	}
}

func TestRunResidentHumanReadable(t *testing.T) {
	svc := &fakeService{
		watchLines: []api.WatchLine{
			{
				SchemaVersion: "v1",
				Scope:         "panes",
				Type:          "snapshot",
				Sequence:      2,
				Cursor:        "stream:2",
				Summary:       api.ListSummary{ByState: map[string]int{"running": 1, "idle": 2}},
				GeneratedAt:   time.Now().UTC(),
			},
		},
	}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"--once"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(out.Bytes(), []byte("by_state={idle=2,running=1}")) {
		t.Fatalf("unexpected human-readable output: %s", out.String())
	}
}

func TestRunViewGlobalJSON(t *testing.T) {
	svc := &fakeService{
		panesResp: api.ListEnvelope[api.PaneItem]{
			SchemaVersion: "v1",
			Summary: api.ListSummary{
				ByState:  map[string]int{"running": 2},
				ByAgent:  map[string]int{"codex": 2},
				ByTarget: map[string]int{"t1": 2},
			},
			Items: []api.PaneItem{
				{
					Identity: api.PaneIdentity{
						Target:      "t1",
						SessionName: "s1",
						WindowID:    "@1",
						PaneID:      "%1",
					},
					State:     "running",
					AgentType: "codex",
					UpdatedAt: "2026-02-13T00:00:00Z",
				},
			},
		},
	}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "global", "--target", "t1", "--json"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.listPanesOpts.Target != "t1" {
		t.Fatalf("expected target filter t1, got %+v", svc.listPanesOpts)
	}
	var env api.ListEnvelope[api.PaneItem]
	if err := json.Unmarshal(out.Bytes(), &env); err != nil {
		t.Fatalf("decode global view json: %v output=%s", err, out.String())
	}
	if len(env.Items) != 1 || env.Summary.ByState["running"] != 2 {
		t.Fatalf("unexpected global payload: %+v", env)
	}
}

func TestRunViewSnapshotJSON(t *testing.T) {
	svc := &fakeService{
		targetsResp: api.TargetsEnvelope{
			SchemaVersion: "v1",
			Targets: []api.TargetResponse{
				{
					TargetID:      "tgt-1",
					TargetName:    "local",
					Kind:          "local",
					ConnectionRef: "",
					IsDefault:     true,
					Health:        "ok",
					UpdatedAt:     "2026-02-13T00:00:00Z",
				},
			},
		},
		sessionsResp: api.ListEnvelope[api.SessionItem]{
			SchemaVersion: "v1",
			Items: []api.SessionItem{
				{
					Identity:   api.SessionIdentity{Target: "local", SessionName: "s1"},
					TotalPanes: 1,
					ByState:    map[string]int{"running": 1},
					ByAgent:    map[string]int{"codex": 1},
				},
			},
		},
		windowsResp: api.ListEnvelope[api.WindowItem]{
			SchemaVersion: "v1",
			Items: []api.WindowItem{
				{
					Identity:     api.WindowIdentity{Target: "local", SessionName: "s1", WindowID: "@1"},
					TopState:     "running",
					RunningCount: 1,
					TotalPanes:   1,
				},
			},
		},
		panesResp: api.ListEnvelope[api.PaneItem]{
			SchemaVersion: "v1",
			Items: []api.PaneItem{
				{
					Identity:  api.PaneIdentity{Target: "local", SessionName: "s1", WindowID: "@1", PaneID: "%1"},
					State:     "running",
					UpdatedAt: "2026-02-13T00:00:00Z",
				},
			},
		},
	}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "snapshot", "--target", "local", "--json"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.listTargetsCalls != 1 || svc.listSessionsCalls != 1 || svc.listWindowsCalls != 1 || svc.listPanesCalls != 1 {
		t.Fatalf("unexpected list call counts targets=%d sessions=%d windows=%d panes=%d", svc.listTargetsCalls, svc.listSessionsCalls, svc.listWindowsCalls, svc.listPanesCalls)
	}
	if svc.listSessionsOpts.Target != "local" || svc.listWindowsOpts.Target != "local" || svc.listPanesOpts.Target != "local" {
		t.Fatalf("expected target filter propagated, got sessions=%q windows=%q panes=%q", svc.listSessionsOpts.Target, svc.listWindowsOpts.Target, svc.listPanesOpts.Target)
	}
	var snapshot snapshotViewEnvelope
	if err := json.Unmarshal(out.Bytes(), &snapshot); err != nil {
		t.Fatalf("decode snapshot view json: %v output=%s", err, out.String())
	}
	if snapshot.Target != "local" {
		t.Fatalf("expected snapshot target local, got %q", snapshot.Target)
	}
	if snapshot.Summary.TargetCount != 1 || snapshot.Summary.SessionCount != 1 || snapshot.Summary.WindowCount != 1 || snapshot.Summary.PaneCount != 1 {
		t.Fatalf("unexpected snapshot summary: %+v", snapshot.Summary)
	}
}

func TestRunViewSnapshotRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "snapshot", "extra"}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunViewSnapshotJSONWriteError(t *testing.T) {
	svc := &fakeService{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "snapshot", "--json"}, failWriter{err: errors.New("broken pipe")}, errOut, svc)
	if code != 1 {
		t.Fatalf("expected code 1, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("broken pipe")) {
		t.Fatalf("expected write error in stderr, got %s", errOut.String())
	}
}

func TestRunViewSnapshotServiceError(t *testing.T) {
	svc := &fakeService{listWindowsErr: errors.New("boom")}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "snapshot", "--json"}, out, errOut, svc)
	if code != 1 {
		t.Fatalf("expected code 1, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("boom")) {
		t.Fatalf("expected service error in stderr, got %s", errOut.String())
	}
}

func TestRunViewSnapshotFollowJSONRefreshesOnWatch(t *testing.T) {
	svc := &fakeService{
		watchLines: []api.WatchLine{
			{SchemaVersion: "v1", Scope: "panes", Type: "delta", Sequence: 2, Cursor: "stream:2"},
		},
		targetsResp: api.TargetsEnvelope{
			SchemaVersion: "v1",
			Targets: []api.TargetResponse{
				{
					TargetID:      "tgt-1",
					TargetName:    "local",
					Kind:          "local",
					ConnectionRef: "",
					IsDefault:     true,
					Health:        "ok",
					UpdatedAt:     "2026-02-13T00:00:00Z",
				},
			},
		},
		sessionsResp: api.ListEnvelope[api.SessionItem]{
			SchemaVersion: "v1",
			Items: []api.SessionItem{
				{
					Identity:   api.SessionIdentity{Target: "local", SessionName: "s1"},
					TotalPanes: 1,
					ByState:    map[string]int{"running": 1},
					ByAgent:    map[string]int{"codex": 1},
				},
			},
		},
		windowsResp: api.ListEnvelope[api.WindowItem]{
			SchemaVersion: "v1",
			Items: []api.WindowItem{
				{
					Identity:     api.WindowIdentity{Target: "local", SessionName: "s1", WindowID: "@1"},
					TopState:     "running",
					RunningCount: 1,
					TotalPanes:   1,
				},
			},
		},
		panesResp: api.ListEnvelope[api.PaneItem]{
			SchemaVersion: "v1",
			Items: []api.PaneItem{
				{
					Identity:  api.PaneIdentity{Target: "local", SessionName: "s1", WindowID: "@1", PaneID: "%1"},
					State:     "running",
					UpdatedAt: "2026-02-13T00:00:00Z",
				},
			},
		},
	}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "snapshot", "--target", "local", "--follow", "--json"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.watchCalls != 1 || svc.watchOpts.Scope != "panes" || svc.watchOpts.Target != "local" {
		t.Fatalf("unexpected watch usage: calls=%d opts=%+v", svc.watchCalls, svc.watchOpts)
	}
	if svc.listTargetsCalls != 2 || svc.listSessionsCalls != 2 || svc.listWindowsCalls != 2 || svc.listPanesCalls != 2 {
		t.Fatalf("expected initial+refresh list calls, got targets=%d sessions=%d windows=%d panes=%d", svc.listTargetsCalls, svc.listSessionsCalls, svc.listWindowsCalls, svc.listPanesCalls)
	}
	lines := bytes.Split(bytes.TrimSpace(out.Bytes()), []byte("\n"))
	if len(lines) != 2 {
		t.Fatalf("expected two JSON lines, got %d output=%s", len(lines), out.String())
	}
	for i, line := range lines {
		var snapshot snapshotViewEnvelope
		if err := json.Unmarshal(line, &snapshot); err != nil {
			t.Fatalf("line %d is not valid snapshot json: %v line=%s", i, err, string(line))
		}
	}
}

func TestRunViewGlobalRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "global", "--target", "t1", "extra"}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunViewTargetsJSON(t *testing.T) {
	svc := &fakeService{
		targetsResp: api.TargetsEnvelope{
			SchemaVersion: "v1",
			Targets: []api.TargetResponse{
				{
					TargetID:      "tgt-1",
					TargetName:    "local",
					Kind:          "local",
					ConnectionRef: "",
					IsDefault:     true,
					Health:        "ok",
					UpdatedAt:     "2026-02-13T00:00:00Z",
				},
			},
		},
	}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "targets", "--json"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.listTargetsCalls != 1 {
		t.Fatalf("expected list targets call once, got %d", svc.listTargetsCalls)
	}
	var env api.TargetsEnvelope
	if err := json.Unmarshal(out.Bytes(), &env); err != nil {
		t.Fatalf("decode targets view json: %v output=%s", err, out.String())
	}
	if len(env.Targets) != 1 || env.Targets[0].TargetName != "local" {
		t.Fatalf("unexpected targets payload: %+v", env.Targets)
	}
}

func TestRunViewTargetsHumanReadable(t *testing.T) {
	lastSeen := "2026-02-13T01:02:03Z"
	svc := &fakeService{
		targetsResp: api.TargetsEnvelope{
			SchemaVersion: "v1",
			Targets: []api.TargetResponse{
				{
					TargetID:      "tgt-1",
					TargetName:    "local",
					Kind:          "local",
					ConnectionRef: "",
					IsDefault:     true,
					LastSeenAt:    &lastSeen,
					Health:        "ok",
					UpdatedAt:     "2026-02-13T01:02:03Z",
				},
				{
					TargetID:      "tgt-2",
					TargetName:    "vm1",
					Kind:          "ssh",
					ConnectionRef: "ssh://vm1",
					IsDefault:     false,
					LastSeenAt:    nil,
					Health:        "degraded",
					UpdatedAt:     "2026-02-13T01:02:03Z",
				},
			},
		},
	}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "targets"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(out.Bytes(), []byte("local\tlocal\thealth=ok\tdefault=true\tlast_seen=2026-02-13T01:02:03Z")) {
		t.Fatalf("expected local target row, got %s", out.String())
	}
	if !bytes.Contains(out.Bytes(), []byte("vm1\tssh\thealth=degraded\tdefault=false\tlast_seen=-")) {
		t.Fatalf("expected vm target row with dash last_seen, got %s", out.String())
	}
}

func TestRunViewTargetsHumanReadableNoTargets(t *testing.T) {
	svc := &fakeService{targetsResp: api.TargetsEnvelope{SchemaVersion: "v1", Targets: []api.TargetResponse{}}}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "targets"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(out.Bytes(), []byte("no targets")) {
		t.Fatalf("expected no targets output, got %s", out.String())
	}
}

func TestRunViewTargetsRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "targets", "extra"}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunViewTargetsHumanWriteError(t *testing.T) {
	svc := &fakeService{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "targets"}, failWriter{err: errors.New("broken pipe")}, errOut, svc)
	if code != 1 {
		t.Fatalf("expected code 1, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("broken pipe")) {
		t.Fatalf("expected write error in stderr, got %s", errOut.String())
	}
}

func TestRunViewTargetsJSONWriteError(t *testing.T) {
	svc := &fakeService{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "targets", "--json"}, failWriter{err: errors.New("broken pipe")}, errOut, svc)
	if code != 1 {
		t.Fatalf("expected code 1, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("broken pipe")) {
		t.Fatalf("expected write error in stderr, got %s", errOut.String())
	}
}

func TestRunViewSessionsRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "sessions", "--target", "t1", "extra"}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunViewWindowsRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "windows", "--target", "t1", "extra"}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunViewPanesRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "panes", "--target", "t1", "extra"}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunViewGlobalHumanWriteError(t *testing.T) {
	svc := &fakeService{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "global"}, failWriter{err: errors.New("broken pipe")}, errOut, svc)
	if code != 1 {
		t.Fatalf("expected code 1, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("broken pipe")) {
		t.Fatalf("expected write error in stderr, got %s", errOut.String())
	}
}

func TestRunViewWindowsSessionFilterJSON(t *testing.T) {
	svc := &fakeService{
		windowsResp: api.ListEnvelope[api.WindowItem]{
			SchemaVersion: "v1",
			Items: []api.WindowItem{
				{
					Identity: api.WindowIdentity{
						Target:      "t1",
						SessionName: "s1",
						WindowID:    "@1",
					},
					TopState:     "running",
					RunningCount: 1,
					TotalPanes:   1,
				},
				{
					Identity: api.WindowIdentity{
						Target:      "t1",
						SessionName: "s2",
						WindowID:    "@2",
					},
					TopState:     "waiting_input",
					WaitingCount: 1,
					TotalPanes:   1,
				},
			},
		},
	}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "windows", "--session", "s2", "--json"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	var env api.ListEnvelope[api.WindowItem]
	if err := json.Unmarshal(out.Bytes(), &env); err != nil {
		t.Fatalf("decode windows view json: %v output=%s", err, out.String())
	}
	if len(env.Items) != 1 || env.Items[0].Identity.SessionName != "s2" {
		t.Fatalf("unexpected windows payload: %+v", env.Items)
	}
	if env.Filters["session"] != "s2" {
		t.Fatalf("expected session filter in json, got %+v", env.Filters)
	}
	if env.Summary.ByState["waiting_input"] != 1 || env.Summary.ByState["running"] != 0 {
		t.Fatalf("unexpected windows summary: %+v", env.Summary.ByState)
	}
}

func TestRunViewWindowsJSONWithoutSessionPreservesServerSummary(t *testing.T) {
	svc := &fakeService{
		windowsResp: api.ListEnvelope[api.WindowItem]{
			SchemaVersion: "v1",
			Summary: api.ListSummary{
				ByState:  map[string]int{"running": 2},
				ByAgent:  map[string]int{"codex": 2},
				ByTarget: map[string]int{"t1": 2},
			},
			Items: []api.WindowItem{
				{
					Identity: api.WindowIdentity{
						Target:      "t1",
						SessionName: "s1",
						WindowID:    "@1",
					},
					TopState:     "running",
					RunningCount: 1,
					TotalPanes:   1,
				},
				{
					Identity: api.WindowIdentity{
						Target:      "t1",
						SessionName: "s2",
						WindowID:    "@2",
					},
					TopState:     "running",
					RunningCount: 1,
					TotalPanes:   1,
				},
			},
		},
	}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "windows", "--json"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	var env api.ListEnvelope[api.WindowItem]
	if err := json.Unmarshal(out.Bytes(), &env); err != nil {
		t.Fatalf("decode windows view json: %v output=%s", err, out.String())
	}
	if env.Summary.ByAgent["codex"] != 2 {
		t.Fatalf("expected server by_agent summary to be preserved, got %+v", env.Summary.ByAgent)
	}
	if env.Summary.ByState["running"] != 2 {
		t.Fatalf("unexpected by_state summary: %+v", env.Summary.ByState)
	}
}

func TestRunViewPanesFiltersHumanReadable(t *testing.T) {
	svc := &fakeService{
		panesResp: api.ListEnvelope[api.PaneItem]{
			SchemaVersion: "v1",
			Items: []api.PaneItem{
				{
					Identity: api.PaneIdentity{
						Target:      "t1",
						SessionName: "s1",
						WindowID:    "@1",
						PaneID:      "%1",
					},
					State:     "running",
					AgentType: "codex",
					RuntimeID: "rt-1",
					UpdatedAt: "2026-02-13T00:00:00Z",
				},
				{
					Identity: api.PaneIdentity{
						Target:      "t1",
						SessionName: "s2",
						WindowID:    "@2",
						PaneID:      "%2",
					},
					State:      "waiting_input",
					ReasonCode: "await_user",
					AgentType:  "claude",
					RuntimeID:  "rt-2",
					UpdatedAt:  "2026-02-13T00:00:00Z",
				},
			},
		},
	}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "panes", "--session", "s2", "--state", "waiting_input"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(out.Bytes(), []byte("t1/s2/@2/%2")) {
		t.Fatalf("expected filtered pane in output, got %s", out.String())
	}
	if bytes.Contains(out.Bytes(), []byte("t1/s1/@1/%1")) {
		t.Fatalf("unexpected unfiltered pane in output, got %s", out.String())
	}
}

func TestRunViewPanesJSONFilterSummaryConsistency(t *testing.T) {
	svc := &fakeService{
		panesResp: api.ListEnvelope[api.PaneItem]{
			SchemaVersion: "v1",
			Summary: api.ListSummary{
				ByState:  map[string]int{"running": 1, "waiting_input": 1},
				ByAgent:  map[string]int{"codex": 1, "claude": 1},
				ByTarget: map[string]int{"t1": 2},
			},
			Items: []api.PaneItem{
				{
					Identity: api.PaneIdentity{
						Target:      "t1",
						SessionName: "s1",
						WindowID:    "@1",
						PaneID:      "%1",
					},
					State:     "running",
					AgentType: "codex",
					RuntimeID: "rt-1",
					UpdatedAt: "2026-02-13T00:00:00Z",
				},
				{
					Identity: api.PaneIdentity{
						Target:      "t1",
						SessionName: "s2",
						WindowID:    "@2",
						PaneID:      "%2",
					},
					State:      "waiting_input",
					ReasonCode: "await_user",
					AgentType:  "claude",
					RuntimeID:  "rt-2",
					UpdatedAt:  "2026-02-13T00:00:00Z",
				},
			},
		},
	}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"view", "panes",
		"--session", "s2",
		"--state", "waiting_input",
		"--json",
	}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	var env api.ListEnvelope[api.PaneItem]
	if err := json.Unmarshal(out.Bytes(), &env); err != nil {
		t.Fatalf("decode panes json: %v output=%s", err, out.String())
	}
	if len(env.Items) != 1 || env.Items[0].Identity.SessionName != "s2" {
		t.Fatalf("unexpected filtered panes: %+v", env.Items)
	}
	if env.Summary.ByState["waiting_input"] != 1 || env.Summary.ByState["running"] != 0 {
		t.Fatalf("unexpected filtered summary by_state: %+v", env.Summary.ByState)
	}
	if env.Filters["session"] != "s2" || env.Filters["state"] != "waiting_input" {
		t.Fatalf("unexpected filters: %+v", env.Filters)
	}
}

func TestRunViewPanesFollowJSONRefreshesOnWatch(t *testing.T) {
	svc := &fakeService{
		watchLines: []api.WatchLine{
			{SchemaVersion: "v1", Scope: "panes", Type: "delta", Sequence: 2, Cursor: "stream:2"},
		},
		panesResp: api.ListEnvelope[api.PaneItem]{
			SchemaVersion: "v1",
			Items: []api.PaneItem{
				{
					Identity: api.PaneIdentity{
						Target:      "t1",
						SessionName: "s1",
						WindowID:    "@1",
						PaneID:      "%1",
					},
					State:     "running",
					AgentType: "codex",
					RuntimeID: "rt-1",
					UpdatedAt: "2026-02-13T00:00:00Z",
				},
			},
		},
	}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "panes", "--target", "t1", "--follow", "--json"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.watchCalls != 1 || svc.watchOpts.Scope != "panes" || svc.watchOpts.Target != "t1" {
		t.Fatalf("unexpected watch usage: calls=%d opts=%+v", svc.watchCalls, svc.watchOpts)
	}
	if svc.listPanesCalls != 2 {
		t.Fatalf("expected 2 pane list calls (initial + refresh), got %d", svc.listPanesCalls)
	}
	lines := bytes.Split(bytes.TrimSpace(out.Bytes()), []byte("\n"))
	if len(lines) != 2 {
		t.Fatalf("expected 2 json lines, got %d output=%s", len(lines), out.String())
	}
}

func TestRunViewWindowsFollowUsesWindowScope(t *testing.T) {
	svc := &fakeService{
		watchLines: []api.WatchLine{
			{SchemaVersion: "v1", Scope: "windows", Type: "delta", Sequence: 2, Cursor: "stream:2"},
		},
		windowsResp: api.ListEnvelope[api.WindowItem]{
			SchemaVersion: "v1",
			Items: []api.WindowItem{
				{
					Identity: api.WindowIdentity{
						Target:      "t1",
						SessionName: "s1",
						WindowID:    "@1",
					},
					TopState:     "running",
					RunningCount: 1,
					TotalPanes:   1,
				},
			},
		},
	}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "windows", "--target", "t1", "--follow"}, out, errOut, svc)
	if code != 0 {
		t.Fatalf("expected code 0, got %d stderr=%s", code, errOut.String())
	}
	if svc.watchOpts.Scope != "windows" || svc.watchOpts.Target != "t1" {
		t.Fatalf("unexpected watch opts: %+v", svc.watchOpts)
	}
	if svc.listWindowsCalls != 2 {
		t.Fatalf("expected 2 windows list calls (initial + refresh), got %d", svc.listWindowsCalls)
	}
}

func TestRunViewWindowsFollowReturnsErrorOnInternalDeadlineExceeded(t *testing.T) {
	svc := &fakeService{
		watchErr: context.DeadlineExceeded,
		windowsResp: api.ListEnvelope[api.WindowItem]{
			SchemaVersion: "v1",
			Items: []api.WindowItem{
				{
					Identity: api.WindowIdentity{
						Target:      "t1",
						SessionName: "s1",
						WindowID:    "@1",
					},
					TopState:     "running",
					RunningCount: 1,
					TotalPanes:   1,
				},
			},
		},
	}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"view", "windows", "--target", "t1", "--follow"}, out, errOut, svc)
	if code != 1 {
		t.Fatalf("expected code 1, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("deadline exceeded")) {
		t.Fatalf("expected deadline error in stderr, got %s", errOut.String())
	}
}

func TestRunActionSendHumanWriteError(t *testing.T) {
	svc := &fakeService{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "send",
		"--request-ref", "req-1",
		"--target", "t1",
		"--pane", "%1",
		"--text", "hello",
	}, failWriter{err: errors.New("broken pipe")}, errOut, svc)
	if code != 1 {
		t.Fatalf("expected code 1, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("broken pipe")) {
		t.Fatalf("expected write error in stderr, got %s", errOut.String())
	}
}

func TestRunActionViewOutputRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "view-output",
		"--request-ref", "req-1",
		"--target", "t1",
		"--pane", "%1",
		"extra",
	}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunActionKillRejectsExtraArgs(t *testing.T) {
	svc := &fakeService{}
	out := &bytes.Buffer{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{
		"action", "kill",
		"--request-ref", "req-1",
		"--target", "t1",
		"--pane", "%1",
		"extra",
	}, out, errOut, svc)
	if code != 2 {
		t.Fatalf("expected code 2, got %d stderr=%s", code, errOut.String())
	}
}

func TestRunAdapterListHumanWriteError(t *testing.T) {
	svc := &fakeService{}
	errOut := &bytes.Buffer{}
	code := run(context.Background(), []string{"adapter", "list"}, failWriter{err: errors.New("broken pipe")}, errOut, svc)
	if code != 1 {
		t.Fatalf("expected code 1, got %d stderr=%s", code, errOut.String())
	}
	if !bytes.Contains(errOut.Bytes(), []byte("broken pipe")) {
		t.Fatalf("expected write error in stderr, got %s", errOut.String())
	}
}
