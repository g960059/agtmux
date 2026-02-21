package ttyv2

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"errors"
	"testing"
)

func TestCodecRoundTrip(t *testing.T) {
	env, err := NewEnvelope("hello", 1, "req-1", HelloPayload{
		ClientID:         "agtmux-desktop",
		ProtocolVersions: []string{SchemaVersion},
		Capabilities:     []string{"raw_output"},
	})
	if err != nil {
		t.Fatalf("new envelope: %v", err)
	}

	var buf bytes.Buffer
	if err := WriteFrame(&buf, env); err != nil {
		t.Fatalf("write frame: %v", err)
	}
	decoded, err := ReadFrame(&buf, DefaultMaxFrame)
	if err != nil {
		t.Fatalf("read frame: %v", err)
	}

	if decoded.Type != "hello" {
		t.Fatalf("unexpected type: %s", decoded.Type)
	}
	var payload HelloPayload
	if err := decoded.DecodePayload(&payload); err != nil {
		t.Fatalf("decode payload: %v", err)
	}
	if payload.ClientID != "agtmux-desktop" {
		t.Fatalf("unexpected payload: %+v", payload)
	}
}

func TestReadFrameRejectsOversized(t *testing.T) {
	body := bytes.Repeat([]byte{'x'}, 64)
	var buf bytes.Buffer
	var lenBuf [4]byte
	binary.BigEndian.PutUint32(lenBuf[:], uint32(len(body)))
	buf.Write(lenBuf[:])
	buf.Write(body)

	_, err := ReadFrame(&buf, 32)
	if !errors.Is(err, ErrFrameTooLarge) {
		t.Fatalf("expected ErrFrameTooLarge, got %v", err)
	}
}

func TestEnvelopeValidateRejectsInvalidVersion(t *testing.T) {
	raw, err := json.Marshal(map[string]any{"ok": true})
	if err != nil {
		t.Fatalf("marshal payload: %v", err)
	}
	env := Envelope{
		SchemaVersion: "v1",
		Type:          "hello",
		FrameSeq:      1,
		Payload:       raw,
	}
	if err := env.Validate(); !errors.Is(err, ErrUnsupportedVers) {
		t.Fatalf("expected ErrUnsupportedVers, got %v", err)
	}
}

func TestPaneRefValidation(t *testing.T) {
	pref := PaneRef{Target: "local", SessionName: "s", WindowID: "@1", PaneID: "%2"}
	if !pref.IsValid() {
		t.Fatalf("expected pane ref valid")
	}
	if pref.CanonicalKey() != "local|s|@1|%2" {
		t.Fatalf("unexpected key: %s", pref.CanonicalKey())
	}
}
