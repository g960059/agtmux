package adapter

import "github.com/g960059/agtmux/internal/model"

type claudeAdapter struct{}

func NewClaudeAdapter() Adapter {
	return claudeAdapter{}
}

func (claudeAdapter) Definition() Definition {
	return Definition{
		Name:            "claude-hook",
		AgentType:       "claude",
		ContractVersion: "v1",
		Capabilities: []string{
			CapabilityEventDriven,
			CapabilitySupportsWaitingApproval,
			CapabilitySupportsWaitingInput,
			CapabilitySupportsCompleted,
		},
	}
}

func (claudeAdapter) Normalize(signal Signal) (NormalizedState, bool) {
	eventType := canonicalEventType(signal.EventType)
	switch {
	case signal.Source == model.SourceHook && containsAny(eventType, "needs-approval", "wait-approval", "approval-requested"):
		return NormalizedState{State: model.StateWaitingApproval, Reason: "approval_requested", Confidence: "high"}, true
	case signal.Source == model.SourceHook && containsAny(eventType, "needs-input", "user-intervention-needed", "prompt-user"):
		return NormalizedState{State: model.StateWaitingInput, Reason: "input_required", Confidence: "high"}, true
	case signal.Source == model.SourceHook && containsAny(eventType, "hook-start", "task-started", "session-started"):
		return NormalizedState{State: model.StateRunning, Reason: "active", Confidence: "medium"}, true
	case signal.Source == model.SourceHook && containsAny(eventType, "hook-done", "task-finished", "session-finished"):
		return NormalizedState{State: model.StateCompleted, Reason: "task_completed", Confidence: "medium"}, true
	case containsAny(eventType, "hook-error", "runtime-error", "runtime-fail", "runtime-panic"):
		return NormalizedState{State: model.StateError, Reason: "runtime_error", Confidence: "high"}, true
	default:
		return NormalizedState{}, false
	}
}
