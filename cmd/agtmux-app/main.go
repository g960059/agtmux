package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"sort"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/g960059/agtmux/internal/api"
	"github.com/g960059/agtmux/internal/appclient"
	"github.com/g960059/agtmux/internal/config"
)

type service interface {
	WatchLoop(ctx context.Context, opts appclient.WatchLoopOptions, onLine func(api.WatchLine) error) error
	Attach(ctx context.Context, req appclient.AttachRequest) (api.ActionResponse, error)
	Send(ctx context.Context, req appclient.SendRequest) (api.ActionResponse, error)
	ViewOutput(ctx context.Context, req appclient.ViewOutputRequest) (api.ActionResponse, error)
	Kill(ctx context.Context, req appclient.KillRequest) (api.ActionResponse, error)
	ListActionEvents(ctx context.Context, actionID string) (api.ActionEventsEnvelope, error)
	ListCapabilities(ctx context.Context) (api.CapabilitiesEnvelope, error)
	TerminalRead(ctx context.Context, req appclient.TerminalReadRequest) (api.TerminalReadEnvelope, error)
	TerminalResize(ctx context.Context, req appclient.TerminalResizeRequest) (api.TerminalResizeResponse, error)
	TerminalAttach(ctx context.Context, req appclient.TerminalAttachRequest) (api.TerminalAttachResponse, error)
	TerminalDetach(ctx context.Context, req appclient.TerminalDetachRequest) (api.TerminalDetachResponse, error)
	TerminalWrite(ctx context.Context, req appclient.TerminalWriteRequest) (api.TerminalWriteResponse, error)
	TerminalStream(ctx context.Context, req appclient.TerminalStreamRequest) (api.TerminalStreamEnvelope, error)
	ListTargets(ctx context.Context) (api.TargetsEnvelope, error)
	CreateTarget(ctx context.Context, req appclient.CreateTargetRequest) (api.TargetsEnvelope, error)
	ConnectTarget(ctx context.Context, targetName string) (api.TargetsEnvelope, error)
	DeleteTarget(ctx context.Context, targetName string) error
	ListPanes(ctx context.Context, opts appclient.ListOptions) (api.ListEnvelope[api.PaneItem], error)
	ListWindows(ctx context.Context, opts appclient.ListOptions) (api.ListEnvelope[api.WindowItem], error)
	ListSessions(ctx context.Context, opts appclient.ListOptions) (api.ListEnvelope[api.SessionItem], error)
	ListAdapters(ctx context.Context, enabled *bool) (api.AdaptersEnvelope, error)
	SetAdapterEnabled(ctx context.Context, adapterName string, enabled bool) (api.AdaptersEnvelope, error)
}

type snapshotViewSummary struct {
	TargetCount  int  `json:"target_count"`
	SessionCount int  `json:"session_count"`
	WindowCount  int  `json:"window_count"`
	PaneCount    int  `json:"pane_count"`
	Partial      bool `json:"partial"`
}

type snapshotViewEnvelope struct {
	SchemaVersion string                            `json:"schema_version"`
	GeneratedAt   time.Time                         `json:"generated_at"`
	Target        string                            `json:"target,omitempty"`
	Targets       []api.TargetResponse              `json:"targets"`
	Sessions      api.ListEnvelope[api.SessionItem] `json:"sessions"`
	Windows       api.ListEnvelope[api.WindowItem]  `json:"windows"`
	Panes         api.ListEnvelope[api.PaneItem]    `json:"panes"`
	Summary       snapshotViewSummary               `json:"summary"`
}

const maxSendStdinBytes int64 = 1 << 20

func main() {
	ctx, cancel := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer cancel()

	cfg := config.DefaultConfig()
	socketPath, unaryTimeout, rest, err := parseGlobalArgsWithTimeout(os.Args[1:], cfg.SocketPath)
	if err != nil {
		_, _ = fmt.Fprintf(os.Stderr, "error: %v\n", err)
		os.Exit(2)
	}
	client := appclient.New(socketPath)
	if unaryTimeout != nil {
		client = client.WithUnaryTimeout(*unaryTimeout)
	}
	os.Exit(run(ctx, rest, os.Stdout, os.Stderr, client))
}

func parseGlobalArgs(args []string, defaultSocket string) (string, []string, error) {
	socketPath, _, rest, err := parseGlobalArgsWithTimeout(args, defaultSocket)
	return socketPath, rest, err
}

func parseGlobalArgsWithTimeout(args []string, defaultSocket string) (string, *time.Duration, []string, error) {
	socket := defaultSocket
	var unaryTimeout *time.Duration
	i := 0
	for i < len(args) {
		if args[i] == "--socket" {
			if i+1 >= len(args) {
				return "", nil, nil, fmt.Errorf("--socket requires value")
			}
			socket = args[i+1]
			if strings.TrimSpace(socket) == "" {
				return "", nil, nil, fmt.Errorf("--socket requires value")
			}
			i += 2
			continue
		}
		if strings.HasPrefix(args[i], "--socket=") {
			socket = strings.TrimPrefix(args[i], "--socket=")
			if strings.TrimSpace(socket) == "" {
				return "", nil, nil, fmt.Errorf("--socket requires value")
			}
			i++
			continue
		}
		if args[i] == "--request-timeout" {
			if i+1 >= len(args) {
				return "", nil, nil, fmt.Errorf("--request-timeout requires value")
			}
			timeout, err := parseUnaryTimeout(args[i+1])
			if err != nil {
				return "", nil, nil, err
			}
			unaryTimeout = &timeout
			i += 2
			continue
		}
		if strings.HasPrefix(args[i], "--request-timeout=") {
			timeout, err := parseUnaryTimeout(strings.TrimPrefix(args[i], "--request-timeout="))
			if err != nil {
				return "", nil, nil, err
			}
			unaryTimeout = &timeout
			i++
			continue
		}
		break
	}
	return socket, unaryTimeout, args[i:], nil
}

func parseUnaryTimeout(raw string) (time.Duration, error) {
	value := strings.TrimSpace(raw)
	if value == "" {
		return 0, fmt.Errorf("--request-timeout requires value")
	}
	timeout, err := time.ParseDuration(value)
	if err != nil {
		return 0, fmt.Errorf("--request-timeout must be a valid non-negative duration")
	}
	if timeout < 0 {
		return 0, fmt.Errorf("--request-timeout must be a valid non-negative duration")
	}
	return timeout, nil
}

func run(ctx context.Context, args []string, out, errOut io.Writer, svc service) int {
	if out == nil {
		out = os.Stdout
	}
	if errOut == nil {
		errOut = os.Stderr
	}
	if len(args) == 0 || args[0] == "run" || strings.HasPrefix(args[0], "-") {
		rest := args
		if len(rest) > 0 && rest[0] == "run" {
			rest = rest[1:]
		}
		return runResident(ctx, rest, out, errOut, svc)
	}

	switch args[0] {
	case "action":
		return runAction(ctx, args[1:], out, errOut, svc)
	case "terminal":
		return runTerminal(ctx, args[1:], out, errOut, svc)
	case "view":
		return runView(ctx, args[1:], out, errOut, svc)
	case "target":
		return runTarget(ctx, args[1:], out, errOut, svc)
	case "adapter":
		return runAdapter(ctx, args[1:], out, errOut, svc)
	default:
		printUsage(errOut)
		return 2
	}
}

