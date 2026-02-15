package model

import "time"

// CanonicalState is the normalized runtime state persisted in the store.
type CanonicalState string

const (
	StateRunning         CanonicalState = "running"
	StateWaitingInput    CanonicalState = "waiting_input"
	StateWaitingApproval CanonicalState = "waiting_approval"
	StateCompleted       CanonicalState = "completed"
	StateIdle            CanonicalState = "idle"
	StateError           CanonicalState = "error"
	StateUnknown         CanonicalState = "unknown"
)

// StatePrecedence resolves competing candidate states.
var StatePrecedence = map[CanonicalState]int{
	StateError:           1,
	StateWaitingApproval: 2,
	StateWaitingInput:    3,
	StateRunning:         4,
	StateCompleted:       5,
	StateIdle:            6,
	StateUnknown:         7,
}

type EventSource string

const (
	SourceHook    EventSource = "hook"
	SourceNotify  EventSource = "notify"
	SourceWrapper EventSource = "wrapper"
	SourcePoller  EventSource = "poller"
)

type InboxStatus string

const (
	InboxPendingBind    InboxStatus = "pending_bind"
	InboxBound          InboxStatus = "bound"
	InboxDroppedUnbound InboxStatus = "dropped_unbound"
)

type TargetKind string

const (
	TargetKindLocal TargetKind = "local"
	TargetKindSSH   TargetKind = "ssh"
)

type TargetHealth string

const (
	TargetHealthOK       TargetHealth = "ok"
	TargetHealthDegraded TargetHealth = "degraded"
	TargetHealthDown     TargetHealth = "down"
)

type EventEnvelope struct {
	EventID       string
	EventType     string
	Source        EventSource
	DedupeKey     string
	SourceEventID string
	SourceSeq     *int64
	EventTime     time.Time
	IngestedAt    time.Time
	RuntimeID     string
	TargetID      string
	PaneID        string
	PID           *int64
	StartHint     *time.Time
	RawPayload    string
	ActionID      *string
}

type Runtime struct {
	RuntimeID        string
	TargetID         string
	PaneID           string
	TmuxServerBootID string
	PaneEpoch        int64
	AgentType        string
	PID              *int64
	StartedAt        time.Time
	EndedAt          *time.Time
}

type ActionType string

const (
	ActionTypeAttach     ActionType = "attach"
	ActionTypeSend       ActionType = "send"
	ActionTypeViewOutput ActionType = "view-output"
	ActionTypeKill       ActionType = "kill"
)

type Action struct {
	ActionID     string
	ActionType   ActionType
	RequestRef   string
	TargetID     string
	PaneID       string
	RuntimeID    *string
	RequestedAt  time.Time
	CompletedAt  *time.Time
	ResultCode   string
	ErrorCode    *string
	MetadataJSON *string
}

type ActionSnapshot struct {
	SnapshotID   string
	ActionID     string
	TargetID     string
	PaneID       string
	RuntimeID    string
	StateVersion int64
	ObservedAt   time.Time
	ExpiresAt    time.Time
	Nonce        string
}

type ActionEvent struct {
	EventID    string
	ActionID   string
	RuntimeID  string
	EventType  string
	Source     EventSource
	EventTime  time.Time
	IngestedAt time.Time
	DedupeKey  string
	RawPayload *string
}

type Pane struct {
	TargetID       string
	PaneID         string
	SessionName    string
	WindowID       string
	WindowName     string
	CurrentCmd     string
	CurrentPath    string
	PaneTitle      string
	HistoryBytes   int64
	LastActivityAt *time.Time
	CurrentPID     *int64
	TTY            string
	UpdatedAt      time.Time
}

type StateRow struct {
	TargetID      string
	PaneID        string
	RuntimeID     string
	State         CanonicalState
	ReasonCode    string
	Confidence    string
	StateVersion  int64
	StateSource   EventSource
	LastEventType string
	LastEventAt   *time.Time
	LastSourceSeq *int64
	LastSeenAt    time.Time
	UpdatedAt     time.Time
}

type Target struct {
	TargetID      string
	TargetName    string
	Kind          TargetKind
	ConnectionRef string
	IsDefault     bool
	LastSeenAt    *time.Time
	Health        TargetHealth
	UpdatedAt     time.Time
}

type AdapterRecord struct {
	AdapterName  string
	AgentType    string
	Version      string
	Capabilities []string
	Enabled      bool
	UpdatedAt    time.Time
}

// OrderKey is the sortable key used for deterministic apply order.
type OrderKey struct {
	HasSourceSeq bool
	SourceSeq    int64
	EventTime    time.Time
	IngestedAt   time.Time
	EventID      string
}

// ReconcileEventType marks synthetic events emitted by reconciler.
type ReconcileEventType string

const (
	ReconcileStaleDetected      ReconcileEventType = "stale_detected"
	ReconcileTargetHealthChange ReconcileEventType = "target_health_changed"
	ReconcileDemotionDue        ReconcileEventType = "demotion_due"
)

// Error codes defined by API contract.
const (
	ErrRefInvalid          = "E_REF_INVALID"
	ErrRefInvalidEncoding  = "E_REF_INVALID_ENCODING"
	ErrRefNotFound         = "E_REF_NOT_FOUND"
	ErrRefAmbiguous        = "E_REF_AMBIGUOUS"
	ErrRuntimeStale        = "E_RUNTIME_STALE"
	ErrPreconditionFailed  = "E_PRECONDITION_FAILED"
	ErrSnapshotExpired     = "E_SNAPSHOT_EXPIRED"
	ErrIdempotencyConflict = "E_IDEMPOTENCY_CONFLICT"
	ErrCursorInvalid       = "E_CURSOR_INVALID"
	ErrCursorExpired       = "E_CURSOR_EXPIRED"
	ErrPIDUnavailable      = "E_PID_UNAVAILABLE"
	ErrTargetUnreachable   = "E_TARGET_UNREACHABLE"
)
