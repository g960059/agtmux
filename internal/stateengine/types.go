package stateengine

import (
	"strings"
	"time"
)

const (
	ProviderClaude  = "claude"
	ProviderCodex   = "codex"
	ProviderGemini  = "gemini"
	ProviderCopilot = "copilot"
	ProviderNone    = "none"
	ProviderUnknown = "unknown"
)

const (
	AgentPresenceManaged   = "managed"
	AgentPresenceUnmanaged = "unmanaged"
	AgentPresenceUnknown   = "unknown"
)

const (
	ActivityRunning         = "running"
	ActivityWaitingInput    = "waiting_input"
	ActivityWaitingApproval = "waiting_approval"
	ActivityIdle            = "idle"
	ActivityError           = "error"
	ActivityUnknown         = "unknown"
)

const (
	AttentionNone                   = "none"
	AttentionActionRequiredInput    = "action_required_input"
	AttentionActionRequiredApproval = "action_required_approval"
	AttentionActionRequiredError    = "action_required_error"
	AttentionInformationalCompleted = "informational_completed"
)

type SessionTime struct {
	At         *time.Time
	Source     string
	Confidence float64
}

type EvidenceKind string

const (
	EvidenceHook        EvidenceKind = "hook"
	EvidenceProtocol    EvidenceKind = "protocol"
	EvidenceWrapper     EvidenceKind = "wrapper"
	EvidenceTmuxControl EvidenceKind = "tmux_control"
	EvidenceCapture     EvidenceKind = "capture"
	EvidenceHeuristic   EvidenceKind = "heuristic"
)

type Evidence struct {
	Provider   string
	Kind       EvidenceKind
	Signal     string
	Weight     float64
	Confidence float64
	Timestamp  time.Time
	TTL        time.Duration
	Source     string
	ReasonCode string
	RawExcerpt string
}

type PaneMeta struct {
	TargetID          string
	PaneID            string
	RuntimeID         string
	AgentType         string
	CurrentCmd        string
	PaneTitle         string
	SessionLabel      string
	RawState          string
	RawReasonCode     string
	RawConfidence     string
	StateSource       string
	LastEventType     string
	LastEventAt       *time.Time
	LastInteractionAt *time.Time
	UpdatedAt         time.Time
}

type Evaluation struct {
	Provider           string
	ProviderConfidence float64
	AgentPresence      string
	ActivityState      string
	ActivityConfidence float64
	ActivitySource     string
	ActivityReasons    []string
	EvidenceTraceID    string
}

type ProviderAdapter interface {
	ID() string
	DetectProvider(meta PaneMeta) (confidence float64, ok bool)
	BuildEvidence(meta PaneMeta, now time.Time) []Evidence
}

type AdapterRegistry interface {
	Adapters() []ProviderAdapter
}

type EngineConfig struct {
	MinScore             float64
	RunningEnterScore    float64
	MinStableDuration    time.Duration
	DefaultEvidenceTTL   time.Duration
	HighConfidenceTTL    time.Duration
	LowConfidenceTTL     time.Duration
	StrongSourceBonus    float64
	WeakSourceMultiplier float64
}

func DefaultConfig() EngineConfig {
	return EngineConfig{
		MinScore:             0.35,
		RunningEnterScore:    0.62,
		MinStableDuration:    1500 * time.Millisecond,
		DefaultEvidenceTTL:   90 * time.Second,
		HighConfidenceTTL:    180 * time.Second,
		LowConfidenceTTL:     30 * time.Second,
		StrongSourceBonus:    0.15,
		WeakSourceMultiplier: 0.75,
	}
}

func NormalizeProvider(provider string) string {
	switch strings.ToLower(strings.TrimSpace(provider)) {
	case ProviderClaude:
		return ProviderClaude
	case ProviderCodex:
		return ProviderCodex
	case ProviderGemini:
		return ProviderGemini
	case ProviderCopilot:
		return ProviderCopilot
	case ProviderNone:
		return ProviderNone
	default:
		return ProviderUnknown
	}
}

func CanonicalActivity(activity string) string {
	switch strings.ToLower(strings.TrimSpace(activity)) {
	case ActivityRunning:
		return ActivityRunning
	case ActivityWaitingInput:
		return ActivityWaitingInput
	case ActivityWaitingApproval:
		return ActivityWaitingApproval
	case ActivityIdle:
		return ActivityIdle
	case ActivityError:
		return ActivityError
	default:
		return ActivityUnknown
	}
}