func runResident(ctx context.Context, args []string, out, errOut io.Writer, svc service) int {
	fs := flag.NewFlagSet("run", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	scope := fs.String("scope", "panes", "watch scope: panes|windows|sessions")
	target := fs.String("target", "", "target name")
	pollInterval := fs.Duration("poll-interval", 2*time.Second, "poll interval")
	retryMin := fs.Duration("retry-min-backoff", 250*time.Millisecond, "minimum retry backoff")
	retryMax := fs.Duration("retry-max-backoff", 4*time.Second, "maximum retry backoff")
	cursor := fs.String("cursor", "", "watch cursor")
	once := fs.Bool("once", false, "run a single watch cycle")
	jsonOut := fs.Bool("json", false, "emit watch lines as JSONL")
	if err := fs.Parse(args); err != nil {
		_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
		return 2
	}
	if fs.NArg() > 0 {
		_, _ = fmt.Fprintln(errOut, "usage: agtmux-app [--socket <path>] [--request-timeout <duration>] [run [--scope ...] [--once] [--json]]")
		return 2
	}
	if *scope != "panes" && *scope != "windows" && *scope != "sessions" {
		_, _ = fmt.Fprintf(errOut, "error: invalid scope %q\n", *scope)
		return 2
	}

	err := svc.WatchLoop(ctx, appclient.WatchLoopOptions{
		Scope:           *scope,
		Target:          strings.TrimSpace(*target),
		Cursor:          strings.TrimSpace(*cursor),
		PollInterval:    *pollInterval,
		RetryMinBackoff: *retryMin,
		RetryMaxBackoff: *retryMax,
		Once:            *once,
	}, func(line api.WatchLine) error {
		if *jsonOut {
			return printJSONLine(out, line)
		}
		_, err := fmt.Fprintf(out, "%s scope=%s type=%s seq=%d cursor=%s by_state=%s\n",
			time.Now().UTC().Format(time.RFC3339Nano),
			line.Scope,
			line.Type,
			line.Sequence,
			line.Cursor,
			formatMap(line.Summary.ByState),
		)
		return err
	})
	if err != nil {
		if ctx.Err() != nil && (errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded)) {
			return 0
		}
		_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
		return 1
	}
	return 0
}

