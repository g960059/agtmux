package daemon

import (
	"bufio"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"
	"unicode"
	"unicode/utf8"

	"github.com/google/uuid"

	adapterpkg "github.com/g960059/agtmux/internal/adapter"
	"github.com/g960059/agtmux/internal/api"
	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/db"
	"github.com/g960059/agtmux/internal/ingest"
	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/target"
	"github.com/g960059/agtmux/internal/tmuxfmt"
)

const defaultAgentType = "unknown"
const unmanagedAgentType = "none"
const defaultLocalTargetName = "local"
const defaultViewOutputLines = 200
const defaultActionSnapshotTTL = 30 * time.Second
const defaultTerminalReadLines = 200
const defaultTerminalStreamLines = 200
const maxTerminalReadLines = 2000
const maxTerminalStateBytes = 256 * 1024
const defaultTerminalStateTTL = 5 * time.Minute
const defaultTerminalProxySessionTTL = 5 * time.Minute
const minTerminalStreamCaptureInterval = 80 * time.Millisecond
const defaultClaudeHistoryCacheTTL = 5 * time.Second
const defaultClaudePreviewCacheTTL = 20 * time.Second
const resizePolicySingleClientApply = "single_client_apply"
const terminalCursorMarkerPrefix = "__AGTMUX_CURSOR_POSITION__"

var codexSessionFileIDPattern = regexp.MustCompile(`([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.jsonl$`)
var claudeSessionIDPattern = regexp.MustCompile(`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`)

type Server struct {
	cfg              config.Config
	httpSrv          *http.Server
	listener         net.Listener
	lockFile         *os.File
	store            *db.Store
	executor         *target.Executor
	engine           *ingest.Engine
	codexEnricher    *codexSessionEnricher
	streamID         string
	sequence         atomic.Int64
	mu               sync.Mutex
	actionMu         sync.Mutex
	actionLocks      map[string]*actionLockEntry
	terminalMu       sync.Mutex
	terminalStates   map[string]terminalReadState
	terminalProxy    map[string]terminalProxySession
	terminalStateTTL time.Duration
	terminalProxyTTL time.Duration
	codexPaneMu      sync.Mutex
	codexPaneCache   map[string]codexPaneCacheEntry
	codexPaneTTL     time.Duration
	claudeHistoryMu  sync.Mutex
	claudeHistory    claudeHistoryCacheEntry
	claudeHistoryTTL time.Duration
	claudePreviewMu  sync.Mutex
	claudePreview    map[string]claudePreviewCacheEntry
	claudePreviewTTL time.Duration
	snapshotTTL      time.Duration
	auditEventHook   func(action model.Action, eventType string) error
	shutdown         sync.Once
	shutdownErr      error
}

type actionLockEntry struct {
	mu   sync.Mutex
	refs int
}

type terminalReadState struct {
	seq       int64
	content   string
	updatedAt time.Time
}

type terminalProxySession struct {
	SessionID    string
	TargetName   string
	TargetID     string
	PaneID       string
	RuntimeID    string
	StateVersion int64
	CreatedAt    time.Time
	UpdatedAt    time.Time
	AttachedSent bool
	LastContent  string
	LastSeq      int64
	LastCursorX  *int
	LastCursorY  *int
	LastPaneCols *int
	LastPaneRows *int
	LastCapture  time.Time
}

func NewServer(cfg config.Config) *Server {
	return NewServerWithDeps(cfg, nil, nil)
}

func NewServerWithDeps(cfg config.Config, store *db.Store, executor *target.Executor) *Server {
	mux := http.NewServeMux()
	s := &Server{
		cfg:              cfg,
		store:            store,
		executor:         executor,
		streamID:         uuid.NewString(),
		actionLocks:      map[string]*actionLockEntry{},
		terminalStates:   map[string]terminalReadState{},
		terminalProxy:    map[string]terminalProxySession{},
		terminalStateTTL: defaultTerminalStateTTL,
		terminalProxyTTL: defaultTerminalProxySessionTTL,
		codexPaneCache:   map[string]codexPaneCacheEntry{},
		codexPaneTTL:     20 * time.Second,
		claudeHistoryTTL: defaultClaudeHistoryCacheTTL,
		claudePreview:    map[string]claudePreviewCacheEntry{},
		claudePreviewTTL: defaultClaudePreviewCacheTTL,
		snapshotTTL:      defaultActionSnapshotTTL,
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
		mux.HandleFunc("/v1/capabilities", s.capabilitiesHandler)
		mux.HandleFunc("/v1/events", s.eventsHandler)
		mux.HandleFunc("/v1/targets", s.targetsHandler)
		mux.HandleFunc("/v1/adapters", s.adaptersHandler)
		mux.HandleFunc("/v1/adapters/", s.adapterByNameHandler)
		mux.HandleFunc("/v1/targets/", s.targetByNameHandler)
		mux.HandleFunc("/v1/panes", s.panesHandler)
		mux.HandleFunc("/v1/windows", s.windowsHandler)
		mux.HandleFunc("/v1/sessions", s.sessionsHandler)
		mux.HandleFunc("/v1/snapshot", s.snapshotHandler)
		mux.HandleFunc("/v1/watch", s.watchHandler)
		mux.HandleFunc("/v1/terminal/attach", s.terminalAttachHandler)
		mux.HandleFunc("/v1/terminal/detach", s.terminalDetachHandler)
		mux.HandleFunc("/v1/terminal/write", s.terminalWriteHandler)
		mux.HandleFunc("/v1/terminal/stream", s.terminalStreamHandler)
		mux.HandleFunc("/v1/terminal/read", s.terminalReadHandler)
		mux.HandleFunc("/v1/terminal/resize", s.terminalResizeHandler)
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

func (s *Server) capabilitiesHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		s.methodNotAllowed(w, http.MethodGet)
		return
	}
	resp := api.CapabilitiesEnvelope{
		SchemaVersion: "v1",
		GeneratedAt:   time.Now().UTC(),
		Capabilities: api.CapabilityFlags{
			EmbeddedTerminal:         true,
			TerminalRead:             true,
			TerminalResize:           true,
			TerminalWriteViaAction:   true,
			TerminalAttach:           true,
			TerminalWrite:            true,
			TerminalStream:           true,
			TerminalProxyMode:        "daemon-proxy-pty-poc",
			TerminalFrameProtocol:    "terminal-stream-v1",
			TerminalFrameProtocolVer: "1",
		},
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

func (s *Server) snapshotHandler(w http.ResponseWriter, r *http.Request) {
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
	targetItems := make([]api.TargetResponse, 0, len(targets))
	for _, t := range targets {
		targetItems = append(targetItems, toTargetResponse(t))
	}
	names := targetNames(targets)
	resp := api.DashboardSnapshotEnvelope{
		SchemaVersion:    "v1",
		GeneratedAt:      time.Now().UTC(),
		Filters:          filters,
		Summary:          summary,
		Partial:          false,
		RequestedTargets: names,
		RespondedTargets: names,
		Targets:          targetItems,
		Sessions:         buildSessionItems(panes),
		Windows:          buildWindowItems(panes),
		Panes:            panes,
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

type terminalReadRequest struct {
	Target string `json:"target"`
	PaneID string `json:"pane_id"`
	Cursor string `json:"cursor"`
	Lines  int    `json:"lines"`
}

type terminalResizeRequest struct {
	Target string `json:"target"`
	PaneID string `json:"pane_id"`
	Cols   int    `json:"cols"`
	Rows   int    `json:"rows"`
}

type terminalAttachRequest struct {
	Target          string `json:"target"`
	PaneID          string `json:"pane_id"`
	IfRuntime       string `json:"if_runtime"`
	IfState         string `json:"if_state"`
	IfUpdatedWithin string `json:"if_updated_within"`
	ForceStale      bool   `json:"force_stale"`
}

type terminalDetachRequest struct {
	SessionID string `json:"session_id"`
}

type terminalWriteRequest struct {
	SessionID string `json:"session_id"`
	Text      string `json:"text"`
	Key       string `json:"key"`
	BytesB64  string `json:"bytes_b64"`
	Enter     bool   `json:"enter"`
	Paste     bool   `json:"paste"`
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
	runResult, runErr := s.executor.Run(r.Context(), tg, target.BuildTmuxCommand("capture-pane", "-t", req.PaneID, "-p", "-e", "-S", fmt.Sprintf("-%d", req.Lines)))
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

func (s *Server) terminalAttachHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		s.methodNotAllowed(w, http.MethodPost)
		return
	}
	var req terminalAttachRequest
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	if err := dec.Decode(&req); err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "invalid request body")
		return
	}
	req.Target = strings.TrimSpace(req.Target)
	req.PaneID = strings.TrimSpace(req.PaneID)
	req.IfRuntime = strings.TrimSpace(req.IfRuntime)
	req.IfState = strings.TrimSpace(strings.ToLower(req.IfState))
	req.IfUpdatedWithin = strings.TrimSpace(req.IfUpdatedWithin)
	if req.Target == "" || req.PaneID == "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "target and pane_id are required")
		return
	}
	s.pruneExpiredTerminalCaches(time.Now().UTC())

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

	sessionID := uuid.NewString()
	now := time.Now().UTC()
	session := terminalProxySession{
		SessionID:  sessionID,
		TargetName: req.Target,
		TargetID:   tg.TargetID,
		PaneID:     req.PaneID,
		CreatedAt:  now,
		UpdatedAt:  now,
	}
	if snapshot != nil {
		session.RuntimeID = strings.TrimSpace(snapshot.RuntimeID)
		session.StateVersion = snapshot.StateVersion
	}

	s.terminalMu.Lock()
	s.terminalProxy[sessionID] = session
	s.terminalMu.Unlock()

	resp := api.TerminalAttachResponse{
		SchemaVersion: "v1",
		GeneratedAt:   now,
		SessionID:     sessionID,
		Target:        req.Target,
		PaneID:        req.PaneID,
		RuntimeID:     session.RuntimeID,
		StateVersion:  session.StateVersion,
		ResultCode:    "completed",
	}
	s.writeJSON(w, http.StatusOK, resp)
}

func (s *Server) terminalDetachHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		s.methodNotAllowed(w, http.MethodPost)
		return
	}
	var req terminalDetachRequest
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	if err := dec.Decode(&req); err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "invalid request body")
		return
	}
	req.SessionID = strings.TrimSpace(req.SessionID)
	if req.SessionID == "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "session_id is required")
		return
	}
	s.pruneExpiredTerminalCaches(time.Now().UTC())

	s.terminalMu.Lock()
	_, ok := s.terminalProxy[req.SessionID]
	if ok {
		delete(s.terminalProxy, req.SessionID)
	}
	s.terminalMu.Unlock()
	if !ok {
		s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "terminal session not found")
		return
	}

	resp := api.TerminalDetachResponse{
		SchemaVersion: "v1",
		GeneratedAt:   time.Now().UTC(),
		SessionID:     req.SessionID,
		ResultCode:    "completed",
	}
	s.writeJSON(w, http.StatusOK, resp)
}

