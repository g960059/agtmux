package security_test

import (
	"strings"
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/ingest"
	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/security"
	"github.com/g960059/agtmux/internal/testutil"
)

func TestRedactPayload(t *testing.T) {
	in := `token=abc123 access_token="quoted-token" password:supersecret password='quoted-pass' Authorization: Basic dXNlcjpwYXNz {"refresh_token":"jsonsecret","api_key":"jsonkey"}`
	out := security.RedactPayload(in)
	if strings.Contains(out, "abc123") || strings.Contains(out, "quoted-token") || strings.Contains(out, "supersecret") || strings.Contains(out, "quoted-pass") ||
		strings.Contains(out, "dXNlcjpwYXNz") ||
		strings.Contains(out, "jsonsecret") || strings.Contains(out, "jsonkey") {
		t.Fatalf("secret value leaked after redaction: %q", out)
	}
	if !strings.Contains(out, "[REDACTED]") {
		t.Fatalf("expected redaction marker in output: %q", out)
	}
}

func TestRedactPayloadCoversAdditionalSecretFormats(t *testing.T) {
	in := "client_secret abc123 bearer tokenxyz cookie: sessionid=abc private_key: xyz"
	out := security.RedactPayload(in)
	if strings.Contains(out, "abc123") || strings.Contains(out, "tokenxyz") || strings.Contains(out, "sessionid=abc") || strings.Contains(out, "xyz") {
		t.Fatalf("secret value leaked after extended redaction: %q", out)
	}
}

func TestRedactPayloadCookieHeaderFullyRedacted(t *testing.T) {
	in := "Cookie: foo=bar; sessionid=secret; csrftoken=token"
	out := security.RedactPayload(in)
	if strings.Contains(out, "foo=bar") || strings.Contains(out, "sessionid=secret") || strings.Contains(out, "csrftoken=token") {
		t.Fatalf("cookie header value leaked after redaction: %q", out)
	}
}

func TestRedactPayloadPrivateKeyBlock(t *testing.T) {
	in := "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----"
	out := security.RedactPayload(in)
	if strings.Contains(out, "OPENSSH PRIVATE KEY") || strings.Contains(out, "\nabc\n") {
		t.Fatalf("private key block should be redacted, got: %q", out)
	}
}

func TestRedactForStorageDropsUnsafePayload(t *testing.T) {
	in := "sessionid=plain-secret"
	out := security.RedactForStorage(in)
	if out != "" {
		t.Fatalf("expected unsafe payload to be dropped, got: %q", out)
	}
}

func TestRedactForStorageDropsUnchangedPayload(t *testing.T) {
	in := "normal status update without secrets"
	out := security.RedactForStorage(in)
	if out != "" {
		t.Fatalf("expected unchanged payload to be dropped in fail-closed mode, got: %q", out)
	}
}

func TestUnredactedPayloadNeverPersisted(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := ingest.NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")

	err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "e-secret",
		EventType:  "running",
		Source:     model.SourceHook,
		DedupeKey:  "dedupe-secret",
		RuntimeID:  rt.RuntimeID,
		EventTime:  time.Now().UTC(),
		IngestedAt: time.Now().UTC(),
		RawPayload: "api_key=live-secret-123",
	})
	if err != nil {
		t.Fatalf("ingest secret payload: %v", err)
	}

	payload, err := store.ReadEventPayload(ctx, "e-secret")
	if err != nil {
		t.Fatalf("read payload: %v", err)
	}
	if payload == nil {
		t.Fatalf("expected stored redacted payload")
	}
	if strings.Contains(*payload, "live-secret-123") {
		t.Fatalf("unredacted secret persisted: %q", *payload)
	}
}

func TestUnsafePayloadIsNotPersisted(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := ingest.NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")

	err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "e-unsafe-secret",
		EventType:  "running",
		Source:     model.SourceHook,
		DedupeKey:  "dedupe-unsafe-secret",
		RuntimeID:  rt.RuntimeID,
		EventTime:  time.Now().UTC(),
		IngestedAt: time.Now().UTC(),
		RawPayload: "sessionid=plain-secret",
	})
	if err != nil {
		t.Fatalf("ingest unsafe payload: %v", err)
	}

	payload, err := store.ReadEventPayload(ctx, "e-unsafe-secret")
	if err != nil {
		t.Fatalf("read payload: %v", err)
	}
	if payload != nil {
		t.Fatalf("expected unsafe payload to be dropped, got %q", *payload)
	}
}

func TestRetentionPurgeSafety(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := ingest.NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")

	old := time.Now().UTC().Add(-30 * 24 * time.Hour)
	newer := time.Now().UTC()

	for i, ts := range []time.Time{old, newer} {
		err := engine.Ingest(ctx, model.EventEnvelope{
			EventID:    []string{"old-event", "new-event"}[i],
			EventType:  "running",
			Source:     model.SourceNotify,
			DedupeKey:  []string{"old-key", "new-key"}[i],
			RuntimeID:  rt.RuntimeID,
			EventTime:  ts,
			IngestedAt: ts,
			RawPayload: "token=value",
		})
		if err != nil {
			t.Fatalf("ingest event %d: %v", i, err)
		}
	}

	if err := store.PurgeRetention(ctx, time.Now().UTC().Add(-14*24*time.Hour), time.Now().UTC().Add(-14*24*time.Hour)); err != nil {
		t.Fatalf("purge retention: %v", err)
	}

	if _, err := store.ReadEventPayload(ctx, "old-event"); err == nil {
		t.Fatalf("old event should be deleted by metadata retention")
	}
	payload, err := store.ReadEventPayload(ctx, "new-event")
	if err != nil {
		t.Fatalf("read new payload: %v", err)
	}
	if payload == nil {
		t.Fatalf("new payload should remain")
	}
}

func TestRetentionScrubsPendingInboxPayloads(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	cfg := config.DefaultConfig()
	engine := ingest.NewEngine(store, cfg)
	rt := testutil.SeedTargetPaneRuntime(t, store, ctx, "host", "%1")
	if _, err := store.DB().ExecContext(ctx, `DELETE FROM runtimes WHERE runtime_id = ?`, rt.RuntimeID); err != nil {
		t.Fatalf("delete runtime: %v", err)
	}

	old := time.Now().UTC().Add(-30 * 24 * time.Hour)
	if err := engine.Ingest(ctx, model.EventEnvelope{
		EventID:    "pending-old",
		EventType:  "running",
		Source:     model.SourceNotify,
		DedupeKey:  "pending-old-key",
		TargetID:   rt.TargetID,
		PaneID:     rt.PaneID,
		EventTime:  old,
		IngestedAt: old,
		RawPayload: "refresh_token=will-be-redacted",
	}); err != nil {
		t.Fatalf("ingest pending: %v", err)
	}

	if err := store.PurgeRetention(ctx, time.Now().UTC().Add(-14*24*time.Hour), time.Now().UTC().Add(-14*24*time.Hour)); err != nil {
		t.Fatalf("purge retention: %v", err)
	}

	var payload *string
	var nullable string
	err := store.DB().QueryRowContext(ctx, `SELECT COALESCE(raw_payload, '') FROM event_inbox WHERE dedupe_key = 'pending-old-key'`).Scan(&nullable)
	if err != nil {
		t.Fatalf("query pending inbox payload: %v", err)
	}
	if nullable != "" {
		payload = &nullable
	}
	if payload != nil {
		t.Fatalf("expected pending inbox payload to be scrubbed, got %q", *payload)
	}
}
