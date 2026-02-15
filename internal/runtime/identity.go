package runtime

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"time"

	"github.com/g960059/agtmux/internal/model"
)

type RuntimeIdentityInput struct {
	TargetID         string
	TmuxServerBootID string
	PaneID           string
	PaneEpoch        int64
	AgentType        string
	StartedAt        time.Time
}

func DeriveRuntimeID(in RuntimeIdentityInput) string {
	payload := fmt.Sprintf("%s|%s|%s|%d|%s|%d", in.TargetID, in.TmuxServerBootID, in.PaneID, in.PaneEpoch, in.AgentType, in.StartedAt.UTC().UnixNano())
	hash := sha256.Sum256([]byte(payload))
	return hex.EncodeToString(hash[:])
}

func ShouldIncrementPaneEpoch(prev model.Runtime, observedPID *int64, observedBootID string) bool {
	if prev.EndedAt != nil {
		return true
	}
	if observedBootID != "" && observedBootID != prev.TmuxServerBootID {
		return true
	}
	if prev.PID != nil && observedPID != nil && *prev.PID != *observedPID {
		return true
	}
	if prev.PID == nil && observedPID != nil {
		return true
	}
	return false
}

func NextPaneEpoch(prev *model.Runtime, observedPID *int64, observedBootID string) int64 {
	if prev == nil {
		return 1
	}
	if ShouldIncrementPaneEpoch(*prev, observedPID, observedBootID) {
		return prev.PaneEpoch + 1
	}
	return prev.PaneEpoch
}

func ValidateRuntimeFreshness(expectedRuntimeID, currentRuntimeID string) error {
	if expectedRuntimeID == "" {
		return nil
	}
	if expectedRuntimeID != currentRuntimeID {
		return fmt.Errorf("%s: expected=%s current=%s", model.ErrRuntimeStale, expectedRuntimeID, currentRuntimeID)
	}
	return nil
}
