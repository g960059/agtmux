package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/g960059/agtmux/internal/adapter"
	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/daemon"
	"github.com/g960059/agtmux/internal/db"
	"github.com/g960059/agtmux/internal/inbox"
	"github.com/g960059/agtmux/internal/ingest"
	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/observer"
	"github.com/g960059/agtmux/internal/reconcile"
	agtruntime "github.com/g960059/agtmux/internal/runtime"
	"github.com/g960059/agtmux/internal/target"
)

const (
	defaultLocalTargetID   = "local"
	defaultLocalTargetName = "local"
	defaultAgentType       = "unknown"
	defaultTmuxBootID      = "tmux-unknown"
)

func main() {
	cfg := config.DefaultConfig()
	flag.StringVar(&cfg.SocketPath, "socket", cfg.SocketPath, "UDS path for agtmuxd")
	flag.StringVar(&cfg.DBPath, "db", cfg.DBPath, "SQLite path")
	flag.BoolVar(&cfg.EnableTTYV2PaneTap, "tty-v2-pane-tap", cfg.EnableTTYV2PaneTap, "enable tty-v2 pane tap stream path (experimental)")
	flag.Parse()

	ctx, cancel := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer cancel()

	store, err := db.Open(ctx, cfg.DBPath)
	if err != nil {
		fatal(err)
	}
	defer store.Close() //nolint:errcheck

	if err := db.ApplyMigrations(ctx, store.DB()); err != nil {
		fatal(err)
	}

	if err := ensureDefaultLocalTarget(ctx, store, time.Now().UTC()); err != nil {
		fatal(err)
	}
	if err := syncAdapterRegistry(ctx, store); err != nil {
		fatal(err)
	}
	executor := target.NewExecutor(cfg)
	startCoreLoops(ctx, store, cfg, executor)
	startRetentionLoop(ctx, store, cfg)

	srv := daemon.NewServerWithDeps(cfg, store, executor)
	if err := srv.Start(ctx); err != nil && err != context.Canceled {
		fatal(err)
	}
}

func startCoreLoops(ctx context.Context, store *db.Store, cfg config.Config, executor *target.Executor) {
	engine := ingest.NewEngine(store, cfg)
	resolver := inbox.NewResolver(store, engine, cfg)
	reconciler := reconcile.NewReconciler(store, engine, cfg)
	tmuxObserver := observer.NewTmuxObserver(executor, store)

	startTopologyLoop(ctx, store, tmuxObserver, executor, engine, cfg)
	startResolverLoop(ctx, resolver, cfg.ActiveReconcileInterval)
	startReconcileLoop(ctx, reconciler, cfg.ActiveReconcileInterval)
}

func syncAdapterRegistry(ctx context.Context, store *db.Store) error {
	now := time.Now().UTC()
	registry := adapter.DefaultRegistry()
	existingRows, err := store.ListAdapters(ctx)
	if err != nil {
		return fmt.Errorf("list adapters: %w", err)
	}
	existing := make(map[string]model.AdapterRecord, len(existingRows))
	for _, row := range existingRows {
		existing[row.AdapterName] = row
	}
	for _, def := range registry.Definitions() {
		enabled := true
		if row, ok := existing[def.Name]; ok {
			enabled = row.Enabled
		}
		if err := store.UpsertAdapter(ctx, model.AdapterRecord{
			AdapterName:  def.Name,
			AgentType:    def.AgentType,
			Version:      def.ContractVersion,
			Capabilities: def.Capabilities,
			Enabled:      enabled,
			UpdatedAt:    now,
		}); err != nil {
			return fmt.Errorf("sync adapter %s: %w", def.Name, err)
		}
	}
	return nil
}

