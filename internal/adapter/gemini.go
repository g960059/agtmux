package adapter

import "github.com/g960059/agtmux/internal/model"

type geminiAdapter struct{}

func NewGeminiAdapter() Adapter {
	return geminiAdapter{}
}

func (geminiAdapter) Definition() Definition {
	return Definition{
		Name:            "gemini-wrapper-parser",
		AgentType:       "gemini",
		ContractVersion: "v1",
		Capabilities: []string{
			CapabilityEventDriven,
			CapabilityPollingRequired,
			CapabilitySupportsWaitingInput,
			CapabilitySupportsCompleted,
		},
	}
}

func (geminiAdapter) Normalize(signal Signal) (NormalizedState, bool) {
	eventType := canonicalEventType(signal.EventType)
	switch {
	case containsAny(eventType, "parser-error", "wrapper-error", "runtime-error", "runtime-fail", "runtime-panic"):
		return NormalizedState{State: model.StateError, Reason: "runtime_error", Confidence: "high"}, true
	case containsAny(eventType, "parser-needs-input", "parser-input-needed", "input-needed", "user-intervention-needed"):
		return NormalizedState{State: model.StateWaitingInput, Reason: "input_required", Confidence: "high"}, true
	case containsAny(eventType, "parser-approval-needed", "approval-requested"):
		return NormalizedState{State: model.StateWaitingApproval, Reason: "approval_requested", Confidence: "high"}, true
	case signal.Source == model.SourceWrapper && containsAny(eventType, "wrapper-start", "agent-start", "session-start"):
		return NormalizedState{State: model.StateRunning, Reason: "active", Confidence: "medium"}, true
	case signal.Source == model.SourceWrapper && containsAny(eventType, "wrapper-exit", "agent-exit", "session-exit"):
		return NormalizedState{State: model.StateCompleted, Reason: "task_completed", Confidence: "medium"}, true
	case containsAny(eventType, "parser-complete", "parser-completed", "task-finished"):
		return NormalizedState{State: model.StateCompleted, Reason: "task_completed", Confidence: "medium"}, true
	default:
		return NormalizedState{}, false
	}
}
