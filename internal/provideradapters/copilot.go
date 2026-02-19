package provideradapters

import (
	"strings"
	"time"

	"github.com/g960059/agtmux/internal/stateengine"
)

type CopilotAdapter struct{}

func NewCopilotAdapter() *CopilotAdapter {
	return &CopilotAdapter{}
}

func (a *CopilotAdapter) ID() string {
	return stateengine.ProviderCopilot
}

func (a *CopilotAdapter) DetectProvider(meta stateengine.PaneMeta) (float64, bool) {
	return detectByAgentOrCmd(meta, stateengine.ProviderCopilot, "copilot", "gh copilot")
}

func (a *CopilotAdapter) BuildEvidence(meta stateengine.PaneMeta, now time.Time) []stateengine.Evidence {
	combined := normalizeForMatch(meta.RawReasonCode, meta.LastEventType, meta.SessionLabel, meta.PaneTitle)
	source := strings.ToLower(strings.TrimSpace(meta.StateSource))
	kind := kindFromSource(source)
	evidence := make([]stateengine.Evidence, 0, 3)

	if hasAnyToken(combined, "waiting_approval", "approval_required", "permission") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityWaitingApproval, kind, source, "copilot:approval", 0.9, 0.88))
	}
	if hasAnyToken(combined, "waiting_input", "input_required", "await_user") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityWaitingInput, kind, source, "copilot:input", 0.88, 0.84))
	}
	if hasAnyToken(combined, "error", "failed", "panic", "exception") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityError, kind, source, "copilot:error", 0.98, 0.9))
	}
	if hasAnyToken(combined, "running", "working", "streaming", "task_started", "wrapper_start") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityRunning, kind, source, "copilot:running_signal", 0.74, 0.8))
	}
	if hasAnyToken(combined, "idle", "completed", "done", "wrapper_exit", "session_end") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityIdle, kind, source, "copilot:idle_signal", 0.78, 0.8))
	}
	return evidence
}
