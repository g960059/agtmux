package ingest

import (
	"context"
	"errors"
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/google/uuid"

	"github.com/g960059/agtmux/internal/adapter"
	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/db"
	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/security"
)

type Engine struct {
	store    *db.Store
	cfg      config.Config
	registry *adapter.Registry
}

func NewEngine(store *db.Store, cfg config.Config) *Engine {
	return NewEngineWithRegistry(store, cfg, adapter.DefaultRegistry())
}

func NewEngineWithRegistry(store *db.Store, cfg config.Config, registry *adapter.Registry) *Engine {
	if registry == nil {
		registry = adapter.DefaultRegistry()
	}
	return &Engine{store: store, cfg: cfg, registry: registry}
}

func (e *Engine) Ingest(ctx context.Context, ev model.EventEnvelope) error {
	if ev.EventID == "" {
		ev.EventID = uuid.NewString()
	}
	if ev.IngestedAt.IsZero() {
		ev.IngestedAt = time.Now().UTC()
	}
	if ev.EventTime.IsZero() {
		ev.EventTime = ev.IngestedAt
	}
	ev.EventTime = clampEventTime(ev.EventTime, ev.IngestedAt, e.cfg.SkewBudget)
	if ev.DedupeKey == "" {
		return fmt.Errorf("dedupe_key required")
	}

	redactedPayload := security.RedactForStorage(ev.RawPayload)
	if ev.RuntimeID == "" {
		return e.ingestPendingBind(ctx, ev, redactedPayload)
	}
	return e.ingestBound(ctx, ev, redactedPayload)
}

func (e *Engine) ingestPendingBind(ctx context.Context, ev model.EventEnvelope, redactedPayload string) error {
	if ev.TargetID == "" || ev.PaneID == "" {
		return fmt.Errorf("target_id and pane_id required for pending bind")
	}
	inboxID := uuid.NewString()
	err := e.store.InsertInboxEvent(ctx, inboxID, ev, model.InboxPendingBind, "", redactedPayload)
	if errors.Is(err, db.ErrDuplicate) {
		return nil
	}
	return err
}

func (e *Engine) ingestBound(ctx context.Context, ev model.EventEnvelope, redactedPayload string) error {
	rt, err := e.store.GetRuntime(ctx, ev.RuntimeID)
	if err != nil {
		if errors.Is(err, db.ErrNotFound) {
			return fmt.Errorf("%s: runtime not found", model.ErrRuntimeStale)
		}
		return err
	}
	if rt.EndedAt != nil {
		return fmt.Errorf("%s: runtime ended", model.ErrRuntimeStale)
	}
	if ev.TargetID != "" && ev.TargetID != rt.TargetID {
		return fmt.Errorf("%s: target mismatch", model.ErrRuntimeStale)
	}
	if ev.PaneID != "" && ev.PaneID != rt.PaneID {
		return fmt.Errorf("%s: pane mismatch", model.ErrRuntimeStale)
	}

	duplicate := false
	if err := e.store.InsertEvent(ctx, ev, redactedPayload); err != nil {
		if errors.Is(err, db.ErrDuplicate) {
			duplicate = true
		} else {
			return err
		}
	}

	cursor, err := e.store.GetSourceCursor(ctx, ev.RuntimeID, ev.Source)
	hasCursor := err == nil
	var storedKey model.OrderKey
	if hasCursor {
		storedKey = model.OrderKey{
			HasSourceSeq: cursor.LastSourceSeq != nil,
			EventTime:    cursor.LastOrderEventTime,
			IngestedAt:   cursor.LastOrderIngestedAt,
			EventID:      cursor.LastOrderEventID,
		}
		if cursor.LastSourceSeq != nil {
			storedKey.SourceSeq = *cursor.LastSourceSeq
		}
	} else if !errors.Is(err, db.ErrNotFound) {
		return err
	}

	if duplicate {
		storedEvent, getErr := e.store.GetEventByRuntimeSourceDedupe(ctx, ev.RuntimeID, ev.Source, ev.DedupeKey)
		if getErr != nil {
			if errors.Is(getErr, db.ErrNotFound) {
				return fmt.Errorf("%s: duplicate dedupe_key conflict", model.ErrIdempotencyConflict)
			}
			return getErr
		}
		if !isReplayCompatible(ev, storedEvent) {
			return fmt.Errorf("%s: duplicate dedupe_key conflict", model.ErrIdempotencyConflict)
		}
		payloadConflict, payloadErr := e.replayPayloadHintsConflict(ctx, rt, ev, storedEvent)
		if payloadErr != nil {
			return payloadErr
		}
		if payloadConflict {
			return fmt.Errorf("%s: duplicate dedupe_key conflict", model.ErrIdempotencyConflict)
		}
		storedEvent.TargetID = rt.TargetID
		storedEvent.PaneID = rt.PaneID

		stateEvent := mergeReplayStateEvent(storedEvent, ev)
		state, reason, confidence, resolveErr := e.resolveStateCandidate(ctx, rt, stateEvent)
		if resolveErr != nil {
			return resolveErr
		}

		replayKey := BuildOrderKey(storedEvent, e.cfg.SkewBudget)
		if !hasCursor {
			if upsertErr := e.store.UpsertSourceCursor(ctx, db.SourceCursor{
				RuntimeID:           ev.RuntimeID,
				Source:              ev.Source,
				LastSourceSeq:       storedEvent.SourceSeq,
				LastOrderEventTime:  replayKey.EventTime,
				LastOrderIngestedAt: replayKey.IngestedAt,
				LastOrderEventID:    replayKey.EventID,
			}); upsertErr != nil {
				return upsertErr
			}
		}
		return e.applyState(ctx, rt, storedEvent, state, reason, confidence)
	}

	state, reason, confidence, err := e.resolveStateCandidate(ctx, rt, ev)
	if err != nil {
		return err
	}
	key := BuildOrderKey(ev, e.cfg.SkewBudget)
	if hasCursor && !IsNewer(key, storedKey) {
		return db.ErrOutOfOrder
	}

	if err := e.store.UpsertSourceCursor(ctx, db.SourceCursor{
		RuntimeID:           ev.RuntimeID,
		Source:              ev.Source,
		LastSourceSeq:       ev.SourceSeq,
		LastOrderEventTime:  key.EventTime,
		LastOrderIngestedAt: key.IngestedAt,
		LastOrderEventID:    key.EventID,
	}); err != nil {
		return err
	}

	return e.applyState(ctx, rt, ev, state, reason, confidence)
}

