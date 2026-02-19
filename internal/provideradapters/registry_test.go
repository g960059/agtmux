package provideradapters

import (
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/stateengine"
)

func TestDefaultRegistryIncludesAllPhase19DProviders(t *testing.T) {
	reg := DefaultRegistry()
	adapters := reg.Adapters()
	if len(adapters) < 4 {
		t.Fatalf("expected >=4 adapters, got %d", len(adapters))
	}
	ids := map[string]bool{}
	for _, adapter := range adapters {
		ids[adapter.ID()] = true
	}
	for _, required := range []string{
		stateengine.ProviderClaude,
		stateengine.ProviderCodex,
		stateengine.ProviderGemini,
		stateengine.ProviderCopilot,
	} {
		if !ids[required] {
			t.Fatalf("missing adapter %q in %+v", required, ids)
		}
	}
}

func TestClaudeAdapterDetectAndEvidence(t *testing.T) {
	adapter := NewClaudeAdapter()
	meta := stateengine.PaneMeta{
		AgentType:     "claude",
		CurrentCmd:    "claude",
		RawState:      "running",
		RawReasonCode: "waiting_input",
		StateSource:   "hook",
		LastEventType: "input_required",
	}
	confidence, ok := adapter.DetectProvider(meta)
	if !ok || confidence < 0.8 {
		t.Fatalf("expected claude provider detection, got ok=%v confidence=%f", ok, confidence)
	}
	ev := adapter.BuildEvidence(meta, time.Now().UTC())
	if len(ev) == 0 {
		t.Fatalf("expected non-empty evidence")
	}
}