func startTopologyLoop(ctx context.Context, store *db.Store, tmuxObserver *observer.TmuxObserver, executor *target.Executor, engine *ingest.Engine, cfg config.Config) {
	interval := loopInterval(cfg.ActiveReconcileInterval, 2*time.Second)
	healthByTarget := map[string]target.HealthState{}
	stabilityByPane := map[string]paneOutputStability{}

	run := func() {
		now := time.Now().UTC()
		seenPaneKeys := map[string]struct{}{}
		targets, err := store.ListTargets(ctx)
		if err != nil {
			logErr("list targets for topology loop", err)
			return
		}
		if len(targets) == 0 {
			if err := ensureDefaultLocalTarget(ctx, store, now); err != nil {
				logErr("ensure default target", err)
				return
			}
			targets, err = store.ListTargets(ctx)
			if err != nil {
				logErr("reload targets after default seed", err)
				return
			}
		}

		for _, tg := range targets {
			panes, collectErr := tmuxObserver.Collect(ctx, tg, now)
			success := collectErr == nil
			if collectErr != nil && !errors.Is(collectErr, context.Canceled) {
				logErr(fmt.Sprintf("tmux collect failed for target=%s", tg.TargetID), collectErr)
			}

			healthState := healthByTarget[tg.TargetID]
			if healthState.Current == "" {
				healthState.Current = tg.Health
				if healthState.Current == "" {
					healthState.Current = model.TargetHealthOK
				}
			}
			healthState = target.NextHealth(cfg, healthState, success, now)
			healthByTarget[tg.TargetID] = healthState

			updated := tg
			updated.Health = healthState.Current
			updated.UpdatedAt = now
			if success {
				updated.LastSeenAt = &now
			}
			if err := store.UpsertTarget(ctx, updated); err != nil {
				logErr(fmt.Sprintf("update target health target=%s", tg.TargetID), err)
			}
			if !success {
				continue
			}

			if err := ensurePaneRuntimesAndEmitPoller(ctx, store, executor, engine, tg, panes, now, interval, stabilityByPane, seenPaneKeys); err != nil && !errors.Is(err, context.Canceled) {
				logErr(fmt.Sprintf("runtime sync for target=%s", tg.TargetID), err)
			}
		}
		cleanupStabilityEntries(stabilityByPane, seenPaneKeys, now, interval)
	}

	run()
	go func() {
		ticker := time.NewTicker(interval)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				run()
			}
		}
	}()
}

func ensurePaneRuntimesAndEmitPoller(
	ctx context.Context,
	store *db.Store,
	executor *target.Executor,
	engine *ingest.Engine,
	tg model.Target,
	panes []model.Pane,
	now time.Time,
	interval time.Duration,
	stabilityByPane map[string]paneOutputStability,
	seenPaneKeys map[string]struct{},
) error {
	for _, pane := range panes {
		if seenPaneKeys != nil {
			seenPaneKeys[topologyPaneKey(tg.TargetID, pane.PaneID)] = struct{}{}
		}
		agentType := classifyPaneAgentType(ctx, executor, tg, pane)
		rt, err := ensureActiveRuntime(ctx, store, tg, pane, agentType, now)
		if err != nil {
			return err
		}
		inference := inferPanePollerEvent(ctx, executor, tg, pane, agentType)
		eventType := stabilizePaneEvent(tg.TargetID, pane.PaneID, inference, now, interval, stabilityByPane)

		dedupeAt := now
		if interval > 0 {
			dedupeAt = now.Truncate(interval)
		}
		err = engine.Ingest(ctx, model.EventEnvelope{
			EventType:  eventType,
			Source:     model.SourcePoller,
			DedupeKey:  fmt.Sprintf("poller:topology:%s:%s:%d", rt.RuntimeID, eventType, dedupeAt.UnixNano()),
			EventTime:  now,
			IngestedAt: now,
			RuntimeID:  rt.RuntimeID,
			TargetID:   tg.TargetID,
			PaneID:     pane.PaneID,
		})
		if err == nil {
			continue
		}
		if errors.Is(err, db.ErrOutOfOrder) || strings.Contains(err.Error(), model.ErrIdempotencyConflict) {
			continue
		}
		return err
	}
	return nil
}