func (e *Engine) resolveStateCandidate(ctx context.Context, rt model.Runtime, ev model.EventEnvelope) (model.CanonicalState, string, string, error) {
	health, err := e.store.GetTargetHealth(ctx, rt.TargetID)
	if err != nil && !errors.Is(err, db.ErrNotFound) {
		return "", "", "", err
	}
	if health == model.TargetHealthDown {
		return model.StateUnknown, "target_unreachable", "low", nil
	}
	state, reason, confidence := e.normalize(ctx, rt.AgentType, ev)
	return state, reason, confidence, nil
}

func (e *Engine) normalize(ctx context.Context, agentType string, ev model.EventEnvelope) (model.CanonicalState, string, string) {
	if ev.IngestedAt.Sub(ev.EventTime) > e.cfg.StaleSignalTTL {
		return model.StateUnknown, "stale_signal", "low"
	}
	if strings.TrimSpace(agentType) != "" {
		adapterRow, err := e.store.GetAdapterByAgentType(ctx, agentType)
		if err == nil {
			if !adapterRow.Enabled || !adapter.IsVersionCompatible(adapterRow.Version) {
				return model.StateUnknown, "unsupported_signal", "low"
			}
		}
	}
	if e.registry != nil {
		out, ok := e.registry.Normalize(agentType, adapter.Signal{
			EventType:  ev.EventType,
			Source:     ev.Source,
			RawPayload: ev.RawPayload,
		})
		if ok {
			return out.State, out.Reason, out.Confidence
		}
	}
	et := strings.ToLower(ev.EventType)
	switch {
	case strings.Contains(et, string(model.ReconcileTargetHealthChange)):
		return model.StateUnknown, "target_unreachable", "low"
	case strings.Contains(et, string(model.ReconcileStaleDetected)):
		return model.StateUnknown, "stale_signal", "low"
	case strings.Contains(et, string(model.ReconcileDemotionDue)):
		return model.StateIdle, "completed_demoted", "medium"
	case strings.Contains(et, "no-agent"), strings.Contains(et, "unmanaged"):
		return model.StateUnknown, "no_agent", "high"
	case strings.Contains(et, "unknown"), strings.Contains(et, "inconclusive"):
		return model.StateUnknown, "inconclusive", "low"
	case strings.Contains(et, "error"), strings.Contains(et, "fail"), strings.Contains(et, "panic"):
		return model.StateError, "runtime_error", "high"
	case strings.Contains(et, "approval"):
		return model.StateWaitingApproval, "approval_requested", "high"
	case strings.Contains(et, "input"), strings.Contains(et, "prompt"):
		return model.StateWaitingInput, "input_required", "high"
	case strings.Contains(et, "start"), strings.Contains(et, "run"), strings.Contains(et, "progress"):
		return model.StateRunning, "active", "medium"
	case strings.Contains(et, "complete"), strings.Contains(et, "exit"):
		return model.StateCompleted, "task_completed", "medium"
	case strings.Contains(et, "idle"):
		return model.StateIdle, "idle", "medium"
	default:
		return model.StateUnknown, "unsupported_signal", "low"
	}
}

