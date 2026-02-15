package adapter

import (
	"testing"

	"github.com/g960059/agtmux/internal/model"
)

type versionedAdapter struct {
	def Definition
}

func (a versionedAdapter) Definition() Definition {
	return a.def
}

func (versionedAdapter) Normalize(Signal) (NormalizedState, bool) {
	return NormalizedState{}, false
}

func TestDefaultRegistryIncludesBuiltins(t *testing.T) {
	reg := DefaultRegistry()

	if _, ok := reg.Resolve("codex"); !ok {
		t.Fatalf("expected codex adapter")
	}
	if _, ok := reg.Resolve("claude"); !ok {
		t.Fatalf("expected claude adapter")
	}
	if _, ok := reg.Resolve("gemini"); !ok {
		t.Fatalf("expected gemini adapter")
	}

	defs := reg.Definitions()
	if len(defs) != 3 {
		t.Fatalf("expected 3 built-in adapters, got %d", len(defs))
	}
}

func TestRegistryRejectsDuplicateAgentType(t *testing.T) {
	reg := NewRegistry(NewCodexAdapter())
	if err := reg.Register(NewCodexAdapter()); err == nil {
		t.Fatalf("expected duplicate agent_type to fail")
	}
}

func TestRegistryRejectsIncompatibleContractVersion(t *testing.T) {
	reg := NewRegistry()
	err := reg.Register(versionedAdapter{
		def: Definition{
			Name:            "future-adapter",
			AgentType:       "future",
			ContractVersion: "v2",
		},
	})
	if err == nil {
		t.Fatalf("expected incompatible contract version to fail")
	}
}

func TestIsVersionCompatible(t *testing.T) {
	cases := []struct {
		version string
		ok      bool
	}{
		{version: "v1", ok: true},
		{version: "v1.0", ok: true},
		{version: "V1.7", ok: true},
		{version: "v2", ok: false},
		{version: "1", ok: false},
		{version: "vx", ok: false},
		{version: "", ok: false},
	}
	for _, tc := range cases {
		if got := IsVersionCompatible(tc.version); got != tc.ok {
			t.Fatalf("version=%q expected %v got %v", tc.version, tc.ok, got)
		}
	}
}

func TestCodexNormalization(t *testing.T) {
	reg := DefaultRegistry()
	out, ok := reg.Normalize("codex", Signal{
		EventType: "agent-turn-finished",
		Source:    model.SourceNotify,
	})
	if !ok {
		t.Fatalf("expected codex adapter to normalize signal")
	}
	if out.State != model.StateCompleted || out.Reason != "task_completed" {
		t.Fatalf("unexpected codex normalization: %+v", out)
	}
}

func TestClaudeNormalization(t *testing.T) {
	reg := DefaultRegistry()
	out, ok := reg.Normalize("claude", Signal{
		EventType: "user-intervention-needed",
		Source:    model.SourceHook,
	})
	if !ok {
		t.Fatalf("expected claude adapter to normalize signal")
	}
	if out.State != model.StateWaitingInput || out.Reason != "input_required" {
		t.Fatalf("unexpected claude normalization: %+v", out)
	}
}

func TestNormalizeUnknownAgent(t *testing.T) {
	reg := DefaultRegistry()
	if _, ok := reg.Normalize("unknown", Signal{EventType: "anything", Source: model.SourceNotify}); ok {
		t.Fatalf("unknown agent should not normalize")
	}
}

func TestGeminiNormalization(t *testing.T) {
	reg := DefaultRegistry()
	out, ok := reg.Normalize("gemini", Signal{
		EventType: "parser-input-needed",
		Source:    model.SourceWrapper,
	})
	if !ok {
		t.Fatalf("expected gemini adapter to normalize signal")
	}
	if out.State != model.StateWaitingInput || out.Reason != "input_required" {
		t.Fatalf("unexpected gemini normalization: %+v", out)
	}
}