func (s *Server) terminalWriteHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		s.methodNotAllowed(w, http.MethodPost)
		return
	}
	var req terminalWriteRequest
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	if err := dec.Decode(&req); err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "invalid request body")
		return
	}
	req.SessionID = strings.TrimSpace(req.SessionID)
	req.Key = strings.TrimSpace(req.Key)
	req.BytesB64 = strings.TrimSpace(req.BytesB64)
	if req.SessionID == "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "session_id is required")
		return
	}
	hasText := req.Text != ""
	hasKey := req.Key != ""
	hasBytes := req.BytesB64 != ""
	inputModeCount := 0
	if hasText {
		inputModeCount++
	}
	if hasKey {
		inputModeCount++
	}
	if hasBytes {
		inputModeCount++
	}
	if inputModeCount == 0 {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "either text, key, or bytes_b64 is required")
		return
	}
	if inputModeCount > 1 {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "text, key, and bytes_b64 are mutually exclusive")
		return
	}
	s.pruneExpiredTerminalCaches(time.Now().UTC())

	s.terminalMu.Lock()
	session, ok := s.terminalProxy[req.SessionID]
	s.terminalMu.Unlock()
	if !ok {
		s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "terminal session not found")
		return
	}
	if guardErr := s.validateTerminalProxySession(r.Context(), session); guardErr != nil {
		s.dropTerminalProxySession(req.SessionID)
		guardErr.write(s, w)
		return
	}

	tg, err := s.resolveTargetAndPane(r.Context(), session.TargetName, session.PaneID)
	if err != nil {
		if errors.Is(err, db.ErrNotFound) {
			s.dropTerminalProxySession(req.SessionID)
		}
		s.writeActionResolveError(w, err)
		return
	}
	cmd := []string{"send-keys", "-t", session.PaneID}
	if hasText {
		if req.Paste {
			cmd = append(cmd, "-l")
		}
		cmd = append(cmd, req.Text)
	} else if hasKey {
		cmd = append(cmd, req.Key)
	} else {
		decoded, decodeErr := base64.StdEncoding.DecodeString(req.BytesB64)
		if decodeErr != nil {
			s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "bytes_b64 must be valid base64")
			return
		}
		if len(decoded) > 0 {
			if literalText, ok := decodeLiteralSendKeysText(decoded); ok {
				cmd = append(cmd, "-l", literalText)
			} else {
				cmd = append(cmd, "-H")
				for _, b := range decoded {
					cmd = append(cmd, fmt.Sprintf("%02x", b))
				}
			}
		}
	}
	if req.Enter {
		cmd = append(cmd, "Enter")
	}
	if len(cmd) <= 3 {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "terminal write payload is empty")
		return
	}
	resultCode := "completed"
	errorCode := ""
	if _, runErr := s.executor.Run(r.Context(), tg, target.BuildTmuxCommand(cmd...)); runErr != nil {
		resultCode = "failed"
		errorCode = model.ErrTargetUnreachable
	}

	if resultCode == "completed" {
		s.terminalMu.Lock()
		if current, exists := s.terminalProxy[req.SessionID]; exists {
			current.UpdatedAt = time.Now().UTC()
			// Force next stream read to capture a fresh pane snapshot after input.
			current.LastCapture = time.Time{}
			s.terminalProxy[req.SessionID] = current
		}
		s.terminalMu.Unlock()
	}

	resp := api.TerminalWriteResponse{
		SchemaVersion: "v1",
		GeneratedAt:   time.Now().UTC(),
		SessionID:     req.SessionID,
		ResultCode:    resultCode,
		ErrorCode:     errorCode,
	}
	s.writeJSON(w, http.StatusOK, resp)
}

func (s *Server) terminalStreamHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		s.methodNotAllowed(w, http.MethodGet)
		return
	}
	sessionID := strings.TrimSpace(r.URL.Query().Get("session_id"))
	if sessionID == "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "session_id is required")
		return
	}
	lines := defaultTerminalStreamLines
	if rawLines := strings.TrimSpace(r.URL.Query().Get("lines")); rawLines != "" {
		parsed, err := strconv.Atoi(rawLines)
		if err != nil {
			s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "lines must be numeric")
			return
		}
		lines = parsed
	}
	if lines <= 0 {
		lines = defaultTerminalStreamLines
	}
	if lines > maxTerminalReadLines {
		lines = maxTerminalReadLines
	}
	s.pruneExpiredTerminalCaches(time.Now().UTC())
	rawCursor := strings.TrimSpace(r.URL.Query().Get("cursor"))
	cursorStreamID, cursorSeq, _, err := parseCursor(rawCursor)
	if err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrCursorInvalid, "invalid cursor")
		return
	}

	s.terminalMu.Lock()
	session, ok := s.terminalProxy[sessionID]
	s.terminalMu.Unlock()
	if !ok {
		s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "terminal session not found")
		return
	}

	now := time.Now().UTC()
	if !session.AttachedSent {
		seq := s.sequence.Add(1)
		cursor := fmt.Sprintf("%s:%d", s.streamID, seq)
		session.AttachedSent = true
		session.LastSeq = seq
		session.UpdatedAt = now
		if !s.updateTerminalProxySession(sessionID, session) {
			s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "terminal session not found")
			return
		}
		resp := api.TerminalStreamEnvelope{
			SchemaVersion: "v1",
			GeneratedAt:   now,
			Frame: api.TerminalStreamFrame{
				FrameType: "attached",
				StreamID:  s.streamID,
				Cursor:    cursor,
				SessionID: sessionID,
				Target:    session.TargetName,
				PaneID:    session.PaneID,
			},
		}
		s.writeJSON(w, http.StatusOK, resp)
		return
	}
	if guardErr := s.validateTerminalProxySession(r.Context(), session); guardErr != nil {
		s.dropTerminalProxySession(sessionID)
		guardErr.write(s, w)
		return
	}

	tg, resolveErr := s.resolveTargetAndPane(r.Context(), session.TargetName, session.PaneID)
	if resolveErr != nil {
		if errors.Is(resolveErr, db.ErrNotFound) {
			s.dropTerminalProxySession(sessionID)
		}
		s.writeActionResolveError(w, resolveErr)
		return
	}
	frameType := "output"
	content := ""
	resetReason := ""
	if rawCursor != "" {
		if cursorStreamID != s.streamID {
			frameType = "reset"
			resetReason = "cursor_mismatch"
		} else if session.LastSeq != cursorSeq {
			frameType = "reset"
			resetReason = "cursor_discontinuity"
		}
	}

	shouldServeCached := rawCursor != "" && !session.LastCapture.IsZero() && now.Sub(session.LastCapture) < minTerminalStreamCaptureInterval
	if shouldServeCached {
		switch frameType {
		case "reset":
			content = session.LastContent
		default:
			frameType = "delta"
			content = ""
		}
		seq := s.sequence.Add(1)
		cursor := fmt.Sprintf("%s:%d", s.streamID, seq)
		session.LastSeq = seq
		session.UpdatedAt = now
		if !s.updateTerminalProxySession(sessionID, session) {
			s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "terminal session not found")
			return
		}

		resp := api.TerminalStreamEnvelope{
			SchemaVersion: "v1",
			GeneratedAt:   now,
			Frame: api.TerminalStreamFrame{
				FrameType:   frameType,
				StreamID:    s.streamID,
				Cursor:      cursor,
				CursorX:     session.LastCursorX,
				CursorY:     session.LastCursorY,
				PaneCols:    session.LastPaneCols,
				PaneRows:    session.LastPaneRows,
				SessionID:   sessionID,
				Target:      session.TargetName,
				PaneID:      session.PaneID,
				Content:     content,
				ResetReason: resetReason,
			},
		}
		s.writeJSON(w, http.StatusOK, resp)
		return
	}

	captureOutput, cursorX, cursorY, paneCols, paneRows, runErr := s.capturePaneSnapshotWithCursor(
		r.Context(),
		tg,
		session.PaneID,
		lines,
	)
	if runErr != nil {
		s.writeError(w, http.StatusBadGateway, model.ErrTargetUnreachable, "failed to read terminal stream output")
		return
	}
	content = captureOutput
	clippedCapture := clipTerminalStateContent(captureOutput)
	if frameType == "output" && session.LastContent == clippedCapture {
		// No visible diff: return lightweight delta frame to avoid resending full snapshots.
		frameType = "delta"
		content = ""
	}

	seq := s.sequence.Add(1)
	cursor := fmt.Sprintf("%s:%d", s.streamID, seq)
	session.LastSeq = seq
	session.LastContent = clippedCapture
	session.LastCursorX = cursorX
	session.LastCursorY = cursorY
	session.LastPaneCols = paneCols
	session.LastPaneRows = paneRows
	session.LastCapture = now
	session.UpdatedAt = now
	if !s.updateTerminalProxySession(sessionID, session) {
		s.writeError(w, http.StatusNotFound, model.ErrRefNotFound, "terminal session not found")
		return
	}

	resp := api.TerminalStreamEnvelope{
		SchemaVersion: "v1",
		GeneratedAt:   now,
		Frame: api.TerminalStreamFrame{
			FrameType:   frameType,
			StreamID:    s.streamID,
			Cursor:      cursor,
			CursorX:     cursorX,
			CursorY:     cursorY,
			PaneCols:    paneCols,
			PaneRows:    paneRows,
			SessionID:   sessionID,
			Target:      session.TargetName,
			PaneID:      session.PaneID,
			Content:     content,
			ResetReason: resetReason,
		},
	}
	s.writeJSON(w, http.StatusOK, resp)
}

func (s *Server) validateTerminalProxySession(ctx context.Context, session terminalProxySession) *apiError {
	runtimeID := strings.TrimSpace(session.RuntimeID)
	if runtimeID == "" {
		return nil
	}
	state, err := s.store.GetState(ctx, session.TargetID, session.PaneID)
	if err != nil {
		if errors.Is(err, db.ErrNotFound) {
			return &apiError{
				status:  http.StatusConflict,
				code:    model.ErrRuntimeStale,
				message: "runtime/state unavailable",
			}
		}
		return &apiError{
			status:  http.StatusInternalServerError,
			code:    model.ErrPreconditionFailed,
			message: "failed to resolve state",
		}
	}
	if strings.TrimSpace(state.RuntimeID) != runtimeID {
		return &apiError{
			status:  http.StatusConflict,
			code:    model.ErrRuntimeStale,
			message: "runtime guard mismatch",
		}
	}
	return nil
}

func (s *Server) updateTerminalProxySession(sessionID string, next terminalProxySession) bool {
	s.terminalMu.Lock()
	defer s.terminalMu.Unlock()
	if _, ok := s.terminalProxy[sessionID]; !ok {
		return false
	}
	s.terminalProxy[sessionID] = next
	return true
}

func (s *Server) dropTerminalProxySession(sessionID string) {
	s.terminalMu.Lock()
	delete(s.terminalProxy, sessionID)
	s.terminalMu.Unlock()
}

func (s *Server) pruneExpiredTerminalCaches(now time.Time) {
	s.terminalMu.Lock()
	defer s.terminalMu.Unlock()

	if s.terminalProxyTTL > 0 {
		for sessionID, session := range s.terminalProxy {
			anchor := session.UpdatedAt
			if anchor.IsZero() {
				anchor = session.CreatedAt
			}
			if anchor.IsZero() {
				continue
			}
			if now.Sub(anchor) > s.terminalProxyTTL {
				delete(s.terminalProxy, sessionID)
			}
		}
	}

	if s.terminalStateTTL > 0 {
		for key, state := range s.terminalStates {
			if state.updatedAt.IsZero() {
				continue
			}
			if now.Sub(state.updatedAt) > s.terminalStateTTL {
				delete(s.terminalStates, key)
			}
		}
	}
}

