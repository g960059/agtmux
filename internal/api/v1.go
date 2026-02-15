package api

import "time"

type APIError struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}

type ErrorResponse struct {
	SchemaVersion string    `json:"schema_version"`
	GeneratedAt   time.Time `json:"generated_at"`
	Error         APIError  `json:"error"`
}

type TargetResponse struct {
	TargetID      string  `json:"target_id"`
	TargetName    string  `json:"target_name"`
	Kind          string  `json:"kind"`
	ConnectionRef string  `json:"connection_ref"`
	IsDefault     bool    `json:"is_default"`
	LastSeenAt    *string `json:"last_seen_at,omitempty"`
	Health        string  `json:"health"`
	UpdatedAt     string  `json:"updated_at"`
}

type TargetsEnvelope struct {
	SchemaVersion string           `json:"schema_version"`
	GeneratedAt   time.Time        `json:"generated_at"`
	Targets       []TargetResponse `json:"targets"`
}

type AdapterResponse struct {
	AdapterName  string   `json:"adapter_name"`
	AgentType    string   `json:"agent_type"`
	Version      string   `json:"version"`
	Compatible   bool     `json:"compatible"`
	Capabilities []string `json:"capabilities"`
	Enabled      bool     `json:"enabled"`
	UpdatedAt    string   `json:"updated_at"`
}

type AdaptersEnvelope struct {
	SchemaVersion string            `json:"schema_version"`
	GeneratedAt   time.Time         `json:"generated_at"`
	Adapters      []AdapterResponse `json:"adapters"`
}

type TargetError struct {
	Target  string `json:"target"`
	Code    string `json:"code"`
	Message string `json:"message"`
}

type ListSummary struct {
	ByState    map[string]int `json:"by_state,omitempty"`
	ByAgent    map[string]int `json:"by_agent,omitempty"`
	ByTarget   map[string]int `json:"by_target,omitempty"`
	ByCategory map[string]int `json:"by_category,omitempty"`
}

type PaneIdentity struct {
	Target      string `json:"target"`
	SessionName string `json:"session_name"`
	WindowID    string `json:"window_id"`
	PaneID      string `json:"pane_id"`
}

type PaneItem struct {
	Identity        PaneIdentity `json:"identity"`
	WindowName      string       `json:"window_name,omitempty"`
	CurrentCmd      string       `json:"current_cmd,omitempty"`
	PaneTitle       string       `json:"pane_title,omitempty"`
	State           string       `json:"state"`
	ReasonCode      string       `json:"reason_code,omitempty"`
	Confidence      string       `json:"confidence,omitempty"`
	RuntimeID       string       `json:"runtime_id,omitempty"`
	AgentType       string       `json:"agent_type,omitempty"`
	AgentPresence   string       `json:"agent_presence,omitempty"`
	ActivityState   string       `json:"activity_state,omitempty"`
	DisplayCategory string       `json:"display_category,omitempty"`
	NeedsUserAction bool         `json:"needs_user_action,omitempty"`
	StateSource     string       `json:"state_source,omitempty"`
	LastEventType   string       `json:"last_event_type,omitempty"`
	LastEventAt     *string      `json:"last_event_at,omitempty"`
	AwaitingKind    string       `json:"awaiting_response_kind,omitempty"`
	SessionLabel    string       `json:"session_label,omitempty"`
	SessionLabelSrc string       `json:"session_label_source,omitempty"`
	LastInputAt     *string      `json:"last_interaction_at,omitempty"`
	UpdatedAt       string       `json:"updated_at"`
}

type WindowIdentity struct {
	Target      string `json:"target"`
	SessionName string `json:"session_name"`
	WindowID    string `json:"window_id"`
}

type WindowItem struct {
	Identity     WindowIdentity `json:"identity"`
	TopState     string         `json:"top_state"`
	TopCategory  string         `json:"top_category,omitempty"`
	ByCategory   map[string]int `json:"by_category,omitempty"`
	WaitingCount int            `json:"waiting_count"`
	RunningCount int            `json:"running_count"`
	TotalPanes   int            `json:"total_panes"`
}

type SessionIdentity struct {
	Target      string `json:"target"`
	SessionName string `json:"session_name"`
}

type SessionItem struct {
	Identity    SessionIdentity `json:"identity"`
	TopCategory string          `json:"top_category,omitempty"`
	TotalPanes  int             `json:"total_panes"`
	ByState     map[string]int  `json:"by_state"`
	ByAgent     map[string]int  `json:"by_agent"`
	ByCategory  map[string]int  `json:"by_category,omitempty"`
}

type ListEnvelope[T any] struct {
	SchemaVersion    string         `json:"schema_version"`
	GeneratedAt      time.Time      `json:"generated_at"`
	Filters          map[string]any `json:"filters"`
	Summary          ListSummary    `json:"summary"`
	Partial          bool           `json:"partial"`
	RequestedTargets []string       `json:"requested_targets"`
	RespondedTargets []string       `json:"responded_targets"`
	TargetErrors     []TargetError  `json:"target_errors,omitempty"`
	Items            []T            `json:"items"`
}

type WatchLine struct {
	SchemaVersion string         `json:"schema_version"`
	GeneratedAt   time.Time      `json:"generated_at"`
	EmittedAt     time.Time      `json:"emitted_at"`
	StreamID      string         `json:"stream_id"`
	Cursor        string         `json:"cursor"`
	Scope         string         `json:"scope"`
	Type          string         `json:"type"`
	Sequence      int64          `json:"sequence"`
	Filters       map[string]any `json:"filters,omitempty"`
	Summary       ListSummary    `json:"summary"`
	Items         any            `json:"items,omitempty"`
	Changes       any            `json:"changes,omitempty"`
}

type ActionResponse struct {
	SchemaVersion string    `json:"schema_version"`
	GeneratedAt   time.Time `json:"generated_at"`
	ActionID      string    `json:"action_id"`
	ResultCode    string    `json:"result_code"`
	CompletedAt   *string   `json:"completed_at,omitempty"`
	ErrorCode     *string   `json:"error_code,omitempty"`
	Output        *string   `json:"output,omitempty"`
}

type ActionEventItem struct {
	EventID    string `json:"event_id"`
	ActionID   string `json:"action_id"`
	RuntimeID  string `json:"runtime_id"`
	EventType  string `json:"event_type"`
	Source     string `json:"source"`
	EventTime  string `json:"event_time"`
	IngestedAt string `json:"ingested_at"`
	DedupeKey  string `json:"dedupe_key"`
}

type ActionEventsEnvelope struct {
	SchemaVersion string            `json:"schema_version"`
	GeneratedAt   time.Time         `json:"generated_at"`
	ActionID      string            `json:"action_id"`
	Events        []ActionEventItem `json:"events"`
}

type EventIngestResponse struct {
	SchemaVersion string    `json:"schema_version"`
	GeneratedAt   time.Time `json:"generated_at"`
	EventID       string    `json:"event_id"`
	Status        string    `json:"status"`
	RuntimeID     string    `json:"runtime_id,omitempty"`
}
