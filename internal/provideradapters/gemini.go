package provideradapters

import (
	"strings"
	"time"

	"github.com/g960059/agtmux/internal/stateengine"
)

type GeminiAdapter struct{}

func NewGeminiAdapter() *GeminiAdapter {
	return &GeminiAdapter{}
}

func (a *GeminiAdapter) ID() string {
	return stateengine.ProviderGemini
}

func (a *GeminiAdapter) DetectProvider(meta stateengine.PaneMeta) (float64, bool) {
	return detectByAgentOrCmd(meta, stateengine.ProviderGemini, "gemini")
}

func (a *GeminiAdapter) BuildEvidence(meta stateengine.PaneMeta, now time.Time) []stateengine.Evidence {
	combined := normalizeForMatch(meta.RawReasonCode, meta.LastEventType, meta.SessionLabel, meta.PaneTitle)
	source := strings.ToLower(strings.TrimSpace(meta.StateSource))
	kind := kindFromSource(source)
	evidence := make([]stateengine.Evidence, 0, 3)

	if hasAnyToken(combined, "waiting_approval", "approval_required", "permission") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityWaitingApproval, kind, source, "gemini:approval", 0.92, 0.9))
	}
	if hasAnyToken(combined, "waiting_input", "input_required", "await_user") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityWaitingInput, kind, source, "gemini:input", 0.9, 0.86))
	}
	if hasAnyToken(combined, "error", "failed", "panic", "exception") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityError, kind, source, "gemini:error", 0.98, 0.92))
	}
	if hasAnyToken(combined, "running", "working", "streaming", "task_started", "wrapper_start") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityRunning, kind, source, "gemini:running_signal", 0.78, 0.8))
	}
	if hasAnyToken(combined, "idle", "completed", "done", "wrapper_exit", "session_end") {
		evidence = append(evidence, buildEvidence(now, a.ID(), stateengine.ActivityIdle, kind, source, "gemini:idle_signal", 0.8, 0.82))
	}
	return evidence
}