func runAction(ctx context.Context, args []string, out, errOut io.Writer, svc service) int {
	if len(args) == 0 {
		_, _ = fmt.Fprintln(errOut, "usage: agtmux-app action <attach|send|view-output|kill|events> ...")
		return 2
	}
	switch args[0] {
	case "attach":
		fs := flag.NewFlagSet("action attach", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		requestRef := fs.String("request-ref", "", "idempotency key")
		target := fs.String("target", "", "target name")
		paneID := fs.String("pane", "", "pane id")
		ifRuntime := fs.String("if-runtime", "", "runtime guard")
		ifState := fs.String("if-state", "", "state guard")
		ifUpdatedWithin := fs.String("if-updated-within", "", "freshness guard duration")
		forceStale := fs.Bool("force-stale", false, "disable stale guard")
		jsonOut := fs.Bool("json", false, "output JSON (JSONL when --follow)")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app action attach --request-ref <id> --target <name> --pane <id>")
			return 2
		}
		if strings.TrimSpace(*requestRef) == "" || strings.TrimSpace(*target) == "" || strings.TrimSpace(*paneID) == "" {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app action attach --request-ref <id> --target <name> --pane <id>")
			return 2
		}
		resp, err := svc.Attach(ctx, appclient.AttachRequest{
			RequestRef:      strings.TrimSpace(*requestRef),
			Target:          strings.TrimSpace(*target),
			PaneID:          strings.TrimSpace(*paneID),
			IfRuntime:       strings.TrimSpace(*ifRuntime),
			IfState:         strings.TrimSpace(*ifState),
			IfUpdatedWithin: strings.TrimSpace(*ifUpdatedWithin),
			ForceStale:      *forceStale,
		})
		return writeActionResponse(out, errOut, resp, err, *jsonOut, "attach")
	case "send":
		fs := flag.NewFlagSet("action send", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		requestRef := fs.String("request-ref", "", "idempotency key")
		target := fs.String("target", "", "target name")
		paneID := fs.String("pane", "", "pane id")
		text := fs.String("text", "", "text payload")
		stdinPayload := fs.Bool("stdin", false, "read text payload from stdin")
		key := fs.String("key", "", "key token")
		enter := fs.Bool("enter", false, "append enter")
		paste := fs.Bool("paste", false, "send literal text")
		ifRuntime := fs.String("if-runtime", "", "runtime guard")
		ifState := fs.String("if-state", "", "state guard")
		ifUpdatedWithin := fs.String("if-updated-within", "", "freshness guard duration")
		forceStale := fs.Bool("force-stale", false, "disable stale guard")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app action send --request-ref <id> --target <name> --pane <id> (--text <text>|--key <key>|--stdin)")
			return 2
		}
		if strings.TrimSpace(*requestRef) == "" || strings.TrimSpace(*target) == "" || strings.TrimSpace(*paneID) == "" {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app action send --request-ref <id> --target <name> --pane <id> (--text <text>|--key <key>|--stdin)")
			return 2
		}
		hasText := false
		hasKey := false
		fs.Visit(func(f *flag.Flag) {
			switch f.Name {
			case "text":
				hasText = true
			case "key":
				hasKey = true
			}
		})
		hasStdin := *stdinPayload
		modeCount := 0
		if hasText {
			modeCount++
		}
		if hasKey {
			modeCount++
		}
		if hasStdin {
			modeCount++
		}
		if modeCount != 1 {
			_, _ = fmt.Fprintln(errOut, "error: exactly one of --text, --key, or --stdin is required")
			return 2
		}
		if hasKey && strings.TrimSpace(*key) == "" {
			_, _ = fmt.Fprintln(errOut, "error: --key requires a non-empty value")
			return 2
		}
		if hasStdin {
			payload, usageErr, err := readSendStdinPayload(os.Stdin, maxSendStdinBytes)
			if err != nil {
				_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
				if usageErr {
					return 2
				}
				return 1
			}
			*text = payload
		}
		resp, err := svc.Send(ctx, appclient.SendRequest{
			RequestRef:      strings.TrimSpace(*requestRef),
			Target:          strings.TrimSpace(*target),
			PaneID:          strings.TrimSpace(*paneID),
			Text:            *text,
			Key:             strings.TrimSpace(*key),
			Enter:           *enter,
			Paste:           *paste,
			IfRuntime:       strings.TrimSpace(*ifRuntime),
			IfState:         strings.TrimSpace(*ifState),
			IfUpdatedWithin: strings.TrimSpace(*ifUpdatedWithin),
			ForceStale:      *forceStale,
		})
		return writeActionResponse(out, errOut, resp, err, *jsonOut, "send")
	case "view-output":
		fs := flag.NewFlagSet("action view-output", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		requestRef := fs.String("request-ref", "", "idempotency key")
		target := fs.String("target", "", "target name")
		paneID := fs.String("pane", "", "pane id")
		lines := fs.Int("lines", 200, "line count")
		ifRuntime := fs.String("if-runtime", "", "runtime guard")
		ifState := fs.String("if-state", "", "state guard")
		ifUpdatedWithin := fs.String("if-updated-within", "", "freshness guard duration")
		forceStale := fs.Bool("force-stale", false, "disable stale guard")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app action view-output --request-ref <id> --target <name> --pane <id> [--lines <n>]")
			return 2
		}
		if strings.TrimSpace(*requestRef) == "" || strings.TrimSpace(*target) == "" || strings.TrimSpace(*paneID) == "" {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app action view-output --request-ref <id> --target <name> --pane <id> [--lines <n>]")
			return 2
		}
		resp, err := svc.ViewOutput(ctx, appclient.ViewOutputRequest{
			RequestRef:      strings.TrimSpace(*requestRef),
			Target:          strings.TrimSpace(*target),
			PaneID:          strings.TrimSpace(*paneID),
			Lines:           *lines,
			IfRuntime:       strings.TrimSpace(*ifRuntime),
			IfState:         strings.TrimSpace(*ifState),
			IfUpdatedWithin: strings.TrimSpace(*ifUpdatedWithin),
			ForceStale:      *forceStale,
		})
		return writeActionResponse(out, errOut, resp, err, *jsonOut, "view-output")
	case "kill":
		fs := flag.NewFlagSet("action kill", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		requestRef := fs.String("request-ref", "", "idempotency key")
		target := fs.String("target", "", "target name")
		paneID := fs.String("pane", "", "pane id")
		mode := fs.String("mode", "key", "kill mode key|signal")
		signalName := fs.String("signal", "INT", "signal INT|TERM|KILL")
		ifRuntime := fs.String("if-runtime", "", "runtime guard")
		ifState := fs.String("if-state", "", "state guard")
		ifUpdatedWithin := fs.String("if-updated-within", "", "freshness guard duration")
		forceStale := fs.Bool("force-stale", false, "disable stale guard")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app action kill --request-ref <id> --target <name> --pane <id> [--mode key|signal] [--signal INT|TERM|KILL]")
			return 2
		}
		if strings.TrimSpace(*requestRef) == "" || strings.TrimSpace(*target) == "" || strings.TrimSpace(*paneID) == "" {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app action kill --request-ref <id> --target <name> --pane <id> [--mode key|signal] [--signal INT|TERM|KILL]")
			return 2
		}
		resp, err := svc.Kill(ctx, appclient.KillRequest{
			RequestRef:      strings.TrimSpace(*requestRef),
			Target:          strings.TrimSpace(*target),
			PaneID:          strings.TrimSpace(*paneID),
			Mode:            strings.TrimSpace(*mode),
			Signal:          strings.TrimSpace(*signalName),
			IfRuntime:       strings.TrimSpace(*ifRuntime),
			IfState:         strings.TrimSpace(*ifState),
			IfUpdatedWithin: strings.TrimSpace(*ifUpdatedWithin),
			ForceStale:      *forceStale,
		})
		return writeActionResponse(out, errOut, resp, err, *jsonOut, "kill")
	case "events":
		fs := flag.NewFlagSet("action events", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		actionID := fs.String("action-id", "", "action id")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app action events --action-id <id> [--json]")
			return 2
		}
		if strings.TrimSpace(*actionID) == "" {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app action events --action-id <id> [--json]")
			return 2
		}
		resp, err := svc.ListActionEvents(ctx, strings.TrimSpace(*actionID))
		if err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if *jsonOut {
			return writeJSONResponse(out, errOut, resp)
		}
		if _, err := fmt.Fprintf(out, "action_id=%s events=%d\n", resp.ActionID, len(resp.Events)); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		for _, event := range resp.Events {
			if _, err := fmt.Fprintf(out, "event=%s type=%s source=%s runtime=%s event_time=%s\n",
				event.EventID,
				event.EventType,
				event.Source,
				valueOrDash(event.RuntimeID),
				event.EventTime,
			); err != nil {
				_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
				return 1
			}
		}
		return 0
	default:
		_, _ = fmt.Fprintf(errOut, "unknown action command: %s\n", args[0])
		return 2
	}
}

