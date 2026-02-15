package main

import (
	"testing"
	"time"
)

func TestStabilizePaneEventIdleNeedsStabilityWindow(t *testing.T) {
	now := time.Now().UTC()
	stability := map[string]paneOutputStability{}

	inf := paneInference{
		EventType: "idle",
		Signature: 100,
		HasOutput: true,
	}

	if got := stabilizePaneEvent("t1", "%1", inf, now, 2*time.Second, stability); got != "unknown" {
		t.Fatalf("first idle observation should be unknown, got %q", got)
	}
	if got := stabilizePaneEvent("t1", "%1", inf, now.Add(2*time.Second), 2*time.Second, stability); got != "unknown" {
		t.Fatalf("idle before stability window should stay unknown, got %q", got)
	}
	if got := stabilizePaneEvent("t1", "%1", inf, now.Add(5*time.Second), 2*time.Second, stability); got != "idle" {
		t.Fatalf("idle after stability window should stay idle, got %q", got)
	}
}

func TestStabilizePaneEventOutputChangePromotesRunning(t *testing.T) {
	now := time.Now().UTC()
	stability := map[string]paneOutputStability{}

	first := paneInference{EventType: "idle", Signature: 10, HasOutput: true}
	second := paneInference{EventType: "idle", Signature: 11, HasOutput: true}

	_ = stabilizePaneEvent("t1", "%1", first, now, 2*time.Second, stability)
	if got := stabilizePaneEvent("t1", "%1", second, now.Add(1*time.Second), 2*time.Second, stability); got != "running" {
		t.Fatalf("changed output should promote to running, got %q", got)
	}
}

func TestStabilizePaneEventNoAgentClearsTracker(t *testing.T) {
	now := time.Now().UTC()
	stability := map[string]paneOutputStability{
		topologyPaneKey("t1", "%1"): {
			Signature:    1,
			LastChangeAt: now,
			LastSeenAt:   now,
		},
	}

	got := stabilizePaneEvent("t1", "%1", paneInference{EventType: "no-agent"}, now.Add(time.Second), 2*time.Second, stability)
	if got != "no-agent" {
		t.Fatalf("no-agent expected, got %q", got)
	}
	if _, ok := stability[topologyPaneKey("t1", "%1")]; ok {
		t.Fatalf("stability entry should be cleared for no-agent")
	}
}

func TestCleanupStabilityEntriesRemovesExpiredUnseen(t *testing.T) {
	now := time.Now().UTC()
	stability := map[string]paneOutputStability{
		"keep": {
			Signature:    1,
			LastChangeAt: now.Add(-2 * time.Second),
			LastSeenAt:   now.Add(-2 * time.Second),
		},
		"drop": {
			Signature:    2,
			LastChangeAt: now.Add(-40 * time.Second),
			LastSeenAt:   now.Add(-40 * time.Second),
		},
	}

	seen := map[string]struct{}{
		"keep": {},
	}
	cleanupStabilityEntries(stability, seen, now, 2*time.Second)

	if _, ok := stability["keep"]; !ok {
		t.Fatalf("keep entry should remain")
	}
	if _, ok := stability["drop"]; ok {
		t.Fatalf("expired unseen entry should be removed")
	}
}
