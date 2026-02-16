package appclient

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"strings"
	"time"

	"github.com/g960059/agtmux/internal/api"
)

type Client struct {
	baseURL      string
	client       *http.Client
	unaryTimeout time.Duration
}

const (
	watchScannerInitialBuffer = 64 * 1024
	watchScannerMaxBuffer     = 10 * 1024 * 1024
	defaultUnaryTimeout       = 10 * time.Second
)

func New(socketPath string) *Client {
	transport := &http.Transport{
		DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
			var d net.Dialer
			return d.DialContext(ctx, "unix", socketPath)
		},
	}
	return NewWithClient("http://unix", &http.Client{Transport: transport})
}

func NewWithClient(baseURL string, client *http.Client) *Client {
	if client == nil {
		client = &http.Client{}
	}
	return &Client{
		baseURL:      strings.TrimRight(baseURL, "/"),
		client:       client,
		unaryTimeout: defaultUnaryTimeout,
	}
}

func (c *Client) WithUnaryTimeout(timeout time.Duration) *Client {
	if c == nil {
		return nil
	}
	clone := *c
	clone.unaryTimeout = timeout
	return &clone
}

type WatchOptions struct {
	Scope  string
	Target string
	Cursor string
}

type WatchLoopOptions struct {
	Scope           string
	Target          string
	Cursor          string
	PollInterval    time.Duration
	RetryMinBackoff time.Duration
	RetryMaxBackoff time.Duration
	Once            bool
}

type ListOptions struct {
	Target string
}

type RequestError struct {
	StatusCode int
	Code       string
	Message    string
}

var ErrWatchPayloadInvalid = errors.New("watch payload invalid")

func (e *RequestError) Error() string {
	if e == nil {
		return ""
	}
	code := strings.TrimSpace(e.Code)
	message := strings.TrimSpace(e.Message)
	if code != "" && message != "" {
		return fmt.Sprintf("%s: %s", code, message)
	}
	if code != "" {
		if e.StatusCode > 0 {
			return fmt.Sprintf("http %d: %s", e.StatusCode, code)
		}
		return code
	}
	if message != "" {
		if e.StatusCode > 0 {
			return fmt.Sprintf("http %d: %s", e.StatusCode, message)
		}
		return message
	}
	if e.StatusCode > 0 {
		return fmt.Sprintf("http %d", e.StatusCode)
	}
	return "http error"
}

func (e *RequestError) Retryable() bool {
	if e == nil {
		return false
	}
	if e.StatusCode == http.StatusTooManyRequests || e.StatusCode == http.StatusRequestTimeout {
		return true
	}
	return e.StatusCode >= 500
}

func (c *Client) WatchOnce(ctx context.Context, opts WatchOptions) ([]api.WatchLine, string, error) {
	scope := strings.TrimSpace(opts.Scope)
	if scope == "" {
		scope = "panes"
	}
	query := url.Values{}
	query.Set("scope", scope)
	if target := strings.TrimSpace(opts.Target); target != "" {
		query.Set("target", target)
	}
	if cursor := strings.TrimSpace(opts.Cursor); cursor != "" {
		query.Set("cursor", cursor)
	}
	body, err := c.request(ctx, http.MethodGet, "/v1/watch", query, nil, true)
	if err != nil {
		return nil, "", err
	}
	lines, nextCursor, err := decodeWatchLines(body)
	if err != nil {
		return nil, "", err
	}
	return lines, nextCursor, nil
}

func (c *Client) WatchLoop(ctx context.Context, opts WatchLoopOptions, onLine func(api.WatchLine) error) error {
	scope := strings.TrimSpace(opts.Scope)
	if scope == "" {
		scope = "panes"
	}
	pollInterval := opts.PollInterval
	if pollInterval <= 0 {
		pollInterval = 2 * time.Second
	}
	minBackoff := opts.RetryMinBackoff
	if minBackoff <= 0 {
		minBackoff = 250 * time.Millisecond
	}
	maxBackoff := opts.RetryMaxBackoff
	if maxBackoff <= 0 {
		maxBackoff = 4 * time.Second
	}
	if maxBackoff < minBackoff {
		maxBackoff = minBackoff
	}
	cursor := strings.TrimSpace(opts.Cursor)
	backoff := minBackoff

	for {
		if err := ctx.Err(); err != nil {
			return err
		}

		lines, nextCursor, err := c.WatchOnce(ctx, WatchOptions{
			Scope:  scope,
			Target: opts.Target,
			Cursor: cursor,
		})
		if err != nil {
			if opts.Once {
				return err
			}
			if errors.Is(err, ErrWatchPayloadInvalid) {
				return err
			}
			var reqErr *RequestError
			if errors.As(err, &reqErr) && !reqErr.Retryable() {
				return err
			}
			if waitErr := sleepWithContext(ctx, backoff); waitErr != nil {
				return waitErr
			}
			backoff *= 2
			if backoff > maxBackoff {
				backoff = maxBackoff
			}
			continue
		}

		backoff = minBackoff
		if nextCursor != "" {
			cursor = nextCursor
		}
		for _, line := range lines {
			if onLine == nil {
				continue
			}
			if err := onLine(line); err != nil {
				return err
			}
		}
		if opts.Once {
			return nil
		}
		if err := sleepWithContext(ctx, pollInterval); err != nil {
			return err
		}
	}
}