func runTerminal(ctx context.Context, args []string, out, errOut io.Writer, svc service) int {
	if len(args) == 0 {
		_, _ = fmt.Fprintln(errOut, "usage: agtmux-app terminal <capabilities|attach|detach|write|stream|read|resize> ...")
		return 2
	}
	switch args[0] {
	case "capabilities":
		fs := flag.NewFlagSet("terminal capabilities", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app terminal capabilities [--json]")
			return 2
		}
		resp, err := svc.ListCapabilities(ctx)
		if err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if *jsonOut {
			return writeJSONResponse(out, errOut, resp)
		}
		if _, err := fmt.Fprintf(out, "embedded_terminal=%t terminal_read=%t terminal_resize=%t write_via_send=%t terminal_attach=%t terminal_write=%t terminal_stream=%t proxy_mode=%s protocol=%s\n",
			resp.Capabilities.EmbeddedTerminal,
			resp.Capabilities.TerminalRead,
			resp.Capabilities.TerminalResize,
			resp.Capabilities.TerminalWriteViaAction,
			resp.Capabilities.TerminalAttach,
			resp.Capabilities.TerminalWrite,
			resp.Capabilities.TerminalStream,
			resp.Capabilities.TerminalProxyMode,
			resp.Capabilities.TerminalFrameProtocol,
		); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		return 0
	case "attach":
		fs := flag.NewFlagSet("terminal attach", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		target := fs.String("target", "", "target name")
		paneID := fs.String("pane", "", "pane id")
		ifRuntime := fs.String("if-runtime", "", "runtime guard")
		ifState := fs.String("if-state", "", "state guard")
		ifUpdatedWithin := fs.String("if-updated-within", "", "freshness guard duration")
		forceStale := fs.Bool("force-stale", false, "disable stale guard")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app terminal attach --target <name> --pane <id> [--if-runtime <id>] [--if-state <state>] [--if-updated-within <duration>] [--force-stale] [--json]")
			return 2
		}
		if strings.TrimSpace(*target) == "" || strings.TrimSpace(*paneID) == "" {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app terminal attach --target <name> --pane <id> [--if-runtime <id>] [--if-state <state>] [--if-updated-within <duration>] [--force-stale] [--json]")
			return 2
		}
		resp, err := svc.TerminalAttach(ctx, appclient.TerminalAttachRequest{
			Target:          strings.TrimSpace(*target),
			PaneID:          strings.TrimSpace(*paneID),
			IfRuntime:       strings.TrimSpace(*ifRuntime),
			IfState:         strings.TrimSpace(*ifState),
			IfUpdatedWithin: strings.TrimSpace(*ifUpdatedWithin),
			ForceStale:      *forceStale,
		})
		if err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if *jsonOut {
			return writeJSONResponse(out, errOut, resp)
		}
		if _, err := fmt.Fprintf(out, "terminal attach session=%s target=%s pane=%s result=%s\n",
			resp.SessionID,
			resp.Target,
			resp.PaneID,
			resp.ResultCode,
		); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		return 0
	case "detach":
		fs := flag.NewFlagSet("terminal detach", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		sessionID := fs.String("session", "", "terminal session id")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app terminal detach --session <id> [--json]")
			return 2
		}
		if strings.TrimSpace(*sessionID) == "" {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app terminal detach --session <id> [--json]")
			return 2
		}
		resp, err := svc.TerminalDetach(ctx, appclient.TerminalDetachRequest{
			SessionID: strings.TrimSpace(*sessionID),
		})
		if err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if *jsonOut {
			return writeJSONResponse(out, errOut, resp)
		}
		if _, err := fmt.Fprintf(out, "terminal detach session=%s result=%s\n", resp.SessionID, resp.ResultCode); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		return 0
	case "write":
		fs := flag.NewFlagSet("terminal write", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		sessionID := fs.String("session", "", "terminal session id")
		text := fs.String("text", "", "text payload")
		key := fs.String("key", "", "tmux key payload")
		bytesB64 := fs.String("bytes-b64", "", "raw bytes payload in base64")
		enter := fs.Bool("enter", false, "append Enter key")
		paste := fs.Bool("paste", false, "send as literal text")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app terminal write --session <id> [--text <payload> | --key <tmux-key> | --bytes-b64 <payload>] [--enter] [--paste] [--json]")
			return 2
		}
		textVal := *text
		keyVal := strings.TrimSpace(*key)
		bytesVal := strings.TrimSpace(*bytesB64)
		modeCount := 0
		if textVal != "" {
			modeCount++
		}
		if keyVal != "" {
			modeCount++
		}
		if bytesVal != "" {
			modeCount++
		}
		if strings.TrimSpace(*sessionID) == "" || modeCount != 1 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app terminal write --session <id> [--text <payload> | --key <tmux-key> | --bytes-b64 <payload>] [--enter] [--paste] [--json]")
			return 2
		}
		resp, err := svc.TerminalWrite(ctx, appclient.TerminalWriteRequest{
			SessionID: strings.TrimSpace(*sessionID),
			Text:      textVal,
			Key:       keyVal,
			BytesB64:  bytesVal,
			Enter:     *enter,
			Paste:     *paste,
		})
		if err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if *jsonOut {
			return writeJSONResponse(out, errOut, resp)
		}
		if _, err := fmt.Fprintf(out, "terminal write session=%s result=%s\n", resp.SessionID, resp.ResultCode); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		return 0
	case "stream":
		fs := flag.NewFlagSet("terminal stream", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		sessionID := fs.String("session", "", "terminal session id")
		cursor := fs.String("cursor", "", "terminal cursor")
		lines := fs.Int("lines", 200, "line count")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app terminal stream --session <id> [--cursor <cursor>] [--lines <n>] [--json]")
			return 2
		}
		if strings.TrimSpace(*sessionID) == "" || *lines <= 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app terminal stream --session <id> [--cursor <cursor>] [--lines <n>] [--json]")
			return 2
		}
		resp, err := svc.TerminalStream(ctx, appclient.TerminalStreamRequest{
			SessionID: strings.TrimSpace(*sessionID),
			Cursor:    strings.TrimSpace(*cursor),
			Lines:     *lines,
		})
		if err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if *jsonOut {
			return writeJSONResponse(out, errOut, resp)
		}
		if _, err := fmt.Fprintf(out, "terminal stream frame=%s cursor=%s stream=%s session=%s\n",
			resp.Frame.FrameType,
			resp.Frame.Cursor,
			resp.Frame.StreamID,
			resp.Frame.SessionID,
		); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		return 0
	case "read":
		fs := flag.NewFlagSet("terminal read", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		target := fs.String("target", "", "target name")
		paneID := fs.String("pane", "", "pane id")
		cursor := fs.String("cursor", "", "terminal cursor")
		lines := fs.Int("lines", 200, "line count")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app terminal read --target <name> --pane <id> [--cursor <cursor>] [--lines <n>] [--json]")
			return 2
		}
		if strings.TrimSpace(*target) == "" || strings.TrimSpace(*paneID) == "" {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app terminal read --target <name> --pane <id> [--cursor <cursor>] [--lines <n>] [--json]")
			return 2
		}
		resp, err := svc.TerminalRead(ctx, appclient.TerminalReadRequest{
			Target: strings.TrimSpace(*target),
			PaneID: strings.TrimSpace(*paneID),
			Cursor: strings.TrimSpace(*cursor),
			Lines:  *lines,
		})
		if err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if *jsonOut {
			return writeJSONResponse(out, errOut, resp)
		}
		if _, err := fmt.Fprintf(out, "terminal frame=%s cursor=%s stream=%s lines=%d target=%s pane=%s\n",
			resp.Frame.FrameType,
			resp.Frame.Cursor,
			resp.Frame.StreamID,
			resp.Frame.Lines,
			resp.Frame.Target,
			resp.Frame.PaneID,
		); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		return 0
	case "resize":
		fs := flag.NewFlagSet("terminal resize", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		target := fs.String("target", "", "target name")
		paneID := fs.String("pane", "", "pane id")
		cols := fs.Int("cols", 0, "column count")
		rows := fs.Int("rows", 0, "row count")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app terminal resize --target <name> --pane <id> --cols <n> --rows <n> [--json]")
			return 2
		}
		if strings.TrimSpace(*target) == "" || strings.TrimSpace(*paneID) == "" || *cols <= 0 || *rows <= 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app terminal resize --target <name> --pane <id> --cols <n> --rows <n> [--json]")
			return 2
		}
		resp, err := svc.TerminalResize(ctx, appclient.TerminalResizeRequest{
			Target: strings.TrimSpace(*target),
			PaneID: strings.TrimSpace(*paneID),
			Cols:   *cols,
			Rows:   *rows,
		})
		if err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if *jsonOut {
			return writeJSONResponse(out, errOut, resp)
		}
		if _, err := fmt.Fprintf(out, "terminal resize %s/%s cols=%d rows=%d result=%s\n",
			resp.Target,
			resp.PaneID,
			resp.Cols,
			resp.Rows,
			resp.ResultCode,
		); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		return 0
	default:
		_, _ = fmt.Fprintf(errOut, "unknown terminal command: %s\n", args[0])
		return 2
	}
}

