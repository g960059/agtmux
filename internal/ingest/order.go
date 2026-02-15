package ingest

import (
	"time"

	"github.com/g960059/agtmux/internal/model"
)

func effectiveEventTime(eventTime, ingestedAt time.Time, skewBudget time.Duration) time.Time {
	delta := eventTime.Sub(ingestedAt)
	if delta < 0 {
		delta = -delta
	}
	if delta > skewBudget {
		return ingestedAt
	}
	return eventTime
}

func BuildOrderKey(ev model.EventEnvelope, skewBudget time.Duration) model.OrderKey {
	key := model.OrderKey{
		HasSourceSeq: ev.SourceSeq != nil,
		EventTime:    effectiveEventTime(ev.EventTime, ev.IngestedAt, skewBudget),
		IngestedAt:   ev.IngestedAt,
		EventID:      ev.EventID,
	}
	if ev.SourceSeq != nil {
		key.SourceSeq = *ev.SourceSeq
	}
	return key
}

func IsNewer(candidate, stored model.OrderKey) bool {
	if candidate.HasSourceSeq && stored.HasSourceSeq {
		if candidate.SourceSeq != stored.SourceSeq {
			return candidate.SourceSeq > stored.SourceSeq
		}
	}
	if !candidate.EventTime.Equal(stored.EventTime) {
		return candidate.EventTime.After(stored.EventTime)
	}
	if !candidate.IngestedAt.Equal(stored.IngestedAt) {
		return candidate.IngestedAt.After(stored.IngestedAt)
	}
	return candidate.EventID > stored.EventID
}
