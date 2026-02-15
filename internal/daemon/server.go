package daemon

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/google/uuid"

	adapterpkg "github.com/g960059/agtmux/internal/adapter"
	"github.com/g960059/agtmux/internal/api"
	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/db"
	"github.com/g960059/agtmux/internal/ingest"
	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/target"
)

const defaultAgentType = "unknown"
const unmanagedAgentType = "none"
const defaultLocalTargetName = "local"
const defaultViewOutputLines = 200
const defaultActionSnapshotTTL = 30 * time.Second

type Server struct {
	cfg            config.Config
	httpSrv        *http.Server
	listener       net.Listener
	lockFile       *os.File
	store          *db.Store
	executor       *target.Executor
	engine         *ingest.Engine
	codexEnricher  *codexSessionEnricher
	streamID       string
	sequence       atomic.Int64
	mu             sync.Mutex
	actionMu       sync.Mutex
	actionLocks    map[string]*actionLockEntry
	snapshotTTL    time.Duration
	auditEventHook func(action model.Action, eventType string) error
	shutdown       sync.Once
	shutdownErr    error
}

type actionLockEntry struct {
	mu   sync.Mutex
	refs int
}

func NewServer(cfg config.Config) *Server {
	return NewServerWithDeps(cfg, nil, nil)
}

func NewServerWithDeps(cfg config.Config, store *db.Store, executor *target.Executor) *Server {
	mux := http.NewServeMux()
	s := &Server{
		cfg:         cfg,
		store:       store,
		executor:    executor,
		streamID:    uuid.NewString(),
		actionLocks: map[string]*actionLockEntry{},
		snapshotTTL: defaultActionSnapshotTTL,
		httpSrv: &http.Server{
			Handler:           mux,
			ReadHeaderTimeout: 5 * time.Second,
		},
	}
	if s.executor == nil {
		s.executor = target.NewExecutor(cfg)
	}
	if s.store != nil {
		s.engine = ingest.NewEngine(s.store, s.cfg)
		s.codexEnricher = newCodexSessionEnricher(nil)
	}

	mux.HandleFunc("/v1/health", s.healthHandler)
	if store != nil {
		mux.HandleFunc("/v1/events", s.eventsHandler)
		mux.HandleFunc("/v1/targets", s.targetsHandler)
		mux.HandleFunc("/v1/adapters", s.adaptersHandler)
		mux.HandleFunc("/v1/adapters/", s.adapterByNameHandler)
		mux.HandleFunc("/v1/targets/", s.targetByNameHandler)
		mux.HandleFunc("/v1/panes", s.panesHandler)
		mux.HandleFunc("/v1/windows", s.windowsHandler)
		mux.HandleFunc("/v1/sessions", s.sessionsHandler)
		mux.HandleFunc("/v1/watch", s.watchHandler)
		mux.HandleFunc("/v1/actions/attach", s.attachActionHandler)
		mux.HandleFunc("/v1/actions/send", s.sendActionHandler)
		mux.HandleFunc("/v1/actions/view-output", s.viewOutputActionHandler)
		mux.HandleFunc("/v1/actions/kill", s.killActionHandler)
		mux.HandleFunc("/v1/actions/", s.actionByIDHandler)
	}
	return s
}

func (s *Server) Start(ctx context.Context) error {
	if err := os.MkdirAll(filepath.Dir(s.cfg.SocketPath), 0o755); err != nil {
		return fmt.Errorf("create socket dir: %w", err)
	}
	if err := s.acquireLock(); err != nil {
		return err
	}
	if st, err := os.Lstat(s.cfg.SocketPath); err == nil {
		if st.Mode()&os.ModeSocket == 0 {
			s.releaseLock() //nolint:errcheck
			return fmt.Errorf("socket path exists and is not unix socket: %s", s.cfg.SocketPath)
		}
		if err := os.Remove(s.cfg.SocketPath); err != nil {
			s.releaseLock() //nolint:errcheck
			return fmt.Errorf("remove stale socket: %w", err)
		}
	} else if !errors.Is(err, os.ErrNotExist) {
		s.releaseLock() //nolint:errcheck
		return fmt.Errorf("stat socket path: %w", err)
	}
	ln, err := net.Listen("unix", s.cfg.SocketPath)
	if err != nil {
		s.releaseLock()
		return fmt.Errorf("listen uds: %w", err)
	}
	if err := os.Chmod(s.cfg.SocketPath, 0o600); err != nil {
		ln.Close() //nolint:errcheck
		s.releaseLock()
		return fmt.Errorf("chmod socket: %w", err)
	}
	s.mu.Lock()
	s.listener = ln
	s.mu.Unlock()

	errCh := make(chan error, 1)
	go func() {
		if err := s.httpSrv.Serve(ln); err != nil && !errors.Is(err, http.ErrServerClosed) {
			errCh <- err
		}
		close(errCh)
	}()

	select {
	case <-ctx.Done():
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		_ = s.Shutdown(shutdownCtx)
		return ctx.Err()
	case err := <-errCh:
		if err != nil {
			_ = s.Shutdown(context.Background())
			return fmt.Errorf("serve uds: %w", err)
		}
		return nil
	}
}

func (s *Server) Shutdown(ctx context.Context) error {
	s.shutdown.Do(func() {
		var errs []error
		if s.httpSrv != nil {
			if err := s.httpSrv.Shutdown(ctx); err != nil {
				errs = append(errs, err)
			}
		}
		s.mu.Lock()
		listener := s.listener
		s.listener = nil
		s.mu.Unlock()
		if listener != nil {
			if err := listener.Close(); err != nil {
				errs = append(errs, err)
			}
		}
		if s.cfg.SocketPath != "" {
			if err := os.Remove(s.cfg.SocketPath); err != nil && !errors.Is(err, os.ErrNotExist) {
				errs = append(errs, err)
			}
		}
		if err := s.releaseLock(); err != nil {
			errs = append(errs, err)
		}
		if len(errs) > 0 {
			s.shutdownErr = fmt.Errorf("shutdown errors: %v", errs)
		}
	})
	return s.shutdownErr
}

func (s *Server) healthHandler(w http.ResponseWriter, _ *http.Request) {
	resp := api.HealthResponse{
		SchemaVersion: "v1",
		GeneratedAt:   time.Now().UTC(),
		Status:        "ok",
	}
	s.writeJSON(w, http.StatusOK, resp)
}

func (s *Server) targetsHandler(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodGet:
		s.listTargets(w, r)
	case http.MethodPost:
		s.createTarget(w, r)
	default:
		s.methodNotAllowed(w, http.MethodGet, http.MethodPost)
	}
}

func (s *Server) adaptersHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		s.methodNotAllowed(w, http.MethodGet)
		return
	}
	s.listAdapters(w, r)
}

func (s *Server) adapterByNameHandler(w http.ResponseWriter, r *http.Request) {
	tail := strings.TrimPrefix(r.URL.Path, "/v1/adapters/")
	parts := strings.Split(strings.Trim(tail, "/"), "/")
	if len(parts) != 2 || strings.TrimSpace(parts[0]) == "" {
		s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "adapter route not found")
		return
	}
	adapterName, err := url.PathUnescape(parts[0])
	if err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalidEncoding, "invalid adapter encoding")
		return
	}
	if r.Method != http.MethodPost {
		s.methodNotAllowed(w, http.MethodPost)
		return
	}
	switch parts[1] {
	case "enable":
		s.setAdapterEnabled(w, r, strings.TrimSpace(adapterName), true)
	case "disable":
		s.setAdapterEnabled(w, r, strings.TrimSpace(adapterName), false)
	default:
		s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "adapter route not found")
	}
}

func (s *Server) targetByNameHandler(w http.ResponseWriter, r *http.Request) {
	tail := strings.TrimPrefix(r.URL.Path, "/v1/targets/")
	parts := strings.Split(strings.Trim(tail, "/"), "/")
	if len(parts) == 0 || parts[0] == "" {
		s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "target not found")
		return
	}
	targetName, err := url.PathUnescape(parts[0])
	if err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalidEncoding, "invalid target encoding")
		return
	}
	if len(parts) == 1 {
		if r.Method != http.MethodDelete {
			s.methodNotAllowed(w, http.MethodDelete)
			return
		}
		s.deleteTarget(w, r, targetName)
		return
	}
	if len(parts) == 2 && parts[1] == "connect" {
		if r.Method != http.MethodPost {
			s.methodNotAllowed(w, http.MethodPost)
			return
		}
		s.connectTarget(w, r, targetName)
		return
	}
	s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "target route not found")
}

func (s *Server) actionByIDHandler(w http.ResponseWriter, r *http.Request) {
	tail := strings.TrimPrefix(r.URL.Path, "/v1/actions/")
	if !strings.HasSuffix(tail, "/events") {
		s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "action route not found")
		return
	}
	rawID := strings.TrimSuffix(tail, "/events")
	if rawID == "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "action_id is required")
		return
	}
	if strings.Contains(rawID, "/") {
		s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "action route not found")
		return
	}
	if r.Method != http.MethodGet {
		s.methodNotAllowed(w, http.MethodGet)
		return
	}
	actionID, err := url.PathUnescape(rawID)
	if err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalidEncoding, "invalid action_id encoding")
		return
	}
	s.listActionEvents(w, r, strings.TrimSpace(actionID))
}

