package main

import (
	"context"
	"hash/fnv"
	"strings"

	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/target"
)

const (
	agentTypeNone = "none"
)

type paneInference struct {
	EventType string
	Signature uint64
	HasOutput bool
}

func classifyPaneAgentType(ctx context.Context, executor *target.Executor, tg model.Target, pane model.Pane) string {
	if agent := classifyAgentByText(pane.CurrentCmd); agent != agentTypeNone {
		return agent
	}
	if executor == nil || !shouldProbeAgentFromTTY(pane.CurrentCmd, pane.TTY) {
		return agentTypeNone
	}
	return classifyPaneAgentTypeFromTTY(ctx, executor, tg, pane.TTY)
}

func classifyAgentByText(text string) string {
	normalized := strings.ToLower(strings.TrimSpace(text))
	switch {
	case containsAny(normalized, "codex"):
		return "codex"
	case containsAny(normalized, "claude"):
		return "claude"
	case containsAny(normalized, "gemini"):
		return "gemini"
	default:
		return agentTypeNone
	}
}

func shouldProbeAgentFromTTY(currentCmd, tty string) bool {
	if strings.TrimSpace(tty) == "" {
		return false
	}
	switch strings.ToLower(strings.TrimSpace(currentCmd)) {
	case "node", "nodejs", "python", "python3", "ruby", "java", "bun", "deno":
		return true
	default:
		return false
	}
}

func classifyPaneAgentTypeFromTTY(ctx context.Context, executor *target.Executor, tg model.Target, tty string) string {
	candidates := []string{strings.TrimSpace(tty)}
	if strings.HasPrefix(tty, "/dev/") {
		candidates = append(candidates, strings.TrimPrefix(tty, "/dev/"))
	}
	for _, candidate := range candidates {
		if strings.TrimSpace(candidate) == "" {
			continue
		}
		res, err := executor.Run(ctx, tg, []string{"ps", "-t", candidate, "-o", "command="})
		if err != nil {
			continue
		}
		lines := strings.Split(res.Output, "\n")
		for _, line := range lines {
			if agent := classifyAgentByText(line); agent != agentTypeNone {
				return agent
			}
		}
	}
	return agentTypeNone
}

func inferPanePollerEventType(ctx context.Context, executor *target.Executor, tg model.Target, pane model.Pane, agentType string) string {
	return inferPanePollerEvent(ctx, executor, tg, pane, agentType).EventType
}

func inferPanePollerEvent(ctx context.Context, executor *target.Executor, tg model.Target, pane model.Pane, agentType string) paneInference {
	if agentType == agentTypeNone {
		return paneInference{EventType: "no-agent"}
	}
	if executor == nil {
		return paneInference{EventType: "unknown"}
	}

	// Heuristic-only path: use recent pane output for waiting/error/idle hints.
	res, err := executor.Run(ctx, tg, target.BuildTmuxCommand(
		"capture-pane",
		"-p",
		"-t", pane.PaneID,
		"-S", "-80",
	))
	if err != nil {
		return paneInference{EventType: "unknown"}
	}
	output := strings.TrimSpace(res.Output)
	if output == "" {
		return paneInference{EventType: "unknown"}
	}

	normalized := strings.ToLower(output)
	return paneInference{
		EventType: classifyPollerEventFromOutput(normalized),
		Signature: hashOutputSignature(normalized),
		HasOutput: true,
	}
}

func classifyPollerEventFromOutput(out string) string {
	lines := strings.Split(out, "\n")
	for i := len(lines) - 1; i >= 0; i-- {
		line := strings.ToLower(strings.TrimSpace(lines[i]))
		if line == "" {
			continue
		}
		switch {
		case isPromptLine(line), containsAny(line, "task completed", "completed successfully", "all done", "ready for input", "? for shortcuts"):
			return "idle"
		case containsAny(line, "fatal:", "panic:", "traceback", "exception", "runtime error"):
			return "runtime_error"
		case containsAny(line, "waiting for approval", "approval required", "requires approval", "approve this", "approve to continue"):
			return "approval_requested"
		case containsAny(line, "waiting for input", "input required", "awaiting input", "your input", "press enter", "(y/n)", "enter to select"):
			return "input_required"
		case containsAny(line, "esc to interrupt", "ctrl+c to interrupt", "processing", "thinking", "generating", "crunched for", "clauding"):
			return "running"
		}
	}

	switch {
	case containsAny(out, "fatal:", "panic:", "traceback", "exception", "runtime error"):
		return "runtime_error"
	case containsAny(out, "waiting for approval", "approval required", "requires approval", "approve this", "approve to continue"):
		return "approval_requested"
	case containsAny(out, "waiting for input", "input required", "awaiting input", "your input", "press enter", "(y/n)", "enter to select"):
		return "input_required"
	case containsAny(out, "esc to interrupt", "ctrl+c to interrupt", "processing", "thinking", "generating", "crunched for", "clauding"):
		return "running"
	case containsAny(out, "task completed", "completed successfully", "all done", "ready for input", "? for shortcuts"), looksPromptLike(out):
		return "idle"
	default:
		return "unknown"
	}
}

func looksPromptLike(out string) bool {
	lines := strings.Split(out, "\n")
	for i := len(lines) - 1; i >= 0; i-- {
		line := strings.TrimSpace(lines[i])
		if line == "" {
			continue
		}
		// Skip common footer/help lines.
		if containsAny(line, "for shortcuts", "ctrl+", "shift+") {
			continue
		}
		return isPromptLine(strings.ToLower(line))
	}
	return false
}

func isPromptLine(line string) bool {
	return line == ">" ||
		strings.HasPrefix(line, "> ") ||
		line == "\u276f" || // ❯
		strings.HasPrefix(line, "\u276f ") ||
		line == "\u203a" || // ›
		strings.HasPrefix(line, "\u203a ")
}

func hashOutputSignature(out string) uint64 {
	h := fnv.New64a()
	_, _ = h.Write([]byte(out))
	return h.Sum64()
}

func containsAny(s string, needles ...string) bool {
	for _, needle := range needles {
		if needle == "" {
			continue
		}
		if strings.Contains(s, needle) {
			return true
		}
	}
	return false
}
