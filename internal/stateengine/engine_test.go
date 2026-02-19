package stateengine_test

import (
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/provideradapters"
	"github.com/g960059/agtmux/internal/stateengine"
)

func TestEngineEvaluateClaudePollerRunningWithoutSignalPrefersIdle(t *testing.T) {
	engine := stateengine.NewEngine(provideradapters.DefaultRegistry())
	now := time.Now().UTC()
	result := engine.Evaluate(stateengine.PaneMeta{
		TargetID:      "t1",
		PaneID:        "%1",
		AgentType:     "claude",
		CurrentCmd:    "claude",
		RawState:      "running",
		RawReasonCode: "unknown",
		RawConfidence: "high",
		StateSource:   "poller",
		LastEventType: "poll_tick",
		UpdatedAt:     now,
	}, now)
	if result.Provider != stateengine.ProviderClaude {
		t.Fatalf("expected provider claude, got %+v", result)
	}
	if result.ActivityState != stateengine.ActivityIdle {
		t.Fatalf("expected idle from claude poller fallback, got %+v", result)
	}
}

func TestEngineEvaluateCodexRunningSignal(t *testing.T) {
	engine := stateengine.NewEngine(provideradapters.DefaultRegistry())
	now := time.Now().UTC()
	result := engine.Evaluate(stateengine.PaneMeta{
		TargetID:      "t1",
		PaneID:        "%2",
		AgentType:     "codex",
		CurrentCmd:    "codex",
		RawState:      "running",
		RawReasonCode: "agent_turn_started",
		RawConfidence: "high",
		StateSource:   "notify",
		LastEventType: "agent_turn_started",
		UpdatedAt:     now,
	}, now)
	if result.Provider != stateengine.ProviderCodex {
		t.Fatalf("expected codex provider, got %+v", result)
	}
	if result.ActivityState != stateengine.ActivityRunning {
		t.Fatalf("expected running, got %+v", result)
	}
	if result.ActivitySource == "" || result.ActivitySource == "none" {
		t.Fatalf("expected activity source, got %+v", result)
	}
}

func TestEngineEvaluateUnmanagedAgent(t *testing.T) {
	engine := stateengine.NewEngine(provideradapters.DefaultRegistry())
	now := time.Now().UTC()
	result := engine.Evaluate(stateengine.PaneMeta{
		TargetID:      "t1",
		PaneID:        "%3",
		AgentType:     "none",
		CurrentCmd:    "zsh",
		RawState:      "unknown",
		RawReasonCode: "unsupported_signal",
		RawConfidence: "low",
		StateSource:   "poller",
		UpdatedAt:     now,
	}, now)
	if result.AgentPresence != stateengine.AgentPresenceUnmanaged {
		t.Fatalf("expected unmanaged presence, got %+v", result)
	}
	if result.Provider != stateengine.ProviderNone {
		t.Fatalf("expected none provider for unmanaged, got %+v", result)
	}
}
