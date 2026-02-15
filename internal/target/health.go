package target

import (
	"time"

	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/model"
)

type HealthState struct {
	Current              model.TargetHealth
	ConsecutiveFailures  int
	ConsecutiveSuccesses int
	LastTransitionAt     time.Time
}

func NextHealth(cfg config.Config, state HealthState, success bool, now time.Time) HealthState {
	if state.Current == "" {
		state.Current = model.TargetHealthOK
	}
	if state.LastTransitionAt.IsZero() {
		state.LastTransitionAt = now
	}

	if success {
		state.ConsecutiveSuccesses++
		state.ConsecutiveFailures = 0
		if (state.Current == model.TargetHealthDegraded || state.Current == model.TargetHealthDown) && state.ConsecutiveSuccesses >= cfg.TargetRecoverSuccesses {
			state.Current = model.TargetHealthOK
			state.LastTransitionAt = now
		}
		return state
	}

	state.ConsecutiveFailures++
	state.ConsecutiveSuccesses = 0
	switch state.Current {
	case model.TargetHealthOK:
		state.Current = model.TargetHealthDegraded
		state.LastTransitionAt = now
	case model.TargetHealthDegraded:
		if now.Sub(state.LastTransitionAt) > cfg.TargetDownWindow {
			// Failure window expired; start a new degraded window from this failure.
			state.ConsecutiveFailures = 1
			state.LastTransitionAt = now
			return state
		}
		if state.ConsecutiveFailures >= cfg.TargetDownFailures {
			state.Current = model.TargetHealthDown
			state.LastTransitionAt = now
		}
	case model.TargetHealthDown:
		// keep down until enough successful probes arrive
	}
	return state
}