func (s *Server) terminalReadHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		s.methodNotAllowed(w, http.MethodPost)
		return
	}
	var req terminalReadRequest
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	if err := dec.Decode(&req); err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "invalid request body")
		return
	}
	req.Target = strings.TrimSpace(req.Target)
	req.PaneID = strings.TrimSpace(req.PaneID)
	req.Cursor = strings.TrimSpace(req.Cursor)
	if req.Target == "" || req.PaneID == "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "target and pane_id are required")
		return
	}
	if req.Lines <= 0 {
		req.Lines = defaultTerminalReadLines
	}
	if req.Lines > maxTerminalReadLines {
		req.Lines = maxTerminalReadLines
	}
	s.pruneExpiredTerminalCaches(time.Now().UTC())
	var (
		cursorStreamID string
		cursorSeq      int64
	)
	if req.Cursor != "" {
		var cursorErr error
		cursorStreamID, cursorSeq, _, cursorErr = parseCursor(req.Cursor)
		if cursorErr != nil {
			s.writeError(w, http.StatusBadRequest, model.ErrCursorInvalid, "invalid cursor")
			return
		}
	}
	tg, err := s.resolveTargetAndPane(r.Context(), req.Target, req.PaneID)
	if err != nil {
		s.writeActionResolveError(w, err)
		return
	}
	captureOutput, cursorX, cursorY, paneCols, paneRows, runErr := s.capturePaneSnapshotWithCursor(
		r.Context(),
		tg,
		req.PaneID,
		req.Lines,
	)
	if runErr != nil {
		s.writeError(w, http.StatusBadGateway, model.ErrTargetUnreachable, "failed to read pane output")
		return
	}
	// Keep read endpoint aligned with stream endpoint:
	// cursor coordinates are pane-local, so normalize snapshot to visible rows.
	captureOutput = trimSnapshotToVisibleRows(captureOutput, paneRows)
	frameType := "snapshot"
	resetReason := ""
	content := captureOutput
	pk := paneKey(tg.TargetID, req.PaneID)
	s.terminalMu.Lock()
	state, hasState := s.terminalStates[pk]
	if req.Cursor != "" {
		if cursorStreamID != s.streamID {
			frameType = "reset"
			resetReason = "cursor_mismatch"
		} else if !hasState || state.seq != cursorSeq {
			frameType = "reset"
			resetReason = "cursor_discontinuity"
		} else {
			if delta, ok := deriveTerminalDelta(state.content, captureOutput); ok {
				frameType = "delta"
				content = delta
			} else {
				frameType = "reset"
				resetReason = "content_discontinuity"
				content = captureOutput
			}
		}
	}
	seq := s.sequence.Add(1)
	s.terminalStates[pk] = terminalReadState{
		seq:       seq,
		content:   clipTerminalStateContent(captureOutput),
		updatedAt: time.Now().UTC(),
	}
	s.terminalMu.Unlock()
	cursor := fmt.Sprintf("%s:%d", s.streamID, seq)
	resp := api.TerminalReadEnvelope{
		SchemaVersion: "v1",
		GeneratedAt:   time.Now().UTC(),
		Frame: api.TerminalFrameItem{
			FrameType: frameType,
			StreamID:  s.streamID,
			Cursor:    cursor,
			CursorX:   cursorX,
			CursorY:   cursorY,
			PaneCols:  paneCols,
			PaneRows:  paneRows,
			PaneID:    req.PaneID,
			Target:    req.Target,
			Lines:     req.Lines,
			Content:   content,
		},
	}
	if resetReason != "" {
		resp.Frame.ResetReason = resetReason
	}
	s.writeJSON(w, http.StatusOK, resp)
}

func (s *Server) terminalResizeHandler(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		s.methodNotAllowed(w, http.MethodPost)
		return
	}
	var req terminalResizeRequest
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	if err := dec.Decode(&req); err != nil {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "invalid request body")
		return
	}
	req.Target = strings.TrimSpace(req.Target)
	req.PaneID = strings.TrimSpace(req.PaneID)
	if req.Target == "" || req.PaneID == "" {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "target and pane_id are required")
		return
	}
	if req.Cols <= 0 || req.Rows <= 0 {
		s.writeError(w, http.StatusBadRequest, model.ErrRefInvalid, "cols and rows must be positive")
		return
	}
	tg, err := s.resolveTargetAndPane(r.Context(), req.Target, req.PaneID)
	if err != nil {
		s.writeActionResolveError(w, err)
		return
	}
	resp := api.TerminalResizeResponse{
		SchemaVersion: "v1",
		GeneratedAt:   time.Now().UTC(),
		Target:        req.Target,
		PaneID:        req.PaneID,
		Cols:          req.Cols,
		Rows:          req.Rows,
		ResultCode:    "completed",
		Policy:        resizePolicySingleClientApply,
	}
	if _, runErr := s.executor.Run(
		r.Context(),
		tg,
		target.BuildTmuxCommand("resize-pane", "-t", req.PaneID, "-x", strconv.Itoa(req.Cols), "-y", strconv.Itoa(req.Rows)),
	); runErr != nil {
		s.writeError(w, http.StatusBadGateway, model.ErrTargetUnreachable, "failed to resize pane")
		return
	}
	s.writeJSON(w, http.StatusOK, resp)
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
	targetByID := make(map[string]model.Target, len(targets))
	requestedIDs := make(map[string]struct{}, len(targets))
	for _, t := range targets {
		targetNameByID[t.TargetID] = t.TargetName
		targetByID[t.TargetID] = t
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
	codexHintsByPath := map[string][]codexThreadHint{}
	codexPaneCountByPath := map[string]int{}
	codexCandidatesByPath := map[string][]codexPaneCandidate{}
	if s.codexEnricher != nil {
		workspacePaths := make([]string, 0, len(panes))
		workspaceSeen := map[string]struct{}{}
		candidateSeen := map[string]struct{}{}
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
			runtimeID := ""
			runtimeStartedAt := pane.UpdatedAt
			if st, ok := stateByPane[key]; ok && st.RuntimeID != "" {
				runtimeID = st.RuntimeID
				if rt, ok := runtimeByID[st.RuntimeID]; ok {
					agent = rt.AgentType
					runtimeStartedAt = rt.StartedAt
				}
			}
			if agent == "" || runtimeID == "" {
				if rt, ok := runtimeByPane[key]; ok {
					agent = rt.AgentType
					runtimeID = rt.RuntimeID
					runtimeStartedAt = rt.StartedAt
				}
			}
			cmd := strings.ToLower(strings.TrimSpace(pane.CurrentCmd))
			if strings.ToLower(strings.TrimSpace(agent)) != "codex" && cmd != "codex" {
				continue
			}
			codexPaneCountByPath[pathKey]++
			candidateKey := runtimeID
			if candidateKey == "" {
				candidateKey = key
			}
			dedupeKey := pathKey + "|" + candidateKey
			if _, ok := candidateSeen[dedupeKey]; !ok {
				candidateSeen[dedupeKey] = struct{}{}
				activityAt := pane.UpdatedAt
				if pane.LastActivityAt != nil && !pane.LastActivityAt.IsZero() {
					activityAt = pane.LastActivityAt.UTC()
				}
				if rid := strings.TrimSpace(runtimeID); rid != "" {
					if last, ok := runtimeLastInput[rid]; ok && !last.at.IsZero() {
						activityAt = last.at.UTC()
					}
				}
				if activityAt.IsZero() {
					activityAt = runtimeStartedAt
				}
				labelHint := normalizePaneTitle(pane.PaneTitle, pane.WindowName, pane.SessionName)
				if rid := strings.TrimSpace(runtimeID); rid != "" {
					if first, ok := runtimeFirstInput[rid]; ok && first.preview != "" {
						labelHint = first.preview
					}
				}
				if latest, ok := paneLastInput[key]; ok && latest.preview != "" {
					labelHint = latest.preview
				}
				codexCandidatesByPath[pathKey] = append(codexCandidatesByPath[pathKey], codexPaneCandidate{
					targetID:   pane.TargetID,
					paneID:     pane.PaneID,
					paneKey:    key,
					runtimeID:  runtimeID,
					labelHint:  labelHint,
					startedAt:  runtimeStartedAt,
					activityAt: activityAt,
				})
			}
			if _, ok := workspaceSeen[pathKey]; ok {
				continue
			}
			workspaceSeen[pathKey] = struct{}{}
			workspacePaths = append(workspacePaths, pathKey)
		}
		codexHintsByPath = s.codexEnricher.GetManyRanked(ctx, workspacePaths)
		codexCandidatesByPath = s.hydrateCodexCandidateThreadIDs(ctx, codexCandidatesByPath, codexHintsByPath, targetByID)
	}
	codexHintByRuntimeID := map[string]codexThreadHint{}
	codexHintByPaneKey := map[string]codexThreadHint{}
	for pathKey, candidates := range codexCandidatesByPath {
		hints := codexHintsByPath[pathKey]
		runtimeAssigned, paneAssigned := assignCodexHintsToCandidates(candidates, hints)
		for runtimeID, hint := range runtimeAssigned {
			codexHintByRuntimeID[runtimeID] = hint
		}
		for paneKey, hint := range paneAssigned {
			codexHintByPaneKey[paneKey] = hint
		}
	}
	claudeHintsByRuntimeID := s.collectClaudeSessionHints(ctx, panes, requestedIDs, stateByPane, runtimeByID, runtimeByPane, targetByID)

	summary := api.ListSummary{
		ByState:              map[string]int{},
		ByAgent:              map[string]int{},
		ByTarget:             map[string]int{},
		ByCategory:           map[string]int{},
		BySessionLabelSource: map[string]int{},
	}
	items := make([]api.PaneItem, 0, len(panes))
	presentationNow := time.Now().UTC()
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
		codexHints := codexHintsByPath[pathKey]
		codexHint := codexThreadHint{}
		hasCodexHint := false
		if runtimeHint, ok := codexHintByRuntimeID[strings.TrimSpace(runtimeID)]; ok {
			codexHint = runtimeHint
			hasCodexHint = true
		} else if paneHint, ok := codexHintByPaneKey[key]; ok {
			codexHint = paneHint
			hasCodexHint = true
		} else if shouldUseCodexWorkspaceHint(pathKey, codexPaneCountByPath) && len(codexHints) > 0 {
			codexHint = codexHints[0]
			hasCodexHint = true
		}
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
		claudeHint := claudeSessionHint{}
		hasClaudeHint := false
		if strings.EqualFold(strings.TrimSpace(agentType), "claude") {
			if hint, ok := claudeHintsByRuntimeID[strings.TrimSpace(runtimeID)]; ok {
				claudeHint = hint
				hasClaudeHint = true
				if hint.label != "" {
					sessionLabel = hint.label
					sessionLabelSource = hint.source
				}
			}
		}
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
		if hasClaudeHint && !claudeHint.at.IsZero() {
			if lastInteractionAt == nil || claudeHint.at.After(*lastInteractionAt) {
				v := claudeHint.at.UTC()
				lastInteractionAt = &v
			}
		}
		agentPresence, activityState, displayCategory, needsUserAction = refinePanePresentationWithSignals(
			agentPresence,
			activityState,
			reason,
			lastEventType,
			stateSource,
			lastInteractionAt,
			presentationNow,
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
		if sessionLabelSource != "" {
			summary.BySessionLabelSource[sessionLabelSource]++
		}
	}
	return items, summary, nil
}

func shouldUseCodexWorkspaceHint(pathKey string, codexPaneCountByPath map[string]int) bool {
	key := normalizeCodexWorkspacePath(pathKey)
	if key == "" {
		return false
	}
	count := codexPaneCountByPath[key]
	return count <= 1
}

func (s *Server) hydrateCodexCandidateThreadIDs(
	ctx context.Context,
	candidatesByPath map[string][]codexPaneCandidate,
	hintsByPath map[string][]codexThreadHint,
	targetByID map[string]model.Target,
) map[string][]codexPaneCandidate {
	if len(candidatesByPath) == 0 {
		return candidatesByPath
	}
	type targetPane struct {
		targetID string
		paneID   string
	}
	targetPaneSet := map[targetPane]struct{}{}
	for pathKey, candidates := range candidatesByPath {
		if len(candidates) <= 1 || len(hintsByPath[pathKey]) == 0 {
			continue
		}
		for _, candidate := range candidates {
			targetID := strings.TrimSpace(candidate.targetID)
			paneID := strings.TrimSpace(candidate.paneID)
			if targetID == "" || paneID == "" {
				continue
			}
			targetPaneSet[targetPane{targetID: targetID, paneID: paneID}] = struct{}{}
		}
	}
	if len(targetPaneSet) == 0 {
		return candidatesByPath
	}
	panesByTarget := map[string][]string{}
	for tp := range targetPaneSet {
		panesByTarget[tp.targetID] = append(panesByTarget[tp.targetID], tp.paneID)
	}
	threadByTargetPane := map[string]map[string]string{}
	for targetID, paneIDs := range panesByTarget {
		targetRecord, ok := targetByID[targetID]
		if !ok {
			continue
		}
		threadByTargetPane[targetID] = s.resolveCodexThreadIDsForTarget(ctx, targetRecord, paneIDs)
	}
	out := make(map[string][]codexPaneCandidate, len(candidatesByPath))
	for pathKey, candidates := range candidatesByPath {
		enriched := append([]codexPaneCandidate(nil), candidates...)
		for idx := range enriched {
			targetID := strings.TrimSpace(enriched[idx].targetID)
			paneID := strings.TrimSpace(enriched[idx].paneID)
			if targetID == "" || paneID == "" {
				continue
			}
			if byPane, ok := threadByTargetPane[targetID]; ok {
				if threadID, ok := byPane[paneID]; ok && threadID != "" {
					enriched[idx].threadID = threadID
				}
			}
		}
		out[pathKey] = enriched
	}
	return out
}

