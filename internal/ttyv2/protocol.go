package ttyv2

import (
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strings"
	"time"
)

const (
	SchemaVersion   = "tty.v2.0"
	DefaultMaxFrame = 1 << 20 // 1 MiB
)

var (
	ErrInvalidFrame    = errors.New("ttyv2: invalid frame")
	ErrFrameTooLarge   = errors.New("ttyv2: frame too large")
	ErrUnsupportedVers = errors.New("ttyv2: unsupported schema version")
)

type Envelope struct {
	SchemaVersion string          `json:"schema_version"`
	Type          string          `json:"type"`
	FrameSeq      uint64          `json:"frame_seq"`
	SentAt        time.Time       `json:"sent_at"`
	RequestID     string          `json:"request_id,omitempty"`
	Payload       json.RawMessage `json:"payload"`
}

func NewEnvelope(frameType string, frameSeq uint64, requestID string, payload any) (Envelope, error) {
	if strings.TrimSpace(frameType) == "" {
		return Envelope{}, fmt.Errorf("%w: type is required", ErrInvalidFrame)
	}
	body, err := json.Marshal(payload)
	if err != nil {
		return Envelope{}, fmt.Errorf("marshal payload: %w", err)
	}
	return Envelope{
		SchemaVersion: SchemaVersion,
		Type:          strings.TrimSpace(frameType),
		FrameSeq:      frameSeq,
		SentAt:        time.Now().UTC(),
		RequestID:     strings.TrimSpace(requestID),
		Payload:       body,
	}, nil
}

func (e Envelope) Validate() error {
	if strings.TrimSpace(e.SchemaVersion) != SchemaVersion {
		return ErrUnsupportedVers
	}
	if strings.TrimSpace(e.Type) == "" {
		return fmt.Errorf("%w: type is required", ErrInvalidFrame)
	}
	if len(e.Payload) == 0 {
		return fmt.Errorf("%w: payload is required", ErrInvalidFrame)
	}
	return nil
}

func (e Envelope) DecodePayload(dst any) error {
	if len(e.Payload) == 0 {
		return fmt.Errorf("%w: empty payload", ErrInvalidFrame)
	}
	if err := json.Unmarshal(e.Payload, dst); err != nil {
		return fmt.Errorf("decode payload: %w", err)
	}
	return nil
}

func WriteFrame(w io.Writer, env Envelope) error {
	if err := env.Validate(); err != nil {
		return err
	}
	body, err := json.Marshal(env)
	if err != nil {
		return fmt.Errorf("marshal frame: %w", err)
	}
	if len(body) > DefaultMaxFrame {
		return ErrFrameTooLarge
	}
	var lenBuf [4]byte
	binary.BigEndian.PutUint32(lenBuf[:], uint32(len(body)))
	if _, err := w.Write(lenBuf[:]); err != nil {
		return fmt.Errorf("write frame length: %w", err)
	}
	if _, err := w.Write(body); err != nil {
		return fmt.Errorf("write frame body: %w", err)
	}
	return nil
}

func ReadFrame(r io.Reader, maxFrameSize int) (Envelope, error) {
	limit := maxFrameSize
	if limit <= 0 {
		limit = DefaultMaxFrame
	}
	var lenBuf [4]byte
	if _, err := io.ReadFull(r, lenBuf[:]); err != nil {
		return Envelope{}, fmt.Errorf("read frame length: %w", err)
	}
	size := int(binary.BigEndian.Uint32(lenBuf[:]))
	if size <= 0 || size > limit {
		return Envelope{}, ErrFrameTooLarge
	}
	body := make([]byte, size)
	if _, err := io.ReadFull(r, body); err != nil {
		return Envelope{}, fmt.Errorf("read frame body: %w", err)
	}
	var env Envelope
	if err := json.Unmarshal(body, &env); err != nil {
		return Envelope{}, fmt.Errorf("decode frame: %w", err)
	}
	if err := env.Validate(); err != nil {
		return Envelope{}, err
	}
	return env, nil
}

type PaneRef struct {
	Target      string `json:"target"`
	SessionName string `json:"session_name"`
	WindowID    string `json:"window_id"`
	PaneID      string `json:"pane_id"`
}

func (p PaneRef) CanonicalKey() string {
	return strings.TrimSpace(p.Target) + "|" + strings.TrimSpace(p.SessionName) + "|" + strings.TrimSpace(p.WindowID) + "|" + strings.TrimSpace(p.PaneID)
}