func (e *Engine) applyState(ctx context.Context, rt model.Runtime, ev model.EventEnvelope, candidate model.CanonicalState, reason, confidence string) error {
	current, err := e.store.GetState(ctx, rt.TargetID, rt.PaneID)
	if err != nil && !errors.Is(err, db.ErrNotFound) {
		return err
	}
	if errors.Is(err, db.ErrNotFound) && isReconcileEvent(ev.EventType) {
		// Reconcile events must only mutate an existing state row.
		return nil
	}
	normalizedEventType := normalizeEventType(ev.EventType)
	var eventAt *time.Time
	eventTime := effectiveEventTime(ev.EventTime, ev.IngestedAt, e.cfg.SkewBudget)
	if !eventTime.IsZero() {
		v := eventTime
		eventAt = &v
	}

	newVersion := int64(1)
	if err == nil {
		if isReconcileEvent(ev.EventType) {
			expectedRuntimeID, expectedVersion, ok := parseReconcileGuard(ev.DedupeKey)
			if !ok {
				return nil
			}
			if expectedRuntimeID != "" && current.RuntimeID != expectedRuntimeID {
				return nil
			}
			if expectedVersion != current.StateVersion {
				return nil
			}
		}
		if shouldSuppressPollerByEventDrivenState(ev, current, rt.RuntimeID, e.cfg.StaleSignalTTL) {
			return nil
		}
		if ev.IngestedAt.Before(current.LastSeenAt) {
			return nil
		}
		if ev.IngestedAt.Equal(current.LastSeenAt) {
			currentPrecedence, okCurrent := model.StatePrecedence[current.State]
			candidatePrecedence, okCandidate := model.StatePrecedence[candidate]
			if okCurrent && okCandidate && candidatePrecedence > currentPrecedence {
				candidate = current.State
				reason = current.ReasonCode
				confidence = current.Confidence
			}
		}
		if current.RuntimeID == rt.RuntimeID &&
			current.State == candidate &&
			current.ReasonCode == reason &&
			current.Confidence == confidence &&
			current.StateSource == ev.Source &&
			current.LastEventType == normalizedEventType &&
			timePtrEqual(current.LastEventAt, eventAt) &&
			current.UpdatedAt.Equal(ev.IngestedAt) &&
			int64PtrEqual(current.LastSourceSeq, ev.SourceSeq) {
			return nil
		}
		newVersion = current.StateVersion + 1
	}

	return e.store.UpsertState(ctx, model.StateRow{
		TargetID:      rt.TargetID,
		PaneID:        rt.PaneID,
		RuntimeID:     rt.RuntimeID,
		State:         candidate,
		ReasonCode:    reason,
		Confidence:    confidence,
		StateVersion:  newVersion,
		StateSource:   ev.Source,
		LastEventType: normalizedEventType,
		LastEventAt:   eventAt,
		LastSourceSeq: ev.SourceSeq,
		LastSeenAt:    ev.IngestedAt,
		UpdatedAt:     ev.IngestedAt,
	})
}

func isReconcileEvent(eventType string) bool {
	et := strings.ToLower(eventType)
	return strings.Contains(et, string(model.ReconcileTargetHealthChange)) ||
		strings.Contains(et, string(model.ReconcileStaleDetected)) ||
		strings.Contains(et, string(model.ReconcileDemotionDue))
}