func runView(ctx context.Context, args []string, out, errOut io.Writer, svc service) int {
	if len(args) == 0 {
		_, _ = fmt.Fprintln(errOut, "usage: agtmux-app view <snapshot|global|sessions|windows|panes|targets> ...")
		return 2
	}
	switch args[0] {
	case "snapshot":
		fs := flag.NewFlagSet("view snapshot", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		target := fs.String("target", "", "target name")
		follow := fs.Bool("follow", false, "keep updating view")
		cursor := fs.String("cursor", "", "watch cursor")
		pollInterval := fs.Duration("poll-interval", 2*time.Second, "poll interval")
		retryMin := fs.Duration("retry-min-backoff", 250*time.Millisecond, "minimum retry backoff")
		retryMax := fs.Duration("retry-max-backoff", 4*time.Second, "maximum retry backoff")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app view snapshot [--target <name>] [--follow] [--json]")
			return 2
		}
		targetFilter := strings.TrimSpace(*target)
		render := func() error {
			targets, err := svc.ListTargets(ctx)
			if err != nil {
				return err
			}
			sessions, err := svc.ListSessions(ctx, appclient.ListOptions{Target: targetFilter})
			if err != nil {
				return err
			}
			windows, err := svc.ListWindows(ctx, appclient.ListOptions{Target: targetFilter})
			if err != nil {
				return err
			}
			panes, err := svc.ListPanes(ctx, appclient.ListOptions{Target: targetFilter})
			if err != nil {
				return err
			}
			snapshot := snapshotViewEnvelope{
				SchemaVersion: "v1",
				GeneratedAt:   time.Now().UTC(),
				Target:        targetFilter,
				Targets:       targets.Targets,
				Sessions:      sessions,
				Windows:       windows,
				Panes:         panes,
				Summary: snapshotViewSummary{
					TargetCount:  len(targets.Targets),
					SessionCount: len(sessions.Items),
					WindowCount:  len(windows.Items),
					PaneCount:    len(panes.Items),
					Partial:      sessions.Partial || windows.Partial || panes.Partial,
				},
			}
			if *jsonOut {
				return printJSONLine(out, snapshot)
			}
			_, err = fmt.Fprintf(out, "snapshot targets=%d sessions=%d windows=%d panes=%d partial=%t\n",
				snapshot.Summary.TargetCount,
				snapshot.Summary.SessionCount,
				snapshot.Summary.WindowCount,
				snapshot.Summary.PaneCount,
				snapshot.Summary.Partial,
			)
			return err
		}
		if err := render(); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if !*follow {
			return 0
		}
		return runFollowLoop(
			ctx,
			errOut,
			svc,
			"panes",
			targetFilter,
			strings.TrimSpace(*cursor),
			*pollInterval,
			*retryMin,
			*retryMax,
			render,
		)
	case "targets":
		fs := flag.NewFlagSet("view targets", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app view targets [--json]")
			return 2
		}
		resp, err := svc.ListTargets(ctx)
		if err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if *jsonOut {
			return writeJSONResponse(out, errOut, resp)
		}
		if len(resp.Targets) == 0 {
			if _, err := fmt.Fprintln(out, "no targets"); err != nil {
				_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
				return 1
			}
			return 0
		}
		for _, target := range resp.Targets {
			lastSeen := "-"
			if target.LastSeenAt != nil && strings.TrimSpace(*target.LastSeenAt) != "" {
				lastSeen = strings.TrimSpace(*target.LastSeenAt)
			}
			if _, err := fmt.Fprintf(out, "%s\t%s\thealth=%s\tdefault=%t\tlast_seen=%s\n",
				target.TargetName,
				target.Kind,
				target.Health,
				target.IsDefault,
				lastSeen,
			); err != nil {
				_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
				return 1
			}
		}
		return 0
	case "global":
		fs := flag.NewFlagSet("view global", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		target := fs.String("target", "", "target name")
		follow := fs.Bool("follow", false, "keep updating view")
		cursor := fs.String("cursor", "", "watch cursor")
		pollInterval := fs.Duration("poll-interval", 2*time.Second, "poll interval")
		retryMin := fs.Duration("retry-min-backoff", 250*time.Millisecond, "minimum retry backoff")
		retryMax := fs.Duration("retry-max-backoff", 4*time.Second, "maximum retry backoff")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app view global [--target <name>] [--follow] [--json]")
			return 2
		}
		render := func() error {
			resp, err := svc.ListPanes(ctx, appclient.ListOptions{Target: strings.TrimSpace(*target)})
			if err != nil {
				return err
			}
			if *jsonOut {
				return printJSONLine(out, resp)
			}
			if _, err := fmt.Fprintf(out, "global panes=%d partial=%t requested=%d responded=%d\n",
				len(resp.Items),
				resp.Partial,
				len(resp.RequestedTargets),
				len(resp.RespondedTargets),
			); err != nil {
				return err
			}
			if _, err := fmt.Fprintf(out, "by_state=%s by_agent=%s by_target=%s\n",
				formatMap(resp.Summary.ByState),
				formatMap(resp.Summary.ByAgent),
				formatMap(resp.Summary.ByTarget),
			); err != nil {
				return err
			}
			for _, te := range resp.TargetErrors {
				if _, err := fmt.Fprintf(out, "target_error target=%s code=%s msg=%s\n", te.Target, te.Code, te.Message); err != nil {
					return err
				}
			}
			return nil
		}
		if err := render(); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if !*follow {
			return 0
		}
		return runFollowLoop(
			ctx,
			errOut,
			svc,
			"panes",
			strings.TrimSpace(*target),
			strings.TrimSpace(*cursor),
			*pollInterval,
			*retryMin,
			*retryMax,
			render,
		)
	case "sessions":
		fs := flag.NewFlagSet("view sessions", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		target := fs.String("target", "", "target name")
		follow := fs.Bool("follow", false, "keep updating view")
		cursor := fs.String("cursor", "", "watch cursor")
		pollInterval := fs.Duration("poll-interval", 2*time.Second, "poll interval")
		retryMin := fs.Duration("retry-min-backoff", 250*time.Millisecond, "minimum retry backoff")
		retryMax := fs.Duration("retry-max-backoff", 4*time.Second, "maximum retry backoff")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app view sessions [--target <name>] [--follow] [--json]")
			return 2
		}
		render := func() error {
			resp, err := svc.ListSessions(ctx, appclient.ListOptions{Target: strings.TrimSpace(*target)})
			if err != nil {
				return err
			}
			if *jsonOut {
				return printJSONLine(out, resp)
			}
			if len(resp.Items) == 0 {
				_, err := fmt.Fprintln(out, "no sessions")
				return err
			}
			for _, item := range resp.Items {
				if _, err := fmt.Fprintf(out, "%s/%s panes=%d by_state=%s by_agent=%s\n",
					item.Identity.Target,
					item.Identity.SessionName,
					item.TotalPanes,
					formatMap(item.ByState),
					formatMap(item.ByAgent),
				); err != nil {
					return err
				}
			}
			return nil
		}
		if err := render(); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if !*follow {
			return 0
		}
		return runFollowLoop(
			ctx,
			errOut,
			svc,
			"sessions",
			strings.TrimSpace(*target),
			strings.TrimSpace(*cursor),
			*pollInterval,
			*retryMin,
			*retryMax,
			render,
		)
	case "windows":
		fs := flag.NewFlagSet("view windows", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		target := fs.String("target", "", "target name")
		session := fs.String("session", "", "session name filter")
		follow := fs.Bool("follow", false, "keep updating view")
		cursor := fs.String("cursor", "", "watch cursor")
		pollInterval := fs.Duration("poll-interval", 2*time.Second, "poll interval")
		retryMin := fs.Duration("retry-min-backoff", 250*time.Millisecond, "minimum retry backoff")
		retryMax := fs.Duration("retry-max-backoff", 4*time.Second, "maximum retry backoff")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app view windows [--target <name>] [--session <name>] [--follow] [--json]")
			return 2
		}
		render := func() error {
			resp, err := svc.ListWindows(ctx, appclient.ListOptions{Target: strings.TrimSpace(*target)})
			if err != nil {
				return err
			}
			sessionFilter := strings.TrimSpace(*session)
			if sessionFilter != "" {
				resp.Items = filterWindowItems(resp.Items, sessionFilter)
				resp.Filters = addStringFilter(resp.Filters, "session", sessionFilter)
				resp.Summary = summarizeWindows(resp.Items)
			}
			if *jsonOut {
				return printJSONLine(out, resp)
			}
			if len(resp.Items) == 0 {
				_, err := fmt.Fprintln(out, "no windows")
				return err
			}
			for _, item := range resp.Items {
				if _, err := fmt.Fprintf(out, "%s/%s/%s panes=%d running=%d waiting=%d top=%s\n",
					item.Identity.Target,
					item.Identity.SessionName,
					item.Identity.WindowID,
					item.TotalPanes,
					item.RunningCount,
					item.WaitingCount,
					item.TopState,
				); err != nil {
					return err
				}
			}
			return nil
		}
		if err := render(); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if !*follow {
			return 0
		}
		return runFollowLoop(
			ctx,
			errOut,
			svc,
			"windows",
			strings.TrimSpace(*target),
			strings.TrimSpace(*cursor),
			*pollInterval,
			*retryMin,
			*retryMax,
			render,
		)
	case "panes":
		fs := flag.NewFlagSet("view panes", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		target := fs.String("target", "", "target name")
		session := fs.String("session", "", "session name filter")
		window := fs.String("window", "", "window id filter")
		state := fs.String("state", "", "state filter")
		follow := fs.Bool("follow", false, "keep updating view")
		cursor := fs.String("cursor", "", "watch cursor")
		pollInterval := fs.Duration("poll-interval", 2*time.Second, "poll interval")
		retryMin := fs.Duration("retry-min-backoff", 250*time.Millisecond, "minimum retry backoff")
		retryMax := fs.Duration("retry-max-backoff", 4*time.Second, "maximum retry backoff")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app view panes [--target <name>] [--session <name>] [--window <id>] [--state <state>] [--follow] [--json]")
			return 2
		}
		render := func() error {
			resp, err := svc.ListPanes(ctx, appclient.ListOptions{Target: strings.TrimSpace(*target)})
			if err != nil {
				return err
			}
			resp.Items = filterPaneItems(
				resp.Items,
				strings.TrimSpace(*session),
				strings.TrimSpace(*window),
				strings.TrimSpace(*state),
			)
			resp.Filters = addStringFilter(resp.Filters, "session", strings.TrimSpace(*session))
			resp.Filters = addStringFilter(resp.Filters, "window", strings.TrimSpace(*window))
			resp.Filters = addStringFilter(resp.Filters, "state", strings.TrimSpace(*state))
			resp.Summary = summarizePanes(resp.Items)
			if *jsonOut {
				return printJSONLine(out, resp)
			}
			if len(resp.Items) == 0 {
				_, err := fmt.Fprintln(out, "no panes")
				return err
			}
			for _, item := range resp.Items {
				if _, err := fmt.Fprintf(out, "%s/%s/%s/%s state=%s agent=%s runtime=%s age=%s reason=%s\n",
					item.Identity.Target,
					item.Identity.SessionName,
					item.Identity.WindowID,
					item.Identity.PaneID,
					item.State,
					valueOrDash(item.AgentType),
					valueOrDash(item.RuntimeID),
					formatAge(item.UpdatedAt),
					valueOrDash(item.ReasonCode),
				); err != nil {
					return err
				}
			}
			return nil
		}
		if err := render(); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if !*follow {
			return 0
		}
		return runFollowLoop(
			ctx,
			errOut,
			svc,
			"panes",
			strings.TrimSpace(*target),
			strings.TrimSpace(*cursor),
			*pollInterval,
			*retryMin,
			*retryMax,
			render,
		)
	default:
		_, _ = fmt.Fprintf(errOut, "unknown view command: %s\n", args[0])
		return 2
	}
}

