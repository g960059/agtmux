package adapter

import (
	"encoding/json"
	"strings"

	"github.com/g960059/agtmux/internal/model"
)

type codexAdapter struct{}

func NewCodexAdapter() Adapter {
	return codexAdapter{}
}

func (codexAdapter) Definition() Definition {
	return Definition{
		Name:            "codex-notify-wrapper",
		AgentType:       "codex",
		ContractVersion: "v1",
		Capabilities: []string{
			CapabilityEventDriven,
			CapabilitySupportsWaitingApproval,
			CapabilitySupportsWaitingInput,
			CapabilitySupportsCompleted,
		},
	}
}

func (codexAdapter) Normalize(signal Signal) (NormalizedState, bool) {
	eventType := canonicalEventType(signal.EventType)
	if signal.Source == model.SourceNotify {
		if hint := codexNotifyEventHint(signal.RawPayload); hint != "" {
			eventType = hint
		}
	}
	switch {
	case signal.Source == model.SourceNotify && containsAny(eventType, "approval-requested", "approval-needed"):
		return NormalizedState{State: model.StateWaitingApproval, Reason: "approval_requested", Confidence: "high"}, true
	case signal.Source == model.SourceNotify && containsAny(eventType, "input-requested", "input-needed", "user-intervention-needed"):
		return NormalizedState{State: model.StateWaitingInput, Reason: "input_required", Confidence: "high"}, true
	case signal.Source == model.SourceNotify && containsAny(eventType, "agent-turn-complete", "agent-turn-finished", "turn-finished"):
		return NormalizedState{State: model.StateCompleted, Reason: "task_completed", Confidence: "medium"}, true
	case signal.Source == model.SourceWrapper && containsAny(eventType, "wrapper-start", "agent-start", "session-start"):
		return NormalizedState{State: model.StateRunning, Reason: "active", Confidence: "medium"}, true
	case signal.Source == model.SourceWrapper && containsAny(eventType, "wrapper-exit", "agent-exit", "session-exit"):
		return NormalizedState{State: model.StateCompleted, Reason: "task_completed", Confidence: "medium"}, true
	case containsAny(eventType, "wrapper-error", "agent-error", "runtime-error", "runtime-fail", "runtime-panic"):
		return NormalizedState{State: model.StateError, Reason: "runtime_error", Confidence: "high"}, true
	default:
		return NormalizedState{}, false
	}
}

func codexNotifyEventHint(rawPayload string) string {
	payload := strings.TrimSpace(rawPayload)
	if payload == "" {
		return ""
	}
	lower := strings.ToLower(payload)

	// Prefer structured markers when payload is JSON.
	var obj map[string]any
	if strings.HasPrefix(payload, "{") && json.Unmarshal([]byte(payload), &obj) == nil {
		if hint := codexNotifyEventHintFromJSON(obj); hint != "" {
			return hint
		}
		structured := strings.ToLower(extractJSONTokens(obj))
		if structured != "" {
			lower = lower + " " + structured
		}
	}

	switch {
	case containsAny(lower,
		"approval-requested",
		"approval-needed",
		"approval required",
		"needs approval",
		"awaiting approval",
	):
		return "approval-requested"
	case containsAny(lower,
		"input-requested",
		"input-needed",
		"user-intervention-needed",
		"waiting for input",
		"awaiting input",
		"prompt-user",
	):
		return "input-requested"
	case containsAny(lower, "agent-turn-complete", "agent-turn-finished", "turn-finished", "task completed"):
		return "agent-turn-complete"
	case containsAny(lower,
		`"status":"error"`,
		`"status":"failed"`,
		`"result":"error"`,
		`"result":"failed"`,
		"runtime-error",
		"runtime error",
		"exception",
		"panic",
	):
		return "runtime-error"
	default:
		return ""
	}
}

func codexNotifyEventHintFromJSON(obj map[string]any) string {
	keys := map[string]struct{}{
		"type":   {},
		"event":  {},
		"status": {},
		"result": {},
		"state":  {},
		"kind":   {},
	}
	values := make([]string, 0, 8)
	collectJSONKeyValues(obj, keys, &values)
	if len(values) == 0 {
		return ""
	}

	switch {
	case valuesContain(values,
		"approval-requested",
		"approval-needed",
		"approval required",
		"needs approval",
		"awaiting approval",
	):
		return "approval-requested"
	case valuesContain(values,
		"input-requested",
		"input-needed",
		"user-intervention-needed",
		"waiting for input",
		"awaiting input",
		"prompt-user",
	):
		return "input-requested"
	case valuesContain(values,
		"agent-turn-complete",
		"agent-turn-finished",
		"turn-finished",
		"task completed",
		"completed",
	):
		return "agent-turn-complete"
	case valuesContain(values,
		"runtime-error",
		"runtime error",
		"error",
		"failed",
		"failure",
		"panic",
		"exception",
	):
		return "runtime-error"
	default:
		return ""
	}
}

func collectJSONKeyValues(v any, keys map[string]struct{}, out *[]string) {
	switch t := v.(type) {
	case map[string]any:
		for key, child := range t {
			lowerKey := strings.ToLower(strings.TrimSpace(key))
			if _, ok := keys[lowerKey]; ok {
				switch raw := child.(type) {
				case string:
					if s := strings.ToLower(strings.TrimSpace(raw)); s != "" {
						*out = append(*out, s)
					}
				case []any:
					for _, entry := range raw {
						if s, ok := entry.(string); ok {
							if normalized := strings.ToLower(strings.TrimSpace(s)); normalized != "" {
								*out = append(*out, normalized)
							}
						}
					}
				}
			}
			collectJSONKeyValues(child, keys, out)
		}
	case []any:
		for _, child := range t {
			collectJSONKeyValues(child, keys, out)
		}
	}
}

func valuesContain(values []string, needles ...string) bool {
	for _, value := range values {
		if containsAny(value, needles...) {
			return true
		}
	}
	return false
}

func extractJSONTokens(v any) string {
	switch t := v.(type) {
	case map[string]any:
		parts := make([]string, 0, len(t))
		for _, child := range t {
			token := strings.TrimSpace(extractJSONTokens(child))
			if token != "" {
				parts = append(parts, token)
			}
		}
		return strings.Join(parts, " ")
	case []any:
		parts := make([]string, 0, len(t))
		for _, child := range t {
			token := strings.TrimSpace(extractJSONTokens(child))
			if token != "" {
				parts = append(parts, token)
			}
		}
		return strings.Join(parts, " ")
	case string:
		return t
	default:
		return ""
	}
}