func (s *Server) eventsHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		s.methodNotAllowed(w, http.MethodPost)
		return
	}
	if s.engine == nil {
		s.writeError(w, http.StatusServiceUnavailable, model.ErrPreconditionFailed, "event ingestion is unavailable")
		return
	}

	var req ingestEventRequest
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	if err := dec.Decode(&req); err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "invalid request body")
		return
	}

	req.EventID = strings.TrimSpace(req.EventID)
	req.EventType = strings.TrimSpace(req.EventType)
	req.Source = strings.TrimSpace(strings.ToLower(req.Source))
	req.DedupeKey = strings.TrimSpace(req.DedupeKey)
	req.SourceEventID = strings.TrimSpace(req.SourceEventID)
	req.EventTime = strings.TrimSpace(req.EventTime)
	req.RuntimeID = strings.TrimSpace(req.RuntimeID)
	req.Target = strings.TrimSpace(req.Target)
	req.TargetID = strings.TrimSpace(req.TargetID)
	req.PaneID = strings.TrimSpace(req.PaneID)
	req.StartHint = strings.TrimSpace(req.StartHint)
	req.AgentType = strings.TrimSpace(strings.ToLower(req.AgentType))

	if req.EventType == "" || req.Source == "" || req.DedupeKey == "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "event_type, source, dedupe_key are required")
		return
	}
	source, ok := parseEventSource(req.Source)
	if !ok {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "source must be hook, notify, wrapper, or poller")
		return
	}

	now := time.Now().UTC()
	eventTime := now
	if req.EventTime != "" {
		parsed, err := time.Parse(time.RFC3339Nano, req.EventTime)
		if err != nil {
			s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "event_time must be RFC3339")
			return
		}
		eventTime = parsed.UTC()
		maxFuture := now.Add(s.cfg.SkewBudget)
		if s.cfg.SkewBudget > 0 && eventTime.After(maxFuture) {
			eventTime = now
		}
	}

	var startHint *time.Time
	if req.StartHint != "" {
		parsed, err := time.Parse(time.RFC3339Nano, req.StartHint)
		if err != nil {
			s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "start_hint must be RFC3339")
			return
		}
		t := parsed.UTC()
		startHint = &t
	}

	ev := model.EventEnvelope{
		EventID:       req.EventID,
		EventType:     req.EventType,
		Source:        source,
		DedupeKey:     req.DedupeKey,
		SourceEventID: req.SourceEventID,
		SourceSeq:     req.SourceSeq,
		EventTime:     eventTime,
		IngestedAt:    now,
		PID:           req.PID,
		StartHint:     startHint,
		RawPayload:    req.RawPayload,
	}
	if ev.EventID == "" {
		ev.EventID = uuid.NewString()
	}

	status := "pending_bind"
	if req.RuntimeID != "" {
		rt, err := s.store.GetRuntime(r.Context(), req.RuntimeID)
		if err != nil {
			if errors.Is(err, db.ErrNotFound) {
				s.writeError(w, http.StatusConflict, model.ErrRuntimeStale, "runtime not found")
				return
			}
			s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to resolve runtime")
			return
		}
		if rt.EndedAt != nil {
			s.writeError(w, http.StatusConflict, model.ErrRuntimeStale, "runtime ended")
			return
		}
		ev.RuntimeID = rt.RuntimeID
		ev.TargetID = rt.TargetID
		ev.PaneID = rt.PaneID
		status = "bound"
	} else {
		if req.PaneID == "" {
			s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "pane_id is required when runtime_id is absent")
			return
		}

		tg, err := s.resolveEventTarget(r.Context(), req.Target, req.TargetID)
		if err != nil {
			if errors.Is(err, db.ErrNotFound) {
				s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "target not found")
				return
			}
			s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to resolve target")
			return
		}
		ev.TargetID = tg.TargetID
		ev.PaneID = req.PaneID

		if err := s.ensureEventPane(r.Context(), tg.TargetID, req.PaneID, now); err != nil {
			s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to ensure pane")
			return
		}

		if runtimeID, ok, err := s.resolveRuntimeCandidateForEvent(r.Context(), tg.TargetID, req.PaneID, req.PID, startHint, req.AgentType); err != nil {
			s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to resolve runtime candidate")
			return
		} else if ok {
			ev.RuntimeID = runtimeID
			status = "bound"
		}
	}

	if err := s.engine.Ingest(r.Context(), ev); err != nil {
		s.writeIngestError(w, err)
		return
	}
	resp := api.EventIngestResponse{
		SchemaVersion: "v1",
		GeneratedAt:   time.Now().UTC(),
		EventID:       ev.EventID,
		Status:        status,
	}
	if strings.TrimSpace(ev.RuntimeID) != "" {
		resp.RuntimeID = strings.TrimSpace(ev.RuntimeID)
	}
	s.writeJSON(w, http.StatusAccepted, resp)
}

func (s *Server) panesHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		s.methodNotAllowed(w, http.MethodGet)
		return
	}
	targets, filters, err := s.resolveTargetFilter(r.Context(), r.URL.Query().Get("target"))
	if err != nil {
		s.writeResolveTargetError(w, err)
		return
	}
	items, summary, err := s.buildPaneItems(r.Context(), targets)
	if err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, err.Error())
		return
	}
	names := targetNames(targets)
	resp := api.ListEnvelope[api.PaneItem]{
		SchemaVersion:    "v1",
		GeneratedAt:      time.Now().UTC(),
		Filters:          filters,
		Summary:          summary,
		Partial:          false,
		RequestedTargets: names,
		RespondedTargets: names,
		Items:            items,
	}
	s.writeJSON(w, http.StatusOK, resp)
}

func (s *Server) windowsHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		s.methodNotAllowed(w, http.MethodGet)
		return
	}
	targets, filters, err := s.resolveTargetFilter(r.Context(), r.URL.Query().Get("target"))
	if err != nil {
		s.writeResolveTargetError(w, err)
		return
	}
	panes, summary, err := s.buildPaneItems(r.Context(), targets)
	if err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, err.Error())
		return
	}
	items := buildWindowItems(panes)
	names := targetNames(targets)
	resp := api.ListEnvelope[api.WindowItem]{
		SchemaVersion:    "v1",
		GeneratedAt:      time.Now().UTC(),
		Filters:          filters,
		Summary:          summary,
		Partial:          false,
		RequestedTargets: names,
		RespondedTargets: names,
		Items:            items,
	}
	s.writeJSON(w, http.StatusOK, resp)
}

func (s *Server) sessionsHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		s.methodNotAllowed(w, http.MethodGet)
		return
	}
	targets, filters, err := s.resolveTargetFilter(r.Context(), r.URL.Query().Get("target"))
	if err != nil {
		s.writeResolveTargetError(w, err)
		return
	}
	panes, summary, err := s.buildPaneItems(r.Context(), targets)
	if err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, err.Error())
		return
	}
	items := buildSessionItems(panes)
	names := targetNames(targets)
	resp := api.ListEnvelope[api.SessionItem]{
		SchemaVersion:    "v1",
		GeneratedAt:      time.Now().UTC(),
		Filters:          filters,
		Summary:          summary,
		Partial:          false,
		RequestedTargets: names,
		RespondedTargets: names,
		Items:            items,
	}
	s.writeJSON(w, http.StatusOK, resp)
}

func (s *Server) watchHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		s.methodNotAllowed(w, http.MethodGet)
		return
	}
	scope := r.URL.Query().Get("scope")
	if scope == "" {
		scope = "panes"
	}
	if scope != "panes" && scope != "windows" && scope != "sessions" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "invalid scope")
		return
	}
	cursorStreamID, cursorSeq, hasCursor, err := parseCursor(r.URL.Query().Get("cursor"))
	if err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrCursorInvalid, "invalid cursor")
		return
	}

	targets, filters, err := s.resolveTargetFilter(r.Context(), r.URL.Query().Get("target"))
	if err != nil {
		s.writeResolveTargetError(w, err)
		return
	}
	panes, summary, err := s.buildPaneItems(r.Context(), targets)
	if err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, err.Error())
		return
	}

	w.Header().Set("Content-Type", "application/x-ndjson")
	enc := json.NewEncoder(w)
	generatedAt := time.Now().UTC()

	currentSeq := s.sequence.Load()
	if hasCursor && (cursorStreamID != s.streamID || cursorSeq < currentSeq) {
		seq := s.nextSequence()
		reset := api.WatchLine{
			SchemaVersion: "v1",
			GeneratedAt:   generatedAt,
			EmittedAt:     time.Now().UTC(),
			StreamID:      s.streamID,
			Cursor:        fmt.Sprintf("%s:%d", s.streamID, seq),
			Scope:         scope,
			Type:          "reset",
			Sequence:      seq,
			Filters:       filters,
			Summary:       summary,
		}
		_ = enc.Encode(reset)
	}

	var items any
	switch scope {
	case "panes":
		items = panes
	case "windows":
		items = buildWindowItems(panes)
	case "sessions":
		items = buildSessionItems(panes)
	}
	seq := s.nextSequence()
	line := api.WatchLine{
		SchemaVersion: "v1",
		GeneratedAt:   generatedAt,
		EmittedAt:     time.Now().UTC(),
		StreamID:      s.streamID,
		Cursor:        fmt.Sprintf("%s:%d", s.streamID, seq),
		Scope:         scope,
		Type:          "snapshot",
		Sequence:      seq,
		Filters:       filters,
		Summary:       summary,
		Items:         items,
	}
	_ = enc.Encode(line)
}

type createTargetRequest struct {
	Name          string `json:"name"`
	Kind          string `json:"kind"`
	ConnectionRef string `json:"connection_ref"`
	IsDefault     bool   `json:"is_default"`
}

type attachActionRequest struct {
	RequestRef      string `json:"request_ref"`
	Target          string `json:"target"`
	PaneID          string `json:"pane_id"`
	IfRuntime       string `json:"if_runtime"`
	IfState         string `json:"if_state"`
	IfUpdatedWithin string `json:"if_updated_within"`
	ForceStale      bool   `json:"force_stale"`
}

type sendActionRequest struct {
	RequestRef      string `json:"request_ref"`
	Target          string `json:"target"`
	PaneID          string `json:"pane_id"`
	Text            string `json:"text"`
	Key             string `json:"key"`
	Enter           bool   `json:"enter"`
	Paste           bool   `json:"paste"`
	IfRuntime       string `json:"if_runtime"`
	IfState         string `json:"if_state"`
	IfUpdatedWithin string `json:"if_updated_within"`
	ForceStale      bool   `json:"force_stale"`
}

type viewOutputActionRequest struct {
	RequestRef      string `json:"request_ref"`
	Target          string `json:"target"`
	PaneID          string `json:"pane_id"`
	Lines           int    `json:"lines"`
	IfRuntime       string `json:"if_runtime"`
	IfState         string `json:"if_state"`
	IfUpdatedWithin string `json:"if_updated_within"`
	ForceStale      bool   `json:"force_stale"`
}

type killActionRequest struct {
	RequestRef      string `json:"request_ref"`
	Target          string `json:"target"`
	PaneID          string `json:"pane_id"`
	Mode            string `json:"mode"`
	Signal          string `json:"signal"`
	IfRuntime       string `json:"if_runtime"`
	IfState         string `json:"if_state"`
	IfUpdatedWithin string `json:"if_updated_within"`
	ForceStale      bool   `json:"force_stale"`
}

type ingestEventRequest struct {
	EventID       string `json:"event_id"`
	EventType     string `json:"event_type"`
	Source        string `json:"source"`
	DedupeKey     string `json:"dedupe_key"`
	SourceEventID string `json:"source_event_id"`
	SourceSeq     *int64 `json:"source_seq"`
	EventTime     string `json:"event_time"`
	RuntimeID     string `json:"runtime_id"`
	Target        string `json:"target"`
	TargetID      string `json:"target_id"`
	PaneID        string `json:"pane_id"`
	PID           *int64 `json:"pid"`
	StartHint     string `json:"start_hint"`
	AgentType     string `json:"agent_type"`
	RawPayload    string `json:"raw_payload"`
}

func (s *Server) listTargets(w http.ResponseWriter, r *http.Request) {
	targets, err := s.store.ListTargets(r.Context())
	if err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, err.Error())
		return
	}
	resp := api.TargetsEnvelope{
		SchemaVersion: "v1",
		GeneratedAt:   time.Now().UTC(),
		Targets:       make([]api.TargetResponse, 0, len(targets)),
	}
	for _, t := range targets {
		resp.Targets = append(resp.Targets, toTargetResponse(t))
	}
	s.writeJSON(w, http.StatusOK, resp)
}

func (s *Server) listAdapters(w http.ResponseWriter, r *http.Request) {
	var enabledFilter *bool
	if rawEnabled := strings.TrimSpace(r.URL.Query().Get("enabled")); rawEnabled != "" {
		parsed, err := strconv.ParseBool(rawEnabled)
		if err != nil {
			s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "enabled must be true or false")
			return
		}
		enabledFilter = &parsed
	}
	adapters, err := s.store.ListAdaptersFiltered(r.Context(), enabledFilter)
	if err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, err.Error())
		return
	}
	resp := api.AdaptersEnvelope{
		SchemaVersion: "v1",
		GeneratedAt:   time.Now().UTC(),
		Adapters:      make([]api.AdapterResponse, 0, len(adapters)),
	}
	for _, adapter := range adapters {
		resp.Adapters = append(resp.Adapters, toAdapterResponse(adapter))
	}
	s.writeJSON(w, http.StatusOK, resp)
}