func (s *Server) resolveCodexThreadIDsForTarget(ctx context.Context, targetRecord model.Target, paneIDs []string) map[string]string {
	out := map[string]string{}
	deduped := dedupePaneIDs(paneIDs)
	if len(deduped) == 0 {
		return out
	}
	now := time.Now().UTC()
	missing := make([]string, 0, len(deduped))
	s.codexPaneMu.Lock()
	for _, paneID := range deduped {
		cacheKey := codexPaneCacheKey(targetRecord.TargetID, paneID)
		if cached, ok := s.codexPaneCache[cacheKey]; ok && now.Before(cached.expiresAt) {
			if cached.threadID != "" {
				out[paneID] = cached.threadID
			}
			continue
		}
		missing = append(missing, paneID)
	}
	s.codexPaneMu.Unlock()
	if len(missing) == 0 {
		return out
	}
	resolved := s.resolveCodexThreadIDsForTargetUncached(ctx, targetRecord, missing)
	expiry := time.Now().UTC().Add(s.codexPaneTTL)
	s.codexPaneMu.Lock()
	for _, paneID := range missing {
		threadID := normalizeCodexThreadID(resolved[paneID])
		s.codexPaneCache[codexPaneCacheKey(targetRecord.TargetID, paneID)] = codexPaneCacheEntry{
			threadID:  threadID,
			expiresAt: expiry,
		}
		if threadID != "" {
			out[paneID] = threadID
		}
	}
	s.codexPaneMu.Unlock()
	return out
}

func (s *Server) resolveCodexThreadIDsForTargetUncached(ctx context.Context, targetRecord model.Target, paneIDs []string) map[string]string {
	out := map[string]string{}
	panePIDByID := s.readPanePIDByID(ctx, targetRecord, paneIDs)
	if len(panePIDByID) == 0 {
		return out
	}
	processByPID := s.readProcessTable(ctx, targetRecord)
	if len(processByPID) == 0 {
		return out
	}
	codexPIDByPane := map[string]int{}
	uniqueCodexPIDs := map[int]struct{}{}
	for paneID, panePID := range panePIDByID {
		codexPID := findLikelyCodexDescendantPID(panePID, processByPID)
		if codexPID <= 0 {
			continue
		}
		codexPIDByPane[paneID] = codexPID
		uniqueCodexPIDs[codexPID] = struct{}{}
	}
	if len(uniqueCodexPIDs) == 0 {
		return out
	}
	threadByPID := map[int]string{}
	for pid := range uniqueCodexPIDs {
		threadID := s.readCodexThreadIDFromProcess(ctx, targetRecord, pid)
		if threadID == "" {
			continue
		}
		threadByPID[pid] = threadID
	}
	for paneID, pid := range codexPIDByPane {
		if threadID, ok := threadByPID[pid]; ok {
			out[paneID] = threadID
		}
	}
	return out
}

func (s *Server) readPanePIDByID(ctx context.Context, targetRecord model.Target, paneIDs []string) map[string]int {
	out := map[string]int{}
	wanted := map[string]struct{}{}
	for _, paneID := range paneIDs {
		id := strings.TrimSpace(paneID)
		if id == "" {
			continue
		}
		wanted[id] = struct{}{}
	}
	if len(wanted) == 0 {
		return out
	}
	res, err := s.executor.Run(ctx, targetRecord, target.BuildTmuxCommand(
		"list-panes",
		"-a",
		"-F",
		tmuxfmt.Join("#{pane_id}", "#{pane_pid}"),
	))
	if err != nil {
		return out
	}
	for _, line := range strings.Split(res.Output, "\n") {
		parts := tmuxfmt.SplitLine(strings.TrimSpace(line), 2)
		if len(parts) < 2 {
			continue
		}
		paneID := strings.TrimSpace(parts[0])
		if _, ok := wanted[paneID]; !ok {
			continue
		}
		pid, err := strconv.Atoi(strings.TrimSpace(parts[1]))
		if err != nil || pid <= 0 {
			continue
		}
		out[paneID] = pid
	}
	return out
}

func (s *Server) readProcessTable(ctx context.Context, targetRecord model.Target) map[int]codexProcessInfo {
	commands := [][]string{
		{"ps", "-axo", "pid=,ppid=,command="},
		{"ps", "-eo", "pid=,ppid=,args="},
	}
	for _, command := range commands {
		res, err := s.executor.Run(ctx, targetRecord, command)
		if err != nil {
			continue
		}
		parsed := parseProcessTable(res.Output)
		if len(parsed) > 0 {
			return parsed
		}
	}
	return map[int]codexProcessInfo{}
}

func (s *Server) readCodexThreadIDFromProcess(ctx context.Context, targetRecord model.Target, pid int) string {
	if pid <= 0 {
		return ""
	}
	pidText := strconv.Itoa(pid)
	commands := [][]string{
		{"lsof", "-Fn", "-p", pidText},
		{"lsof", "-p", pidText},
	}
	for _, command := range commands {
		res, err := s.executor.Run(ctx, targetRecord, command)
		if err != nil {
			continue
		}
		threadID := extractCodexThreadIDFromLsofOutput(res.Output)
		if threadID != "" {
			return threadID
		}
	}
	return ""
}

func assignCodexHintsToCandidates(candidates []codexPaneCandidate, hints []codexThreadHint) (map[string]codexThreadHint, map[string]codexThreadHint) {
	byRuntime := map[string]codexThreadHint{}
	byPane := map[string]codexThreadHint{}
	if len(candidates) <= 1 || len(hints) == 0 {
		return byRuntime, byPane
	}
	assign := func(candidate codexPaneCandidate, hint codexThreadHint) {
		if candidate.runtimeID != "" {
			byRuntime[candidate.runtimeID] = hint
			return
		}
		byPane[candidate.paneKey] = hint
	}
	candidateIndices := make([]int, 0, len(candidates))
	for idx := range candidates {
		candidateIndices = append(candidateIndices, idx)
	}
	sort.Slice(candidateIndices, func(i, j int) bool {
		return codexCandidateComesBefore(candidates[candidateIndices[i]], candidates[candidateIndices[j]])
	})
	usedCandidates := map[int]struct{}{}
	usedHints := map[int]struct{}{}

	hintIndexByThreadID := map[string]int{}
	for hintIdx, hint := range hints {
		threadID := normalizeCodexThreadID(hint.id)
		if threadID == "" {
			continue
		}
		if _, exists := hintIndexByThreadID[threadID]; !exists {
			hintIndexByThreadID[threadID] = hintIdx
		}
	}
	for _, candidateIdx := range candidateIndices {
		threadID := normalizeCodexThreadID(candidates[candidateIdx].threadID)
		if threadID == "" {
			continue
		}
		hintIdx, ok := hintIndexByThreadID[threadID]
		if !ok {
			continue
		}
		if _, alreadyUsed := usedHints[hintIdx]; alreadyUsed {
			continue
		}
		assign(candidates[candidateIdx], hints[hintIdx])
		usedCandidates[candidateIdx] = struct{}{}
		usedHints[hintIdx] = struct{}{}
	}

	type textMatch struct {
		candidateIdx int
		hintIdx      int
		score        int
		delta        time.Duration
	}
	textMatches := make([]textMatch, 0, len(candidates))
	for candidateIdx, candidate := range candidates {
		if _, alreadyUsed := usedCandidates[candidateIdx]; alreadyUsed {
			continue
		}
		for hintIdx, hint := range hints {
			if _, alreadyUsed := usedHints[hintIdx]; alreadyUsed {
				continue
			}
			score := codexLabelMatchScore(candidate.labelHint, hint.label)
			if score <= 0 {
				continue
			}
			delta := absDuration(candidate.activityAt.Sub(hint.at))
			textMatches = append(textMatches, textMatch{
				candidateIdx: candidateIdx,
				hintIdx:      hintIdx,
				score:        score,
				delta:        delta,
			})
		}
	}
	sort.Slice(textMatches, func(i, j int) bool {
		lhs := textMatches[i]
		rhs := textMatches[j]
		if lhs.score != rhs.score {
			return lhs.score > rhs.score
		}
		if lhs.delta != rhs.delta {
			return lhs.delta < rhs.delta
		}
		return codexCandidateComesBefore(candidates[lhs.candidateIdx], candidates[rhs.candidateIdx])
	})
	for _, match := range textMatches {
		if _, alreadyUsed := usedCandidates[match.candidateIdx]; alreadyUsed {
			continue
		}
		if _, alreadyUsed := usedHints[match.hintIdx]; alreadyUsed {
			continue
		}
		assign(candidates[match.candidateIdx], hints[match.hintIdx])
		usedCandidates[match.candidateIdx] = struct{}{}
		usedHints[match.hintIdx] = struct{}{}
	}

	remainingCandidates := make([]int, 0, len(candidates))
	for _, candidateIdx := range candidateIndices {
		if _, alreadyUsed := usedCandidates[candidateIdx]; alreadyUsed {
			continue
		}
		remainingCandidates = append(remainingCandidates, candidateIdx)
	}
	remainingHints := make([]int, 0, len(hints))
	for hintIdx := range hints {
		if _, alreadyUsed := usedHints[hintIdx]; alreadyUsed {
			continue
		}
		remainingHints = append(remainingHints, hintIdx)
	}
	limit := len(remainingCandidates)
	if len(remainingHints) < limit {
		limit = len(remainingHints)
	}
	for idx := 0; idx < limit; idx++ {
		assign(candidates[remainingCandidates[idx]], hints[remainingHints[idx]])
	}
	return byRuntime, byPane
}

func codexCandidateComesBefore(lhs, rhs codexPaneCandidate) bool {
	if !lhs.activityAt.Equal(rhs.activityAt) {
		if lhs.activityAt.IsZero() {
			return false
		}
		if rhs.activityAt.IsZero() {
			return true
		}
		return lhs.activityAt.After(rhs.activityAt)
	}
	if !lhs.startedAt.Equal(rhs.startedAt) {
		return lhs.startedAt.After(rhs.startedAt)
	}
	if lhs.runtimeID != rhs.runtimeID {
		return lhs.runtimeID < rhs.runtimeID
	}
	return lhs.paneKey < rhs.paneKey
}

func normalizeCodexThreadID(raw string) string {
	return strings.ToLower(strings.TrimSpace(raw))
}

func normalizeCodexHintLabel(raw string) string {
	normalized := strings.ToLower(strings.TrimSpace(raw))
	if normalized == "" {
		return ""
	}
	normalized = strings.ReplaceAll(normalized, "…", "")
	normalized = strings.ReplaceAll(normalized, "...", "")
	normalized = strings.Join(strings.Fields(normalized), " ")
	return normalized
}

func codexLabelMatchScore(candidateLabel, hintLabel string) int {
	candidate := normalizeCodexHintLabel(candidateLabel)
	hint := normalizeCodexHintLabel(hintLabel)
	if candidate == "" || hint == "" {
		return 0
	}
	if candidate == hint {
		return 1000 + len([]rune(candidate))
	}
	shorter := len([]rune(candidate))
	if value := len([]rune(hint)); value < shorter {
		shorter = value
	}
	prefix := sharedPrefixRuneLen(candidate, hint)
	if prefix >= 6 {
		return 800 + prefix
	}
	if shorter >= 6 && (strings.Contains(candidate, hint) || strings.Contains(hint, candidate)) {
		return 600 + shorter
	}
	return 0
}

func sharedPrefixRuneLen(lhs, rhs string) int {
	left := []rune(lhs)
	right := []rune(rhs)
	limit := len(left)
	if len(right) < limit {
		limit = len(right)
	}
	count := 0
	for idx := 0; idx < limit; idx++ {
		if left[idx] != right[idx] {
			break
		}
		count++
	}
	return count
}

func absDuration(v time.Duration) time.Duration {
	if v < 0 {
		return -v
	}
	return v
}

func dedupePaneIDs(paneIDs []string) []string {
	seen := map[string]struct{}{}
	out := make([]string, 0, len(paneIDs))
	for _, paneID := range paneIDs {
		id := strings.TrimSpace(paneID)
		if id == "" {
			continue
		}
		if _, exists := seen[id]; exists {
			continue
		}
		seen[id] = struct{}{}
		out = append(out, id)
	}
	return out
}

