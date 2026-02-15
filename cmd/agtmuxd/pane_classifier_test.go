package main

import (
	"context"
	"strings"
	"testing"

	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/target"
)

func TestClassifyPaneAgentType(t *testing.T) {
	ctx := context.Background()
	tests := []struct {
		name string
		cmd  string
		want string
	}{
		{name: "codex", cmd: "codex", want: "codex"},
		{name: "claude", cmd: "claude", want: "claude"},
		{name: "gemini", cmd: "gemini", want: "gemini"},
		{name: "shell-is-none", cmd: "zsh", want: agentTypeNone},
		{name: "empty-is-none", cmd: "", want: agentTypeNone},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got := classifyPaneAgentType(ctx, nil, model.Target{}, model.Pane{CurrentCmd: tc.cmd})
			if got != tc.want {
				t.Fatalf("classifyPaneAgentType(%q)=%q want=%q", tc.cmd, got, tc.want)
			}
		})
	}
}

type agentProbeRunner struct {
	psOutput string
}

func (r agentProbeRunner) Run(_ context.Context, name string, args ...string) ([]byte, error) {
	if name != "ps" {
		return []byte{}, nil
	}
	joined := strings.Join(args, " ")
	if !strings.Contains(joined, "-t") || !strings.Contains(joined, "command=") {
		return []byte{}, nil
	}
	return []byte(r.psOutput), nil
}

func TestClassifyPaneAgentTypeFromTTYForNodeWrapper(t *testing.T) {
	ctx := context.Background()
	exec := target.NewExecutorWithRunner(config.DefaultConfig(), agentProbeRunner{
		psOutput: "-zsh\nnode /Users/test/.nvm/versions/node/v24.12.0/bin/codex --yolo\n",
	})
	pane := model.Pane{
		CurrentCmd: "node",
		TTY:        "/dev/ttys005",
	}
	got := classifyPaneAgentType(ctx, exec, model.Target{TargetID: "local", Kind: model.TargetKindLocal}, pane)
	if got != "codex" {
		t.Fatalf("expected codex from tty probe, got %q", got)
	}
}

func TestClassifyPaneAgentTypeSkipsTTYProbeForShell(t *testing.T) {
	ctx := context.Background()
	exec := target.NewExecutorWithRunner(config.DefaultConfig(), agentProbeRunner{
		psOutput: "claude --resume abc\n",
	})
	pane := model.Pane{
		CurrentCmd: "zsh",
		TTY:        "/dev/ttys001",
	}
	got := classifyPaneAgentType(ctx, exec, model.Target{TargetID: "local", Kind: model.TargetKindLocal}, pane)
	if got != agentTypeNone {
		t.Fatalf("expected no agent for shell pane without direct markers, got %q", got)
	}
}

type classifierRunner struct {
	output string
}

func (r classifierRunner) Run(_ context.Context, _ string, args ...string) ([]byte, error) {
	joined := strings.Join(args, " ")
	if !strings.Contains(joined, "capture-pane") {
		return []byte{}, nil
	}
	return []byte(r.output), nil
}

func TestInferPanePollerEventType(t *testing.T) {
	ctx := context.Background()
	tg := model.Target{TargetID: "t1", Kind: model.TargetKindLocal}
	pane := model.Pane{PaneID: "%1"}

	if got := inferPanePollerEventType(ctx, nil, tg, pane, agentTypeNone); got != "no-agent" {
		t.Fatalf("unmanaged pane expected no-agent, got %q", got)
	}

	exec := target.NewExecutorWithRunner(config.DefaultConfig(), classifierRunner{output: "waiting for input"})
	if got := inferPanePollerEventType(ctx, exec, tg, pane, "codex"); got != "input_required" {
		t.Fatalf("input_required expected, got %q", got)
	}

	exec = target.NewExecutorWithRunner(config.DefaultConfig(), classifierRunner{output: "approval required"})
	if got := inferPanePollerEventType(ctx, exec, tg, pane, "codex"); got != "approval_requested" {
		t.Fatalf("approval_requested expected, got %q", got)
	}

	exec = target.NewExecutorWithRunner(config.DefaultConfig(), classifierRunner{output: "approval required\nruntime error"})
	if got := inferPanePollerEventType(ctx, exec, tg, pane, "codex"); got != "runtime_error" {
		t.Fatalf("runtime_error expected precedence over approval, got %q", got)
	}

	exec = target.NewExecutorWithRunner(config.DefaultConfig(), classifierRunner{output: "task completed successfully"})
	if got := inferPanePollerEventType(ctx, exec, tg, pane, "codex"); got != "idle" {
		t.Fatalf("idle expected, got %q", got)
	}

	exec = target.NewExecutorWithRunner(config.DefaultConfig(), classifierRunner{output: "some arbitrary output without strong markers"})
	if got := inferPanePollerEventType(ctx, exec, tg, pane, "codex"); got != "unknown" {
		t.Fatalf("unknown expected for inconclusive output, got %q", got)
	}

	exec = target.NewExecutorWithRunner(config.DefaultConfig(), classifierRunner{output: "esc to interrupt"})
	if got := inferPanePollerEventType(ctx, exec, tg, pane, "codex"); got != "running" {
		t.Fatalf("running expected when active marker exists, got %q", got)
	}

	exec = target.NewExecutorWithRunner(config.DefaultConfig(), classifierRunner{output: "✻ Crunched for 3m 2s"})
	if got := inferPanePollerEventType(ctx, exec, tg, pane, "claude"); got != "running" {
		t.Fatalf("running expected for claude crunch marker, got %q", got)
	}

	exec = target.NewExecutorWithRunner(config.DefaultConfig(), classifierRunner{output: "esc to interrupt\n? for shortcuts\n\u276f"})
	if got := inferPanePollerEventType(ctx, exec, tg, pane, "codex"); got != "idle" {
		t.Fatalf("latest prompt should win over stale running marker, got %q", got)
	}
}