func runTarget(ctx context.Context, args []string, out, errOut io.Writer, svc service) int {
	if len(args) == 0 {
		_, _ = fmt.Fprintln(errOut, "usage: agtmux-app target <list|add|connect|remove> ...")
		return 2
	}
	switch args[0] {
	case "list":
		fs := flag.NewFlagSet("target list", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app target list [--json]")
			return 2
		}
		resp, err := svc.ListTargets(ctx)
		if err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if *jsonOut {
			return writeJSONResponse(out, errOut, resp)
		}
		if len(resp.Targets) == 0 {
			if _, err := fmt.Fprintln(out, "no targets"); err != nil {
				_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
				return 1
			}
			return 0
		}
		for _, target := range resp.Targets {
			lastSeen := "-"
			if target.LastSeenAt != nil && strings.TrimSpace(*target.LastSeenAt) != "" {
				lastSeen = strings.TrimSpace(*target.LastSeenAt)
			}
			if _, err := fmt.Fprintf(out, "%s\t%s\thealth=%s\tdefault=%t\tlast_seen=%s\n",
				target.TargetName,
				target.Kind,
				target.Health,
				target.IsDefault,
				lastSeen,
			); err != nil {
				_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
				return 1
			}
		}
		return 0
	case "add":
		fs := flag.NewFlagSet("target add", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		kind := fs.String("kind", "local", "target kind")
		connectionRef := fs.String("connection-ref", "", "connection ref")
		isDefault := fs.Bool("default", false, "set default target")
		jsonOut := fs.Bool("json", false, "output JSON")
		rest := args[1:]
		name := ""
		if len(rest) > 0 && !strings.HasPrefix(rest[0], "-") {
			name = rest[0]
			rest = rest[1:]
		}
		if err := fs.Parse(rest); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if name == "" {
			if fs.NArg() > 0 {
				name = fs.Arg(0)
			}
			if fs.NArg() > 1 {
				_, _ = fmt.Fprintln(errOut, "usage: agtmux-app target add <name> [--kind <local|ssh>] [--connection-ref <ref>] [--default] [--json]")
				return 2
			}
		} else if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app target add <name> [--kind <local|ssh>] [--connection-ref <ref>] [--default] [--json]")
			return 2
		}
		name = strings.TrimSpace(name)
		if name == "" {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app target add <name> [--kind <local|ssh>] [--connection-ref <ref>] [--default] [--json]")
			return 2
		}
		resp, err := svc.CreateTarget(ctx, appclient.CreateTargetRequest{
			Name:          name,
			Kind:          strings.TrimSpace(*kind),
			ConnectionRef: strings.TrimSpace(*connectionRef),
			IsDefault:     *isDefault,
		})
		if err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if *jsonOut {
			return writeJSONResponse(out, errOut, resp)
		}
		if len(resp.Targets) == 0 {
			if _, err := fmt.Fprintf(out, "added target %s\n", name); err != nil {
				_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
				return 1
			}
			return 0
		}
		if _, err := fmt.Fprintf(out, "added target %s (%s)\n", resp.Targets[0].TargetName, resp.Targets[0].Kind); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		return 0
	case "connect":
		fs := flag.NewFlagSet("target connect", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		jsonOut := fs.Bool("json", false, "output JSON")
		rest := args[1:]
		name := ""
		if len(rest) > 0 && !strings.HasPrefix(rest[0], "-") {
			name = rest[0]
			rest = rest[1:]
		}
		if err := fs.Parse(rest); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if name == "" {
			if fs.NArg() > 0 {
				name = fs.Arg(0)
			}
			if fs.NArg() > 1 {
				_, _ = fmt.Fprintln(errOut, "usage: agtmux-app target connect <name> [--json]")
				return 2
			}
		} else if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app target connect <name> [--json]")
			return 2
		}
		name = strings.TrimSpace(name)
		if name == "" {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app target connect <name> [--json]")
			return 2
		}
		resp, err := svc.ConnectTarget(ctx, name)
		if err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if *jsonOut {
			return writeJSONResponse(out, errOut, resp)
		}
		if len(resp.Targets) == 0 {
			if _, err := fmt.Fprintf(out, "connected target %s\n", name); err != nil {
				_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
				return 1
			}
			return 0
		}
		if _, err := fmt.Fprintf(out, "connected target %s\n", resp.Targets[0].TargetName); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		return 0
	case "remove":
		fs := flag.NewFlagSet("target remove", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		jsonOut := fs.Bool("json", false, "output JSON")
		rest := args[1:]
		name := ""
		if len(rest) > 0 && !strings.HasPrefix(rest[0], "-") {
			name = rest[0]
			rest = rest[1:]
		}
		if err := fs.Parse(rest); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if name == "" {
			if fs.NArg() > 0 {
				name = fs.Arg(0)
			}
			if fs.NArg() > 1 {
				_, _ = fmt.Fprintln(errOut, "usage: agtmux-app target remove <name> [--json]")
				return 2
			}
		} else if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app target remove <name> [--json]")
			return 2
		}
		name = strings.TrimSpace(name)
		if name == "" {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app target remove <name> [--json]")
			return 2
		}
		if err := svc.DeleteTarget(ctx, name); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if *jsonOut {
			return writeJSONResponse(out, errOut, map[string]any{
				"target":  name,
				"removed": true,
			})
		}
		if _, err := fmt.Fprintf(out, "removed target %s\n", name); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		return 0
	default:
		_, _ = fmt.Fprintf(errOut, "unknown target command: %s\n", args[0])
		return 2
	}
}

