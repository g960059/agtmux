package target

import (
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/model"
)

func TestHealthTransitionPolicy(t *testing.T) {
	cfg := config.DefaultConfig()
	now := time.Now().UTC()
	state := HealthState{Current: model.TargetHealthOK, LastTransitionAt: now}

	state = NextHealth(cfg, state, false, now.Add(1*time.Second))
	if state.Current != model.TargetHealthDegraded {
		t.Fatalf("ok->degraded expected, got %s", state.Current)
	}
	state = NextHealth(cfg, state, false, now.Add(2*time.Second))
	state = NextHealth(cfg, state, false, now.Add(3*time.Second))
	if state.Current != model.TargetHealthDown {
		t.Fatalf("degraded->down expected after failures, got %s", state.Current)
	}

	state = NextHealth(cfg, state, true, now.Add(4*time.Second))
	if state.Current != model.TargetHealthDown {
		t.Fatalf("still down until enough success, got %s", state.Current)
	}
	state = NextHealth(cfg, state, true, now.Add(5*time.Second))
	if state.Current != model.TargetHealthOK {
		t.Fatalf("down->ok expected on recovery threshold, got %s", state.Current)
	}
}

func TestDownTransitionRequiresFailureWindow(t *testing.T) {
	cfg := config.DefaultConfig()
	cfg.TargetDownWindow = 2 * time.Second
	now := time.Now().UTC()

	state := HealthState{Current: model.TargetHealthOK, LastTransitionAt: now}
	state = NextHealth(cfg, state, false, now.Add(1*time.Second))  // degraded
	state = NextHealth(cfg, state, false, now.Add(10*time.Second)) // outside window, should reset
	state = NextHealth(cfg, state, false, now.Add(11*time.Second)) // second within new window

	if state.Current != model.TargetHealthDegraded {
		t.Fatalf("expected degraded (not down) with failures outside window, got %s", state.Current)
	}
}