func codexPaneCacheKey(targetID, paneID string) string {
	return strings.TrimSpace(targetID) + "|" + strings.TrimSpace(paneID)
}

func parseProcessTable(raw string) map[int]codexProcessInfo {
	out := map[int]codexProcessInfo{}
	for _, line := range strings.Split(raw, "\n") {
		fields := strings.Fields(strings.TrimSpace(line))
		if len(fields) < 3 {
			continue
		}
		pid, err := strconv.Atoi(fields[0])
		if err != nil || pid <= 0 {
			continue
		}
		ppid, err := strconv.Atoi(fields[1])
		if err != nil || ppid < 0 {
			continue
		}
		out[pid] = codexProcessInfo{
			pid:     pid,
			ppid:    ppid,
			command: strings.Join(fields[2:], " "),
		}
	}
	return out
}

func findLikelyCodexDescendantPID(rootPID int, processByPID map[int]codexProcessInfo) int {
	if rootPID <= 0 || len(processByPID) == 0 {
		return 0
	}
	childrenByPID := map[int][]int{}
	for pid, proc := range processByPID {
		childrenByPID[proc.ppid] = append(childrenByPID[proc.ppid], pid)
	}
	type queueItem struct {
		pid   int
		depth int
	}
	stack := []queueItem{{pid: rootPID, depth: 0}}
	visited := map[int]struct{}{}
	bestPID := 0
	bestScore := 0
	bestDepth := -1
	for len(stack) > 0 {
		last := len(stack) - 1
		item := stack[last]
		stack = stack[:last]
		if _, seen := visited[item.pid]; seen {
			continue
		}
		visited[item.pid] = struct{}{}
		proc, ok := processByPID[item.pid]
		if !ok {
			continue
		}
		score := codexProcessScore(proc.command)
		if score > bestScore ||
			(score == bestScore && item.depth > bestDepth) ||
			(score == bestScore && item.depth == bestDepth && item.pid > bestPID) {
			bestScore = score
			bestDepth = item.depth
			bestPID = item.pid
		}
		for _, child := range childrenByPID[item.pid] {
			stack = append(stack, queueItem{pid: child, depth: item.depth + 1})
		}
	}
	if bestScore <= 0 {
		return 0
	}
	return bestPID
}

func codexProcessScore(command string) int {
	normalized := strings.ToLower(strings.TrimSpace(command))
	switch {
	case normalized == "":
		return 0
	case strings.Contains(normalized, "/codex/codex"):
		return 3
	case strings.Contains(normalized, "@openai/codex") && strings.Contains(normalized, "node"):
		return 2
	case strings.HasPrefix(normalized, "codex "),
		strings.Contains(normalized, " codex "),
		strings.Contains(normalized, "/bin/codex"):
		return 1
	default:
		return 0
	}
}

func extractCodexThreadIDFromLsofOutput(raw string) string {
	candidatePath := ""
	for _, line := range strings.Split(raw, "\n") {
		trimmed := strings.TrimSpace(line)
		if trimmed == "" {
			continue
		}
		path := ""
		switch {
		case strings.HasPrefix(trimmed, "n/"):
			path = strings.TrimPrefix(trimmed, "n")
		case strings.HasPrefix(trimmed, "/"):
			path = trimmed
		default:
			fields := strings.Fields(trimmed)
			if len(fields) > 0 {
				last := strings.TrimSpace(fields[len(fields)-1])
				if strings.HasPrefix(last, "/") {
					path = last
				}
			}
		}
		if extractCodexThreadIDFromPath(path) == "" {
			continue
		}
		if path > candidatePath {
			candidatePath = path
		}
	}
	return extractCodexThreadIDFromPath(candidatePath)
}

func extractCodexThreadIDFromPath(path string) string {
	trimmed := strings.TrimSpace(path)
	if trimmed == "" {
		return ""
	}
	normalized := strings.ReplaceAll(trimmed, "\\", "/")
	if !strings.Contains(normalized, "/.codex/sessions/") {
		return ""
	}
	base := strings.ToLower(filepath.Base(normalized))
	matches := codexSessionFileIDPattern.FindStringSubmatch(base)
	if len(matches) != 2 {
		return ""
	}
	return strings.ToLower(matches[1])
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

type codexPaneCandidate struct {
	targetID   string
	paneID     string
	paneKey    string
	runtimeID  string
	threadID   string
	labelHint  string
	startedAt  time.Time
	activityAt time.Time
}

type codexPaneCacheEntry struct {
	threadID  string
	expiresAt time.Time
}

type codexProcessInfo struct {
	pid     int
	ppid    int
	command string
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

type claudeSessionHint struct {
	label  string
	source string
	at     time.Time
}

type claudeRuntimeProbe struct {
	runtimeID   string
	targetID    string
	currentPath string
	pid         int64
	startedAt   time.Time
	targetKind  model.TargetKind
	resumeID    string
}

type claudeHistoryRecord struct {
	sessionID   string
	projectPath string
	display     string
	at          time.Time
}

type claudeWorkspaceSessionHint struct {
	sessionID string
	hint      claudeSessionHint
}

type claudeProjectSessionFile struct {
	sessionID string
	path      string
	at        time.Time
}

type claudeHistoryCacheEntry struct {
	path      string
	modTime   time.Time
	fetchedAt time.Time
	records   []claudeHistoryRecord
}

type claudePreviewCacheEntry struct {
	modTime   time.Time
	fetchedAt time.Time
	preview   string
}

func (s *Server) collectClaudeSessionHints(
	ctx context.Context,
	panes []model.Pane,
	requestedIDs map[string]struct{},
	stateByPane map[string]model.StateRow,
	runtimeByID map[string]model.Runtime,
	runtimeByPane map[string]model.Runtime,
	targetByID map[string]model.Target,
) map[string]claudeSessionHint {
	hints := map[string]claudeSessionHint{}
	probeByRuntimeID := map[string]claudeRuntimeProbe{}
	pidsByTargetID := map[string][]int64{}
	pidSeen := map[string]map[int64]struct{}{}

	for _, pane := range panes {
		if _, ok := requestedIDs[pane.TargetID]; !ok {
			continue
		}
		key := paneKey(pane.TargetID, pane.PaneID)
		runtimeID := ""
		rt := model.Runtime{}
		hasRuntime := false

		if st, ok := stateByPane[key]; ok {
			if rid := strings.TrimSpace(st.RuntimeID); rid != "" {
				runtimeID = rid
				if candidate, ok := runtimeByID[rid]; ok {
					rt = candidate
					hasRuntime = true
				}
			}
		}
		if !hasRuntime {
			if candidate, ok := runtimeByPane[key]; ok {
				runtimeID = strings.TrimSpace(candidate.RuntimeID)
				rt = candidate
				hasRuntime = true
			}
		}
		if !hasRuntime || runtimeID == "" || rt.PID == nil || *rt.PID <= 0 {
			continue
		}
		if !strings.EqualFold(strings.TrimSpace(rt.AgentType), "claude") {
			continue
		}
		if _, exists := probeByRuntimeID[runtimeID]; exists {
			continue
		}
		probeByRuntimeID[runtimeID] = claudeRuntimeProbe{
			runtimeID:   runtimeID,
			targetID:    rt.TargetID,
			currentPath: pane.CurrentPath,
			pid:         *rt.PID,
			startedAt:   rt.StartedAt,
			targetKind:  targetByID[rt.TargetID].Kind,
		}

		seenForTarget, ok := pidSeen[rt.TargetID]
		if !ok {
			seenForTarget = map[int64]struct{}{}
			pidSeen[rt.TargetID] = seenForTarget
		}
		if _, exists := seenForTarget[*rt.PID]; !exists {
			seenForTarget[*rt.PID] = struct{}{}
			pidsByTargetID[rt.TargetID] = append(pidsByTargetID[rt.TargetID], *rt.PID)
		}
	}
	if len(probeByRuntimeID) == 0 {
		return hints
	}

	commandByTarget := map[string]map[int64]string{}
	for targetID, pids := range pidsByTargetID {
		tg, ok := targetByID[targetID]
		if !ok || len(pids) == 0 {
			continue
		}
		byPID, err := s.listProcessCommandsByPID(ctx, tg, pids)
		if err != nil {
			continue
		}
		commandByTarget[targetID] = byPID
	}

	homeDir, _ := os.UserHomeDir()
	localGroups := map[string][]claudeRuntimeProbe{}
	runtimeIDs := make([]string, 0, len(probeByRuntimeID))
	for runtimeID := range probeByRuntimeID {
		runtimeIDs = append(runtimeIDs, runtimeID)
	}
	sort.Strings(runtimeIDs)
	for _, runtimeID := range runtimeIDs {
		probe := probeByRuntimeID[runtimeID]
		if commands := commandByTarget[probe.targetID]; len(commands) > 0 {
			probe.resumeID = extractClaudeResumeID(strings.TrimSpace(commands[probe.pid]))
		}
		if probe.targetKind != model.TargetKindLocal {
			if probe.resumeID != "" {
				if hint := s.resolveClaudeSessionHintCached(homeDir, probe.currentPath, probe.resumeID, probe.targetKind); hint.label != "" {
					hints[runtimeID] = hint
				}
			}
			continue
		}
		workspaceKey := normalizeClaudeWorkspacePath(probe.currentPath)
		if workspaceKey == "" {
			if probe.resumeID != "" {
				if hint := s.resolveClaudeSessionHintCached(homeDir, probe.currentPath, probe.resumeID, probe.targetKind); hint.label != "" {
					hints[runtimeID] = hint
				}
			}
			continue
		}
		probe.currentPath = workspaceKey
		localGroups[workspaceKey] = append(localGroups[workspaceKey], probe)
	}
	if len(localGroups) == 0 {
		return hints
	}

	historyRecords := s.getClaudeHistoryRecords(homeDir)
	workspaces := make([]string, 0, len(localGroups))
	for workspace := range localGroups {
		workspaces = append(workspaces, workspace)
	}
	sort.Strings(workspaces)
	for _, workspace := range workspaces {
		group := append([]claudeRuntimeProbe(nil), localGroups[workspace]...)
		sort.Slice(group, func(i, j int) bool {
			return claudeProbeComesBefore(group[i], group[j])
		})
		sessionHints := s.buildClaudeWorkspaceSessionHints(homeDir, workspace, historyRecords)
		sessionHintByID := map[string]claudeSessionHint{}
		for _, candidate := range sessionHints {
			sessionID := normalizeSessionID(candidate.sessionID)
			if sessionID == "" || candidate.hint.label == "" {
				continue
			}
			if _, exists := sessionHintByID[sessionID]; !exists {
				sessionHintByID[sessionID] = candidate.hint
			}
		}
		usedSessionIDs := map[string]struct{}{}
		unresolved := make([]claudeRuntimeProbe, 0, len(group))
		for _, probe := range group {
			runtimeID := strings.TrimSpace(probe.runtimeID)
			if runtimeID == "" {
				continue
			}
			resumeID := normalizeSessionID(probe.resumeID)
			if resumeID == "" {
				unresolved = append(unresolved, probe)
				continue
			}
			hint, ok := sessionHintByID[resumeID]
			if !ok || hint.label == "" {
				hint = s.resolveClaudeSessionHintCached(homeDir, workspace, resumeID, model.TargetKindLocal)
			}
			if hint.label == "" {
				unresolved = append(unresolved, probe)
				continue
			}
			hints[runtimeID] = hint
			usedSessionIDs[resumeID] = struct{}{}
		}
		assigned := assignClaudeWorkspaceHintsToProbes(unresolved, sessionHints, usedSessionIDs)
		for runtimeID, hint := range assigned {
			hints[runtimeID] = hint
		}
	}
	return hints
}

func (s *Server) listProcessCommandsByPID(ctx context.Context, tg model.Target, pids []int64) (map[int64]string, error) {
	out := map[int64]string{}
	dedup := make([]int64, 0, len(pids))
	seen := map[int64]struct{}{}
	for _, pid := range pids {
		if pid <= 0 {
			continue
		}
		if _, ok := seen[pid]; ok {
			continue
		}
		seen[pid] = struct{}{}
		dedup = append(dedup, pid)
	}
	if len(dedup) == 0 {
		return out, nil
	}
	sort.Slice(dedup, func(i, j int) bool { return dedup[i] < dedup[j] })
	cmd := []string{"ps", "-o", "pid=", "-o", "command="}
	for _, pid := range dedup {
		cmd = append(cmd, "-p", strconv.FormatInt(pid, 10))
	}
	res, err := s.executor.Run(ctx, tg, cmd)
	if err != nil {
		return out, err
	}
	return parsePSPIDCommandOutput(res.Output), nil
}

func parsePSPIDCommandOutput(output string) map[int64]string {
	out := map[int64]string{}
	scanner := bufio.NewScanner(strings.NewReader(output))
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		splitIdx := strings.IndexAny(line, " \t")
		if splitIdx <= 0 || splitIdx >= len(line)-1 {
			continue
		}
		pidStr := strings.TrimSpace(line[:splitIdx])
		pid, err := strconv.ParseInt(pidStr, 10, 64)
		if err != nil || pid <= 0 {
			continue
		}
		cmdline := strings.TrimSpace(line[splitIdx+1:])
		if cmdline == "" {
			continue
		}
		out[pid] = cmdline
	}
	return out
}

func claudeProbeComesBefore(lhs, rhs claudeRuntimeProbe) bool {
	if !lhs.startedAt.Equal(rhs.startedAt) {
		if lhs.startedAt.IsZero() {
			return false
		}
		if rhs.startedAt.IsZero() {
			return true
		}
		return lhs.startedAt.After(rhs.startedAt)
	}
	if lhs.runtimeID != rhs.runtimeID {
		return lhs.runtimeID < rhs.runtimeID
	}
	if lhs.targetID != rhs.targetID {
		return lhs.targetID < rhs.targetID
	}
	return lhs.pid < rhs.pid
}

func normalizeClaudeWorkspacePath(path string) string {
	normalized := normalizeCodexWorkspacePath(path)
	if normalized == "" {
		return ""
	}
	if resolved, err := filepath.EvalSymlinks(normalized); err == nil {
		if clean := normalizeCodexWorkspacePath(resolved); clean != "" {
			return clean
		}
	}
	return normalized
}

func readClaudeHistoryRecords(homeDir string) []claudeHistoryRecord {
	base := strings.TrimSpace(homeDir)
	if base == "" {
		return nil
	}
	path := filepath.Join(base, ".claude", "history.jsonl")
	file, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer file.Close()

	records := make([]claudeHistoryRecord, 0, 32)
	scanner := bufio.NewScanner(file)
	scanner.Buffer(make([]byte, 0, 64*1024), 4*1024*1024)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" || !strings.HasPrefix(line, "{") {
			continue
		}
		var payload map[string]any
		if err := json.Unmarshal([]byte(line), &payload); err != nil {
			continue
		}
		sessionID := normalizeSessionID(anyString(payload["sessionId"]))
		if sessionID == "" {
			continue
		}
		projectPath := normalizeClaudeWorkspacePath(anyString(payload["project"]))
		if projectPath == "" {
			continue
		}
		records = append(records, claudeHistoryRecord{
			sessionID:   sessionID,
			projectPath: projectPath,
			display:     compactPreview(anyString(payload["display"]), 72),
			at:          parseClaudeHistoryTimestamp(payload["timestamp"]),
		})
	}
	return records
}