func (s *Server) setAdapterEnabled(w http.ResponseWriter, r *http.Request, adapterName string, enabled bool) {
	current, err := s.store.GetAdapterByName(r.Context(), adapterName)
	if err != nil {
		if errors.Is(err, db.ErrNotFound) {
			s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "adapter not found")
			return
		}
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to fetch adapter")
		return
	}
	if enabled && !adapterpkg.IsVersionCompatible(current.Version) {
		s.writeError(w, http.StatusPreconditionFailed, model.ErrPreconditionFailed, "adapter contract version is incompatible")
		return
	}
	updated, err := s.store.SetAdapterEnabledByName(r.Context(), adapterName, enabled, time.Now().UTC())
	if err != nil {
		if errors.Is(err, db.ErrNotFound) {
			s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "adapter not found")
			return
		}
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to update adapter")
		return
	}
	resp := api.AdaptersEnvelope{
		SchemaVersion: "v1",
		GeneratedAt:   time.Now().UTC(),
		Adapters:      []api.AdapterResponse{toAdapterResponse(updated)},
	}
	s.writeJSON(w, http.StatusOK, resp)
}

func (s *Server) createTarget(w http.ResponseWriter, r *http.Request) {
	var req createTargetRequest
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	if err := dec.Decode(&req); err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "invalid request body")
		return
	}
	name := strings.TrimSpace(req.Name)
	if name == "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "name is required")
		return
	}
	kind := model.TargetKind(strings.TrimSpace(req.Kind))
	if kind == "" {
		kind = model.TargetKindLocal
	}
	if kind != model.TargetKindLocal && kind != model.TargetKindSSH {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "kind must be local or ssh")
		return
	}
	now := time.Now().UTC()
	if req.IsDefault {
		current, err := s.store.ListTargets(r.Context())
		if err == nil {
			for _, t := range current {
				if !t.IsDefault {
					continue
				}
				if t.TargetName == name {
					continue
				}
				t.IsDefault = false
				t.UpdatedAt = now
				_ = s.store.UpsertTarget(r.Context(), t)
			}
		}
	}
	t := model.Target{
		TargetID:      name,
		TargetName:    name,
		Kind:          kind,
		ConnectionRef: strings.TrimSpace(req.ConnectionRef),
		IsDefault:     req.IsDefault,
		Health:        model.TargetHealthOK,
		UpdatedAt:     now,
	}
	if err := s.store.UpsertTarget(r.Context(), t); err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, err.Error())
		return
	}
	targetRow, err := s.store.GetTargetByName(r.Context(), name)
	if err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, err.Error())
		return
	}
	resp := api.TargetsEnvelope{
		SchemaVersion: "v1",
		GeneratedAt:   time.Now().UTC(),
		Targets:       []api.TargetResponse{toTargetResponse(targetRow)},
	}
	s.writeJSON(w, http.StatusCreated, resp)
}

func (s *Server) connectTarget(w http.ResponseWriter, r *http.Request, targetName string) {
	tg, err := s.store.GetTargetByName(r.Context(), targetName)
	if err != nil {
		if errors.Is(err, db.ErrNotFound) {
			s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "target not found")
			return
		}
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, err.Error())
		return
	}

	now := time.Now().UTC()
	_, runErr := s.executor.Run(r.Context(), tg, target.BuildTmuxCommand("list-sessions"))
	if runErr != nil {
		tg.Health = model.TargetHealthDown
		tg.UpdatedAt = now
		_ = s.store.UpsertTarget(r.Context(), tg)
		s.writeError(w, http.StatusBadGateway, model.ErrTargetUnreachable, runErr.Error())
		return
	}

	tg.Health = model.TargetHealthOK
	tg.UpdatedAt = now
	tg.LastSeenAt = &now
	if err := s.store.UpsertTarget(r.Context(), tg); err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, err.Error())
		return
	}
	resp := api.TargetsEnvelope{
		SchemaVersion: "v1",
		GeneratedAt:   now,
		Targets:       []api.TargetResponse{toTargetResponse(tg)},
	}
	s.writeJSON(w, http.StatusOK, resp)
}

func (s *Server) deleteTarget(w http.ResponseWriter, r *http.Request, targetName string) {
	if err := s.store.DeleteTargetByName(r.Context(), targetName); err != nil {
		if errors.Is(err, db.ErrNotFound) {
			s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "target not found")
			return
		}
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, err.Error())
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) listActionEvents(w http.ResponseWriter, r *http.Request, actionID string) {
	if actionID == "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "action_id is required")
		return
	}
	if _, err := s.store.GetActionByID(r.Context(), actionID); err != nil {
		if errors.Is(err, db.ErrNotFound) {
			s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "action not found")
			return
		}
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to fetch action")
		return
	}
	events, err := s.store.ListEventsByActionID(r.Context(), actionID)
	if err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to list action events")
		return
	}
	items := make([]api.ActionEventItem, 0, len(events))
	for _, ev := range events {
		items = append(items, toActionEventItem(ev))
	}
	resp := api.ActionEventsEnvelope{
		SchemaVersion: "v1",
		GeneratedAt:   time.Now().UTC(),
		ActionID:      actionID,
		Events:        items,
	}
	s.writeJSON(w, http.StatusOK, resp)
}

func (s *Server) attachActionHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		s.methodNotAllowed(w, http.MethodPost)
		return
	}
	var req attachActionRequest
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	if err := dec.Decode(&req); err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "invalid request body")
		return
	}
	req.RequestRef = strings.TrimSpace(req.RequestRef)
	req.Target = strings.TrimSpace(req.Target)
	req.PaneID = strings.TrimSpace(req.PaneID)
	req.IfRuntime = strings.TrimSpace(req.IfRuntime)
	req.IfState = strings.TrimSpace(strings.ToLower(req.IfState))
	req.IfUpdatedWithin = strings.TrimSpace(req.IfUpdatedWithin)
	if req.RequestRef == "" || req.Target == "" || req.PaneID == "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "request_ref, target, pane_id are required")
		return
	}
	unlock := s.lockActionKey(model.ActionTypeAttach, req.RequestRef)
	defer unlock()

	if existing, replay, conflict, err := s.lookupIdempotentAction(r.Context(), model.ActionTypeAttach, req.RequestRef, req.Target, req.PaneID, ""); err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to resolve idempotent action")
		return
	} else if conflict {
		s.writeError(w, http.StatusConflict, model.ErrIdempotencyConflict, "idempotency payload mismatch")
		return
	} else if replay {
		if err := s.emitActionAuditEvent(r.Context(), existing, "action.attach"); err != nil {
			s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action audit event")
			return
		}
		s.writeJSON(w, http.StatusOK, toActionResponse(existing))
		return
	}

	tg, err := s.store.GetTargetByName(r.Context(), req.Target)
	if err != nil {
		if errors.Is(err, db.ErrNotFound) {
			s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "target not found")
			return
		}
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to resolve target")
		return
	}
	panes, err := s.store.ListPanes(r.Context())
	if err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to list panes")
		return
	}
	foundPane := false
	for _, pane := range panes {
		if pane.TargetID == tg.TargetID && pane.PaneID == req.PaneID {
			foundPane = true
			break
		}
	}
	if !foundPane {
		s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "pane not found")
		return
	}
	guards, guardErr := parseActionGuardOptions(req.IfRuntime, req.IfState, req.IfUpdatedWithin, req.ForceStale)
	if guardErr != nil {
		guardErr.write(s, w)
		return
	}
	snapshot, preErr := s.prepareActionSnapshot(r.Context(), tg.TargetID, req.PaneID, guards)
	if preErr != nil {
		preErr.write(s, w)
		return
	}
	var runtimeID *string
	if snapshot != nil {
		v := strings.TrimSpace(snapshot.RuntimeID)
		if v != "" {
			runtimeID = &v
		}
	}

	now := time.Now().UTC()
	completedAt := now
	action := model.Action{
		ActionID:    uuid.NewString(),
		ActionType:  model.ActionTypeAttach,
		RequestRef:  req.RequestRef,
		TargetID:    tg.TargetID,
		PaneID:      req.PaneID,
		RuntimeID:   runtimeID,
		RequestedAt: now,
		CompletedAt: &completedAt,
		ResultCode:  "completed",
	}
	if err := s.store.InsertAction(r.Context(), action); err != nil {
		if errors.Is(err, db.ErrDuplicate) {
			existing, getErr := s.store.GetActionByTypeRequestRef(r.Context(), model.ActionTypeAttach, req.RequestRef)
			if getErr != nil {
				s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to fetch idempotent action")
				return
			}
			if existing.TargetID != action.TargetID || existing.PaneID != action.PaneID {
				s.writeError(w, http.StatusConflict, model.ErrIdempotencyConflict, "idempotency payload mismatch")
				return
			}
			if err := s.emitActionAuditEvent(r.Context(), existing, "action.attach"); err != nil {
				s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action audit event")
				return
			}
			s.writeJSON(w, http.StatusOK, toActionResponse(existing))
			return
		}
		if errors.Is(err, db.ErrNotFound) {
			s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "target or pane not found")
			return
		}
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to insert action")
		return
	}
	if err := s.persistActionSnapshot(r.Context(), action, snapshot); err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action snapshot")
		return
	}
	if err := s.emitActionAuditEvent(r.Context(), action, "action.attach"); err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action audit event")
		return
	}
	s.writeJSON(w, http.StatusOK, toActionResponse(action))
}

