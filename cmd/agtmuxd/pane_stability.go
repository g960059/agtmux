package main

import "time"

type paneOutputStability struct {
	Signature    uint64
	LastChangeAt time.Time
	LastSeenAt   time.Time
}

func stabilizePaneEvent(
	targetID string,
	paneID string,
	inference paneInference,
	now time.Time,
	interval time.Duration,
	stability map[string]paneOutputStability,
) string {
	if stability == nil {
		return inference.EventType
	}
	key := topologyPaneKey(targetID, paneID)

	if inference.EventType == "no-agent" {
		delete(stability, key)
		return inference.EventType
	}
	if !inference.HasOutput {
		return inference.EventType
	}

	entry, ok := stability[key]
	if !ok {
		stability[key] = paneOutputStability{
			Signature:    inference.Signature,
			LastChangeAt: now,
			LastSeenAt:   now,
		}
		if inference.EventType == "idle" {
			// Guard against false-idle on the first observation.
			return "unknown"
		}
		return inference.EventType
	}

	if inference.Signature != entry.Signature {
		stability[key] = paneOutputStability{
			Signature:    inference.Signature,
			LastChangeAt: now,
			LastSeenAt:   now,
		}
		if inference.EventType == "idle" || inference.EventType == "unknown" {
			// Output changed, so treat as active until the pane stabilizes.
			return "running"
		}
		return inference.EventType
	}

	entry.LastSeenAt = now
	stability[key] = entry

	if inference.EventType != "idle" {
		return inference.EventType
	}

	if now.Sub(entry.LastChangeAt) < idleStabilityThreshold(interval) {
		return "unknown"
	}
	return "idle"
}

func idleStabilityThreshold(interval time.Duration) time.Duration {
	const minThreshold = 4 * time.Second
	if interval <= 0 {
		return minThreshold
	}
	threshold := interval * 2
	if threshold < minThreshold {
		return minThreshold
	}
	return threshold
}

func cleanupStabilityEntries(stability map[string]paneOutputStability, seen map[string]struct{}, now time.Time, interval time.Duration) {
	if stability == nil {
		return
	}
	expireAfter := idleStabilityThreshold(interval) * 3
	for key, entry := range stability {
		if _, ok := seen[key]; ok {
			continue
		}
		if now.Sub(entry.LastSeenAt) >= expireAfter {
			delete(stability, key)
		}
	}
}

func topologyPaneKey(targetID, paneID string) string {
	return targetID + "|" + paneID
}