func parseClaudeHistoryTimestamp(raw any) time.Time {
	switch value := raw.(type) {
	case float64:
		return normalizeUnixTimestamp(int64(value))
	case float32:
		return normalizeUnixTimestamp(int64(value))
	case int64:
		return normalizeUnixTimestamp(value)
	case int:
		return normalizeUnixTimestamp(int64(value))
	case json.Number:
		if asInt, err := value.Int64(); err == nil {
			return normalizeUnixTimestamp(asInt)
		}
		if asFloat, err := value.Float64(); err == nil {
			return normalizeUnixTimestamp(int64(asFloat))
		}
	case string:
		trimmed := strings.TrimSpace(value)
		if trimmed == "" {
			return time.Time{}
		}
		if asInt, err := strconv.ParseInt(trimmed, 10, 64); err == nil {
			return normalizeUnixTimestamp(asInt)
		}
		if parsed, err := time.Parse(time.RFC3339Nano, trimmed); err == nil {
			return parsed.UTC()
		}
		if parsed, err := time.Parse(time.RFC3339, trimmed); err == nil {
			return parsed.UTC()
		}
	}
	return time.Time{}
}

func normalizeUnixTimestamp(value int64) time.Time {
	if value <= 0 {
		return time.Time{}
	}
	abs := value
	if abs < 0 {
		abs = -abs
	}
	switch {
	case abs >= 1_000_000_000_000_000_000:
		return time.Unix(0, value).UTC()
	case abs >= 1_000_000_000_000_000:
		return time.Unix(0, value*int64(time.Microsecond)).UTC()
	case abs >= 1_000_000_000_000:
		return time.Unix(0, value*int64(time.Millisecond)).UTC()
	default:
		return time.Unix(value, 0).UTC()
	}
}

func claudeWorkspaceMatchScore(workspacePath, projectPath string) int {
	workspace := normalizeClaudeWorkspacePath(workspacePath)
	project := normalizeClaudeWorkspacePath(projectPath)
	if workspace == "" || project == "" {
		return 0
	}
	if workspace == project {
		return 3
	}
	sep := string(filepath.Separator)
	if strings.HasPrefix(workspace+sep, project+sep) {
		return 2
	}
	if strings.HasPrefix(project+sep, workspace+sep) {
		return 1
	}
	return 0
}

func (s *Server) getClaudeHistoryRecords(homeDir string) []claudeHistoryRecord {
	base := strings.TrimSpace(homeDir)
	if base == "" {
		return nil
	}
	historyPath := filepath.Join(base, ".claude", "history.jsonl")
	now := time.Now().UTC()
	stat, err := os.Stat(historyPath)
	if err != nil {
		s.claudeHistoryMu.Lock()
		s.claudeHistory = claudeHistoryCacheEntry{
			path:      historyPath,
			fetchedAt: now,
			records:   nil,
		}
		s.claudeHistoryMu.Unlock()
		return nil
	}
	modTime := stat.ModTime().UTC()

	s.claudeHistoryMu.Lock()
	cached := s.claudeHistory
	cacheHit := cached.path == historyPath &&
		!cached.fetchedAt.IsZero() &&
		now.Sub(cached.fetchedAt) < s.claudeHistoryTTL &&
		cached.modTime.Equal(modTime)
	if cacheHit {
		records := append([]claudeHistoryRecord(nil), cached.records...)
		s.claudeHistoryMu.Unlock()
		return records
	}
	s.claudeHistoryMu.Unlock()

	records := readClaudeHistoryRecords(homeDir)
	s.claudeHistoryMu.Lock()
	s.claudeHistory = claudeHistoryCacheEntry{
		path:      historyPath,
		modTime:   modTime,
		fetchedAt: now,
		records:   append([]claudeHistoryRecord(nil), records...),
	}
	s.claudeHistoryMu.Unlock()
	return records
}

func (s *Server) resolveClaudeSessionHintCached(homeDir, workspacePath, sessionID string, targetKind model.TargetKind) claudeSessionHint {
	normalizedID := normalizeSessionID(sessionID)
	if normalizedID == "" {
		return claudeSessionHint{}
	}
	if targetKind == model.TargetKindLocal {
		if preview, at, ok := s.readClaudeSessionPreviewCached(homeDir, workspacePath, normalizedID); ok && preview != "" {
			return claudeSessionHint{
				label:  preview,
				source: "claude_session_jsonl",
				at:     at,
			}
		}
	}
	return claudeSessionHint{
		label:  "claude " + shortSessionID(normalizedID),
		source: "claude_resume_id",
	}
}

func (s *Server) readClaudeSessionPreviewCached(homeDir, workspacePath, sessionID string) (string, time.Time, bool) {
	candidates := claudeSessionJSONLCandidates(homeDir, workspacePath, sessionID)
	now := time.Now().UTC()
	for _, path := range candidates {
		stat, err := os.Stat(path)
		if err != nil {
			continue
		}
		modTime := stat.ModTime().UTC()
		s.claudePreviewMu.Lock()
		cached, ok := s.claudePreview[path]
		if ok && now.Sub(cached.fetchedAt) < s.claudePreviewTTL && cached.modTime.Equal(modTime) {
			preview := cached.preview
			s.claudePreviewMu.Unlock()
			if preview != "" {
				return preview, modTime, true
			}
			continue
		}
		s.claudePreviewMu.Unlock()

		preview := readFirstClaudeUserPrompt(path)
		s.claudePreviewMu.Lock()
		s.claudePreview[path] = claudePreviewCacheEntry{
			modTime:   modTime,
			fetchedAt: now,
			preview:   preview,
		}
		for key, entry := range s.claudePreview {
			if now.Sub(entry.fetchedAt) > s.claudePreviewTTL*3 {
				delete(s.claudePreview, key)
			}
		}
		s.claudePreviewMu.Unlock()
		if preview != "" {
			return preview, modTime, true
		}
	}
	return "", time.Time{}, false
}

func (s *Server) buildClaudeWorkspaceSessionHints(homeDir, workspacePath string, historyRecords []claudeHistoryRecord) []claudeWorkspaceSessionHint {
	workspace := normalizeClaudeWorkspacePath(workspacePath)
	if workspace == "" {
		return nil
	}
	type historyCandidate struct {
		record claudeHistoryRecord
		score  int
	}
	bySession := map[string]historyCandidate{}
	for _, record := range historyRecords {
		score := claudeWorkspaceMatchScore(workspace, record.projectPath)
		if score <= 0 {
			continue
		}
		existing, ok := bySession[record.sessionID]
		if !ok || score > existing.score || (score == existing.score && record.at.After(existing.record.at)) {
			bySession[record.sessionID] = historyCandidate{record: record, score: score}
		}
	}
	historyCandidates := make([]historyCandidate, 0, len(bySession))
	for _, candidate := range bySession {
		historyCandidates = append(historyCandidates, candidate)
	}
	sort.Slice(historyCandidates, func(i, j int) bool {
		lhs := historyCandidates[i]
		rhs := historyCandidates[j]
		if lhs.score != rhs.score {
			return lhs.score > rhs.score
		}
		if !lhs.record.at.Equal(rhs.record.at) {
			if lhs.record.at.IsZero() {
				return false
			}
			if rhs.record.at.IsZero() {
				return true
			}
			return lhs.record.at.After(rhs.record.at)
		}
		return lhs.record.sessionID < rhs.record.sessionID
	})
	if len(historyCandidates) > 12 {
		historyCandidates = historyCandidates[:12]
	}
	out := make([]claudeWorkspaceSessionHint, 0, len(historyCandidates))
	for _, candidate := range historyCandidates {
		hint := s.resolveClaudeSessionHintCached(homeDir, candidate.record.projectPath, candidate.record.sessionID, model.TargetKindLocal)
		if candidate.record.display != "" && (hint.label == "" || hint.source == "claude_resume_id") {
			hint = claudeSessionHint{
				label:  candidate.record.display,
				source: "claude_history_display",
				at:     candidate.record.at,
			}
		}
		if hint.label == "" {
			continue
		}
		if hint.at.IsZero() && !candidate.record.at.IsZero() {
			hint.at = candidate.record.at
		}
		out = append(out, claudeWorkspaceSessionHint{
			sessionID: candidate.record.sessionID,
			hint:      hint,
		})
	}
	if len(out) > 0 {
		return out
	}
	return s.scanRecentClaudeProjectSessionHints(homeDir, workspace, 8)
}