func (s *Server) sendActionHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		s.methodNotAllowed(w, http.MethodPost)
		return
	}
	var req sendActionRequest
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	if err := dec.Decode(&req); err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "invalid request body")
		return
	}
	req.RequestRef = strings.TrimSpace(req.RequestRef)
	req.Target = strings.TrimSpace(req.Target)
	req.PaneID = strings.TrimSpace(req.PaneID)
	req.Key = strings.TrimSpace(req.Key)
	req.IfRuntime = strings.TrimSpace(req.IfRuntime)
	req.IfState = strings.TrimSpace(strings.ToLower(req.IfState))
	req.IfUpdatedWithin = strings.TrimSpace(req.IfUpdatedWithin)
	if req.RequestRef == "" || req.Target == "" || req.PaneID == "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "request_ref, target, pane_id are required")
		return
	}
	if req.Text == "" && req.Key == "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "either text or key is required")
		return
	}
	if req.Text != "" && req.Key != "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "text and key are mutually exclusive")
		return
	}
	unlock := s.lockActionKey(model.ActionTypeSend, req.RequestRef)
	defer unlock()

	metaRaw, err := marshalActionMetadata(map[string]any{
		"text":  req.Text,
		"key":   req.Key,
		"enter": req.Enter,
		"paste": req.Paste,
	})
	if err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to encode action metadata")
		return
	}
	if existing, replay, conflict, err := s.lookupIdempotentAction(r.Context(), model.ActionTypeSend, req.RequestRef, req.Target, req.PaneID, metaRaw); err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to resolve idempotent action")
		return
	} else if conflict {
		s.writeError(w, http.StatusConflict, model.ErrIdempotencyConflict, "idempotency payload mismatch")
		return
	} else if replay {
		if err := s.emitActionAuditEvent(r.Context(), existing, "action.send"); err != nil {
			s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action audit event")
			return
		}
		s.writeJSON(w, http.StatusOK, toActionResponse(existing))
		return
	}

	tg, err := s.resolveTargetAndPane(r.Context(), req.Target, req.PaneID)
	if err != nil {
		s.writeActionResolveError(w, err)
		return
	}
	guards, guardErr := parseActionGuardOptions(req.IfRuntime, req.IfState, req.IfUpdatedWithin, req.ForceStale)
	if guardErr != nil {
		guardErr.write(s, w)
		return
	}
	snapshot, preErr := s.prepareActionSnapshot(r.Context(), tg.TargetID, req.PaneID, guards)
	if preErr != nil {
		preErr.write(s, w)
		return
	}
	var runtimeID *string
	if snapshot != nil {
		v := strings.TrimSpace(snapshot.RuntimeID)
		if v != "" {
			runtimeID = &v
		}
	}

	cmd := []string{"send-keys", "-t", req.PaneID}
	if req.Text != "" {
		if req.Paste {
			cmd = append(cmd, "-l")
		}
		cmd = append(cmd, req.Text)
	} else {
		cmd = append(cmd, req.Key)
	}
	if req.Enter {
		cmd = append(cmd, "Enter")
	}
	resultCode := "completed"
	var errorCode *string
	if _, runErr := s.executor.Run(r.Context(), tg, target.BuildTmuxCommand(cmd...)); runErr != nil {
		resultCode = "failed"
		v := model.ErrTargetUnreachable
		errorCode = &v
	}
	action, created, err := s.insertActionWithReplay(r.Context(), model.ActionTypeSend, req.RequestRef, tg.TargetID, req.PaneID, runtimeID, metaRaw, resultCode, errorCode)
	if err != nil {
		if errors.Is(err, db.ErrDuplicate) {
			s.writeError(w, http.StatusConflict, model.ErrIdempotencyConflict, "idempotency payload mismatch")
			return
		}
		if errors.Is(err, db.ErrNotFound) {
			s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "target or pane not found")
			return
		}
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action")
		return
	}
	if created {
		if err := s.persistActionSnapshot(r.Context(), action, snapshot); err != nil {
			s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action snapshot")
			return
		}
	}
	if err := s.emitActionAuditEvent(r.Context(), action, "action.send"); err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action audit event")
		return
	}
	s.writeJSON(w, http.StatusOK, toActionResponse(action))
}

func (s *Server) viewOutputActionHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		s.methodNotAllowed(w, http.MethodPost)
		return
	}
	var req viewOutputActionRequest
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	if err := dec.Decode(&req); err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "invalid request body")
		return
	}
	req.RequestRef = strings.TrimSpace(req.RequestRef)
	req.Target = strings.TrimSpace(req.Target)
	req.PaneID = strings.TrimSpace(req.PaneID)
	req.IfRuntime = strings.TrimSpace(req.IfRuntime)
	req.IfState = strings.TrimSpace(strings.ToLower(req.IfState))
	req.IfUpdatedWithin = strings.TrimSpace(req.IfUpdatedWithin)
	if req.RequestRef == "" || req.Target == "" || req.PaneID == "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "request_ref, target, pane_id are required")
		return
	}
	if req.Lines <= 0 {
		req.Lines = defaultViewOutputLines
	}
	unlock := s.lockActionKey(model.ActionTypeViewOutput, req.RequestRef)
	defer unlock()

	preMeta, err := marshalActionMetadata(map[string]any{"lines": req.Lines})
	if err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to encode action metadata")
		return
	}
	if existing, replay, conflict, err := s.lookupIdempotentAction(r.Context(), model.ActionTypeViewOutput, req.RequestRef, req.Target, req.PaneID, preMeta); err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to resolve idempotent action")
		return
	} else if conflict {
		s.writeError(w, http.StatusConflict, model.ErrIdempotencyConflict, "idempotency payload mismatch")
		return
	} else if replay {
		if err := s.emitActionAuditEvent(r.Context(), existing, "action.view-output"); err != nil {
			s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action audit event")
			return
		}
		s.writeJSON(w, http.StatusOK, toActionResponse(existing))
		return
	}

	tg, err := s.resolveTargetAndPane(r.Context(), req.Target, req.PaneID)
	if err != nil {
		s.writeActionResolveError(w, err)
		return
	}
	guards, guardErr := parseActionGuardOptions(req.IfRuntime, req.IfState, req.IfUpdatedWithin, req.ForceStale)
	if guardErr != nil {
		guardErr.write(s, w)
		return
	}
	snapshot, preErr := s.prepareActionSnapshot(r.Context(), tg.TargetID, req.PaneID, guards)
	if preErr != nil {
		preErr.write(s, w)
		return
	}
	var runtimeID *string
	if snapshot != nil {
		v := strings.TrimSpace(snapshot.RuntimeID)
		if v != "" {
			runtimeID = &v
		}
	}

	resultCode := "completed"
	var errorCode *string
	output := ""
	runResult, runErr := s.executor.Run(r.Context(), tg, target.BuildTmuxCommand("capture-pane", "-t", req.PaneID, "-p", "-S", fmt.Sprintf("-%d", req.Lines)))
	if runErr != nil {
		resultCode = "failed"
		v := model.ErrTargetUnreachable
		errorCode = &v
	} else {
		output = runResult.Output
	}
	action, created, err := s.insertActionWithReplay(r.Context(), model.ActionTypeViewOutput, req.RequestRef, tg.TargetID, req.PaneID, runtimeID, preMeta, resultCode, errorCode)
	if err != nil {
		if errors.Is(err, db.ErrDuplicate) {
			s.writeError(w, http.StatusConflict, model.ErrIdempotencyConflict, "idempotency payload mismatch")
			return
		}
		if errors.Is(err, db.ErrNotFound) {
			s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "target or pane not found")
			return
		}
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action")
		return
	}
	if created {
		if err := s.persistActionSnapshot(r.Context(), action, snapshot); err != nil {
			s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action snapshot")
			return
		}
	}
	if err := s.emitActionAuditEvent(r.Context(), action, "action.view-output"); err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action audit event")
		return
	}
	resp := toActionResponse(action)
	if output != "" {
		resp.Output = &output
	}
	s.writeJSON(w, http.StatusOK, resp)
}

func (s *Server) killActionHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		s.methodNotAllowed(w, http.MethodPost)
		return
	}
	var req killActionRequest
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	if err := dec.Decode(&req); err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "invalid request body")
		return
	}
	req.RequestRef = strings.TrimSpace(req.RequestRef)
	req.Target = strings.TrimSpace(req.Target)
	req.PaneID = strings.TrimSpace(req.PaneID)
	req.Mode = strings.ToLower(strings.TrimSpace(req.Mode))
	req.Signal = strings.ToUpper(strings.TrimSpace(req.Signal))
	req.IfRuntime = strings.TrimSpace(req.IfRuntime)
	req.IfState = strings.TrimSpace(strings.ToLower(req.IfState))
	req.IfUpdatedWithin = strings.TrimSpace(req.IfUpdatedWithin)
	if req.RequestRef == "" || req.Target == "" || req.PaneID == "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "request_ref, target, pane_id are required")
		return
	}
	if req.Mode == "" {
		req.Mode = "key"
	}
	if req.Signal == "" {
		req.Signal = "INT"
	}
	unlock := s.lockActionKey(model.ActionTypeKill, req.RequestRef)
	defer unlock()

	metaRaw, err := marshalActionMetadata(map[string]any{
		"mode":   req.Mode,
		"signal": req.Signal,
	})
	if err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to encode action metadata")
		return
	}
	if existing, replay, conflict, err := s.lookupIdempotentAction(r.Context(), model.ActionTypeKill, req.RequestRef, req.Target, req.PaneID, metaRaw); err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to resolve idempotent action")
		return
	} else if conflict {
		s.writeError(w, http.StatusConflict, model.ErrIdempotencyConflict, "idempotency payload mismatch")
		return
	} else if replay {
		if err := s.emitActionAuditEvent(r.Context(), existing, "action.kill"); err != nil {
			s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action audit event")
			return
		}
		s.writeJSON(w, http.StatusOK, toActionResponse(existing))
		return
	}

	tg, err := s.resolveTargetAndPane(r.Context(), req.Target, req.PaneID)
	if err != nil {
		s.writeActionResolveError(w, err)
		return
	}
	guards, guardErr := parseActionGuardOptions(req.IfRuntime, req.IfState, req.IfUpdatedWithin, req.ForceStale)
	if guardErr != nil {
		guardErr.write(s, w)
		return
	}
	snapshot, preErr := s.prepareActionSnapshot(r.Context(), tg.TargetID, req.PaneID, guards)
	if preErr != nil {
		preErr.write(s, w)
		return
	}
	var runtimeID *string
	if snapshot != nil {
		v := strings.TrimSpace(snapshot.RuntimeID)
		if v != "" {
			runtimeID = &v
		}
	}

	var cmd []string
	switch req.Mode {
	case "key":
		if req.Signal != "INT" {
			s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "key mode only supports INT signal")
			return
		}
		cmd = target.BuildTmuxCommand("send-keys", "-t", req.PaneID, "C-c")
	case "signal":
		if req.Signal != "INT" && req.Signal != "TERM" && req.Signal != "KILL" {
			s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "signal must be INT, TERM, or KILL")
			return
		}
		if runtimeID == nil || strings.TrimSpace(*runtimeID) == "" {
			s.writeError(w, http.StatusBadRequest, model.ErrPIDUnavailable, "runtime pid unavailable")
			return
		}
		pid, pidErr := s.findRuntimePID(r.Context(), strings.TrimSpace(*runtimeID))
		if pidErr != nil {
			s.writeError(w, http.StatusBadRequest, model.ErrPIDUnavailable, "runtime pid unavailable")
			return
		}
		cmd = []string{"kill", "-" + req.Signal, strconv.FormatInt(pid, 10)}
	default:
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "mode must be key or signal")
		return
	}

	resultCode := "completed"
	var errorCode *string
	if _, runErr := s.executor.Run(r.Context(), tg, cmd); runErr != nil {
		resultCode = "failed"
		v := model.ErrTargetUnreachable
		errorCode = &v
	}
	action, created, err := s.insertActionWithReplay(r.Context(), model.ActionTypeKill, req.RequestRef, tg.TargetID, req.PaneID, runtimeID, metaRaw, resultCode, errorCode)
	if err != nil {
		if errors.Is(err, db.ErrDuplicate) {
			s.writeError(w, http.StatusConflict, model.ErrIdempotencyConflict, "idempotency payload mismatch")
			return
		}
		if errors.Is(err, db.ErrNotFound) {
			s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "target or pane not found")
			return
		}
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action")
		return
	}
	if created {
		if err := s.persistActionSnapshot(r.Context(), action, snapshot); err != nil {
			s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action snapshot")
			return
		}
	}
	if err := s.emitActionAuditEvent(r.Context(), action, "action.kill"); err != nil {
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to persist action audit event")
		return
	}
	s.writeJSON(w, http.StatusOK, toActionResponse(action))
}

