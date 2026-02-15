package adapter

import (
	"testing"

	"github.com/g960059/agtmux/internal/model"
)

func TestCodexNormalizeNotifyPayloadOverridesEventType(t *testing.T) {
	ad := NewCodexAdapter()
	tests := []struct {
		name       string
		eventType  string
		payload    string
		wantState  model.CanonicalState
		wantReason string
	}{
		{
			name:       "approval from payload type",
			eventType:  "agent-turn-complete",
			payload:    `{"type":"approval-requested"}`,
			wantState:  model.StateWaitingApproval,
			wantReason: "approval_requested",
		},
		{
			name:       "input from payload event",
			eventType:  "agent-turn-complete",
			payload:    `{"event":"input-requested"}`,
			wantState:  model.StateWaitingInput,
			wantReason: "input_required",
		},
		{
			name:       "error from payload status",
			eventType:  "agent-turn-complete",
			payload:    `{"status":"error","message":"runtime failed"}`,
			wantState:  model.StateError,
			wantReason: "runtime_error",
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			out, ok := ad.Normalize(Signal{
				EventType:  tc.eventType,
				Source:     model.SourceNotify,
				RawPayload: tc.payload,
			})
			if !ok {
				t.Fatalf("expected normalize success")
			}
			if out.State != tc.wantState || out.Reason != tc.wantReason {
				t.Fatalf("unexpected normalized state: %+v", out)
			}
		})
	}
}

func TestCodexNotifyHintAvoidsFalsePositiveError(t *testing.T) {
	ad := NewCodexAdapter()
	tests := []struct {
		name      string
		eventType string
		payload   string
		wantState model.CanonicalState
	}{
		{
			name:      "benign error substring in message",
			eventType: "agent-turn-complete",
			payload:   `{"message":"no errors found in output"}`,
			wantState: model.StateCompleted,
		},
		{
			name:      "approval with error context",
			eventType: "agent-turn-complete",
			payload:   `{"type":"approval-requested","details":"error in previous step was handled"}`,
			wantState: model.StateWaitingApproval,
		},
		{
			name:      "completed with error mention",
			eventType: "agent-turn-complete",
			payload:   `{"type":"agent-turn-complete","summary":"error handling improved"}`,
			wantState: model.StateCompleted,
		},
		{
			name:      "plain text failed should not force error",
			eventType: "agent-turn-complete",
			payload:   "all tests failed checks are disabled in docs context",
			wantState: model.StateCompleted,
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			out, ok := ad.Normalize(Signal{
				EventType:  tc.eventType,
				Source:     model.SourceNotify,
				RawPayload: tc.payload,
			})
			if !ok {
				t.Fatalf("expected normalize success")
			}
			if out.State != tc.wantState {
				t.Fatalf("unexpected state: got=%s want=%s out=%+v", out.State, tc.wantState, out)
			}
		})
	}
}
