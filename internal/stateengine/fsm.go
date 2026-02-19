package stateengine

import (
	"math"
	"sort"
	"time"
)

var activityPrecedence = map[string]int{
	ActivityError:           1,
	ActivityWaitingApproval: 2,
	ActivityWaitingInput:    3,
	ActivityRunning:         4,
	ActivityIdle:            5,
	ActivityUnknown:         6,
}

type signalScore struct {
	activity string
	score    float64
}

func resolveActivityState(
	scores map[string]float64,
	cfg EngineConfig,
) (activity string, score float64) {
	candidates := make([]signalScore, 0, len(scores))
	for rawActivity, rawScore := range scores {
		activity := CanonicalActivity(rawActivity)
		score := clamp01(rawScore)
		if activity == ActivityUnknown {
			continue
		}
		if score <= 0 {
			continue
		}
		candidates = append(candidates, signalScore{
			activity: activity,
			score:    score,
		})
	}
	if len(candidates) == 0 {
		return ActivityUnknown, 0
	}
	sort.SliceStable(candidates, func(i, j int) bool {
		if activityPrecedence[candidates[i].activity] != activityPrecedence[candidates[j].activity] {
			return activityPrecedence[candidates[i].activity] < activityPrecedence[candidates[j].activity]
		}
		return candidates[i].score > candidates[j].score
	})

	for _, c := range candidates {
		threshold := cfg.MinScore
		if c.activity == ActivityRunning {
			threshold = math.Max(threshold, cfg.RunningEnterScore)
		}
		if c.score >= threshold {
			return c.activity, c.score
		}
	}
	return ActivityUnknown, 0
}

func sourceWeight(source string) float64 {
	switch source {
	case "hook":
		return 1.0
	case "notify":
		return 0.95
	case "wrapper":
		return 0.9
	case "poller":
		return 0.55
	default:
		return 0.45
	}
}

func confidenceWeight(raw string) float64 {
	switch raw {
	case "high":
		return 0.95
	case "medium":
		return 0.75
	case "low":
		return 0.55
	default:
		return 0.6
	}
}

func ttlForConfidence(cfg EngineConfig, raw string) time.Duration {
	switch raw {
	case "high":
		return cfg.HighConfidenceTTL
	case "low":
		return cfg.LowConfidenceTTL
	default:
		return cfg.DefaultEvidenceTTL
	}
}

func clamp01(v float64) float64 {
	if v < 0 {
		return 0
	}
	if v > 1 {
		return 1
	}
	return v
}