type actionGuardOptions struct {
	ifRuntime       string
	ifState         string
	ifUpdatedWithin *time.Duration
	forceStale      bool
}

type apiError struct {
	status  int
	code    string
	message string
}

func (e *apiError) write(s *Server, w http.ResponseWriter) {
	if e == nil {
		return
	}
	s.writeError(w, e.status, e.code, e.message)
}

func parseActionGuardOptions(ifRuntime, ifState, ifUpdatedWithin string, forceStale bool) (actionGuardOptions, *apiError) {
	opts := actionGuardOptions{
		ifRuntime:  strings.TrimSpace(ifRuntime),
		ifState:    strings.TrimSpace(strings.ToLower(ifState)),
		forceStale: forceStale,
	}
	if opts.ifState != "" {
		if _, ok := model.StatePrecedence[model.CanonicalState(opts.ifState)]; !ok {
			return actionGuardOptions{}, &apiError{
				status:  http.StatusBadRequest,
				code:    model.ErrRefInvalid,
				message: "if_state is invalid",
			}
		}
	}
	if strings.TrimSpace(ifUpdatedWithin) == "" {
		return opts, nil
	}
	d, err := time.ParseDuration(strings.TrimSpace(ifUpdatedWithin))
	if err != nil || d <= 0 {
		return actionGuardOptions{}, &apiError{
			status:  http.StatusBadRequest,
			code:    model.ErrRefInvalid,
			message: "if_updated_within must be a positive duration",
		}
	}
	opts.ifUpdatedWithin = &d
	return opts, nil
}

func (s *Server) prepareActionSnapshot(ctx context.Context, targetID, paneID string, opts actionGuardOptions) (*model.ActionSnapshot, *apiError) {
	st, err := s.store.GetState(ctx, targetID, paneID)
	if err != nil {
		if errors.Is(err, db.ErrNotFound) {
			if opts.forceStale {
				return nil, nil
			}
			if opts.ifRuntime != "" || opts.ifState != "" || opts.ifUpdatedWithin != nil {
				return nil, &apiError{
					status:  http.StatusConflict,
					code:    model.ErrRuntimeStale,
					message: "runtime/state unavailable",
				}
			}
			return nil, nil
		}
		return nil, &apiError{
			status:  http.StatusInternalServerError,
			code:    model.ErrPreconditionFailed,
			message: "failed to resolve state",
		}
	}
	now := time.Now().UTC()
	if !opts.forceStale {
		if opts.ifRuntime != "" && st.RuntimeID != opts.ifRuntime {
			return nil, &apiError{
				status:  http.StatusConflict,
				code:    model.ErrRuntimeStale,
				message: "runtime guard mismatch",
			}
		}
		if opts.ifState != "" && string(st.State) != opts.ifState {
			return nil, &apiError{
				status:  http.StatusConflict,
				code:    model.ErrPreconditionFailed,
				message: "state guard mismatch",
			}
		}
		if opts.ifUpdatedWithin != nil && now.Sub(st.UpdatedAt) > *opts.ifUpdatedWithin {
			return nil, &apiError{
				status:  http.StatusConflict,
				code:    model.ErrPreconditionFailed,
				message: "state freshness guard mismatch",
			}
		}
	}
	if strings.TrimSpace(st.RuntimeID) == "" {
		if opts.forceStale {
			return nil, nil
		}
		return nil, &apiError{
			status:  http.StatusConflict,
			code:    model.ErrRuntimeStale,
			message: "runtime unavailable",
		}
	}
	snapshot := &model.ActionSnapshot{
		TargetID:     targetID,
		PaneID:       paneID,
		RuntimeID:    st.RuntimeID,
		StateVersion: st.StateVersion,
		ObservedAt:   now,
		ExpiresAt:    now.Add(s.snapshotTTL),
		Nonce:        uuid.NewString(),
	}
	if opts.forceStale {
		return snapshot, nil
	}
	if !snapshot.ExpiresAt.After(now) {
		return nil, &apiError{
			status:  http.StatusConflict,
			code:    model.ErrSnapshotExpired,
			message: "action snapshot expired",
		}
	}
	current, currentErr := s.store.GetState(ctx, targetID, paneID)
	if currentErr != nil {
		if errors.Is(currentErr, db.ErrNotFound) {
			return nil, &apiError{
				status:  http.StatusConflict,
				code:    model.ErrRuntimeStale,
				message: "runtime became unavailable",
			}
		}
		return nil, &apiError{
			status:  http.StatusInternalServerError,
			code:    model.ErrPreconditionFailed,
			message: "failed to revalidate state",
		}
	}
	if current.RuntimeID != snapshot.RuntimeID || current.StateVersion != snapshot.StateVersion {
		return nil, &apiError{
			status:  http.StatusConflict,
			code:    model.ErrRuntimeStale,
			message: "runtime/state changed before action execution",
		}
	}
	return snapshot, nil
}

func (s *Server) persistActionSnapshot(ctx context.Context, action model.Action, snapshot *model.ActionSnapshot) error {
	if snapshot == nil {
		return nil
	}
	if action.ActionID == "" {
		return nil
	}
	if _, err := s.store.GetActionSnapshotByActionID(ctx, action.ActionID); err == nil {
		return nil
	} else if err != nil && !errors.Is(err, db.ErrNotFound) {
		return err
	}
	snapshotID := uuid.NewString()
	insert := model.ActionSnapshot{
		SnapshotID:   snapshotID,
		ActionID:     action.ActionID,
		TargetID:     snapshot.TargetID,
		PaneID:       snapshot.PaneID,
		RuntimeID:    snapshot.RuntimeID,
		StateVersion: snapshot.StateVersion,
		ObservedAt:   snapshot.ObservedAt,
		ExpiresAt:    snapshot.ExpiresAt,
		Nonce:        snapshot.Nonce,
	}
	if err := s.store.InsertActionSnapshot(ctx, insert); err != nil {
		if errors.Is(err, db.ErrDuplicate) {
			return nil
		}
		return err
	}
	return nil
}

func (s *Server) resolveTargetAndPane(ctx context.Context, targetName, paneID string) (model.Target, error) {
	tg, err := s.store.GetTargetByName(ctx, targetName)
	if err != nil {
		return model.Target{}, err
	}
	panes, err := s.store.ListPanes(ctx)
	if err != nil {
		return model.Target{}, err
	}
	for _, pane := range panes {
		if pane.TargetID == tg.TargetID && pane.PaneID == paneID {
			return tg, nil
		}
	}
	return model.Target{}, db.ErrNotFound
}

func (s *Server) writeActionResolveError(w http.ResponseWriter, err error) {
	if errors.Is(err, db.ErrNotFound) {
		s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "target or pane not found")
		return
	}
	s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to resolve target/pane")
}

func (s *Server) lookupIdempotentAction(ctx context.Context, actionType model.ActionType, requestRef, targetID, paneID, metadataJSON string) (model.Action, bool, bool, error) {
	existing, err := s.store.GetActionByTypeRequestRef(ctx, actionType, requestRef)
	if err == nil {
		if existing.TargetID != targetID || existing.PaneID != paneID || !metadataEquals(existing.MetadataJSON, metadataJSON) {
			return model.Action{}, false, true, nil
		}
		return existing, true, false, nil
	}
	if errors.Is(err, db.ErrNotFound) {
		return model.Action{}, false, false, nil
	}
	return model.Action{}, false, false, err
}

func (s *Server) insertActionWithReplay(ctx context.Context, actionType model.ActionType, requestRef, targetID, paneID string, runtimeID *string, metadataJSON, resultCode string, errorCode *string) (model.Action, bool, error) {
	now := time.Now().UTC()
	completedAt := now
	meta := metadataJSON
	action := model.Action{
		ActionID:     uuid.NewString(),
		ActionType:   actionType,
		RequestRef:   requestRef,
		TargetID:     targetID,
		PaneID:       paneID,
		RuntimeID:    runtimeID,
		RequestedAt:  now,
		CompletedAt:  &completedAt,
		ResultCode:   resultCode,
		ErrorCode:    errorCode,
		MetadataJSON: &meta,
	}
	if err := s.store.InsertAction(ctx, action); err != nil {
		if errors.Is(err, db.ErrDuplicate) {
			existing, getErr := s.store.GetActionByTypeRequestRef(ctx, actionType, requestRef)
			if getErr != nil {
				return model.Action{}, false, getErr
			}
			if existing.TargetID != targetID || existing.PaneID != paneID || !metadataEquals(existing.MetadataJSON, metadataJSON) {
				return model.Action{}, false, db.ErrDuplicate
			}
			return existing, false, nil
		}
		return model.Action{}, false, err
	}
	return action, true, nil
}

func (s *Server) lockActionKey(actionType model.ActionType, requestRef string) func() {
	key := string(actionType) + "|" + requestRef

	s.actionMu.Lock()
	entry, ok := s.actionLocks[key]
	if !ok {
		entry = &actionLockEntry{}
		s.actionLocks[key] = entry
	}
	entry.refs++
	s.actionMu.Unlock()

	entry.mu.Lock()
	return func() {
		entry.mu.Unlock()
		s.actionMu.Lock()
		entry.refs--
		if entry.refs == 0 {
			delete(s.actionLocks, key)
		}
		s.actionMu.Unlock()
	}
}

func findPaneState(states []model.StateRow, targetID, paneID string) (model.StateRow, bool) {
	for _, st := range states {
		if st.TargetID == targetID && st.PaneID == paneID {
			return st, true
		}
	}
	return model.StateRow{}, false
}

func (s *Server) findRuntimeIDForPane(ctx context.Context, targetID, paneID string) (*string, error) {
	states, err := s.store.ListStates(ctx)
	if err != nil {
		return nil, err
	}
	st, ok := findPaneState(states, targetID, paneID)
	if !ok || strings.TrimSpace(st.RuntimeID) == "" {
		return nil, db.ErrNotFound
	}
	runtimeID := strings.TrimSpace(st.RuntimeID)
	return &runtimeID, nil
}

func (s *Server) findRuntimePIDForPane(ctx context.Context, targetID, paneID string) (int64, error) {
	runtimeID, err := s.findRuntimeIDForPane(ctx, targetID, paneID)
	if err != nil {
		return 0, err
	}
	return s.findRuntimePID(ctx, strings.TrimSpace(*runtimeID))
}

func (s *Server) findRuntimePID(ctx context.Context, runtimeID string) (int64, error) {
	rt, err := s.store.GetRuntime(ctx, runtimeID)
	if err != nil {
		return 0, err
	}
	if rt.PID == nil {
		return 0, db.ErrNotFound
	}
	return *rt.PID, nil
}

func (s *Server) emitActionAuditEvent(ctx context.Context, action model.Action, eventType string) error {
	if action.RuntimeID == nil || strings.TrimSpace(*action.RuntimeID) == "" {
		return nil
	}
	if s.auditEventHook != nil {
		if err := s.auditEventHook(action, eventType); err != nil {
			return err
		}
	}
	now := time.Now().UTC()
	actionID := action.ActionID
	ev := model.EventEnvelope{
		EventID:    uuid.NewString(),
		RuntimeID:  strings.TrimSpace(*action.RuntimeID),
		EventType:  eventType,
		Source:     model.SourceWrapper,
		DedupeKey:  "action:" + action.ActionID,
		EventTime:  now,
		IngestedAt: now,
		ActionID:   &actionID,
	}
	if err := s.store.InsertEvent(ctx, ev, ""); err != nil && !errors.Is(err, db.ErrDuplicate) {
		return err
	}
	return nil
}