func (c *Client) ListPanes(ctx context.Context, opts ListOptions) (api.ListEnvelope[api.PaneItem], error) {
	return listScope[api.PaneItem](ctx, c, "/v1/panes", opts)
}

func (c *Client) ListWindows(ctx context.Context, opts ListOptions) (api.ListEnvelope[api.WindowItem], error) {
	return listScope[api.WindowItem](ctx, c, "/v1/windows", opts)
}

func (c *Client) ListSessions(ctx context.Context, opts ListOptions) (api.ListEnvelope[api.SessionItem], error) {
	return listScope[api.SessionItem](ctx, c, "/v1/sessions", opts)
}

func (c *Client) ListTargets(ctx context.Context) (api.TargetsEnvelope, error) {
	body, err := c.request(ctx, http.MethodGet, "/v1/targets", nil, nil, false)
	if err != nil {
		return api.TargetsEnvelope{}, err
	}
	var env api.TargetsEnvelope
	if err := json.Unmarshal(body, &env); err != nil {
		return api.TargetsEnvelope{}, fmt.Errorf("decode targets envelope: %w", err)
	}
	return env, nil
}

type CreateTargetRequest struct {
	Name          string `json:"name"`
	Kind          string `json:"kind,omitempty"`
	ConnectionRef string `json:"connection_ref,omitempty"`
	IsDefault     bool   `json:"is_default,omitempty"`
}

func (c *Client) CreateTarget(ctx context.Context, req CreateTargetRequest) (api.TargetsEnvelope, error) {
	body, err := c.request(ctx, http.MethodPost, "/v1/targets", nil, req, false)
	if err != nil {
		return api.TargetsEnvelope{}, err
	}
	var env api.TargetsEnvelope
	if err := json.Unmarshal(body, &env); err != nil {
		return api.TargetsEnvelope{}, fmt.Errorf("decode targets envelope: %w", err)
	}
	return env, nil
}

func (c *Client) ConnectTarget(ctx context.Context, targetName string) (api.TargetsEnvelope, error) {
	name := strings.TrimSpace(targetName)
	if name == "" {
		return api.TargetsEnvelope{}, fmt.Errorf("target name is required")
	}
	path := "/v1/targets/" + url.PathEscape(name) + "/connect"
	body, err := c.request(ctx, http.MethodPost, path, nil, nil, false)
	if err != nil {
		return api.TargetsEnvelope{}, err
	}
	var env api.TargetsEnvelope
	if err := json.Unmarshal(body, &env); err != nil {
		return api.TargetsEnvelope{}, fmt.Errorf("decode targets envelope: %w", err)
	}
	return env, nil
}

func (c *Client) DeleteTarget(ctx context.Context, targetName string) error {
	name := strings.TrimSpace(targetName)
	if name == "" {
		return fmt.Errorf("target name is required")
	}
	path := "/v1/targets/" + url.PathEscape(name)
	_, err := c.request(ctx, http.MethodDelete, path, nil, nil, false)
	return err
}

type TerminalReadRequest struct {
	Target string `json:"target"`
	PaneID string `json:"pane_id"`
	Cursor string `json:"cursor,omitempty"`
	Lines  int    `json:"lines,omitempty"`
}

type TerminalResizeRequest struct {
	Target string `json:"target"`
	PaneID string `json:"pane_id"`
	Cols   int    `json:"cols"`
	Rows   int    `json:"rows"`
}

type AttachRequest struct {
	RequestRef      string `json:"request_ref"`
	Target          string `json:"target"`
	PaneID          string `json:"pane_id"`
	IfRuntime       string `json:"if_runtime,omitempty"`
	IfState         string `json:"if_state,omitempty"`
	IfUpdatedWithin string `json:"if_updated_within,omitempty"`
	ForceStale      bool   `json:"force_stale,omitempty"`
}

type SendRequest struct {
	RequestRef      string `json:"request_ref"`
	Target          string `json:"target"`
	PaneID          string `json:"pane_id"`
	Text            string `json:"text,omitempty"`
	Key             string `json:"key,omitempty"`
	Enter           bool   `json:"enter,omitempty"`
	Paste           bool   `json:"paste,omitempty"`
	IfRuntime       string `json:"if_runtime,omitempty"`
	IfState         string `json:"if_state,omitempty"`
	IfUpdatedWithin string `json:"if_updated_within,omitempty"`
	ForceStale      bool   `json:"force_stale,omitempty"`
}

