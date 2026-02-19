package stateengine

import (
	"crypto/sha1"
	"encoding/hex"
	"fmt"
	"sort"
	"strings"
	"time"
)

type Engine struct {
	cfg      EngineConfig
	registry AdapterRegistry
}

func NewEngine(registry AdapterRegistry) *Engine {
	return &Engine{
		cfg:      DefaultConfig(),
		registry: registry,
	}
}

func (e *Engine) Evaluate(meta PaneMeta, now time.Time) Evaluation {
	if now.IsZero() {
		now = time.Now().UTC()
	}
	provider, providerConfidence := e.detectProvider(meta)
	presence := deriveAgentPresence(meta.AgentType, provider)
	if presence == AgentPresenceUnmanaged {
		return Evaluation{
			Provider:           ProviderNone,
			ProviderConfidence: 1,
			AgentPresence:      presence,
			ActivityState:      ActivityUnknown,
			ActivityConfidence: 1,
			ActivitySource:     "unmanaged",
			ActivityReasons:    []string{"unmanaged_agent"},
			EvidenceTraceID:    traceID(meta.TargetID, meta.PaneID, ProviderNone, nil),
		}
	}

	evidence := make([]Evidence, 0, 8)
	if adapter := e.lookupAdapter(provider); adapter != nil {
		evidence = append(evidence, adapter.BuildEvidence(meta, now)...)
	}
	evidence = append(evidence, buildRawStateEvidence(meta, now, e.cfg)...)
	activity, score, source, reasons := e.resolve(evidence, meta, now)
	if activity == ActivityUnknown && presence == AgentPresenceManaged {
		// managed pane prefers stable idle over noisy unknown when no strong evidence exists.
		activity = ActivityIdle
		source = "fallback"
		reasons = []string{"managed_no_strong_signal"}
		if score < 0.3 {
			score = 0.3
		}
	}

	return Evaluation{
		Provider:           provider,
		ProviderConfidence: providerConfidence,
		AgentPresence:      presence,
		ActivityState:      activity,
		ActivityConfidence: clamp01(score),
		ActivitySource:     source,
		ActivityReasons:    reasons,
		EvidenceTraceID:    traceID(meta.TargetID, meta.PaneID, provider, evidence),
	}
}

func (e *Engine) resolve(evidence []Evidence, meta PaneMeta, now time.Time) (activity string, score float64, source string, reasons []string) {
	type evidenceScore struct {
		activity string
		score    float64
		source   string
		reason   string
	}
	scores := map[string]float64{}
	primary := map[string]evidenceScore{}
	for _, ev := range evidence {
		if CanonicalActivity(ev.Signal) == ActivityUnknown {
			continue
		}
		ts := ev.Timestamp
		if ts.IsZero() {
			ts = now
		}
		ttl := ev.TTL
		if ttl <= 0 {
			ttl = e.cfg.DefaultEvidenceTTL
		}
		if now.Sub(ts) > ttl {
			continue
		}
		rawScore := clamp01(ev.Weight * ev.Confidence)
		if rawScore <= 0 {
			continue
		}
		signal := CanonicalActivity(ev.Signal)
		scores[signal] += rawScore
		current := primary[signal]
		if rawScore >= current.score {
			primary[signal] = evidenceScore{
				activity: signal,
				score:    rawScore,
				source:   strings.TrimSpace(ev.Source),
				reason:   strings.TrimSpace(ev.ReasonCode),
			}
		}
	}

	activity, score = resolveActivityState(scores, e.cfg)
	if activity == ActivityUnknown {
		return activity, 0, "none", []string{"no_active_evidence"}
	}
	best := primary[activity]
	source = best.source
	if source == "" {
		source = "composite"
	}
	if best.reason != "" {
		reasons = append(reasons, best.reason)
	}
	if raw := strings.TrimSpace(meta.RawReasonCode); raw != "" && raw != best.reason {
		reasons = append(reasons, "raw:"+raw)
	}
	if len(reasons) == 0 {
		reasons = append(reasons, "composite_score")
	}
	return activity, score, source, reasons
}

