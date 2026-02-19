package provideradapters

import (
	"strings"
	"time"

	"github.com/g960059/agtmux/internal/stateengine"
)

type CodexAdapter struct{}

func NewCodexAdapter() *CodexAdapter {
	return &CodexAdapter{}
}

func (a *CodexAdapter) ID() string {
	return stateengine.ProviderCodex
}

func (a *CodexAdapter) DetectProvider(meta stateengine.PaneMeta) (float64, bool) {
	return detectByAgentOrCmd(meta, stateengine.ProviderCodex, "codex", "openai codex")
}

func (a *CodexAdapter) BuildEvidence(meta stateengine.PaneMeta, now time.Time) []stateengine.Evidence {
	combined := normalizeForMatch(meta.RawReasonCode, meta.LastEventType, meta.SessionLabel, meta.PaneTitle)
	source := strings.ToLower(strings.TrimSpace(meta.StateSource))
	kind := kindFromSource(source)
	evidence := make([]stateengine.Evidence, 0, 4)

	if hasAnyToken(combined, "waiting_approval", "approval_required", "permission", "approval") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityWaitingApproval, kind, source, "codex:approval", 0.97, 0.95))
	}
	if hasAnyToken(combined, "waiting_input", "await_user", "input_required", "for shortcuts", "shortcut") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityWaitingInput, kind, source, "codex:input", 0.94, 0.92))
	}
	if hasAnyToken(combined, "error", "failed", "panic", "exception") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityError, kind, source, "codex:error", 1.0, 0.95))
	}
	if hasAnyToken(combined,
		"running",
		"working",
		"in_progress",
		"streaming",
		"task_started",
		"agent_turn_started",
		"wrapper_start",
	) {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityRunning, kind, source, "codex:running_signal", 0.92, 0.88))
	}
	if hasAnyToken(combined, "idle", "completed", "done", "task_finished", "wrapper_exit", "session_end") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityIdle, kind, source, "codex:idle_signal", 0.88, 0.86))
	}
	return evidence
}