func (p PaneRef) IsValid() bool {
	return strings.TrimSpace(p.Target) != "" &&
		strings.TrimSpace(p.SessionName) != "" &&
		strings.TrimSpace(p.WindowID) != "" &&
		strings.TrimSpace(p.PaneID) != ""
}

type TTYState struct {
	ActivityState       string `json:"activity_state"`
	AttentionState      string `json:"attention_state"`
	SessionLastActiveAt string `json:"session_last_active_at,omitempty"`
}

type HelloPayload struct {
	ClientID         string   `json:"client_id"`
	ProtocolVersions []string `json:"protocol_versions"`
	Capabilities     []string `json:"capabilities,omitempty"`
}

type HelloAckPayload struct {
	ServerID        string   `json:"server_id"`
	ProtocolVersion string   `json:"protocol_version"`
	Features        []string `json:"features,omitempty"`
}

type AttachPayload struct {
	PaneRef             PaneRef `json:"pane_ref"`
	AttachMode          string  `json:"attach_mode,omitempty"`
	WantInitialSnapshot *bool   `json:"want_initial_snapshot,omitempty"`
	Cols                *int    `json:"cols,omitempty"`
	Rows                *int    `json:"rows,omitempty"`
}

type AttachedPayload struct {
	PaneRef                PaneRef  `json:"pane_ref"`
	PaneAlias              string   `json:"pane_alias,omitempty"`
	OutputSeq              uint64   `json:"output_seq"`
	InitialSnapshotANSIB64 string   `json:"initial_snapshot_ansi_base64,omitempty"`
	SnapshotMode           string   `json:"snapshot_mode,omitempty"`
	CursorX                *int     `json:"cursor_x,omitempty"`
	CursorY                *int     `json:"cursor_y,omitempty"`
	PaneCols               *int     `json:"pane_cols,omitempty"`
	PaneRows               *int     `json:"pane_rows,omitempty"`
	State                  TTYState `json:"state"`
}

type WritePayload struct {
	PaneRef     PaneRef `json:"pane_ref"`
	InputSeq    uint64  `json:"input_seq"`
	BytesBase64 string  `json:"bytes_base64"`
}

type ResizePayload struct {
	PaneRef   PaneRef `json:"pane_ref"`
	ResizeSeq uint64  `json:"resize_seq"`
	Cols      int     `json:"cols"`
	Rows      int     `json:"rows"`
}

type FocusPayload struct {
	PaneRef PaneRef `json:"pane_ref"`
}

type DetachPayload struct {
	PaneRef PaneRef `json:"pane_ref"`
}

type ResyncPayload struct {
	PaneRef PaneRef `json:"pane_ref"`
	Reason  string  `json:"reason,omitempty"`
}

type PingPayload struct {
	TS string `json:"ts"`
}

type PongPayload struct {
	TS string `json:"ts"`
}

type OutputPayload struct {
	PaneRef          *PaneRef `json:"pane_ref,omitempty"`
	PaneAlias        string   `json:"pane_alias,omitempty"`
	OutputSeq        uint64   `json:"output_seq"`
	BytesBase64      string   `json:"bytes_base64"`
	Source           string   `json:"source,omitempty"`
	CursorX          *int     `json:"cursor_x,omitempty"`
	CursorY          *int     `json:"cursor_y,omitempty"`
	PaneCols         *int     `json:"pane_cols,omitempty"`
	PaneRows         *int     `json:"pane_rows,omitempty"`
	Coalesced        bool     `json:"coalesced,omitempty"`
	CoalescedFromSeq uint64   `json:"coalesced_from_seq,omitempty"`
	DroppedChunks    int      `json:"dropped_chunks,omitempty"`
}

type StatePayload struct {
	PaneRef PaneRef  `json:"pane_ref"`
	State   TTYState `json:"state"`
}

type AckPayload struct {
	PaneRef    *PaneRef `json:"pane_ref,omitempty"`
	AckKind    string   `json:"ack_kind"`
	InputSeq   uint64   `json:"input_seq,omitempty"`
	ResizeSeq  uint64   `json:"resize_seq,omitempty"`
	ResultCode string   `json:"result_code"`
}

type ErrorPayload struct {
	Code        string   `json:"code"`
	Message     string   `json:"message"`
	Recoverable bool     `json:"recoverable"`
	PaneRef     *PaneRef `json:"pane_ref,omitempty"`
}
