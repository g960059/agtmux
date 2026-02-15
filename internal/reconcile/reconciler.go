package reconcile

import (
	"context"
	"fmt"
	"time"

	"github.com/google/uuid"

	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/ingest"
	"github.com/g960059/agtmux/internal/model"
)

type StateLister interface {
	ListStates(ctx context.Context) ([]model.StateRow, error)
	ListTargets(ctx context.Context) ([]model.Target, error)
}

type Reconciler struct {
	store  StateLister
	engine *ingest.Engine
	cfg    config.Config
}

func NewReconciler(store StateLister, engine *ingest.Engine, cfg config.Config) *Reconciler {
	return &Reconciler{store: store, engine: engine, cfg: cfg}
}

func (r *Reconciler) Tick(ctx context.Context, now time.Time) error {
	targets, err := r.store.ListTargets(ctx)
	if err != nil {
		return fmt.Errorf("list targets for reconcile: %w", err)
	}
	targetHealth := make(map[string]model.TargetHealth, len(targets))
	for _, t := range targets {
		targetHealth[t.TargetID] = t.Health
	}

	states, err := r.store.ListStates(ctx)
	if err != nil {
		return fmt.Errorf("list states for reconcile: %w", err)
	}

	for _, st := range states {
		health := targetHealth[st.TargetID]
		syntheticType := ""
		reasonToken := ""
		source := model.SourcePoller

		switch {
		case health == model.TargetHealthDown:
			if st.State == model.StateUnknown && st.ReasonCode == "target_unreachable" {
				continue
			}
			syntheticType = string(model.ReconcileTargetHealthChange)
			reasonToken = fmt.Sprintf("state-v%d", st.StateVersion)
		case st.State == model.StateCompleted && now.Sub(st.UpdatedAt) > r.cfg.CompletedDemotionAfter:
			syntheticType = string(model.ReconcileDemotionDue)
			reasonToken = fmt.Sprintf("state-v%d", st.StateVersion)
		case now.Sub(st.LastSeenAt) > r.cfg.StaleSignalTTL:
			if st.State == model.StateUnknown && st.ReasonCode == "stale_signal" {
				continue
			}
			syntheticType = string(model.ReconcileStaleDetected)
			reasonToken = fmt.Sprintf("state-v%d", st.StateVersion)
		default:
			continue
		}

		event := model.EventEnvelope{
			EventID:    uuid.NewString(),
			EventType:  syntheticType,
			Source:     source,
			DedupeKey:  fmt.Sprintf("reconcile:%s:%s:%s:%s", syntheticType, st.RuntimeID, st.PaneID, reasonToken),
			EventTime:  now,
			IngestedAt: now,
			RuntimeID:  st.RuntimeID,
			TargetID:   st.TargetID,
			PaneID:     st.PaneID,
		}
		if err := r.engine.Ingest(ctx, event); err != nil {
			return fmt.Errorf("reconcile ingest %s: %w", syntheticType, err)
		}
	}

	return nil
}
