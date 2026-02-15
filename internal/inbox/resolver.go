package inbox

import (
	"context"
	"math"
	"time"

	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/db"
	"github.com/g960059/agtmux/internal/ingest"
	"github.com/g960059/agtmux/internal/model"
)

type Resolver struct {
	store  *db.Store
	engine *ingest.Engine
	cfg    config.Config
}

func NewResolver(store *db.Store, engine *ingest.Engine, cfg config.Config) *Resolver {
	return &Resolver{store: store, engine: engine, cfg: cfg}
}

func (r *Resolver) Resolve(ctx context.Context, now time.Time) error {
	pending, err := r.store.ListPendingInbox(ctx)
	if err != nil {
		return err
	}

	for _, item := range pending {
		if now.Sub(item.IngestedAt) > r.cfg.PendingBindTTL {
			if err := r.store.UpdateInboxBinding(ctx, item.InboxID, "", model.InboxDroppedUnbound, "bind_ttl_expired"); err != nil {
				return err
			}
			continue
		}

		candidates, err := r.store.ListActiveRuntimesForPane(ctx, item.TargetID, item.PaneID)
		if err != nil {
			return err
		}
		filtered := make([]model.Runtime, 0, len(candidates))
		for _, candidate := range candidates {
			if item.PID != nil {
				if candidate.PID == nil || *candidate.PID != *item.PID {
					continue
				}
			}
			if item.StartHint != nil {
				delta := candidate.StartedAt.Sub(*item.StartHint)
				if math.Abs(delta.Seconds()) > r.cfg.BindWindow.Seconds() {
					continue
				}
			}
			filtered = append(filtered, candidate)
		}

		switch len(filtered) {
		case 0:
			if err := r.store.UpdateInboxBinding(ctx, item.InboxID, "", model.InboxDroppedUnbound, "bind_no_candidate"); err != nil {
				return err
			}
		case 1:
			selected := filtered[0]
			err := r.engine.Ingest(ctx, model.EventEnvelope{
				EventID:    item.InboxID,
				EventType:  item.EventType,
				Source:     item.Source,
				DedupeKey:  item.DedupeKey,
				EventTime:  item.EventTime,
				IngestedAt: item.IngestedAt,
				RuntimeID:  selected.RuntimeID,
				TargetID:   item.TargetID,
				PaneID:     item.PaneID,
				RawPayload: item.RawPayload,
			})
			if err != nil && err != db.ErrOutOfOrder {
				return err
			}
			if err := r.store.UpdateInboxBinding(ctx, item.InboxID, selected.RuntimeID, model.InboxBound, ""); err != nil {
				return err
			}
		default:
			if err := r.store.UpdateInboxBinding(ctx, item.InboxID, "", model.InboxDroppedUnbound, "bind_ambiguous"); err != nil {
				return err
			}
		}
	}
	return nil
}