type ViewOutputRequest struct {
	RequestRef      string `json:"request_ref"`
	Target          string `json:"target"`
	PaneID          string `json:"pane_id"`
	Lines           int    `json:"lines,omitempty"`
	IfRuntime       string `json:"if_runtime,omitempty"`
	IfState         string `json:"if_state,omitempty"`
	IfUpdatedWithin string `json:"if_updated_within,omitempty"`
	ForceStale      bool   `json:"force_stale,omitempty"`
}

type KillRequest struct {
	RequestRef      string `json:"request_ref"`
	Target          string `json:"target"`
	PaneID          string `json:"pane_id"`
	Mode            string `json:"mode,omitempty"`
	Signal          string `json:"signal,omitempty"`
	IfRuntime       string `json:"if_runtime,omitempty"`
	IfState         string `json:"if_state,omitempty"`
	IfUpdatedWithin string `json:"if_updated_within,omitempty"`
	ForceStale      bool   `json:"force_stale,omitempty"`
}

func (c *Client) Attach(ctx context.Context, req AttachRequest) (api.ActionResponse, error) {
	return c.postAction(ctx, "/v1/actions/attach", req)
}

func (c *Client) Send(ctx context.Context, req SendRequest) (api.ActionResponse, error) {
	return c.postAction(ctx, "/v1/actions/send", req)
}

func (c *Client) ViewOutput(ctx context.Context, req ViewOutputRequest) (api.ActionResponse, error) {
	return c.postAction(ctx, "/v1/actions/view-output", req)
}

func (c *Client) Kill(ctx context.Context, req KillRequest) (api.ActionResponse, error) {
	return c.postAction(ctx, "/v1/actions/kill", req)
}

func (c *Client) ListActionEvents(ctx context.Context, actionID string) (api.ActionEventsEnvelope, error) {
	id := strings.TrimSpace(actionID)
	if id == "" {
		return api.ActionEventsEnvelope{}, fmt.Errorf("action id is required")
	}
	path := "/v1/actions/" + url.PathEscape(id) + "/events"
	body, err := c.request(ctx, http.MethodGet, path, nil, nil, false)
	if err != nil {
		return api.ActionEventsEnvelope{}, err
	}
	var env api.ActionEventsEnvelope
	if err := json.Unmarshal(body, &env); err != nil {
		return api.ActionEventsEnvelope{}, fmt.Errorf("decode action events envelope: %w", err)
	}
	return env, nil
}

func (c *Client) ListCapabilities(ctx context.Context) (api.CapabilitiesEnvelope, error) {
	body, err := c.request(ctx, http.MethodGet, "/v1/capabilities", nil, nil, false)
	if err != nil {
		return api.CapabilitiesEnvelope{}, err
	}
	var env api.CapabilitiesEnvelope
	if err := json.Unmarshal(body, &env); err != nil {
		return api.CapabilitiesEnvelope{}, fmt.Errorf("decode capabilities envelope: %w", err)
	}
	return env, nil
}

func (c *Client) TerminalRead(ctx context.Context, req TerminalReadRequest) (api.TerminalReadEnvelope, error) {
	body, err := c.request(ctx, http.MethodPost, "/v1/terminal/read", nil, req, false)
	if err != nil {
		return api.TerminalReadEnvelope{}, err
	}
	var env api.TerminalReadEnvelope
	if err := json.Unmarshal(body, &env); err != nil {
		return api.TerminalReadEnvelope{}, fmt.Errorf("decode terminal read envelope: %w", err)
	}
	return env, nil
}

func (c *Client) TerminalResize(ctx context.Context, req TerminalResizeRequest) (api.TerminalResizeResponse, error) {
	body, err := c.request(ctx, http.MethodPost, "/v1/terminal/resize", nil, req, false)
	if err != nil {
		return api.TerminalResizeResponse{}, err
	}
	var resp api.TerminalResizeResponse
	if err := json.Unmarshal(body, &resp); err != nil {
		return api.TerminalResizeResponse{}, fmt.Errorf("decode terminal resize response: %w", err)
	}
	return resp, nil
}

func (c *Client) ListAdapters(ctx context.Context, enabled *bool) (api.AdaptersEnvelope, error) {
	query := url.Values{}
	if enabled != nil {
		query.Set("enabled", fmt.Sprintf("%t", *enabled))
	}
	body, err := c.request(ctx, http.MethodGet, "/v1/adapters", query, nil, false)
	if err != nil {
		return api.AdaptersEnvelope{}, err
	}
	var env api.AdaptersEnvelope
	if err := json.Unmarshal(body, &env); err != nil {
		return api.AdaptersEnvelope{}, fmt.Errorf("decode adapters envelope: %w", err)
	}
	return env, nil
}