func marshalActionMetadata(v any) (string, error) {
	b, err := json.Marshal(v)
	if err != nil {
		return "", err
	}
	return string(b), nil
}

func metadataEquals(existing *string, expected string) bool {
	if existing == nil {
		return expected == ""
	}
	return *existing == expected
}

func (s *Server) resolveTargetFilter(ctx context.Context, targetName string) ([]model.Target, map[string]any, error) {
	filters := map[string]any{}
	name := strings.TrimSpace(targetName)
	if name != "" {
		filters["target"] = name
		tg, err := s.store.GetTargetByName(ctx, name)
		if err != nil {
			return nil, filters, err
		}
		return []model.Target{tg}, filters, nil
	}
	targets, err := s.store.ListTargets(ctx)
	if err != nil {
		return nil, filters, err
	}
	return targets, filters, nil
}

func (s *Server) buildPaneItems(ctx context.Context, targets []model.Target) ([]api.PaneItem, api.ListSummary, error) {
	targetNameByID := make(map[string]string, len(targets))
	requestedIDs := make(map[string]struct{}, len(targets))
	for _, t := range targets {
		targetNameByID[t.TargetID] = t.TargetName
		requestedIDs[t.TargetID] = struct{}{}
	}
	panes, err := s.store.ListPanes(ctx)
	if err != nil {
		return nil, api.ListSummary{}, err
	}
	states, err := s.store.ListStates(ctx)
	if err != nil {
		return nil, api.ListSummary{}, err
	}
	runtimes, err := s.store.ListActiveRuntimes(ctx)
	if err != nil {
		return nil, api.ListSummary{}, err
	}

	stateByPane := make(map[string]model.StateRow, len(states))
	for _, st := range states {
		key := paneKey(st.TargetID, st.PaneID)
		stateByPane[key] = st
	}
	runtimeByID := make(map[string]model.Runtime, len(runtimes))
	runtimeByPane := make(map[string]model.Runtime, len(runtimes))
	for _, rt := range runtimes {
		runtimeByID[rt.RuntimeID] = rt
		k := paneKey(rt.TargetID, rt.PaneID)
		current, ok := runtimeByPane[k]
		if !ok || rt.StartedAt.After(current.StartedAt) {
			runtimeByPane[k] = rt
		}
	}

	targetIDs := make([]string, 0, len(requestedIDs))
	for targetID := range requestedIDs {
		targetIDs = append(targetIDs, targetID)
	}
	paneIDSet := map[string]struct{}{}
	for _, p := range panes {
		if _, ok := requestedIDs[p.TargetID]; !ok {
			continue
		}
		paneIDSet[p.PaneID] = struct{}{}
	}
	paneIDs := make([]string, 0, len(paneIDSet))
	for paneID := range paneIDSet {
		paneIDs = append(paneIDs, paneID)
	}

	sendActions, err := s.store.ListSendActionsForPanes(ctx, targetIDs, paneIDs)
	if err != nil {
		return nil, api.ListSummary{}, err
	}
	runtimeFirstInput := map[string]actionInputHint{}
	runtimeLastInput := map[string]actionInputHint{}
	paneLastInput := map[string]actionInputHint{}
	runtimeIDSet := map[string]struct{}{}
	for _, action := range sendActions {
		preview := extractActionInputPreview(action.MetadataJSON)
		if preview == "" {
			continue
		}
		hint := actionInputHint{
			preview: preview,
			at:      action.RequestedAt.UTC(),
		}
		pk := paneKey(action.TargetID, action.PaneID)
		paneLastInput[pk] = hint
		if action.RuntimeID != nil {
			rid := strings.TrimSpace(*action.RuntimeID)
			if rid != "" {
				runtimeIDSet[rid] = struct{}{}
				if _, ok := runtimeFirstInput[rid]; !ok {
					runtimeFirstInput[rid] = hint
				}
				runtimeLastInput[rid] = hint
			}
		}
	}
	for _, rt := range runtimeByPane {
		if rid := strings.TrimSpace(rt.RuntimeID); rid != "" {
			runtimeIDSet[rid] = struct{}{}
		}
	}
	runtimeIDs := make([]string, 0, len(runtimeIDSet))
	for rid := range runtimeIDSet {
		runtimeIDs = append(runtimeIDs, rid)
	}
	runtimeLatestEvent := map[string]runtimeEventHint{}
	latestEvents, err := s.store.ListLatestRuntimeEvents(ctx, runtimeIDs)
	if err != nil {
		return nil, api.ListSummary{}, err
	}
	for _, event := range latestEvents {
		preview := extractEventPreview(event.EventType, event.RawPayload)
		at := event.IngestedAt
		if !event.EventTime.IsZero() {
			at = event.EventTime
		}
		runtimeLatestEvent[event.RuntimeID] = runtimeEventHint{
			preview: preview,
			at:      at.UTC(),
			event:   strings.TrimSpace(event.EventType),
		}
	}
	codexHintsByPath := map[string]codexThreadHint{}
	if s.codexEnricher != nil {
		workspacePaths := make([]string, 0, len(panes))
		workspaceSeen := map[string]struct{}{}
		for _, pane := range panes {
			if _, ok := requestedIDs[pane.TargetID]; !ok {
				continue
			}
			pathKey := normalizeCodexWorkspacePath(pane.CurrentPath)
			if pathKey == "" {
				continue
			}
			key := paneKey(pane.TargetID, pane.PaneID)
			agent := ""
			if st, ok := stateByPane[key]; ok && st.RuntimeID != "" {
				if rt, ok := runtimeByID[st.RuntimeID]; ok {
					agent = rt.AgentType
				}
			}
			if agent == "" {
				if rt, ok := runtimeByPane[key]; ok {
					agent = rt.AgentType
				}
			}
			cmd := strings.ToLower(strings.TrimSpace(pane.CurrentCmd))
			if strings.ToLower(strings.TrimSpace(agent)) != "codex" && cmd != "codex" {
				continue
			}
			if _, ok := workspaceSeen[pathKey]; ok {
				continue
			}
			workspaceSeen[pathKey] = struct{}{}
			workspacePaths = append(workspacePaths, pathKey)
		}
		codexHintsByPath = s.codexEnricher.GetMany(ctx, workspacePaths)
	}

	summary := api.ListSummary{
		ByState:    map[string]int{},
		ByAgent:    map[string]int{},
		ByTarget:   map[string]int{},
		ByCategory: map[string]int{},
	}
	items := make([]api.PaneItem, 0, len(panes))
	for _, p := range panes {
		if _, ok := requestedIDs[p.TargetID]; !ok {
			continue
		}
		key := paneKey(p.TargetID, p.PaneID)
		st, hasState := stateByPane[key]

		state := string(model.StateUnknown)
		reason := "unsupported_signal"
		confidence := "low"
		runtimeID := ""
		stateSource := ""
		lastEventType := ""
		var lastEventAt *string
		updatedAt := p.UpdatedAt
		if hasState {
			state = string(st.State)
			reason = st.ReasonCode
			confidence = st.Confidence
			runtimeID = st.RuntimeID
			stateSource = string(st.StateSource)
			lastEventType = st.LastEventType
			if st.LastEventAt != nil {
				v := st.LastEventAt.Format(time.RFC3339Nano)
				lastEventAt = &v
			}
			updatedAt = st.UpdatedAt
		}
		agentType := defaultAgentType
		if runtimeID != "" {
			if rt, ok := runtimeByID[runtimeID]; ok {
				agentType = rt.AgentType
			} else if rt, getErr := s.store.GetRuntime(ctx, runtimeID); getErr == nil {
				agentType = rt.AgentType
				runtimeByID[runtimeID] = rt
			} else if getErr != nil && !errors.Is(getErr, db.ErrNotFound) {
				return nil, api.ListSummary{}, getErr
			}
		} else if rt, ok := runtimeByPane[key]; ok {
			runtimeID = rt.RuntimeID
			agentType = rt.AgentType
		}
		targetName := targetNameByID[p.TargetID]
		agentPresence, activityState, displayCategory, needsUserAction := derivePanePresentation(agentType, state)
		awaitingKind := deriveAwaitingResponseKind(state, reason, lastEventType)
		pathKey := normalizeCodexWorkspacePath(p.CurrentPath)
		codexHint, hasCodexHint := codexHintsByPath[pathKey]
		sessionLabel, sessionLabelSource := derivePaneSessionLabel(
			agentPresence,
			p,
			runtimeID,
			key,
			runtimeFirstInput,
			paneLastInput,
			runtimeLatestEvent,
			agentType,
			codexHint,
			hasCodexHint,
		)
		lastInteractionAt := derivePaneLastInteractionAt(
			agentPresence,
			runtimeID,
			key,
			paneLastInput,
			runtimeLastInput,
			runtimeLatestEvent,
			agentType,
			codexHint,
			hasCodexHint,
			stateSource,
			lastEventType,
			st.LastEventAt,
			p.LastActivityAt,
			updatedAt,
		)
		var lastInteractionAtStr *string
		if lastInteractionAt != nil {
			v := lastInteractionAt.Format(time.RFC3339Nano)
			lastInteractionAtStr = &v
		}
		item := api.PaneItem{
			Identity: api.PaneIdentity{
				Target:      targetName,
				SessionName: p.SessionName,
				WindowID:    p.WindowID,
				PaneID:      p.PaneID,
			},
			WindowName:      p.WindowName,
			CurrentCmd:      strings.TrimSpace(p.CurrentCmd),
			PaneTitle:       strings.TrimSpace(p.PaneTitle),
			State:           state,
			ReasonCode:      reason,
			Confidence:      confidence,
			RuntimeID:       runtimeID,
			AgentType:       agentType,
			AgentPresence:   agentPresence,
			ActivityState:   activityState,
			DisplayCategory: displayCategory,
			NeedsUserAction: needsUserAction,
			StateSource:     stateSource,
			LastEventType:   lastEventType,
			LastEventAt:     lastEventAt,
			AwaitingKind:    awaitingKind,
			SessionLabel:    sessionLabel,
			SessionLabelSrc: sessionLabelSource,
			LastInputAt:     lastInteractionAtStr,
			UpdatedAt:       updatedAt.Format(time.RFC3339Nano),
		}
		items = append(items, item)
		summary.ByState[state]++
		summary.ByAgent[agentType]++
		summary.ByTarget[targetName]++
		summary.ByCategory[displayCategory]++
	}
	return items, summary, nil
}