func ensureActiveRuntime(ctx context.Context, store *db.Store, tg model.Target, pane model.Pane, agentType string, now time.Time) (model.Runtime, error) {
	agentType = strings.TrimSpace(agentType)
	if agentType == "" {
		agentType = defaultAgentType
	}

	active, err := store.ListActiveRuntimesForPane(ctx, tg.TargetID, pane.PaneID)
	if err != nil {
		return model.Runtime{}, err
	}
	if len(active) > 0 {
		current := active[0]
		for i := 1; i < len(active); i++ {
			_ = store.EndRuntime(ctx, active[i].RuntimeID, now)
		}
		if current.AgentType == agentType && !agtruntime.ShouldIncrementPaneEpoch(current, pane.CurrentPID, defaultTmuxBootID) {
			return current, nil
		}
		if err := store.EndRuntime(ctx, current.RuntimeID, now); err != nil && !errors.Is(err, db.ErrNotFound) {
			return model.Runtime{}, err
		}
	}

	nextEpoch, err := store.NextPaneEpoch(ctx, tg.TargetID, pane.PaneID)
	if err != nil {
		return model.Runtime{}, err
	}
	rt := model.Runtime{
		RuntimeID: agtruntime.DeriveRuntimeID(agtruntime.RuntimeIdentityInput{
			TargetID:         tg.TargetID,
			TmuxServerBootID: defaultTmuxBootID,
			PaneID:           pane.PaneID,
			PaneEpoch:        nextEpoch,
			AgentType:        agentType,
			StartedAt:        now,
		}),
		TargetID:         tg.TargetID,
		PaneID:           pane.PaneID,
		TmuxServerBootID: defaultTmuxBootID,
		PaneEpoch:        nextEpoch,
		AgentType:        agentType,
		PID:              pane.CurrentPID,
		StartedAt:        now,
	}
	if err := store.InsertRuntime(ctx, rt); err != nil {
		if errors.Is(err, db.ErrDuplicate) {
			active, listErr := store.ListActiveRuntimesForPane(ctx, tg.TargetID, pane.PaneID)
			if listErr == nil && len(active) > 0 {
				return active[0], nil
			}
		}
		return model.Runtime{}, err
	}
	return rt, nil
}

func startResolverLoop(ctx context.Context, resolver *inbox.Resolver, interval time.Duration) {
	loop := loopInterval(interval, 2*time.Second)
	run := func() {
		if err := resolver.Resolve(ctx, time.Now().UTC()); err != nil && !errors.Is(err, context.Canceled) {
			logErr("resolver loop", err)
		}
	}
	run()
	go func() {
		ticker := time.NewTicker(loop)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				run()
			}
		}
	}()
}

func startReconcileLoop(ctx context.Context, reconciler *reconcile.Reconciler, interval time.Duration) {
	loop := loopInterval(interval, 2*time.Second)
	run := func() {
		if err := reconciler.Tick(ctx, time.Now().UTC()); err != nil && !errors.Is(err, context.Canceled) {
			logErr("reconcile loop", err)
		}
	}
	run()
	go func() {
		ticker := time.NewTicker(loop)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				run()
			}
		}
	}()
}

func ensureDefaultLocalTarget(ctx context.Context, store *db.Store, now time.Time) error {
	local := model.Target{
		TargetID:      defaultLocalTargetID,
		TargetName:    defaultLocalTargetName,
		Kind:          model.TargetKindLocal,
		ConnectionRef: "",
		IsDefault:     true,
		Health:        model.TargetHealthOK,
		UpdatedAt:     now,
		LastSeenAt:    &now,
	}
	return store.UpsertTarget(ctx, local)
}

func startRetentionLoop(ctx context.Context, store *db.Store, cfg config.Config) {
	run := func() {
		now := time.Now().UTC()
		payloadCutoff := now.Add(-cfg.EventPayloadTTL)
		metadataCutoff := now.Add(-cfg.EventMetadataTTL)
		if err := store.PurgeRetention(ctx, payloadCutoff, metadataCutoff); err != nil {
			_, _ = fmt.Fprintf(os.Stderr, "agtmuxd: retention purge failed: %v\n", err)
		}
	}

	run()
	go func() {
		ticker := time.NewTicker(1 * time.Hour)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				run()
			}
		}
	}()
}

func loopInterval(interval, fallback time.Duration) time.Duration {
	if interval <= 0 {
		return fallback
	}
	return interval
}

func logErr(scope string, err error) {
	_, _ = fmt.Fprintf(os.Stderr, "agtmuxd: %s: %v\n", scope, err)
}

func fatal(err error) {
	_, _ = fmt.Fprintf(os.Stderr, "agtmuxd: %v\n", err)
	os.Exit(1)
}