func (e *Engine) detectProvider(meta PaneMeta) (string, float64) {
	bestProvider := ProviderUnknown
	bestConfidence := 0.0
	if e.registry == nil {
		return fallbackProvider(meta), 0.4
	}
	for _, adapter := range e.registry.Adapters() {
		confidence, ok := adapter.DetectProvider(meta)
		if !ok {
			continue
		}
		confidence = clamp01(confidence)
		if confidence > bestConfidence {
			bestConfidence = confidence
			bestProvider = NormalizeProvider(adapter.ID())
		}
	}
	if bestProvider == ProviderUnknown {
		return fallbackProvider(meta), 0.4
	}
	return bestProvider, bestConfidence
}

func (e *Engine) lookupAdapter(provider string) ProviderAdapter {
	if e.registry == nil {
		return nil
	}
	provider = NormalizeProvider(provider)
	for _, adapter := range e.registry.Adapters() {
		if NormalizeProvider(adapter.ID()) == provider {
			return adapter
		}
	}
	return nil
}

func buildRawStateEvidence(meta PaneMeta, now time.Time, cfg EngineConfig) []Evidence {
	activity := CanonicalActivity(meta.RawState)
	if activity == ActivityUnknown {
		return nil
	}
	source := strings.ToLower(strings.TrimSpace(meta.StateSource))
	scoreWeight := sourceWeight(source)
	conf := confidenceWeight(strings.ToLower(strings.TrimSpace(meta.RawConfidence)))
	ts := now
	if meta.LastEventAt != nil && !meta.LastEventAt.IsZero() {
		ts = meta.LastEventAt.UTC()
	} else if !meta.UpdatedAt.IsZero() {
		ts = meta.UpdatedAt.UTC()
	}
	ttl := ttlForConfidence(cfg, strings.ToLower(strings.TrimSpace(meta.RawConfidence)))
	reason := strings.TrimSpace(meta.RawReasonCode)
	if reason == "" {
		reason = "raw_state"
	}
	return []Evidence{
		{
			Provider:   fallbackProvider(meta),
			Kind:       kindFromStateSource(source),
			Signal:     activity,
			Weight:     scoreWeight,
			Confidence: conf,
			Timestamp:  ts,
			TTL:        ttl,
			Source:     source,
			ReasonCode: "raw:" + reason,
		},
	}
}

func kindFromStateSource(source string) EvidenceKind {
	switch source {
	case "hook":
		return EvidenceHook
	case "notify":
		return EvidenceProtocol
	case "wrapper":
		return EvidenceWrapper
	case "poller":
		return EvidenceCapture
	default:
		return EvidenceHeuristic
	}
}

func deriveAgentPresence(agentType string, provider string) string {
	agent := strings.ToLower(strings.TrimSpace(agentType))
	if agent == "" || agent == ProviderUnknown {
		if provider == ProviderUnknown {
			return AgentPresenceUnknown
		}
		return AgentPresenceManaged
	}
	if agent == "none" || agent == "unmanaged" || provider == ProviderNone {
		return AgentPresenceUnmanaged
	}
	return AgentPresenceManaged
}

func fallbackProvider(meta PaneMeta) string {
	agent := NormalizeProvider(meta.AgentType)
	if agent != ProviderUnknown {
		return agent
	}
	cmd := strings.ToLower(strings.TrimSpace(meta.CurrentCmd))
	switch {
	case strings.Contains(cmd, "claude"):
		return ProviderClaude
	case strings.Contains(cmd, "codex"):
		return ProviderCodex
	case strings.Contains(cmd, "gemini"):
		return ProviderGemini
	case strings.Contains(cmd, "copilot"):
		return ProviderCopilot
	default:
		return ProviderUnknown
	}
}

func traceID(targetID, paneID, provider string, evidence []Evidence) string {
	base := fmt.Sprintf("%s|%s|%s", strings.TrimSpace(targetID), strings.TrimSpace(paneID), strings.TrimSpace(provider))
	parts := make([]string, 0, len(evidence))
	for _, ev := range evidence {
		parts = append(parts, fmt.Sprintf("%s:%s:%s", ev.Kind, ev.Signal, ev.ReasonCode))
	}
	sort.Strings(parts)
	if len(parts) > 0 {
		base += "|" + strings.Join(parts, ",")
	}
	sum := sha1.Sum([]byte(base))
	return hex.EncodeToString(sum[:8])
}