func buildWindowItems(panes []api.PaneItem) []api.WindowItem {
	type windowKey struct {
		target      string
		sessionName string
		windowID    string
	}
	type agg struct {
		item            api.WindowItem
		statePrecedence int
		catPrecedence   int
	}
	windowMap := map[windowKey]agg{}
	for _, p := range panes {
		k := windowKey{
			target:      p.Identity.Target,
			sessionName: p.Identity.SessionName,
			windowID:    p.Identity.WindowID,
		}
		entry, ok := windowMap[k]
		if !ok {
			entry = agg{
				item: api.WindowItem{
					Identity: api.WindowIdentity{
						Target:      p.Identity.Target,
						SessionName: p.Identity.SessionName,
						WindowID:    p.Identity.WindowID,
					},
					TopState:    p.State,
					TopCategory: p.DisplayCategory,
					ByCategory:  map[string]int{},
				},
				statePrecedence: statePrecedence(p.State),
				catPrecedence:   categoryPrecedence(p.DisplayCategory),
			}
		}
		entry.item.TotalPanes++
		if p.State == string(model.StateRunning) {
			entry.item.RunningCount++
		}
		if p.State == string(model.StateWaitingInput) || p.State == string(model.StateWaitingApproval) {
			entry.item.WaitingCount++
		}
		entry.item.ByCategory[p.DisplayCategory]++
		currentPrec := statePrecedence(p.State)
		if currentPrec < entry.statePrecedence {
			entry.statePrecedence = currentPrec
			entry.item.TopState = p.State
		}
		currentCatPrec := categoryPrecedence(p.DisplayCategory)
		if currentCatPrec < entry.catPrecedence {
			entry.catPrecedence = currentCatPrec
			entry.item.TopCategory = p.DisplayCategory
		}
		windowMap[k] = entry
	}
	out := make([]api.WindowItem, 0, len(windowMap))
	for _, v := range windowMap {
		out = append(out, v.item)
	}
	sort.Slice(out, func(i, j int) bool {
		li := out[i].Identity
		lj := out[j].Identity
		if li.Target != lj.Target {
			return li.Target < lj.Target
		}
		if li.SessionName != lj.SessionName {
			return li.SessionName < lj.SessionName
		}
		return li.WindowID < lj.WindowID
	})
	return out
}

func buildSessionItems(panes []api.PaneItem) []api.SessionItem {
	type sessionKey struct {
		target      string
		sessionName string
	}
	sessionMap := map[sessionKey]api.SessionItem{}
	for _, p := range panes {
		k := sessionKey{
			target:      p.Identity.Target,
			sessionName: p.Identity.SessionName,
		}
		entry, ok := sessionMap[k]
		if !ok {
			entry = api.SessionItem{
				Identity: api.SessionIdentity{
					Target:      p.Identity.Target,
					SessionName: p.Identity.SessionName,
				},
				ByState:    map[string]int{},
				ByAgent:    map[string]int{},
				ByCategory: map[string]int{},
			}
		}
		entry.TotalPanes++
		entry.ByState[p.State]++
		entry.ByAgent[p.AgentType]++
		entry.ByCategory[p.DisplayCategory]++
		if categoryPrecedence(p.DisplayCategory) < categoryPrecedence(entry.TopCategory) {
			entry.TopCategory = p.DisplayCategory
		}
		sessionMap[k] = entry
	}
	out := make([]api.SessionItem, 0, len(sessionMap))
	for _, v := range sessionMap {
		out = append(out, v)
	}
	sort.Slice(out, func(i, j int) bool {
		li := out[i].Identity
		lj := out[j].Identity
		if li.Target != lj.Target {
			return li.Target < lj.Target
		}
		return li.SessionName < lj.SessionName
	})
	return out
}

func toTargetResponse(t model.Target) api.TargetResponse {
	var lastSeen *string
	if t.LastSeenAt != nil {
		v := t.LastSeenAt.Format(time.RFC3339Nano)
		lastSeen = &v
	}
	return api.TargetResponse{
		TargetID:      t.TargetID,
		TargetName:    t.TargetName,
		Kind:          string(t.Kind),
		ConnectionRef: t.ConnectionRef,
		IsDefault:     t.IsDefault,
		LastSeenAt:    lastSeen,
		Health:        string(t.Health),
		UpdatedAt:     t.UpdatedAt.Format(time.RFC3339Nano),
	}
}

func toAdapterResponse(a model.AdapterRecord) api.AdapterResponse {
	return api.AdapterResponse{
		AdapterName:  a.AdapterName,
		AgentType:    a.AgentType,
		Version:      a.Version,
		Compatible:   adapterpkg.IsVersionCompatible(a.Version),
		Capabilities: append([]string(nil), a.Capabilities...),
		Enabled:      a.Enabled,
		UpdatedAt:    a.UpdatedAt.Format(time.RFC3339Nano),
	}
}

func toActionResponse(a model.Action) api.ActionResponse {
	var completedAt *string
	if a.CompletedAt != nil {
		v := a.CompletedAt.Format(time.RFC3339Nano)
		completedAt = &v
	}
	var output *string
	if a.ActionType == model.ActionTypeViewOutput && a.MetadataJSON != nil {
		var m map[string]any
		if err := json.Unmarshal([]byte(*a.MetadataJSON), &m); err == nil {
			if v, ok := m["output"].(string); ok {
				output = &v
			}
		}
	}
	return api.ActionResponse{
		SchemaVersion: "v1",
		GeneratedAt:   time.Now().UTC(),
		ActionID:      a.ActionID,
		ResultCode:    a.ResultCode,
		CompletedAt:   completedAt,
		ErrorCode:     a.ErrorCode,
		Output:        output,
	}
}

func toActionEventItem(ev model.ActionEvent) api.ActionEventItem {
	return api.ActionEventItem{
		EventID:    ev.EventID,
		ActionID:   ev.ActionID,
		RuntimeID:  ev.RuntimeID,
		EventType:  ev.EventType,
		Source:     string(ev.Source),
		EventTime:  ev.EventTime.Format(time.RFC3339Nano),
		IngestedAt: ev.IngestedAt.Format(time.RFC3339Nano),
		DedupeKey:  ev.DedupeKey,
	}
}

func targetNames(targets []model.Target) []string {
	out := make([]string, 0, len(targets))
	for _, t := range targets {
		out = append(out, t.TargetName)
	}
	return out
}

func paneKey(targetID, paneID string) string {
	return targetID + "|" + paneID
}

type actionInputHint struct {
	preview string
	at      time.Time
}

type runtimeEventHint struct {
	preview string
	at      time.Time
	event   string
}

func derivePaneSessionLabel(
	agentPresence string,
	p model.Pane,
	runtimeID string,
	key string,
	runtimeFirstInput map[string]actionInputHint,
	paneLastInput map[string]actionInputHint,
	runtimeLatestEvent map[string]runtimeEventHint,
	agentType string,
	codexHint codexThreadHint,
	hasCodexHint bool,
) (string, string) {
	if strings.EqualFold(strings.TrimSpace(agentType), "codex") && hasCodexHint && codexHint.label != "" {
		return codexHint.label, "codex_thread_list"
	}
	if rid := strings.TrimSpace(runtimeID); rid != "" {
		if first, ok := runtimeFirstInput[rid]; ok && first.preview != "" {
			return first.preview, "runtime_first_input"
		}
		if event, ok := runtimeLatestEvent[rid]; ok && event.preview != "" {
			return event.preview, "runtime_last_event"
		}
	}
	if latest, ok := paneLastInput[key]; ok && latest.preview != "" {
		return latest.preview, "pane_last_input"
	}
	if title := normalizePaneTitle(p.PaneTitle, p.WindowName, p.SessionName); title != "" {
		return title, "pane_title"
	}
	if agentPresence == "none" {
		if cmd := strings.TrimSpace(p.CurrentCmd); cmd != "" {
			return cmd, "current_cmd"
		}
		if windowName := strings.TrimSpace(p.WindowName); windowName != "" {
			return windowName, "window_name"
		}
		return p.PaneID, "pane_id"
	}
	return p.SessionName, "session_name"
}

func derivePaneLastInteractionAt(
	agentPresence string,
	runtimeID string,
	key string,
	paneLastInput map[string]actionInputHint,
	runtimeLastInput map[string]actionInputHint,
	runtimeLatestEvent map[string]runtimeEventHint,
	agentType string,
	codexHint codexThreadHint,
	hasCodexHint bool,
	stateSource string,
	lastEventType string,
	lastEventAt *time.Time,
	lastActivityAt *time.Time,
	updatedAt time.Time,
) *time.Time {
	if rid := strings.TrimSpace(runtimeID); rid != "" {
		if last, ok := runtimeLastInput[rid]; ok {
			v := last.at
			return &v
		}
		if event, ok := runtimeLatestEvent[rid]; ok {
			if !isAdministrativeEventType(event.event) {
				v := event.at
				return &v
			}
		}
	}
	if strings.EqualFold(strings.TrimSpace(agentType), "codex") && hasCodexHint && !codexHint.at.IsZero() {
		v := codexHint.at.UTC()
		return &v
	}
	if last, ok := paneLastInput[key]; ok {
		v := last.at
		return &v
	}
	if strings.ToLower(strings.TrimSpace(stateSource)) != string(model.SourcePoller) &&
		lastEventAt != nil &&
		!isAdministrativeEventType(lastEventType) {
		v := lastEventAt.UTC()
		return &v
	}
	if agentPresence != "none" {
		return nil
	}
	if lastActivityAt != nil {
		v := lastActivityAt.UTC()
		return &v
	}
	if agentPresence == "none" {
		v := updatedAt.UTC()
		return &v
	}
	return nil
}

func isAdministrativeEventType(eventType string) bool {
	normalized := strings.ToLower(strings.TrimSpace(eventType))
	if normalized == "" {
		return false
	}
	if strings.Contains(normalized, "wrapper-start") || strings.Contains(normalized, "wrapper-exit") {
		return true
	}
	if strings.HasPrefix(normalized, "action.") || strings.HasPrefix(normalized, "action:") {
		if strings.Contains(normalized, "view-output") {
			return true
		}
		if strings.Contains(normalized, "kill") {
			return true
		}
		if strings.Contains(normalized, "attach") {
			return true
		}
	}
	return false
}

func normalizePaneTitle(raw, windowName, sessionName string) string {
	title := compactPreview(strings.TrimSpace(raw), 72)
	if title == "" {
		return ""
	}
	lower := strings.ToLower(title)
	windowLower := strings.ToLower(strings.TrimSpace(windowName))
	sessionLower := strings.ToLower(strings.TrimSpace(sessionName))
	if lower == windowLower || lower == sessionLower {
		return ""
	}
	if strings.Contains(lower, "apple-virtual-machine") {
		return ""
	}
	if strings.Contains(lower, ".local") && !strings.Contains(lower, " ") {
		return ""
	}
	return title
}

func extractEventPreview(eventType, rawPayload string) string {
	raw := strings.TrimSpace(rawPayload)
	if raw == "" {
		return ""
	}
	if strings.EqualFold(strings.TrimSpace(eventType), "wrapper-exit") ||
		strings.EqualFold(strings.TrimSpace(eventType), "wrapper-start") {
		return ""
	}
	if strings.Contains(raw, "exit_code=") {
		return ""
	}
	if strings.HasPrefix(raw, "{") || strings.HasPrefix(raw, "[") {
		var payload any
		if err := json.Unmarshal([]byte(raw), &payload); err == nil {
			if candidate := extractPreviewFromJSON(payload); candidate != "" {
				return compactPreview(candidate, 72)
			}
		}
	}
	return compactPreview(raw, 72)
}

