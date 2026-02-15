package cli

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"time"

	"github.com/g960059/agtmux/internal/api"
	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/integration"
)

type Runner struct {
	baseURL string
	client  *http.Client
	out     io.Writer
	errOut  io.Writer
}

const maxSendStdinBytes int64 = 1 << 20

func NewRunner(socketPath string, out, errOut io.Writer) *Runner {
	transport := &http.Transport{
		DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
			var d net.Dialer
			return d.DialContext(ctx, "unix", socketPath)
		},
	}
	return NewRunnerWithClient("http://unix", &http.Client{Transport: transport}, out, errOut)
}

func NewRunnerWithClient(baseURL string, client *http.Client, out, errOut io.Writer) *Runner {
	if out == nil {
		out = os.Stdout
	}
	if errOut == nil {
		errOut = os.Stderr
	}
	if client == nil {
		client = &http.Client{}
	}
	return &Runner{
		baseURL: strings.TrimRight(baseURL, "/"),
		client:  client,
		out:     out,
		errOut:  errOut,
	}
}

func (r *Runner) Run(ctx context.Context, args []string) int {
	socketPath, rest, err := parseGlobalArgs(args)
	if err != nil {
		_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
		return 2
	}
	if socketPath != "" && r.baseURL == "http://unix" {
		*r = *NewRunner(socketPath, r.out, r.errOut)
	}
	if len(rest) == 0 {
		r.printUsage()
		return 2
	}
	switch rest[0] {
	case "target":
		return r.runTarget(ctx, rest[1:])
	case "adapter":
		return r.runAdapter(ctx, rest[1:])
	case "event":
		return r.runEvent(ctx, rest[1:])
	case "integration":
		return r.runIntegration(ctx, rest[1:])
	case "list":
		return r.runList(ctx, rest[1:])
	case "watch":
		return r.runWatch(ctx, rest[1:])
	case "send":
		return r.runSend(ctx, rest[1:])
	case "view-output":
		return r.runViewOutput(ctx, rest[1:])
	case "kill":
		return r.runKill(ctx, rest[1:])
	case "events":
		return r.runActionEvents(ctx, rest[1:])
	case "app":
		return r.runAppCommand(ctx, rest[1:], socketPath)
	default:
		_, _ = fmt.Fprintf(r.errOut, "unknown command: %s\n", rest[0])
		r.printUsage()
		return 2
	}
}

func (r *Runner) runAdapter(ctx context.Context, args []string) int {
	if len(args) == 0 {
		_, _ = fmt.Fprintln(r.errOut, "usage: agtmux adapter <list|enable|disable>")
		return 2
	}
	switch args[0] {
	case "list":
		fs := flag.NewFlagSet("adapter list", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
			return 2
		}
		body, err := r.request(ctx, http.MethodGet, "/v1/adapters", nil, nil)
		if err != nil {
			return r.handleErr(err)
		}
		if *jsonOut {
			_, _ = r.out.Write(body)
			_, _ = fmt.Fprintln(r.out)
			return 0
		}
		var env api.AdaptersEnvelope
		if err := json.Unmarshal(body, &env); err != nil {
			return r.handleErr(err)
		}
		for _, a := range env.Adapters {
			enabled := "disabled"
			if a.Enabled {
				enabled = "enabled"
			}
			compat := "incompatible"
			if a.Compatible {
				compat = "compatible"
			}
			_, _ = fmt.Fprintf(r.out, "%s\t%s\t%s\t%s\t%s\n", a.AdapterName, a.AgentType, a.Version, enabled, compat)
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
			_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
			return 2
		}
		if name == "" && fs.NArg() > 0 {
			name = fs.Arg(0)
		}
		name = strings.TrimSpace(name)
		if name == "" {
			_, _ = fmt.Fprintf(r.errOut, "usage: agtmux adapter %s <name>\n", args[0])
			return 2
		}
		path := "/v1/adapters/" + url.PathEscape(name) + "/" + args[0]
		body, err := r.request(ctx, http.MethodPost, path, nil, nil)
		if err != nil {
			return r.handleErr(err)
		}
		if *jsonOut {
			_, _ = r.out.Write(body)
			_, _ = fmt.Fprintln(r.out)
			return 0
		}
		var env api.AdaptersEnvelope
		if err := json.Unmarshal(body, &env); err != nil {
			return r.handleErr(err)
		}
		if len(env.Adapters) == 0 {
			_, _ = fmt.Fprintf(r.out, "%s adapter %s\n", args[0], name)
			return 0
		}
		state := "disabled"
		if env.Adapters[0].Enabled {
			state = "enabled"
		}
		_, _ = fmt.Fprintf(r.out, "%s adapter %s (%s)\n", args[0], env.Adapters[0].AdapterName, state)
		return 0
	default:
		_, _ = fmt.Fprintf(r.errOut, "unknown adapter command: %s\n", args[0])
		return 2
	}
}

