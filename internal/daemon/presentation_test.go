package daemon

import (
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/model"
)

func TestDerivePaneSessionLabelPrefersCodexThreadHint(t *testing.T) {
	pane := model.Pane{
		PaneID:      "%1",
		SessionName: "exp/go-codex-implementation-poc",
	}
	label, source := derivePaneSessionLabel(
		"managed",
		pane,
		"rt-1",
		"t1|%1",
		map[string]actionInputHint{
			"rt-1": {preview: "fallback first input"},
		},
		nil,
		nil,
		"codex",
		codexThreadHint{label: "Fix status grouping in AGTMUX UI"},
		true,
	)
	if label != "Fix status grouping in AGTMUX UI" {
		t.Fatalf("expected codex thread label, got %q", label)
	}
	if source != "codex_thread_list" {
		t.Fatalf("expected codex_thread_list source, got %q", source)
	}
}

func TestDerivePaneLastInteractionAtManagedDoesNotFallbackToTmuxActivity(t *testing.T) {
	now := time.Now().UTC()
	lastActivity := now.Add(-10 * time.Second)
	updatedAt := now.Add(-2 * time.Second)
	got := derivePaneLastInteractionAt(
		"managed",
		"",
		"t1|%1",
		nil,
		nil,
		nil,
		"codex",
		codexThreadHint{},
		false,
		string(model.SourcePoller),
		"",
		nil,
		&lastActivity,
		updatedAt,
	)
	if got != nil {
		t.Fatalf("expected nil last interaction for managed pane without interaction signals, got %s", got.Format(time.RFC3339Nano))
	}
}

func TestDerivePaneLastInteractionAtUnmanagedFallsBackToLastActivity(t *testing.T) {
	now := time.Now().UTC()
	lastActivity := now.Add(-45 * time.Second)
	updatedAt := now.Add(-30 * time.Second)
	got := derivePaneLastInteractionAt(
		"none",
		"",
		"t1|%2",
		nil,
		nil,
		nil,
		"none",
		codexThreadHint{},
		false,
		string(model.SourcePoller),
		"",
		nil,
		&lastActivity,
		updatedAt,
	)
	if got == nil {
		t.Fatalf("expected last interaction timestamp")
	}
	if !got.Equal(lastActivity) {
		t.Fatalf("expected %s, got %s", lastActivity.Format(time.RFC3339Nano), got.Format(time.RFC3339Nano))
	}
}

func TestDerivePaneLastInteractionAtUsesCodexHintTimestamp(t *testing.T) {
	now := time.Now().UTC()
	hintTime := now.Add(-5 * time.Minute)
	got := derivePaneLastInteractionAt(
		"managed",
		"",
		"t1|%3",
		nil,
		nil,
		nil,
		"codex",
		codexThreadHint{
			label: "Investigate daemon reconnect behavior",
			at:    hintTime,
		},
		true,
		string(model.SourcePoller),
		"",
		nil,
		nil,
		now,
	)
	if got == nil {
		t.Fatalf("expected codex hint timestamp")
	}
	if !got.Equal(hintTime) {
		t.Fatalf("expected %s, got %s", hintTime.Format(time.RFC3339Nano), got.Format(time.RFC3339Nano))
	}
}

func TestDerivePaneLastInteractionAtIgnoresAdministrativeRuntimeEvent(t *testing.T) {
	now := time.Now().UTC()
	userInputAt := now.Add(-4 * time.Minute)
	got := derivePaneLastInteractionAt(
		"managed",
		"rt-1",
		"t1|%4",
		map[string]actionInputHint{
			"t1|%4": {preview: "user prompt", at: userInputAt},
		},
		nil,
		map[string]runtimeEventHint{
			"rt-1": {at: now, event: "action.view-output"},
		},
		"claude",
		codexThreadHint{},
		false,
		string(model.SourcePoller),
		"",
		nil,
		nil,
		now,
	)
	if got == nil {
		t.Fatalf("expected fallback to user input timestamp")
	}
	if !got.Equal(userInputAt) {
		t.Fatalf("expected %s, got %s", userInputAt.Format(time.RFC3339Nano), got.Format(time.RFC3339Nano))
	}
}

func TestDerivePaneLastInteractionAtIgnoresAdministrativeStateEvent(t *testing.T) {
	now := time.Now().UTC()
	stateEventAt := now.Add(-1 * time.Second)
	got := derivePaneLastInteractionAt(
		"managed",
		"",
		"t1|%5",
		nil,
		nil,
		nil,
		"claude",
		codexThreadHint{},
		false,
		string(model.SourceWrapper),
		"action.view-output",
		&stateEventAt,
		nil,
		now,
	)
	if got != nil {
		t.Fatalf("expected nil for managed pane when only administrative state event exists, got %s", got.Format(time.RFC3339Nano))
	}
}