func parseReconcileGuard(dedupeKey string) (expectedRuntimeID string, expectedVersion int64, ok bool) {
	parts := strings.SplitN(dedupeKey, ":", 5)
	if len(parts) != 5 || parts[0] != "reconcile" {
		return "", 0, false
	}
	expectedRuntimeID = parts[2]
	if !strings.HasPrefix(parts[4], "state-v") {
		return "", 0, false
	}
	v, err := strconv.ParseInt(strings.TrimPrefix(parts[4], "state-v"), 10, 64)
	if err != nil || v <= 0 {
		return "", 0, false
	}
	return expectedRuntimeID, v, true
}

func clampEventTime(eventTime, ingestedAt time.Time, skewBudget time.Duration) time.Time {
	if skewBudget <= 0 {
		if eventTime.After(ingestedAt) {
			return ingestedAt
		}
		return eventTime
	}
	maxFuture := ingestedAt.Add(skewBudget)
	if eventTime.After(maxFuture) {
		return ingestedAt
	}
	return eventTime
}

func normalizeEventType(eventType string) string {
	normalized := strings.ToLower(strings.TrimSpace(eventType))
	normalized = strings.ReplaceAll(normalized, "_", "-")
	normalized = strings.ReplaceAll(normalized, ".", "-")
	normalized = strings.ReplaceAll(normalized, " ", "-")
	return normalized
}

func int64PtrEqual(a, b *int64) bool {
	if a == nil && b == nil {
		return true
	}
	if a == nil || b == nil {
		return false
	}
	return *a == *b
}

func timePtrEqual(a, b *time.Time) bool {
	if a == nil && b == nil {
		return true
	}
	if a == nil || b == nil {
		return false
	}
	return a.Equal(*b)
}

func shouldSuppressPollerByEventDrivenState(
	ev model.EventEnvelope,
	current model.StateRow,
	currentRuntimeID string,
	ttl time.Duration,
) bool {
	if ev.Source != model.SourcePoller {
		return false
	}
	if current.RuntimeID == "" || current.RuntimeID != currentRuntimeID {
		return false
	}
	if !isEventDrivenSource(current.StateSource) {
		return false
	}
	if ttl <= 0 {
		return false
	}
	return ev.IngestedAt.Sub(current.LastSeenAt) <= ttl
}

func isEventDrivenSource(source model.EventSource) bool {
	return source == model.SourceHook || source == model.SourceNotify || source == model.SourceWrapper
}

func isReplayCompatible(candidate, stored model.EventEnvelope) bool {
	if normalizeEventType(candidate.EventType) != normalizeEventType(stored.EventType) {
		return false
	}
	if strings.TrimSpace(candidate.SourceEventID) != "" &&
		strings.TrimSpace(stored.SourceEventID) != "" &&
		strings.TrimSpace(candidate.SourceEventID) != strings.TrimSpace(stored.SourceEventID) {
		return false
	}
	if candidate.SourceSeq != nil && stored.SourceSeq != nil && *candidate.SourceSeq != *stored.SourceSeq {
		return false
	}
	if candidate.ActionID != nil && stored.ActionID != nil && *candidate.ActionID != *stored.ActionID {
		return false
	}
	return true
}

func mergeReplayStateEvent(stored, retry model.EventEnvelope) model.EventEnvelope {
	merged := stored
	if payload := strings.TrimSpace(retry.RawPayload); payload != "" {
		merged.RawPayload = payload
	}
	return merged
}

func (e *Engine) replayPayloadHintsConflict(
	ctx context.Context,
	rt model.Runtime,
	retry, stored model.EventEnvelope,
) (bool, error) {
	retryPayload := strings.TrimSpace(retry.RawPayload)
	storedPayload := strings.TrimSpace(stored.RawPayload)
	if retryPayload == "" || storedPayload == "" {
		return false, nil
	}

	storedStateEvent := stored
	storedStateEvent.RawPayload = storedPayload
	storedState, _, _, err := e.resolveStateCandidate(ctx, rt, storedStateEvent)
	if err != nil {
		return false, err
	}

	retryStateEvent := stored
	retryStateEvent.RawPayload = retryPayload
	retryState, _, _, err := e.resolveStateCandidate(ctx, rt, retryStateEvent)
	if err != nil {
		return false, err
	}
	return storedState != retryState, nil
}