func parseGlobalArgs(args []string) (string, []string, error) {
	socket := config.DefaultConfig().SocketPath
	rest := make([]string, 0, len(args))
	for i := 0; i < len(args); i++ {
		if args[i] == "--socket" {
			if i+1 >= len(args) {
				return "", nil, fmt.Errorf("--socket requires value")
			}
			socket = args[i+1]
			i++
			continue
		}
		rest = append(rest, args[i])
	}
	return socket, rest, nil
}

func (r *Runner) runTarget(ctx context.Context, args []string) int {
	if len(args) == 0 {
		_, _ = fmt.Fprintln(r.errOut, "usage: agtmux target <list|add|connect|remove>")
		return 2
	}
	switch args[0] {
	case "list":
		fs := flag.NewFlagSet("target list", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
			return 2
		}
		body, err := r.request(ctx, http.MethodGet, "/v1/targets", nil, nil)
		if err != nil {
			return r.handleErr(err)
		}
		if *jsonOut {
			_, _ = r.out.Write(body)
			_, _ = fmt.Fprintln(r.out)
			return 0
		}
		var env api.TargetsEnvelope
		if err := json.Unmarshal(body, &env); err != nil {
			return r.handleErr(err)
		}
		for _, t := range env.Targets {
			_, _ = fmt.Fprintf(r.out, "%s\t%s\t%s\n", t.TargetName, t.Kind, t.Health)
		}
		return 0
	case "add":
		fs := flag.NewFlagSet("target add", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		kind := fs.String("kind", "local", "target kind")
		connRef := fs.String("connection-ref", "", "connection ref")
		isDefault := fs.Bool("default", false, "set default target")
		jsonOut := fs.Bool("json", false, "output JSON")
		rest := args[1:]
		name := ""
		if len(rest) > 0 && !strings.HasPrefix(rest[0], "-") {
			name = rest[0]
			rest = rest[1:]
		}
		if err := fs.Parse(rest); err != nil {
			_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
			return 2
		}
		if name == "" && fs.NArg() > 0 {
			name = fs.Arg(0)
		}
		if name == "" {
			_, _ = fmt.Fprintln(r.errOut, "usage: agtmux target add <name> [--kind ...]")
			return 2
		}
		req := map[string]any{
			"name":           name,
			"kind":           *kind,
			"connection_ref": *connRef,
			"is_default":     *isDefault,
		}
		body, err := r.request(ctx, http.MethodPost, "/v1/targets", nil, req)
		if err != nil {
			return r.handleErr(err)
		}
		if *jsonOut {
			_, _ = r.out.Write(body)
			_, _ = fmt.Fprintln(r.out)
			return 0
		}
		_, _ = fmt.Fprintf(r.out, "added target %s\n", name)
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
			_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
			return 2
		}
		if name == "" && fs.NArg() > 0 {
			name = fs.Arg(0)
		}
		if name == "" {
			_, _ = fmt.Fprintln(r.errOut, "usage: agtmux target connect <name>")
			return 2
		}
		path := "/v1/targets/" + url.PathEscape(name) + "/connect"
		body, err := r.request(ctx, http.MethodPost, path, nil, nil)
		if err != nil {
			return r.handleErr(err)
		}
		if *jsonOut {
			_, _ = r.out.Write(body)
			_, _ = fmt.Fprintln(r.out)
			return 0
		}
		_, _ = fmt.Fprintf(r.out, "connected target %s\n", name)
		return 0
	case "remove":
		fs := flag.NewFlagSet("target remove", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		rest := args[1:]
		name := ""
		if len(rest) > 0 && !strings.HasPrefix(rest[0], "-") {
			name = rest[0]
			rest = rest[1:]
		}
		if err := fs.Parse(rest); err != nil {
			_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
			return 2
		}
		if name == "" && fs.NArg() > 0 {
			name = fs.Arg(0)
		}
		if name == "" {
			_, _ = fmt.Fprintln(r.errOut, "usage: agtmux target remove <name>")
			return 2
		}
		path := "/v1/targets/" + url.PathEscape(name)
		if _, err := r.request(ctx, http.MethodDelete, path, nil, nil); err != nil {
			return r.handleErr(err)
		}
		_, _ = fmt.Fprintf(r.out, "removed target %s\n", name)
		return 0
	default:
		_, _ = fmt.Fprintf(r.errOut, "unknown target command: %s\n", args[0])
		return 2
	}
}

func (r *Runner) runList(ctx context.Context, args []string) int {
	if len(args) == 0 {
		_, _ = fmt.Fprintln(r.errOut, "usage: agtmux list <panes|windows|sessions> [--target ...] [--json]")
		return 2
	}
	scope := args[0]
	if scope != "panes" && scope != "windows" && scope != "sessions" {
		_, _ = fmt.Fprintf(r.errOut, "invalid list scope: %s\n", scope)
		return 2
	}
	fs := flag.NewFlagSet("list "+scope, flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	target := fs.String("target", "", "target name")
	jsonOut := fs.Bool("json", false, "output JSON")
	if err := fs.Parse(args[1:]); err != nil {
		_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
		return 2
	}
	query := url.Values{}
	if strings.TrimSpace(*target) != "" {
		query.Set("target", strings.TrimSpace(*target))
	}
	body, err := r.request(ctx, http.MethodGet, "/v1/"+scope, query, nil)
	if err != nil {
		return r.handleErr(err)
	}
	if *jsonOut {
		_, _ = r.out.Write(body)
		_, _ = fmt.Fprintln(r.out)
		return 0
	}
	var env map[string]any
	if err := json.Unmarshal(body, &env); err != nil {
		return r.handleErr(err)
	}
	items, _ := env["items"].([]any)
	_, _ = fmt.Fprintf(r.out, "%s: %d items\n", scope, len(items))
	return 0
}

func (r *Runner) runWatch(ctx context.Context, args []string) int {
	fs := flag.NewFlagSet("watch", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	scope := fs.String("scope", "panes", "watch scope")
	target := fs.String("target", "", "target name")
	cursor := fs.String("cursor", "", "watch cursor")
	jsonOut := fs.Bool("json", false, "output jsonl")
	once := fs.Bool("once", false, "single call")
	if err := fs.Parse(args); err != nil {
		_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
		return 2
	}
	_ = jsonOut
	_ = once
	query := url.Values{}
	query.Set("scope", *scope)
	if strings.TrimSpace(*target) != "" {
		query.Set("target", strings.TrimSpace(*target))
	}
	if strings.TrimSpace(*cursor) != "" {
		query.Set("cursor", strings.TrimSpace(*cursor))
	}
	body, err := r.request(ctx, http.MethodGet, "/v1/watch", query, nil)
	if err != nil {
		return r.handleErr(err)
	}
	_, _ = r.out.Write(body)
	if !bytes.HasSuffix(body, []byte("\n")) {
		_, _ = fmt.Fprintln(r.out)
	}
	return 0
}

func (r *Runner) runSend(ctx context.Context, args []string) int {
	fs := flag.NewFlagSet("send", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	requestRef := fs.String("request-ref", "", "idempotency key")
	targetName := fs.String("target", "", "target name")
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
	if err := fs.Parse(args); err != nil {
		_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
		return 2
	}
	if strings.TrimSpace(*requestRef) == "" || strings.TrimSpace(*targetName) == "" || strings.TrimSpace(*paneID) == "" {
		_, _ = fmt.Fprintln(r.errOut, "usage: agtmux send --request-ref <id> --target <name> --pane <id> (--text <text>|--key <key>|--stdin)")
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
		_, _ = fmt.Fprintln(r.errOut, "error: exactly one of --text, --key, or --stdin is required")
		return 2
	}
	if hasKey && strings.TrimSpace(*key) == "" {
		_, _ = fmt.Fprintln(r.errOut, "error: --key requires a non-empty value")
		return 2
	}
	if hasStdin {
		payload, usageErr, err := readSendStdinPayload(os.Stdin, maxSendStdinBytes)
		if err != nil {
			_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
			if usageErr {
				return 2
			}
			return 1
		}
		*text = payload
	}
	req := map[string]any{
		"request_ref":       strings.TrimSpace(*requestRef),
		"target":            strings.TrimSpace(*targetName),
		"pane_id":           strings.TrimSpace(*paneID),
		"text":              *text,
		"key":               strings.TrimSpace(*key),
		"enter":             *enter,
		"paste":             *paste,
		"if_runtime":        strings.TrimSpace(*ifRuntime),
		"if_state":          strings.TrimSpace(*ifState),
		"if_updated_within": strings.TrimSpace(*ifUpdatedWithin),
		"force_stale":       *forceStale,
	}
	body, err := r.request(ctx, http.MethodPost, "/v1/actions/send", nil, req)
	if err != nil {
		return r.handleErr(err)
	}
	if *jsonOut {
		_, _ = r.out.Write(body)
		_, _ = fmt.Fprintln(r.out)
		return 0
	}
	var resp api.ActionResponse
	if err := json.Unmarshal(body, &resp); err != nil {
		return r.handleErr(err)
	}
	_, _ = fmt.Fprintf(r.out, "send action %s: %s\n", resp.ActionID, resp.ResultCode)
	return 0
}

func (r *Runner) runViewOutput(ctx context.Context, args []string) int {
	fs := flag.NewFlagSet("view-output", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	requestRef := fs.String("request-ref", "", "idempotency key")
	targetName := fs.String("target", "", "target name")
	paneID := fs.String("pane", "", "pane id")
	lines := fs.Int("lines", 200, "line count")
	ifRuntime := fs.String("if-runtime", "", "runtime guard")
	ifState := fs.String("if-state", "", "state guard")
	ifUpdatedWithin := fs.String("if-updated-within", "", "freshness guard duration")
	forceStale := fs.Bool("force-stale", false, "disable stale guard")
	jsonOut := fs.Bool("json", false, "output JSON")
	if err := fs.Parse(args); err != nil {
		_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
		return 2
	}
	if strings.TrimSpace(*requestRef) == "" || strings.TrimSpace(*targetName) == "" || strings.TrimSpace(*paneID) == "" {
		_, _ = fmt.Fprintln(r.errOut, "usage: agtmux view-output --request-ref <id> --target <name> --pane <id> [--lines <n>]")
		return 2
	}
	req := map[string]any{
		"request_ref":       strings.TrimSpace(*requestRef),
		"target":            strings.TrimSpace(*targetName),
		"pane_id":           strings.TrimSpace(*paneID),
		"lines":             *lines,
		"if_runtime":        strings.TrimSpace(*ifRuntime),
		"if_state":          strings.TrimSpace(*ifState),
		"if_updated_within": strings.TrimSpace(*ifUpdatedWithin),
		"force_stale":       *forceStale,
	}
	body, err := r.request(ctx, http.MethodPost, "/v1/actions/view-output", nil, req)
	if err != nil {
		return r.handleErr(err)
	}
	if *jsonOut {
		_, _ = r.out.Write(body)
		_, _ = fmt.Fprintln(r.out)
		return 0
	}
	var resp api.ActionResponse
	if err := json.Unmarshal(body, &resp); err != nil {
		return r.handleErr(err)
	}
	if resp.Output != nil {
		_, _ = fmt.Fprint(r.out, *resp.Output)
		if !strings.HasSuffix(*resp.Output, "\n") {
			_, _ = fmt.Fprintln(r.out)
		}
		return 0
	}
	_, _ = fmt.Fprintf(r.out, "view-output action %s: %s\n", resp.ActionID, resp.ResultCode)
	return 0
}

func (r *Runner) runKill(ctx context.Context, args []string) int {
	fs := flag.NewFlagSet("kill", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	requestRef := fs.String("request-ref", "", "idempotency key")
	targetName := fs.String("target", "", "target name")
	paneID := fs.String("pane", "", "pane id")
	mode := fs.String("mode", "key", "kill mode key|signal")
	signal := fs.String("signal", "INT", "INT|TERM|KILL")
	ifRuntime := fs.String("if-runtime", "", "runtime guard")
	ifState := fs.String("if-state", "", "state guard")
	ifUpdatedWithin := fs.String("if-updated-within", "", "freshness guard duration")
	forceStale := fs.Bool("force-stale", false, "disable stale guard")
	jsonOut := fs.Bool("json", false, "output JSON")
	if err := fs.Parse(args); err != nil {
		_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
		return 2
	}
	if strings.TrimSpace(*requestRef) == "" || strings.TrimSpace(*targetName) == "" || strings.TrimSpace(*paneID) == "" {
		_, _ = fmt.Fprintln(r.errOut, "usage: agtmux kill --request-ref <id> --target <name> --pane <id> [--mode key|signal] [--signal INT|TERM|KILL]")
		return 2
	}
	req := map[string]any{
		"request_ref":       strings.TrimSpace(*requestRef),
		"target":            strings.TrimSpace(*targetName),
		"pane_id":           strings.TrimSpace(*paneID),
		"mode":              strings.TrimSpace(*mode),
		"signal":            strings.TrimSpace(*signal),
		"if_runtime":        strings.TrimSpace(*ifRuntime),
		"if_state":          strings.TrimSpace(*ifState),
		"if_updated_within": strings.TrimSpace(*ifUpdatedWithin),
		"force_stale":       *forceStale,
	}
	body, err := r.request(ctx, http.MethodPost, "/v1/actions/kill", nil, req)
	if err != nil {
		return r.handleErr(err)
	}
	if *jsonOut {
		_, _ = r.out.Write(body)
		_, _ = fmt.Fprintln(r.out)
		return 0
	}
	var resp api.ActionResponse
	if err := json.Unmarshal(body, &resp); err != nil {
		return r.handleErr(err)
	}
	_, _ = fmt.Fprintf(r.out, "kill action %s: %s\n", resp.ActionID, resp.ResultCode)
	return 0
}

func (r *Runner) runActionEvents(ctx context.Context, args []string) int {
	fs := flag.NewFlagSet("events", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	actionID := fs.String("action-id", "", "action id")
	jsonOut := fs.Bool("json", false, "output JSON")
	if err := fs.Parse(args); err != nil {
		_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
		return 2
	}
	if fs.NArg() > 0 || strings.TrimSpace(*actionID) == "" {
		_, _ = fmt.Fprintln(r.errOut, "usage: agtmux events --action-id <id> [--json]")
		return 2
	}
	path := "/v1/actions/" + url.PathEscape(strings.TrimSpace(*actionID)) + "/events"
	body, err := r.request(ctx, http.MethodGet, path, nil, nil)
	if err != nil {
		return r.handleErr(err)
	}
	if *jsonOut {
		_, _ = r.out.Write(body)
		_, _ = fmt.Fprintln(r.out)
		return 0
	}
	var env api.ActionEventsEnvelope
	if err := json.Unmarshal(body, &env); err != nil {
		return r.handleErr(err)
	}
	if _, err := fmt.Fprintf(r.out, "action_id=%s events=%d\n", env.ActionID, len(env.Events)); err != nil {
		return r.handleErr(err)
	}
	for _, event := range env.Events {
		runtimeID := strings.TrimSpace(event.RuntimeID)
		if runtimeID == "" {
			runtimeID = "-"
		}
		if _, err := fmt.Fprintf(r.out, "event=%s type=%s source=%s runtime=%s event_time=%s\n",
			event.EventID,
			event.EventType,
			event.Source,
			runtimeID,
			event.EventTime,
		); err != nil {
			return r.handleErr(err)
		}
	}
	return 0
}

func (r *Runner) runEvent(ctx context.Context, args []string) int {
	if len(args) == 0 {
		_, _ = fmt.Fprintln(r.errOut, "usage: agtmux event emit --source <hook|notify|wrapper|poller> --type <event_type> [--target <name>] [--pane <id>|--runtime <id>]")
		return 2
	}
	switch args[0] {
	case "emit":
		fs := flag.NewFlagSet("event emit", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		targetName := fs.String("target", "local", "target name")
		paneID := fs.String("pane", "", "pane id")
		runtimeID := fs.String("runtime", "", "runtime id")
		agentType := fs.String("agent", "", "agent type")
		source := fs.String("source", "", "event source hook|notify|wrapper|poller")
		eventType := fs.String("type", "", "event type")
		dedupeKey := fs.String("dedupe", "", "dedupe key")
		sourceEventID := fs.String("source-event-id", "", "source event id")
		sourceSeq := fs.String("source-seq", "", "source sequence")
		eventTime := fs.String("event-time", "", "event time RFC3339")
		pid := fs.String("pid", "", "pid hint")
		startHint := fs.String("start-hint", "", "start hint RFC3339")
		payload := fs.String("payload", "", "raw payload")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(r.errOut, "usage: agtmux event emit --source <hook|notify|wrapper|poller> --type <event_type> [--target <name>] [--pane <id>|--runtime <id>]")
			return 2
		}
		if strings.TrimSpace(*source) == "" || strings.TrimSpace(*eventType) == "" {
			_, _ = fmt.Fprintln(r.errOut, "usage: agtmux event emit --source <hook|notify|wrapper|poller> --type <event_type> [--target <name>] [--pane <id>|--runtime <id>]")
			return 2
		}
		runtime := strings.TrimSpace(*runtimeID)
		pane := strings.TrimSpace(*paneID)
		if runtime != "" && pane != "" {
			_, _ = fmt.Fprintln(r.errOut, "usage: agtmux event emit --source <hook|notify|wrapper|poller> --type <event_type> [--target <name>] [--pane <id>|--runtime <id>]")
			return 2
		}
		if runtime == "" && pane == "" {
			_, _ = fmt.Fprintln(r.errOut, "usage: agtmux event emit --source <hook|notify|wrapper|poller> --type <event_type> [--target <name>] [--pane <id>|--runtime <id>]")
			return 2
		}

		dedupe := strings.TrimSpace(*dedupeKey)
		if dedupe == "" {
			dedupe = fmt.Sprintf("cli:%s:%s:%d", strings.TrimSpace(*source), strings.TrimSpace(*eventType), time.Now().UTC().UnixNano())
		}

		req := map[string]any{
			"target":          strings.TrimSpace(*targetName),
			"pane_id":         pane,
			"runtime_id":      runtime,
			"agent_type":      strings.TrimSpace(*agentType),
			"source":          strings.TrimSpace(*source),
			"event_type":      strings.TrimSpace(*eventType),
			"dedupe_key":      dedupe,
			"source_event_id": strings.TrimSpace(*sourceEventID),
			"event_time":      strings.TrimSpace(*eventTime),
			"start_hint":      strings.TrimSpace(*startHint),
			"raw_payload":     *payload,
		}

		if v := strings.TrimSpace(*sourceSeq); v != "" {
			parsed, err := parseInt64(v)
			if err != nil {
				_, _ = fmt.Fprintf(r.errOut, "error: --source-seq must be int64\n")
				return 2
			}
			req["source_seq"] = parsed
		}
		if v := strings.TrimSpace(*pid); v != "" {
			parsed, err := parseInt64(v)
			if err != nil {
				_, _ = fmt.Fprintf(r.errOut, "error: --pid must be int64\n")
				return 2
			}
			req["pid"] = parsed
		}

		body, err := r.request(ctx, http.MethodPost, "/v1/events", nil, req)
		if err != nil {
			return r.handleErr(err)
		}
		if *jsonOut {
			_, _ = r.out.Write(body)
			_, _ = fmt.Fprintln(r.out)
			return 0
		}
		var resp api.EventIngestResponse
		if err := json.Unmarshal(body, &resp); err != nil {
			return r.handleErr(err)
		}
		if strings.TrimSpace(resp.RuntimeID) == "" {
			_, _ = fmt.Fprintf(r.out, "event %s: %s\n", resp.EventID, resp.Status)
			return 0
		}
		_, _ = fmt.Fprintf(r.out, "event %s: %s runtime=%s\n", resp.EventID, resp.Status, resp.RuntimeID)
		return 0
	default:
		_, _ = fmt.Fprintf(r.errOut, "unknown event command: %s\n", args[0])
		return 2
	}
}

func (r *Runner) runIntegration(_ context.Context, args []string) int {
	if len(args) == 0 {
		_, _ = fmt.Fprintln(r.errOut, "usage: agtmux integration <install|doctor> [options]")
		return 2
	}
	switch args[0] {
	case "install":
		fs := flag.NewFlagSet("integration install", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		homeDir := fs.String("home", "", "home directory")
		binDir := fs.String("bin-dir", "", "managed wrapper directory")
		agtmuxBin := fs.String("agtmux-bin", "agtmux", "agtmux binary path")
		dryRun := fs.Bool("dry-run", false, "print plan without writing files")
		jsonOut := fs.Bool("json", false, "output JSON")
		forceCodexNotify := fs.Bool("force-codex-notify", false, "replace existing codex notify")
		skipClaude := fs.Bool("skip-claude", false, "skip claude hook setup")
		skipCodex := fs.Bool("skip-codex", false, "skip codex notify setup")
		skipWrappers := fs.Bool("skip-wrappers", false, "skip wrapper script setup")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(r.errOut, "usage: agtmux integration install [--dry-run] [--json]")
			return 2
		}

		result, err := integration.Install(integration.InstallOptions{
			HomeDir:          strings.TrimSpace(*homeDir),
			BinDir:           strings.TrimSpace(*binDir),
			AGTMUXBin:        strings.TrimSpace(*agtmuxBin),
			TogglesExplicit:  true,
			DryRun:           *dryRun,
			InstallClaude:    !*skipClaude,
			InstallCodex:     !*skipCodex,
			InstallWrappers:  !*skipWrappers,
			ForceCodexNotify: *forceCodexNotify,
		})
		if err != nil {
			return r.handleErr(err)
		}
		if *jsonOut {
			raw, err := json.Marshal(result)
			if err != nil {
				return r.handleErr(err)
			}
			_, _ = r.out.Write(raw)
			_, _ = fmt.Fprintln(r.out)
			return 0
		}

		if result.DryRun {
			_, _ = fmt.Fprintln(r.out, "integration install dry-run:")
		} else {
			_, _ = fmt.Fprintln(r.out, "integration install complete:")
		}
		for _, path := range result.FilesWritten {
			_, _ = fmt.Fprintf(r.out, "  write %s\n", path)
		}
		for _, path := range result.Backups {
			_, _ = fmt.Fprintf(r.out, "  backup %s\n", path)
		}
		for _, warn := range result.Warnings {
			_, _ = fmt.Fprintf(r.out, "  warn: %s\n", warn)
		}
		return 0
	case "doctor":
		fs := flag.NewFlagSet("integration doctor", flag.ContinueOnError)
		fs.SetOutput(io.Discard)
		homeDir := fs.String("home", "", "home directory")
		binDir := fs.String("bin-dir", "", "managed wrapper directory")
		jsonOut := fs.Bool("json", false, "output JSON")
		if err := fs.Parse(args[1:]); err != nil {
			_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
			return 2
		}
		if fs.NArg() > 0 {
			_, _ = fmt.Fprintln(r.errOut, "usage: agtmux integration doctor [--home <dir>] [--bin-dir <dir>] [--json]")
			return 2
		}

		result, err := integration.Doctor(integration.DoctorOptions{
			HomeDir: strings.TrimSpace(*homeDir),
			BinDir:  strings.TrimSpace(*binDir),
		})
		if err != nil {
			return r.handleErr(err)
		}
		if *jsonOut {
			raw, err := json.Marshal(result)
			if err != nil {
				return r.handleErr(err)
			}
			_, _ = r.out.Write(raw)
			_, _ = fmt.Fprintln(r.out)
		} else {
			for _, check := range result.Checks {
				_, _ = fmt.Fprintf(r.out, "[%s] %s: %s", strings.ToUpper(check.Status), check.Name, check.Message)
				if strings.TrimSpace(check.Path) != "" {
					_, _ = fmt.Fprintf(r.out, " (%s)", check.Path)
				}
				_, _ = fmt.Fprintln(r.out)
			}
			if result.OK {
				_, _ = fmt.Fprintln(r.out, "integration doctor: OK")
			} else {
				_, _ = fmt.Fprintln(r.out, "integration doctor: FAIL")
			}
		}
		if result.OK {
			return 0
		}
		return 1
	default:
		_, _ = fmt.Fprintf(r.errOut, "unknown integration command: %s\n", args[0])
		return 2
	}
}

func parseInt64(raw string) (int64, error) {
	return strconv.ParseInt(strings.TrimSpace(raw), 10, 64)
}

func (r *Runner) runAppCommand(ctx context.Context, args []string, socketPath string) int {
	bin := strings.TrimSpace(os.Getenv("AGTMUX_APP_BIN"))
	if bin == "" {
		bin = "agtmux-app"
	}
	cmdArgs := make([]string, 0, len(args)+2)
	if strings.TrimSpace(socketPath) != "" {
		cmdArgs = append(cmdArgs, "--socket", strings.TrimSpace(socketPath))
	}
	cmdArgs = append(cmdArgs, args...)

	cmd := exec.CommandContext(ctx, bin, cmdArgs...)
	cmd.Stdout = r.out
	cmd.Stderr = r.errOut
	cmd.Stdin = os.Stdin
	cmd.Env = os.Environ()
	if err := cmd.Run(); err != nil {
		var exitErr *exec.ExitError
		if errors.As(err, &exitErr) {
			return exitErr.ExitCode()
		}
		if errors.Is(err, exec.ErrNotFound) || errors.Is(err, os.ErrNotExist) {
			_, _ = fmt.Fprintf(r.errOut, "error: agtmux-app binary not found: %s (set AGTMUX_APP_BIN to override)\n", bin)
			return 1
		}
		_, _ = fmt.Fprintf(r.errOut, "error: failed to execute agtmux-app: %v\n", err)
		return 1
	}
	return 0
}

func (r *Runner) request(ctx context.Context, method, path string, query url.Values, body any) ([]byte, error) {
	u := r.baseURL + path
	if len(query) > 0 {
		u += "?" + query.Encode()
	}
	var reqBody io.Reader
	if body != nil {
		buf := &bytes.Buffer{}
		if err := json.NewEncoder(buf).Encode(body); err != nil {
			return nil, fmt.Errorf("encode request body: %w", err)
		}
		reqBody = buf
	}
	req, err := http.NewRequestWithContext(ctx, method, u, reqBody)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Accept", "application/json")
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := r.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close() //nolint:errcheck
	payload, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode >= 400 {
		var er api.ErrorResponse
		if unmarshalErr := json.Unmarshal(payload, &er); unmarshalErr == nil && er.Error.Code != "" {
			return nil, fmt.Errorf("%s: %s", er.Error.Code, er.Error.Message)
		}
		return nil, fmt.Errorf("http %d: %s", resp.StatusCode, strings.TrimSpace(string(payload)))
	}
	return payload, nil
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

func (r *Runner) handleErr(err error) int {
	_, _ = fmt.Fprintf(r.errOut, "error: %v\n", err)
	return 1
}

func (r *Runner) printUsage() {
	_, _ = fmt.Fprintln(r.errOut, "usage: agtmux [--socket <path>] <target|adapter|event|integration|list|watch|send|view-output|kill|events|app> ...")
}