func runAdapter(ctx context.Context, args []string, out, errOut io.Writer, svc service) int {
	if len(args) == 0 {
		_, _ = fmt.Fprintln(errOut, "usage: agtmux-app adapter <list|enable|disable>")
		return 2
	}
	switch args[0] {
	case "list":
		fs := flag.NewFlagSet("adapter list", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		enabledFlag := fs.String("enabled", "", "true|false")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(errOut, "usage: agtmux-app adapter list [--enabled true|false] [--json]")
			return 2
		}
		var enabled *bool
		if strings.TrimSpace(*enabledFlag) != "" {
			v, err := strconv.ParseBool(strings.TrimSpace(*enabledFlag))
			if err != nil {
				_, _ = fmt.Fprintln(errOut, "error: --enabled must be true or false")
				return 2
			}
			enabled = &v
		}
		resp, err := svc.ListAdapters(ctx, enabled)
		if err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if *jsonOut {
			return writeJSONResponse(out, errOut, resp)
		}
		for _, a := range resp.Adapters {
			state := "disabled"
			if a.Enabled {
				state = "enabled"
			}
			compat := "incompatible"
			if a.Compatible {
				compat = "compatible"
			}
			if _, err := fmt.Fprintf(out, "%s\t%s\t%s\t%s\t%s\n", a.AdapterName, a.AgentType, a.Version, state, compat); err != nil {
				_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
				return 1
			}
		}
		return 0
	case "enable", "disable":
		fs := flag.NewFlagSet("adapter "+args[0], flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		jsonOut := fs.Bool("json", false, "output JSON")
		rest := args[1:]
		name := ""
		if len(rest) > 0 && !strings.HasPrefix(rest[0], "-") {
			name = rest[0]
			rest = rest[1:]
		}
		if err := fs.Parse(rest); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 2
		}
		if name == "" {
			if fs.NArg() > 0 {
				name = fs.Arg(0)
			}
			if fs.NArg() > 1 {
				_, _ = fmt.Fprintf(errOut, "usage: agtmux-app adapter %s <name>\n", args[0])
				return 2
			}
		} else if fs.NArg() > 0 {
			_, _ = fmt.Fprintf(errOut, "usage: agtmux-app adapter %s <name>\n", args[0])
			return 2
		}
		name = strings.TrimSpace(name)
		if name == "" {
			_, _ = fmt.Fprintf(errOut, "usage: agtmux-app adapter %s <name>\n", args[0])
			return 2
		}
		enable := args[0] == "enable"
		resp, err := svc.SetAdapterEnabled(ctx, name, enable)
		if err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if *jsonOut {
			return writeJSONResponse(out, errOut, resp)
		}
		if len(resp.Adapters) == 0 {
			if _, err := fmt.Fprintf(out, "%s adapter %s\n", args[0], name); err != nil {
				_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
				return 1
			}
			return 0
		}
		state := "disabled"
		if resp.Adapters[0].Enabled {
			state = "enabled"
		}
		if _, err := fmt.Fprintf(out, "%s adapter %s (%s)\n", args[0], resp.Adapters[0].AdapterName, state); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		return 0
	default:
		_, _ = fmt.Fprintf(errOut, "unknown adapter command: %s\n", args[0])
		return 2
	}
}

