package provideradapters

import (
	"strings"
	"time"

	"github.com/g960059/agtmux/internal/stateengine"
)

type ClaudeAdapter struct{}

func NewClaudeAdapter() *ClaudeAdapter {
	return &ClaudeAdapter{}
}

func (a *ClaudeAdapter) ID() string {
	return stateengine.ProviderClaude
}

func (a *ClaudeAdapter) DetectProvider(meta stateengine.PaneMeta) (float64, bool) {
	return detectByAgentOrCmd(meta, stateengine.ProviderClaude, "claude", "claude-code", "cc")
}

func (a *ClaudeAdapter) BuildEvidence(meta stateengine.PaneMeta, now time.Time) []stateengine.Evidence {
	combined := normalizeForMatch(meta.RawReasonCode, meta.LastEventType, meta.SessionLabel, meta.PaneTitle)
	source := strings.ToLower(strings.TrimSpace(meta.StateSource))
	kind := kindFromSource(source)
	evidence := make([]stateengine.Evidence, 0, 4)

	if hasAnyToken(combined, "approval", "waiting_approval", "needs_approval", "permission") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityWaitingApproval, kind, source, "claude:approval", 0.98, 0.96))
	}
	if hasAnyToken(combined, "waiting_input", "input_required", "await_user", "prompt") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityWaitingInput, kind, source, "claude:input", 0.92, 0.9))
	}
	if hasAnyToken(combined, "error", "failed", "panic", "exception") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityError, kind, source, "claude:error", 1.0, 0.95))
	}
	runningHint := hasAnyToken(combined,
		"working",
		"running",
		"in_progress",
		"streaming",
		"task_started",
		"agent_turn_started",
		"pretooluse",
	)
	if runningHint {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityRunning, kind, source, "claude:running_signal", 0.9, 0.86))
	}
	if hasAnyToken(combined, "idle", "completed", "done", "stop", "wrapper_exit", "session_end") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityIdle, kind, source, "claude:idle_signal", 0.88, 0.88))
	}
	// Claude false-positive suppression: poller-running without explicit running hints should prefer idle.
	if strings.EqualFold(strings.TrimSpace(meta.RawState), stateengine.ActivityRunning) &&
		source == "poller" &&
		!runningHint {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityIdle, stateengine.EvidenceCapture, source, "claude:poller_running_without_signal", 0.93, 0.9))
	}
	return evidence
}
