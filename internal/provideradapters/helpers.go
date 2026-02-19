package provideradapters

import (
	"strings"
	"time"

	"github.com/g960059/agtmux/internal/stateengine"
)

func normalizeForMatch(values ...string) string {
	var parts []string
	for _, value := range values {
		clean := strings.ToLower(strings.TrimSpace(value))
		if clean != "" {
			parts = append(parts, clean)
		}
	}
	return strings.Join(parts, " ")
}

func hasAnyToken(haystack string, tokens ...string) bool {
	if haystack == "" {
		return false
	}
	for _, token := range tokens {
		if token == "" {
			continue
		}
		if strings.Contains(haystack, strings.ToLower(strings.TrimSpace(token))) {
			return true
		}
	}
	return false
}

func detectByAgentOrCmd(meta stateengine.PaneMeta, provider string, cmdTokens ...string) (float64, bool) {
	agent := strings.ToLower(strings.TrimSpace(meta.AgentType))
	if agent == provider {
		return 1.0, true
	}
	cmd := strings.ToLower(strings.TrimSpace(meta.CurrentCmd))
	for _, token := range cmdTokens {
		token = strings.ToLower(strings.TrimSpace(token))
		if token != "" && strings.Contains(cmd, token) {
			return 0.86, true
		}
	}
	label := normalizeForMatch(meta.PaneTitle, meta.SessionLabel)
	for _, token := range cmdTokens {
		token = strings.ToLower(strings.TrimSpace(token))
		if token != "" && strings.Contains(label, token) {
			return 0.66, true
		}
	}
	return 0, false
}

func buildEvidence(now time.Time, provider string, signal string, kind stateengine.EvidenceKind, source string, reason string, weight float64, confidence float64) stateengine.Evidence {
	return stateengine.Evidence{
		Provider:   provider,
		Signal:     signal,
		Kind:       kind,
		Weight:     weight,
		Confidence: confidence,
		Timestamp:  now,
		TTL:        120 * time.Second,
		Source:     source,
		ReasonCode: reason,
	}
}

func kindFromSource(stateSource string) stateengine.EvidenceKind {
	switch strings.ToLower(strings.TrimSpace(stateSource)) {
	case "hook":
		return stateengine.EvidenceHook
	case "wrapper":
		return stateengine.EvidenceWrapper
	case "notify":
		return stateengine.EvidenceProtocol
	case "poller":
		return stateengine.EvidenceCapture
	default:
		return stateengine.EvidenceHeuristic
	}
}