func (c *Client) SetAdapterEnabled(ctx context.Context, adapterName string, enabled bool) (api.AdaptersEnvelope, error) {
	name := strings.TrimSpace(adapterName)
	if name == "" {
		return api.AdaptersEnvelope{}, fmt.Errorf("adapter name is required")
	}
	action := "disable"
	if enabled {
		action = "enable"
	}
	path := "/v1/adapters/" + url.PathEscape(name) + "/" + action
	body, err := c.request(ctx, http.MethodPost, path, nil, nil, false)
	if err != nil {
		return api.AdaptersEnvelope{}, err
	}
	var env api.AdaptersEnvelope
	if err := json.Unmarshal(body, &env); err != nil {
		return api.AdaptersEnvelope{}, fmt.Errorf("decode adapters envelope: %w", err)
	}
	return env, nil
}

func listScope[T any](ctx context.Context, c *Client, path string, opts ListOptions) (api.ListEnvelope[T], error) {
	query := url.Values{}
	if target := strings.TrimSpace(opts.Target); target != "" {
		query.Set("target", target)
	}
	body, err := c.request(ctx, http.MethodGet, path, query, nil, false)
	if err != nil {
		return api.ListEnvelope[T]{}, err
	}
	var env api.ListEnvelope[T]
	if err := json.Unmarshal(body, &env); err != nil {
		return api.ListEnvelope[T]{}, fmt.Errorf("decode list envelope: %w", err)
	}
	return env, nil
}

func (c *Client) postAction(ctx context.Context, path string, req any) (api.ActionResponse, error) {
	body, err := c.request(ctx, http.MethodPost, path, nil, req, false)
	if err != nil {
		return api.ActionResponse{}, err
	}
	var resp api.ActionResponse
	if err := json.Unmarshal(body, &resp); err != nil {
		return api.ActionResponse{}, fmt.Errorf("decode action response: %w", err)
	}
	return resp, nil
}

func (c *Client) request(ctx context.Context, method, path string, query url.Values, body any, longLived bool) ([]byte, error) {
	u := c.baseURL + path
	if len(query) > 0 {
		u += "?" + query.Encode()
	}
	reqCtx := ctx
	if !longLived && c.unaryTimeout > 0 {
		if deadline, ok := ctx.Deadline(); !ok || time.Until(deadline) > c.unaryTimeout {
			var cancel context.CancelFunc
			reqCtx, cancel = context.WithTimeout(ctx, c.unaryTimeout)
			defer cancel()
		}
	}
	var reqBody io.Reader
	if body != nil {
		buf := &bytes.Buffer{}
		if err := json.NewEncoder(buf).Encode(body); err != nil {
			return nil, fmt.Errorf("encode request body: %w", err)
		}
		reqBody = buf
	}
	req, err := http.NewRequestWithContext(reqCtx, method, u, reqBody)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Accept", "application/json")
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := c.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close() //nolint:errcheck

	payload, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}
	if resp.StatusCode >= 400 {
		var er api.ErrorResponse
		if err := json.Unmarshal(payload, &er); err == nil && er.Error.Code != "" {
			return nil, &RequestError{
				StatusCode: resp.StatusCode,
				Code:       er.Error.Code,
				Message:    er.Error.Message,
			}
		}
		return nil, &RequestError{
			StatusCode: resp.StatusCode,
			Code:       fmt.Sprintf("HTTP_%d", resp.StatusCode),
			Message:    strings.TrimSpace(string(payload)),
		}
	}
	return payload, nil
}

func decodeWatchLines(body []byte) ([]api.WatchLine, string, error) {
	scanner := bufio.NewScanner(bytes.NewReader(body))
	scanner.Buffer(make([]byte, watchScannerInitialBuffer), watchScannerMaxBuffer)
	lines := make([]api.WatchLine, 0)
	nextCursor := ""
	for scanner.Scan() {
		raw := strings.TrimSpace(scanner.Text())
		if raw == "" {
			continue
		}
		var line api.WatchLine
		if err := json.Unmarshal([]byte(raw), &line); err != nil {
			return nil, "", fmt.Errorf("%w: decode watch line: %v", ErrWatchPayloadInvalid, err)
		}
		if strings.TrimSpace(line.Cursor) != "" {
			nextCursor = strings.TrimSpace(line.Cursor)
		}
		lines = append(lines, line)
	}
	if err := scanner.Err(); err != nil {
		return nil, "", fmt.Errorf("%w: scan watch lines: %v", ErrWatchPayloadInvalid, err)
	}
	return lines, nextCursor, nil
}

func sleepWithContext(ctx context.Context, wait time.Duration) error {
	if wait <= 0 {
		return nil
	}
	timer := time.NewTimer(wait)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-timer.C:
		return nil
	}
}