func (s *Server) scanRecentClaudeProjectSessionHints(homeDir, workspacePath string, limit int) []claudeWorkspaceSessionHint {
	if limit <= 0 {
		return nil
	}
	files := make([]claudeProjectSessionFile, 0, limit)
	for _, projectDir := range collectClaudeProjectDirsForWorkspace(homeDir, workspacePath) {
		for _, candidate := range listClaudeProjectSessionFiles(projectDir) {
			files = append(files, candidate)
		}
	}
	if len(files) == 0 {
		return nil
	}
	sort.Slice(files, func(i, j int) bool {
		lhs := files[i]
		rhs := files[j]
		if !lhs.at.Equal(rhs.at) {
			return lhs.at.After(rhs.at)
		}
		if lhs.sessionID != rhs.sessionID {
			return lhs.sessionID < rhs.sessionID
		}
		return lhs.path < rhs.path
	})
	seen := map[string]struct{}{}
	out := make([]claudeWorkspaceSessionHint, 0, limit)
	for _, file := range files {
		if len(out) >= limit {
			break
		}
		if _, exists := seen[file.sessionID]; exists {
			continue
		}
		preview, _, ok := s.readClaudeSessionPreviewCached(homeDir, workspacePath, file.sessionID)
		if !ok || preview == "" {
			continue
		}
		seen[file.sessionID] = struct{}{}
		out = append(out, claudeWorkspaceSessionHint{
			sessionID: file.sessionID,
			hint: claudeSessionHint{
				label:  preview,
				source: "claude_project_recent_jsonl",
				at:     file.at.UTC(),
			},
		})
	}
	return out
}

func buildClaudeWorkspaceSessionHints(homeDir, workspacePath string, historyRecords []claudeHistoryRecord) []claudeWorkspaceSessionHint {
	workspace := normalizeClaudeWorkspacePath(workspacePath)
	if workspace == "" {
		return nil
	}
	type historyCandidate struct {
		record claudeHistoryRecord
		score  int
	}
	bySession := map[string]historyCandidate{}
	for _, record := range historyRecords {
		score := claudeWorkspaceMatchScore(workspace, record.projectPath)
		if score <= 0 {
			continue
		}
		existing, ok := bySession[record.sessionID]
		if !ok || score > existing.score || (score == existing.score && record.at.After(existing.record.at)) {
			bySession[record.sessionID] = historyCandidate{record: record, score: score}
		}
	}
	historyCandidates := make([]historyCandidate, 0, len(bySession))
	for _, candidate := range bySession {
		historyCandidates = append(historyCandidates, candidate)
	}
	sort.Slice(historyCandidates, func(i, j int) bool {
		lhs := historyCandidates[i]
		rhs := historyCandidates[j]
		if lhs.score != rhs.score {
			return lhs.score > rhs.score
		}
		if !lhs.record.at.Equal(rhs.record.at) {
			if lhs.record.at.IsZero() {
				return false
			}
			if rhs.record.at.IsZero() {
				return true
			}
			return lhs.record.at.After(rhs.record.at)
		}
		return lhs.record.sessionID < rhs.record.sessionID
	})
	if len(historyCandidates) > 12 {
		historyCandidates = historyCandidates[:12]
	}

	out := make([]claudeWorkspaceSessionHint, 0, len(historyCandidates))
	for _, candidate := range historyCandidates {
		hint := resolveClaudeSessionHint(homeDir, candidate.record.projectPath, candidate.record.sessionID, model.TargetKindLocal)
		if candidate.record.display != "" && (hint.label == "" || hint.source == "claude_resume_id") {
			hint = claudeSessionHint{
				label:  candidate.record.display,
				source: "claude_history_display",
				at:     candidate.record.at,
			}
		}
		if hint.label == "" {
			continue
		}
		if hint.at.IsZero() && !candidate.record.at.IsZero() {
			hint.at = candidate.record.at
		}
		out = append(out, claudeWorkspaceSessionHint{
			sessionID: candidate.record.sessionID,
			hint:      hint,
		})
	}
	if len(out) > 0 {
		return out
	}
	return scanRecentClaudeProjectSessionHints(homeDir, workspace, 8)
}

func scanRecentClaudeProjectSessionHints(homeDir, workspacePath string, limit int) []claudeWorkspaceSessionHint {
	if limit <= 0 {
		return nil
	}
	files := make([]claudeProjectSessionFile, 0, limit)
	for _, projectDir := range collectClaudeProjectDirsForWorkspace(homeDir, workspacePath) {
		for _, candidate := range listClaudeProjectSessionFiles(projectDir) {
			files = append(files, candidate)
		}
	}
	if len(files) == 0 {
		return nil
	}
	sort.Slice(files, func(i, j int) bool {
		lhs := files[i]
		rhs := files[j]
		if !lhs.at.Equal(rhs.at) {
			return lhs.at.After(rhs.at)
		}
		if lhs.sessionID != rhs.sessionID {
			return lhs.sessionID < rhs.sessionID
		}
		return lhs.path < rhs.path
	})
	seen := map[string]struct{}{}
	out := make([]claudeWorkspaceSessionHint, 0, limit)
	for _, file := range files {
		if len(out) >= limit {
			break
		}
		if _, exists := seen[file.sessionID]; exists {
			continue
		}
		preview := readFirstClaudeUserPrompt(file.path)
		if preview == "" {
			continue
		}
		seen[file.sessionID] = struct{}{}
		out = append(out, claudeWorkspaceSessionHint{
			sessionID: file.sessionID,
			hint: claudeSessionHint{
				label:  preview,
				source: "claude_project_recent_jsonl",
				at:     file.at.UTC(),
			},
		})
	}
	return out
}

func collectClaudeProjectDirsForWorkspace(homeDir, workspacePath string) []string {
	baseDir := filepath.Join(strings.TrimSpace(homeDir), ".claude", "projects")
	if strings.TrimSpace(homeDir) == "" {
		return nil
	}
	workspace := normalizeClaudeWorkspacePath(workspacePath)
	if workspace == "" {
		return nil
	}
	out := make([]string, 0, 6)
	seen := map[string]struct{}{}
	add := func(path string) {
		normalized := strings.TrimSpace(path)
		if normalized == "" {
			return
		}
		if _, ok := seen[normalized]; ok {
			return
		}
		if info, err := os.Stat(normalized); err != nil || !info.IsDir() {
			return
		}
		seen[normalized] = struct{}{}
		out = append(out, normalized)
	}

	current := workspace
	for depth := 0; depth < 10; depth++ {
		if current == "" {
			break
		}
		add(filepath.Join(baseDir, claudeProjectKey(current)))
		parent := filepath.Dir(current)
		if parent == current {
			break
		}
		current = parent
	}
	return out
}

func listClaudeProjectSessionFiles(projectDir string) []claudeProjectSessionFile {
	entries, err := os.ReadDir(projectDir)
	if err != nil {
		return nil
	}
	out := make([]claudeProjectSessionFile, 0, len(entries))
	for _, entry := range entries {
		if entry.IsDir() {
			continue
		}
		sessionID := extractClaudeSessionIDFromFilename(entry.Name())
		if sessionID == "" {
			continue
		}
		info, err := entry.Info()
		if err != nil {
			continue
		}
		out = append(out, claudeProjectSessionFile{
			sessionID: sessionID,
			path:      filepath.Join(projectDir, entry.Name()),
			at:        info.ModTime().UTC(),
		})
	}
	return out
}

func extractClaudeSessionIDFromFilename(name string) string {
	trimmed := strings.TrimSpace(name)
	if !strings.HasSuffix(strings.ToLower(trimmed), ".jsonl") {
		return ""
	}
	sessionID := strings.TrimSuffix(trimmed, filepath.Ext(trimmed))
	sessionID = normalizeSessionID(sessionID)
	if sessionID == "" {
		return ""
	}
	if !claudeSessionIDPattern.MatchString(sessionID) {
		return ""
	}
	return strings.ToLower(sessionID)
}

func assignClaudeWorkspaceHintsToProbes(
	probes []claudeRuntimeProbe,
	sessionHints []claudeWorkspaceSessionHint,
	usedSessionIDs map[string]struct{},
) map[string]claudeSessionHint {
	out := map[string]claudeSessionHint{}
	if len(probes) == 0 || len(sessionHints) == 0 {
		return out
	}
	used := map[string]struct{}{}
	for sessionID := range usedSessionIDs {
		id := normalizeSessionID(sessionID)
		if id != "" {
			used[id] = struct{}{}
		}
	}
	candidates := append([]claudeWorkspaceSessionHint(nil), sessionHints...)
	probeOrder := append([]claudeRuntimeProbe(nil), probes...)
	sort.Slice(probeOrder, func(i, j int) bool {
		return claudeProbeComesBefore(probeOrder[i], probeOrder[j])
	})
	for _, probe := range probeOrder {
		runtimeID := strings.TrimSpace(probe.runtimeID)
		if runtimeID == "" {
			continue
		}
		bestIdx := -1
		bestHasDelta := false
		bestDelta := time.Duration(0)
		bestAt := time.Time{}
		for idx, candidate := range candidates {
			sessionID := normalizeSessionID(candidate.sessionID)
			if sessionID == "" || candidate.hint.label == "" {
				continue
			}
			if _, taken := used[sessionID]; taken {
				continue
			}
			hasDelta := !probe.startedAt.IsZero() && !candidate.hint.at.IsZero()
			delta := time.Duration(0)
			if hasDelta {
				delta = absDuration(candidate.hint.at.Sub(probe.startedAt))
			}
			if bestIdx < 0 || claudeSessionCandidateBetter(hasDelta, delta, candidate.hint.at, bestHasDelta, bestDelta, bestAt) {
				bestIdx = idx
				bestHasDelta = hasDelta
				bestDelta = delta
				bestAt = candidate.hint.at
			}
		}
		if bestIdx < 0 {
			continue
		}
		chosen := candidates[bestIdx]
		out[runtimeID] = chosen.hint
		if sessionID := normalizeSessionID(chosen.sessionID); sessionID != "" {
			used[sessionID] = struct{}{}
		}
	}
	return out
}

func claudeSessionCandidateBetter(
	candidateHasDelta bool,
	candidateDelta time.Duration,
	candidateAt time.Time,
	bestHasDelta bool,
	bestDelta time.Duration,
	bestAt time.Time,
) bool {
	if candidateHasDelta != bestHasDelta {
		return candidateHasDelta
	}
	if candidateHasDelta && candidateDelta != bestDelta {
		return candidateDelta < bestDelta
	}
	if !candidateAt.Equal(bestAt) {
		if candidateAt.IsZero() {
			return false
		}
		if bestAt.IsZero() {
			return true
		}
		return candidateAt.After(bestAt)
	}
	return false
}

func extractClaudeResumeID(cmdline string) string {
	fields := strings.Fields(strings.TrimSpace(cmdline))
	if len(fields) == 0 {
		return ""
	}
	for i, token := range fields {
		switch {
		case token == "--resume" || token == "-r":
			if i+1 >= len(fields) {
				continue
			}
			return normalizeSessionID(fields[i+1])
		case strings.HasPrefix(token, "--resume="):
			return normalizeSessionID(strings.TrimPrefix(token, "--resume="))
		}
	}
	return ""
}

func normalizeSessionID(raw string) string {
	candidate := strings.TrimSpace(strings.Trim(raw, "\"'"))
	if candidate == "" {
		return ""
	}
	if strings.ContainsAny(candidate, "/\\ \t\r\n") {
		return ""
	}
	return candidate
}

func resolveClaudeSessionHint(homeDir, workspacePath, sessionID string, targetKind model.TargetKind) claudeSessionHint {
	normalizedID := normalizeSessionID(sessionID)
	if normalizedID == "" {
		return claudeSessionHint{}
	}
	if targetKind == model.TargetKindLocal {
		if preview, at, ok := readClaudeSessionPreview(homeDir, workspacePath, normalizedID); ok && preview != "" {
			return claudeSessionHint{
				label:  preview,
				source: "claude_session_jsonl",
				at:     at,
			}
		}
	}
	return claudeSessionHint{
		label:  "claude " + shortSessionID(normalizedID),
		source: "claude_resume_id",
	}
}

func shortSessionID(sessionID string) string {
	id := strings.TrimSpace(sessionID)
	if id == "" {
		return ""
	}
	if len(id) <= 8 {
		return id
	}
	return id[:8]
}

func readClaudeSessionPreview(homeDir, workspacePath, sessionID string) (string, time.Time, bool) {
	candidates := claudeSessionJSONLCandidates(homeDir, workspacePath, sessionID)
	for _, path := range candidates {
		if preview := readFirstClaudeUserPrompt(path); preview != "" {
			if stat, err := os.Stat(path); err == nil {
				return preview, stat.ModTime().UTC(), true
			}
			return preview, time.Time{}, true
		}
	}
	return "", time.Time{}, false
}

