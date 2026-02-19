package provideradapters

import "github.com/g960059/agtmux/internal/stateengine"

type Registry struct {
	adapters []stateengine.ProviderAdapter
}

func NewRegistry(adapters ...stateengine.ProviderAdapter) *Registry {
	filtered := make([]stateengine.ProviderAdapter, 0, len(adapters))
	for _, adapter := range adapters {
		if adapter == nil {
			continue
		}
		filtered = append(filtered, adapter)
	}
	return &Registry{adapters: filtered}
}

func DefaultRegistry() *Registry {
	return NewRegistry(
		NewClaudeAdapter(),
		NewCodexAdapter(),
		NewGeminiAdapter(),
		NewCopilotAdapter(),
	)
}

func (r *Registry) Adapters() []stateengine.ProviderAdapter {
	if r == nil {
		return nil
	}
	return append([]stateengine.ProviderAdapter(nil), r.adapters...)
}