func writeActionResponse(out, errOut io.Writer, resp api.ActionResponse, err error, jsonOut bool, verb string) int {
	if err != nil {
		_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
		return 1
	}
	if jsonOut {
		return writeJSONResponse(out, errOut, resp)
	}
	if verb == "view-output" && resp.Output != nil {
		if _, err := fmt.Fprint(out, *resp.Output); err != nil {
			_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
			return 1
		}
		if !strings.HasSuffix(*resp.Output, "\n") {
			if _, err := fmt.Fprintln(out); err != nil {
				_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
				return 1
			}
		}
		return 0
	}
	if _, err := fmt.Fprintf(out, "%s action %s: %s\n", verb, resp.ActionID, resp.ResultCode); err != nil {
		_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
		return 1
	}
	return 0
}

func printUsage(w io.Writer) {
	_, _ = fmt.Fprintln(w, "usage: agtmux-app [--socket <path>] [--request-timeout <duration>] [run [--scope ...] [--once] [--json]]")
	_, _ = fmt.Fprintln(w, "       agtmux-app view <snapshot|global|sessions|windows|panes|targets> ...")
	_, _ = fmt.Fprintln(w, "       agtmux-app target <list|add|connect|remove> ...")
	_, _ = fmt.Fprintln(w, "       agtmux-app action <attach|send|view-output|kill|events> ...")
	_, _ = fmt.Fprintln(w, "       agtmux-app terminal <capabilities|attach|detach|write|stream|read|resize> ...")
	_, _ = fmt.Fprintln(w, "       agtmux-app adapter <list|enable|disable> ...")
}

func writeJSONResponse(out, errOut io.Writer, payload any) int {
	if err := printJSONLine(out, payload); err != nil {
		_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
		return 1
	}
	return 0
}

func printJSONLine(out io.Writer, payload any) error {
	b, err := json.Marshal(payload)
	if err != nil {
		return err
	}
	_, err = fmt.Fprintln(out, string(b))
	return err
}

func readSendStdinPayload(stdin *os.File, maxBytes int64) (payload string, usageError bool, err error) {
	if stdin == nil {
		return "", false, fmt.Errorf("stdin unavailable")
	}
	stat, err := stdin.Stat()
	if err != nil {
		return "", false, fmt.Errorf("read stdin: %w", err)
	}
	if stat.Mode()&os.ModeCharDevice != 0 {
		return "", true, fmt.Errorf("--stdin requires piped input")
	}
	body, err := io.ReadAll(io.LimitReader(stdin, maxBytes+1))
	if err != nil {
		return "", false, fmt.Errorf("read stdin: %w", err)
	}
	if int64(len(body)) > maxBytes {
		return "", true, fmt.Errorf("--stdin payload exceeds %d bytes", maxBytes)
	}
	if len(body) == 0 {
		return "", true, fmt.Errorf("--stdin requires non-empty payload")
	}
	return string(body), false, nil
}

func runFollowLoop(
	ctx context.Context,
	errOut io.Writer,
	svc service,
	scope, target, cursor string,
	pollInterval, retryMin, retryMax time.Duration,
	onUpdate func() error,
) int {
	err := svc.WatchLoop(ctx, appclient.WatchLoopOptions{
		Scope:           scope,
		Target:          target,
		Cursor:          cursor,
		PollInterval:    pollInterval,
		RetryMinBackoff: retryMin,
		RetryMaxBackoff: retryMax,
	}, func(_ api.WatchLine) error {
		return onUpdate()
	})
	if err != nil {
		if ctx.Err() != nil && (errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded)) {
			return 0
		}
		_, _ = fmt.Fprintf(errOut, "error: %v\n", err)
		return 1
	}
	return 0
}

func filterWindowItems(items []api.WindowItem, sessionName string) []api.WindowItem {
	session := strings.TrimSpace(sessionName)
	if session == "" {
		return items
	}
	out := make([]api.WindowItem, 0, len(items))
	for _, item := range items {
		if item.Identity.SessionName == session {
			out = append(out, item)
		}
	}
	return out
}

func filterPaneItems(items []api.PaneItem, sessionName, windowID, state string) []api.PaneItem {
	session := strings.TrimSpace(sessionName)
	window := strings.TrimSpace(windowID)
	stateFilter := strings.ToLower(strings.TrimSpace(state))
	out := make([]api.PaneItem, 0, len(items))
	for _, item := range items {
		if session != "" && item.Identity.SessionName != session {
			continue
		}
		if window != "" && item.Identity.WindowID != window {
			continue
		}
		if stateFilter != "" && strings.ToLower(item.State) != stateFilter {
			continue
		}
		out = append(out, item)
	}
	return out
}

func addStringFilter(filters map[string]any, key, value string) map[string]any {
	if strings.TrimSpace(value) == "" {
		return filters
	}
	out := map[string]any{}
	for k, v := range filters {
		out[k] = v
	}
	out[key] = value
	return out
}

func summarizePanes(items []api.PaneItem) api.ListSummary {
	summary := api.ListSummary{
		ByState:  map[string]int{},
		ByAgent:  map[string]int{},
		ByTarget: map[string]int{},
	}
	for _, item := range items {
		agentType := strings.TrimSpace(item.AgentType)
		if agentType == "" {
			agentType = "unknown"
		}
		summary.ByState[item.State]++
		summary.ByAgent[agentType]++
		summary.ByTarget[item.Identity.Target]++
	}
	return summary
}

func summarizeWindows(items []api.WindowItem) api.ListSummary {
	summary := api.ListSummary{
		ByState:  map[string]int{},
		ByAgent:  map[string]int{},
		ByTarget: map[string]int{},
	}
	for _, item := range items {
		summary.ByState[item.TopState]++
		summary.ByTarget[item.Identity.Target]++
	}
	return summary
}

func formatAge(updatedAt string) string {
	ts, err := time.Parse(time.RFC3339Nano, strings.TrimSpace(updatedAt))
	if err != nil {
		return "unknown"
	}
	age := time.Since(ts)
	if age < 0 {
		age = 0
	}
	return age.Round(time.Second).String()
}

func valueOrDash(v string) string {
	if strings.TrimSpace(v) == "" {
		return "-"
	}
	return v
}

func formatMap(m map[string]int) string {
	if len(m) == 0 {
		return "{}"
	}
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	parts := make([]string, 0, len(keys))
	for _, k := range keys {
		parts = append(parts, fmt.Sprintf("%s=%d", k, m[k]))
	}
	return "{" + strings.Join(parts, ",") + "}"
}