func extractPreviewFromJSON(payload any) string {
	priorityKeys := []string{
		"session_title",
		"session_name",
		"title",
		"name",
		"summary",
		"message",
		"prompt",
		"input",
		"user_input",
		"text",
		"response",
		"output",
		"content",
	}
	switch v := payload.(type) {
	case map[string]any:
		for _, key := range priorityKeys {
			for rawKey, rawValue := range v {
				if strings.EqualFold(strings.TrimSpace(rawKey), key) {
					if candidate := extractPreviewFromJSON(rawValue); candidate != "" {
						return candidate
					}
				}
			}
		}
		for _, child := range v {
			if candidate := extractPreviewFromJSON(child); candidate != "" {
				return candidate
			}
		}
	case []any:
		for _, child := range v {
			if candidate := extractPreviewFromJSON(child); candidate != "" {
				return candidate
			}
		}
	case string:
		value := strings.TrimSpace(v)
		if value != "" {
			return value
		}
	}
	return ""
}

func extractActionInputPreview(metadataJSON *string) string {
	if metadataJSON == nil {
		return ""
	}
	raw := strings.TrimSpace(*metadataJSON)
	if raw == "" {
		return ""
	}
	var payload struct {
		Text string `json:"text"`
		Key  string `json:"key"`
	}
	if err := json.Unmarshal([]byte(raw), &payload); err != nil {
		return ""
	}
	candidate := strings.TrimSpace(payload.Text)
	if candidate == "" {
		candidate = strings.TrimSpace(payload.Key)
	}
	if candidate == "" {
		return ""
	}
	return compactPreview(candidate, 72)
}

func compactPreview(raw string, maxRunes int) string {
	normalized := strings.Join(strings.Fields(strings.ReplaceAll(raw, "\n", " ")), " ")
	if normalized == "" {
		return ""
	}
	runes := []rune(normalized)
	if maxRunes <= 0 || len(runes) <= maxRunes {
		return normalized
	}
	if maxRunes <= 3 {
		return string(runes[:maxRunes])
	}
	return string(runes[:maxRunes-3]) + "..."
}

func statePrecedence(state string) int {
	if v, ok := model.StatePrecedence[model.CanonicalState(state)]; ok {
		return v
	}
	return 999
}

func derivePanePresentation(agentType, state string) (agentPresence, activityState, displayCategory string, needsUserAction bool) {
	normalizedAgent := strings.ToLower(strings.TrimSpace(agentType))
	switch normalizedAgent {
	case "", defaultAgentType:
		agentPresence = "unknown"
	case unmanagedAgentType:
		agentPresence = "none"
	default:
		agentPresence = "managed"
	}

	switch model.CanonicalState(state) {
	case model.StateRunning:
		activityState = string(model.StateRunning)
	case model.StateWaitingInput:
		activityState = string(model.StateWaitingInput)
	case model.StateWaitingApproval:
		activityState = string(model.StateWaitingApproval)
	case model.StateError:
		activityState = string(model.StateError)
	case model.StateIdle, model.StateCompleted:
		activityState = string(model.StateIdle)
	default:
		activityState = string(model.StateUnknown)
	}

	switch {
	case agentPresence == "none":
		displayCategory = "unmanaged"
	case activityState == string(model.StateWaitingInput), activityState == string(model.StateWaitingApproval), activityState == string(model.StateError):
		displayCategory = "attention"
	case activityState == string(model.StateRunning):
		displayCategory = "running"
	case activityState == string(model.StateIdle):
		displayCategory = "idle"
	default:
		displayCategory = "unknown"
	}

	needsUserAction = activityState == string(model.StateWaitingInput) ||
		activityState == string(model.StateWaitingApproval) ||
		activityState == string(model.StateError)
	return agentPresence, activityState, displayCategory, needsUserAction
}

func deriveAwaitingResponseKind(state, reasonCode, lastEventType string) string {
	switch model.CanonicalState(state) {
	case model.StateWaitingInput:
		return "input"
	case model.StateWaitingApproval:
		return "approval"
	}
	normalizedReason := strings.ToLower(strings.TrimSpace(reasonCode))
	normalizedEventType := strings.ToLower(strings.TrimSpace(lastEventType))
	switch {
	case strings.Contains(normalizedEventType, "approval"), strings.Contains(normalizedReason, "approval"):
		return "approval"
	case strings.Contains(normalizedEventType, "input"),
		strings.Contains(normalizedEventType, "prompt"),
		strings.Contains(normalizedReason, "input"):
		return "input"
	default:
		return ""
	}
}

func categoryPrecedence(category string) int {
	switch strings.ToLower(strings.TrimSpace(category)) {
	case "attention":
		return 1
	case "running":
		return 2
	case "idle":
		return 3
	case "unmanaged":
		return 4
	case "unknown":
		return 5
	default:
		return 999
	}
}

func parseEventSource(raw string) (model.EventSource, bool) {
	switch strings.ToLower(strings.TrimSpace(raw)) {
	case string(model.SourceHook):
		return model.SourceHook, true
	case string(model.SourceNotify):
		return model.SourceNotify, true
	case string(model.SourceWrapper):
		return model.SourceWrapper, true
	case string(model.SourcePoller):
		return model.SourcePoller, true
	default:
		return "", false
	}
}

func (s *Server) resolveEventTarget(ctx context.Context, targetName, targetID string) (model.Target, error) {
	targetName = strings.TrimSpace(targetName)
	targetID = strings.TrimSpace(targetID)
	if targetName != "" {
		tg, err := s.store.GetTargetByName(ctx, targetName)
		if err != nil {
			return model.Target{}, err
		}
		if targetID != "" && targetID != tg.TargetID {
			return model.Target{}, db.ErrNotFound
		}
		return tg, nil
	}
	targets, err := s.store.ListTargets(ctx)
	if err != nil {
		return model.Target{}, err
	}
	if len(targets) == 0 {
		return model.Target{}, db.ErrNotFound
	}
	if targetID != "" {
		for _, tg := range targets {
			if tg.TargetID == targetID {
				return tg, nil
			}
		}
		return model.Target{}, db.ErrNotFound
	}
	for _, tg := range targets {
		if tg.IsDefault {
			return tg, nil
		}
	}
	for _, tg := range targets {
		if tg.TargetName == defaultLocalTargetName || tg.TargetID == defaultLocalTargetName {
			return tg, nil
		}
	}
	return targets[0], nil
}

func (s *Server) ensureEventPane(ctx context.Context, targetID, paneID string, updatedAt time.Time) error {
	panes, err := s.store.ListPanes(ctx)
	if err != nil {
		return err
	}
	for _, pane := range panes {
		if pane.TargetID == targetID && pane.PaneID == paneID {
			return nil
		}
	}
	return s.store.UpsertPane(ctx, model.Pane{
		TargetID:    targetID,
		PaneID:      paneID,
		SessionName: "unknown-session",
		WindowID:    "@0",
		WindowName:  "unknown-window",
		UpdatedAt:   updatedAt,
	})
}

func (s *Server) resolveRuntimeCandidateForEvent(ctx context.Context, targetID, paneID string, pid *int64, startHint *time.Time, agentType string) (string, bool, error) {
	runtimes, err := s.store.ListActiveRuntimesForPane(ctx, targetID, paneID)
	if err != nil {
		return "", false, err
	}
	candidates := make([]model.Runtime, 0, len(runtimes))
	for _, rt := range runtimes {
		if agentType != "" && strings.TrimSpace(strings.ToLower(rt.AgentType)) != agentType {
			continue
		}
		if pid != nil {
			if rt.PID == nil || *rt.PID != *pid {
				continue
			}
		}
		if startHint != nil {
			delta := rt.StartedAt.Sub(*startHint)
			if delta < 0 {
				delta = -delta
			}
			bindWindow := s.cfg.BindWindow
			if bindWindow <= 0 {
				bindWindow = 5 * time.Second
			}
			if delta > bindWindow {
				continue
			}
		}
		candidates = append(candidates, rt)
	}
	if len(candidates) != 1 {
		return "", false, nil
	}
	return candidates[0].RuntimeID, true, nil
}

func (s *Server) writeIngestError(w http.ResponseWriter, err error) {
	if errors.Is(err, db.ErrOutOfOrder) {
		s.writeError(w, http.StatusConflict, model.ErrPreconditionFailed, "event out of order")
		return
	}
	msg := err.Error()
	switch {
	case strings.Contains(msg, model.ErrIdempotencyConflict):
		s.writeError(w, http.StatusConflict, model.ErrIdempotencyConflict, "idempotency conflict")
	case strings.Contains(msg, model.ErrRuntimeStale):
		s.writeError(w, http.StatusConflict, model.ErrRuntimeStale, "runtime stale")
	default:
		s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to ingest event")
	}
}

func (s *Server) nextSequence() int64 {
	return s.sequence.Add(1)
}

func (s *Server) writeJSON(w http.ResponseWriter, status int, payload any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(payload)
}

func (s *Server) writeError(w http.ResponseWriter, status int, code, msg string) {
	resp := api.ErrorResponse{
		SchemaVersion: "v1",
		GeneratedAt:   time.Now().UTC(),
		Error: api.APIError{
			Code:    code,
			Message: msg,
		},
	}
	s.writeJSON(w, status, resp)
}

func (s *Server) methodNotAllowed(w http.ResponseWriter, allow ...string) {
	if len(allow) > 0 {
		w.Header().Set("Allow", strings.Join(allow, ", "))
	}
	s.writeError(w, http.StatusMethodNotAllowed, model.ErrRefInvalid, "method not allowed")
}

func (s *Server) writeResolveTargetError(w http.ResponseWriter, err error) {
	if errors.Is(err, db.ErrNotFound) {
		s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "target not found")
		return
	}
	s.writeError(w, http.StatusInternalServerError, model.ErrPreconditionFailed, "failed to resolve target")
}

func parseCursor(raw string) (string, int64, bool, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return "", 0, false, nil
	}
	parts := strings.SplitN(raw, ":", 2)
	if len(parts) != 2 || strings.TrimSpace(parts[0]) == "" {
		return "", 0, false, fmt.Errorf("invalid cursor format")
	}
	seq, err := strconv.ParseInt(parts[1], 10, 64)
	if err != nil || seq < 0 {
		return "", 0, false, fmt.Errorf("invalid cursor sequence")
	}
	return parts[0], seq, true, nil
}

func (s *Server) acquireLock() error {
	lockPath := s.cfg.SocketPath + ".lock"
	if err := os.MkdirAll(filepath.Dir(lockPath), 0o755); err != nil {
		return fmt.Errorf("create lock dir: %w", err)
	}
	f, err := os.OpenFile(lockPath, os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return fmt.Errorf("open lock file: %w", err)
	}
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		f.Close() //nolint:errcheck
		return fmt.Errorf("daemon already running")
	}
	s.mu.Lock()
	s.lockFile = f
	s.mu.Unlock()
	return nil
}

func (s *Server) releaseLock() error {
	s.mu.Lock()
	f := s.lockFile
	s.lockFile = nil
	s.mu.Unlock()
	if f == nil {
		return nil
	}
	if err := syscall.Flock(int(f.Fd()), syscall.LOCK_UN); err != nil {
		f.Close() //nolint:errcheck
		return err
	}
	return f.Close()
}