func claudeSessionJSONLCandidates(homeDir, workspacePath, sessionID string) []string {
	baseDir := filepath.Join(strings.TrimSpace(homeDir), ".claude", "projects")
	if strings.TrimSpace(homeDir) == "" || strings.TrimSpace(sessionID) == "" {
		return nil
	}
	out := make([]string, 0, 4)
	seen := map[string]struct{}{}
	add := func(path string) {
		normalized := strings.TrimSpace(path)
		if normalized == "" {
			return
		}
		if _, ok := seen[normalized]; ok {
			return
		}
		seen[normalized] = struct{}{}
		out = append(out, normalized)
	}

	if key := claudeProjectKey(workspacePath); key != "" {
		add(filepath.Join(baseDir, key, sessionID+".jsonl"))
	}
	matches, _ := filepath.Glob(filepath.Join(baseDir, "*", sessionID+".jsonl"))
	sort.Strings(matches)
	for _, match := range matches {
		add(match)
	}
	return out
}

func claudeProjectKey(workspacePath string) string {
	normalized := strings.TrimSpace(workspacePath)
	if normalized == "" {
		return ""
	}
	return strings.ReplaceAll(normalized, "/", "-")
}

func readFirstClaudeUserPrompt(path string) string {
	file, err := os.Open(strings.TrimSpace(path))
	if err != nil {
		return ""
	}
	defer file.Close()

	scanner := bufio.NewScanner(file)
	scanner.Buffer(make([]byte, 0, 64*1024), 4*1024*1024)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" || !strings.HasPrefix(line, "{") {
			continue
		}
		var payload map[string]any
		if err := json.Unmarshal([]byte(line), &payload); err != nil {
			continue
		}
		if !strings.EqualFold(strings.TrimSpace(anyString(payload["type"])), "user") {
			continue
		}
		if msg, ok := payload["message"]; ok {
			if preview := compactPreview(extractPreviewFromJSON(msg), 72); preview != "" {
				return preview
			}
		}
		if preview := compactPreview(extractPreviewFromJSON(payload), 72); preview != "" {
			return preview
		}
	}
	return ""
}

func anyString(v any) string {
	s, _ := v.(string)
	return s
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
	if strings.ToLower(strings.TrimSpace(stateSource)) == string(model.SourcePoller) &&
		agentPresence == "none" &&
		lastActivityAt != nil {
		v := lastActivityAt.UTC()
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

func refinePanePresentationWithSignals(
	agentPresence string,
	activityState string,
	reasonCode string,
	lastEventType string,
	stateSource string,
	lastInteractionAt *time.Time,
	now time.Time,
) (string, string, string, bool) {
	state := strings.ToLower(strings.TrimSpace(activityState))
	reason := strings.ToLower(strings.TrimSpace(reasonCode))
	eventType := strings.ToLower(strings.TrimSpace(lastEventType))
	source := strings.ToLower(strings.TrimSpace(stateSource))
	hasRunningSignal := hasRunningHint(reason, eventType)
	hasIdleOrCompletionSignal := hasIdleOrCompletionHint(reason, eventType)

	if state == "" || state == string(model.StateUnknown) || state == string(model.StateIdle) {
		switch {
		case strings.Contains(reason, "waiting_approval"), strings.Contains(reason, "approval_required"), reason == "approval":
			state = string(model.StateWaitingApproval)
		case strings.Contains(reason, "waiting_input"), strings.Contains(reason, "needs_input"), reason == "input":
			state = string(model.StateWaitingInput)
		case strings.Contains(reason, "error"), strings.Contains(reason, "failed"):
			state = string(model.StateError)
		}
	}

	if agentPresence == "managed" &&
		(state == string(model.StateIdle) || state == string(model.StateUnknown)) &&
		lastInteractionAt != nil &&
		!lastInteractionAt.IsZero() &&
		source == string(model.SourcePoller) &&
		hasRunningSignal &&
		!hasIdleOrCompletionSignal {
		at := lastInteractionAt.UTC()
		if now.Sub(at) <= 8*time.Second {
			state = string(model.StateRunning)
		}
	}

	if agentPresence == "managed" &&
		state == string(model.StateRunning) &&
		source == string(model.SourcePoller) &&
		!hasRunningSignal {
		staleByInteraction := true
		if lastInteractionAt != nil && !lastInteractionAt.IsZero() {
			staleByInteraction = now.Sub(lastInteractionAt.UTC()) > 45*time.Second
		}
		if staleByInteraction {
			state = string(model.StateIdle)
		}
	}

	var category string
	switch {
	case agentPresence == "none":
		category = "unmanaged"
	case state == string(model.StateWaitingInput),
		state == string(model.StateWaitingApproval),
		state == string(model.StateError):
		category = "attention"
	case state == string(model.StateRunning):
		category = "running"
	case state == string(model.StateIdle):
		category = "idle"
	default:
		category = "unknown"
	}

	needsUserAction := state == string(model.StateWaitingInput) ||
		state == string(model.StateWaitingApproval) ||
		state == string(model.StateError)
	return agentPresence, state, category, needsUserAction
}

func hasRunningHint(reasonCode, lastEventType string) bool {
	reason := normalizeSignalToken(reasonCode)
	eventType := normalizeSignalToken(lastEventType)
	if reason == "" && eventType == "" {
		return false
	}
	if hasAttentionHint(reason) || hasAttentionHint(eventType) {
		return false
	}
	if hasIdleLikeHint(reason) || hasIdleLikeHint(eventType) {
		return false
	}
	for _, token := range []string{
		"running",
		"active",
		"working",
		"in_progress",
		"progress",
		"streaming",
		"task_started",
		"session_started",
		"agent_turn_started",
		"hook_start",
		"wrapper_start",
	} {
		if strings.Contains(reason, token) || strings.Contains(eventType, token) {
			return true
		}
	}
	return false
}

func hasIdleOrCompletionHint(reasonCode, lastEventType string) bool {
	reason := normalizeSignalToken(reasonCode)
	eventType := normalizeSignalToken(lastEventType)
	return hasIdleLikeHint(reason) || hasIdleLikeHint(eventType)
}

func hasIdleLikeHint(value string) bool {
	if value == "" {
		return false
	}
	for _, token := range []string{
		"idle",
		"complete",
		"completed",
		"finished",
		"exit",
		"stopped",
		"done",
	} {
		if strings.Contains(value, token) {
			return true
		}
	}
	return false
}

func hasAttentionHint(value string) bool {
	if value == "" {
		return false
	}
	for _, token := range []string{
		"waiting_input",
		"needs_input",
		"input_requested",
		"waiting_approval",
		"approval_required",
		"approval_requested",
		"error",
		"failed",
		"panic",
	} {
		if strings.Contains(value, token) {
			return true
		}
	}
	return false
}

func normalizeSignalToken(value string) string {
	normalized := strings.ToLower(strings.TrimSpace(value))
	if normalized == "" {
		return ""
	}
	replacer := strings.NewReplacer(
		".", "_",
		"-", "_",
		":", "_",
		" ", "_",
	)
	return replacer.Replace(normalized)
}

func isCompletionLikeEventType(eventType string) bool {
	normalized := strings.ToLower(strings.TrimSpace(eventType))
	if normalized == "" {
		return false
	}
	if strings.Contains(normalized, "input") || strings.Contains(normalized, "approval") {
		return false
	}
	return strings.Contains(normalized, "complete") ||
		strings.Contains(normalized, "finished") ||
		strings.Contains(normalized, "exit")
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

func (s *Server) capturePaneSnapshotWithCursor(
	ctx context.Context,
	tg model.Target,
	paneID string,
	lines int,
) (string, *int, *int, *int, *int, error) {
	result, runErr := s.executor.Run(
		ctx,
		tg,
		target.BuildTmuxCommand(
			"capture-pane", "-t", paneID, "-p", "-e", "-S", fmt.Sprintf("-%d", lines),
			";",
			"display-message", "-p", "-t", paneID,
			terminalCursorMarkerPrefix+"#{cursor_x},#{cursor_y},#{pane_width},#{pane_height}",
		),
	)
	if runErr != nil {
		return "", nil, nil, nil, nil, runErr
	}
	content, cursorX, cursorY, paneCols, paneRows := parseTerminalSnapshotWithCursor(result.Output)
	return content, cursorX, cursorY, paneCols, paneRows, nil
}

func parseTerminalSnapshotWithCursor(raw string) (string, *int, *int, *int, *int) {
	markerPos := strings.LastIndex(raw, terminalCursorMarkerPrefix)
	if markerPos < 0 {
		return raw, nil, nil, nil, nil
	}
	cursorLine := raw[markerPos:]
	if idx := strings.IndexByte(cursorLine, '\n'); idx >= 0 {
		cursorLine = cursorLine[:idx]
	}
	payload := strings.TrimSpace(strings.TrimPrefix(cursorLine, terminalCursorMarkerPrefix))
	parts := strings.Split(payload, ",")
	if len(parts) != 4 {
		return raw, nil, nil, nil, nil
	}
	x, errX := strconv.Atoi(strings.TrimSpace(parts[0]))
	y, errY := strconv.Atoi(strings.TrimSpace(parts[1]))
	cols, errCols := strconv.Atoi(strings.TrimSpace(parts[2]))
	rows, errRows := strconv.Atoi(strings.TrimSpace(parts[3]))
	if errX != nil || errY != nil || errCols != nil || errRows != nil {
		return raw, nil, nil, nil, nil
	}
	content := normalizeCapturedSnapshotContent(raw[:markerPos])
	return content, intPtr(x), intPtr(y), intPtr(cols), intPtr(rows)
}

func normalizeCapturedSnapshotContent(content string) string {
	normalized := strings.ReplaceAll(content, "\r\n", "\n")
	normalized = strings.ReplaceAll(normalized, "\r", "\n")
	// tmux capture-pane output includes a trailing newline separator.
	// Keep pane rows stable by removing exactly one terminal newline.
	if strings.HasSuffix(normalized, "\n") {
		normalized = normalized[:len(normalized)-1]
	}
	return normalized
}

func trimSnapshotToVisibleRows(content string, paneRows *int) string {
	if content == "" {
		return content
	}
	normalized := strings.ReplaceAll(content, "\r\n", "\n")
	normalized = strings.ReplaceAll(normalized, "\r", "\n")
	if strings.HasSuffix(normalized, "\n") {
		normalized = normalized[:len(normalized)-1]
	}
	if paneRows == nil || *paneRows <= 0 {
		return normalized
	}
	lines := strings.Split(normalized, "\n")
	if len(lines) <= *paneRows {
		return normalized
	}
	start := len(lines) - *paneRows
	return strings.Join(lines[start:], "\n")
}

func intPtr(v int) *int {
	return &v
}

func decodeLiteralSendKeysText(decoded []byte) (string, bool) {
	if len(decoded) == 0 || !utf8.Valid(decoded) {
		return "", false
	}
	text := string(decoded)
	for _, r := range text {
		if r == '\x00' || r == '\x1b' {
			return "", false
		}
		if unicode.IsControl(r) && r != '\n' && r != '\r' && r != '\t' {
			return "", false
		}
	}
	return text, true
}

func deriveTerminalDelta(previous, current string) (string, bool) {
	if previous == "" {
		return current, true
	}
	if current == previous {
		return "", true
	}
	if strings.HasPrefix(current, previous) {
		return current[len(previous):], true
	}
	overlap := longestSuffixPrefixOverlap(previous, current)
	if overlap > 0 {
		return current[overlap:], true
	}
	return "", false
}

func longestSuffixPrefixOverlap(previous, current string) int {
	max := len(previous)
	if len(current) < max {
		max = len(current)
	}
	for n := max; n > 0; n-- {
		if previous[len(previous)-n:] == current[:n] {
			return n
		}
	}
	return 0
}

func clipTerminalStateContent(content string) string {
	if len(content) <= maxTerminalStateBytes {
		return content
	}
	return content[len(content)-maxTerminalStateBytes:]
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
