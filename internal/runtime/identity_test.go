package runtime

import (
	"strings"
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/model"
)

func TestDeriveRuntimeIDDeterministic(t *testing.T) {
	started := time.Unix(1700000000, 123).UTC()
	in := RuntimeIdentityInput{
		TargetID:         "host",
		TmuxServerBootID: "boot",
		PaneID:           "%1",
		PaneEpoch:        1,
		AgentType:        "codex",
		StartedAt:        started,
	}
	id1 := DeriveRuntimeID(in)
	id2 := DeriveRuntimeID(in)
	if id1 != id2 {
		t.Fatalf("runtime id not deterministic: %s vs %s", id1, id2)
	}
	in.PaneEpoch = 2
	id3 := DeriveRuntimeID(in)
	if id3 == id1 {
		t.Fatalf("runtime id should change when epoch changes")
	}
}

func TestPaneEpochIncrementRules(t *testing.T) {
	pid1 := int64(100)
	pid2 := int64(200)
	prev := model.Runtime{
		PaneEpoch:        3,
		TmuxServerBootID: "boot-1",
		PID:              &pid1,
	}
	if got := NextPaneEpoch(&prev, &pid1, "boot-1"); got != 3 {
		t.Fatalf("epoch should stay same, got %d", got)
	}
	if got := NextPaneEpoch(&prev, &pid2, "boot-1"); got != 4 {
		t.Fatalf("epoch should increment on pid change, got %d", got)
	}
	if got := NextPaneEpoch(&prev, &pid1, "boot-2"); got != 4 {
		t.Fatalf("epoch should increment on boot-id change, got %d", got)
	}
}

func TestValidateRuntimeFreshness(t *testing.T) {
	err := ValidateRuntimeFreshness("runtime-1", "runtime-2")
	if err == nil {
		t.Fatalf("expected stale runtime error")
	}
	if !strings.Contains(err.Error(), model.ErrRuntimeStale) {
		t.Fatalf("unexpected error: %v", err)
	}
}
