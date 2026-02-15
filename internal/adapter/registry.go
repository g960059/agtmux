package adapter

import (
	"fmt"
	"sort"
	"strconv"
	"strings"
	"sync"

	"github.com/g960059/agtmux/internal/model"
)

const (
	CapabilityEventDriven             = "event_driven"
	CapabilityPollingRequired         = "polling_required"
	CapabilitySupportsWaitingApproval = "supports_waiting_approval"
	CapabilitySupportsWaitingInput    = "supports_waiting_input"
	CapabilitySupportsCompleted       = "supports_completed"
	supportedContractMajor            = 1
)

type Definition struct {
	Name            string
	AgentType       string
	ContractVersion string
	Capabilities    []string
}

type Signal struct {
	EventType  string
	Source     model.EventSource
	RawPayload string
}

type NormalizedState struct {
	State      model.CanonicalState
	Reason     string
	Confidence string
}

type Adapter interface {
	Definition() Definition
	Normalize(signal Signal) (NormalizedState, bool)
}

type Registry struct {
	mu      sync.RWMutex
	byAgent map[string]Adapter
}

func NewRegistry(adapters ...Adapter) *Registry {
	r := &Registry{
		byAgent: map[string]Adapter{},
	}
	for _, a := range adapters {
		_ = r.Register(a)
	}
	return r
}

func DefaultRegistry() *Registry {
	return NewRegistry(
		NewClaudeAdapter(),
		NewCodexAdapter(),
		NewGeminiAdapter(),
	)
}

func (r *Registry) Register(adapter Adapter) error {
	if adapter == nil {
		return fmt.Errorf("adapter is nil")
	}
	def := adapter.Definition()
	agentType := normalizeAgentType(def.AgentType)
	if agentType == "" {
		return fmt.Errorf("agent_type is required")
	}
	if strings.TrimSpace(def.ContractVersion) == "" {
		return fmt.Errorf("contract_version is required")
	}
	if !IsVersionCompatible(def.ContractVersion) {
		return fmt.Errorf("unsupported contract_version=%s", def.ContractVersion)
	}

	r.mu.Lock()
	defer r.mu.Unlock()
	if _, exists := r.byAgent[agentType]; exists {
		return fmt.Errorf("adapter already registered for agent_type=%s", agentType)
	}
	r.byAgent[agentType] = adapter
	return nil
}

func (r *Registry) Resolve(agentType string) (Adapter, bool) {
	if r == nil {
		return nil, false
	}
	normalized := normalizeAgentType(agentType)
	if normalized == "" {
		return nil, false
	}
	r.mu.RLock()
	defer r.mu.RUnlock()
	a, ok := r.byAgent[normalized]
	return a, ok
}

func (r *Registry) Normalize(agentType string, signal Signal) (NormalizedState, bool) {
	a, ok := r.Resolve(agentType)
	if !ok {
		return NormalizedState{}, false
	}
	return a.Normalize(signal)
}

func (r *Registry) Definitions() []Definition {
	if r == nil {
		return nil
	}
	r.mu.RLock()
	defer r.mu.RUnlock()

	defs := make([]Definition, 0, len(r.byAgent))
	for _, a := range r.byAgent {
		def := a.Definition()
		def.AgentType = normalizeAgentType(def.AgentType)
		def.Capabilities = append([]string(nil), def.Capabilities...)
		sort.Strings(def.Capabilities)
		defs = append(defs, def)
	}
	sort.Slice(defs, func(i, j int) bool {
		return defs[i].AgentType < defs[j].AgentType
	})
	return defs
}

func normalizeAgentType(agentType string) string {
	return strings.ToLower(strings.TrimSpace(agentType))
}

func IsVersionCompatible(version string) bool {
	major, ok := contractMajor(version)
	return ok && major == supportedContractMajor
}

func contractMajor(version string) (int, bool) {
	v := strings.ToLower(strings.TrimSpace(version))
	if v == "" || !strings.HasPrefix(v, "v") {
		return 0, false
	}
	v = strings.TrimPrefix(v, "v")
	if v == "" {
		return 0, false
	}
	parts := strings.SplitN(v, ".", 2)
	major, err := strconv.Atoi(parts[0])
	if err != nil || major <= 0 {
		return 0, false
	}
	return major, true
}
