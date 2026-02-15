package daemon

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	codexSessionCacheTTL       = 45 * time.Second
	codexSessionRetryBackoff   = 20 * time.Second
	codexAppServerRequestLimit = 8 * time.Second
)

type codexThreadHint struct {
	label string
	at    time.Time
}

type codexSessionCacheEntry struct {
	hint       codexThreadHint
	fetchedAt  time.Time
	nextRetry  time.Time
	hasValue   bool
	lastErrMsg string
}

type codexSessionFetcher func(ctx context.Context, workspacePath string) (codexThreadHint, error)

type codexSessionEnricher struct {
	mu      sync.Mutex
	cache   map[string]codexSessionCacheEntry
	ttl     time.Duration
	backoff time.Duration
	fetch   codexSessionFetcher
}

func newCodexSessionEnricher(fetch codexSessionFetcher) *codexSessionEnricher {
	if fetch == nil {
		fetch = fetchCodexThreadHint
	}
	return &codexSessionEnricher{
		cache:   map[string]codexSessionCacheEntry{},
		ttl:     codexSessionCacheTTL,
		backoff: codexSessionRetryBackoff,
		fetch:   fetch,
	}
}

func (e *codexSessionEnricher) GetMany(ctx context.Context, workspacePaths []string) map[string]codexThreadHint {
	out := map[string]codexThreadHint{}
	seen := map[string]struct{}{}
	for _, raw := range workspacePaths {
		key := normalizeCodexWorkspacePath(raw)
		if key == "" {
			continue
		}
		if _, ok := seen[key]; ok {
			continue
		}
		seen[key] = struct{}{}
		hint, ok := e.get(ctx, key)
		if ok && hint.label != "" {
			out[key] = hint
		}
	}
	return out
}

func (e *codexSessionEnricher) get(ctx context.Context, key string) (codexThreadHint, bool) {
	now := time.Now().UTC()

	e.mu.Lock()
	entry, exists := e.cache[key]
	if exists && entry.hasValue && now.Sub(entry.fetchedAt) < e.ttl {
		hint := entry.hint
		e.mu.Unlock()
		return hint, true
	}
	if exists && now.Before(entry.nextRetry) {
		hint := entry.hint
		hasValue := entry.hasValue && hint.label != ""
		e.mu.Unlock()
		if hasValue {
			return hint, true
		}
		return codexThreadHint{}, false
	}
	e.mu.Unlock()

	fetchCtx, cancel := context.WithTimeout(ctx, codexAppServerRequestLimit)
	defer cancel()

	hint, err := e.fetch(fetchCtx, key)
	e.mu.Lock()
	defer e.mu.Unlock()
	entry = e.cache[key]
	if err != nil {
		entry.nextRetry = now.Add(e.backoff)
		entry.lastErrMsg = err.Error()
		e.cache[key] = entry
		if entry.hasValue && entry.hint.label != "" {
			return entry.hint, true
		}
		return codexThreadHint{}, false
	}
	entry.lastErrMsg = ""
	entry.nextRetry = time.Time{}
	if hint.label == "" {
		entry.nextRetry = now.Add(e.backoff)
		e.cache[key] = entry
		if entry.hasValue && entry.hint.label != "" {
			return entry.hint, true
		}
		return codexThreadHint{}, false
	}

	entry.hint = hint
	entry.fetchedAt = now
	entry.hasValue = true
	e.cache[key] = entry
	return hint, true
}

func fetchCodexThreadHint(ctx context.Context, workspacePath string) (codexThreadHint, error) {
	cmd := exec.CommandContext(ctx, "codex", "app-server")
	cmd.Dir = workspacePath
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return codexThreadHint{}, fmt.Errorf("codex app-server stdin: %w", err)
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return codexThreadHint{}, fmt.Errorf("codex app-server stdout: %w", err)
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		return codexThreadHint{}, fmt.Errorf("codex app-server stderr: %w", err)
	}
	if err := cmd.Start(); err != nil {
		return codexThreadHint{}, fmt.Errorf("start codex app-server: %w", err)
	}
	defer func() {
		_ = stdin.Close()
		if cmd.Process != nil {
			_ = cmd.Process.Kill()
		}
		_ = cmd.Wait()
	}()

	var stderrBuf bytes.Buffer
	go func() {
		_, _ = io.Copy(&stderrBuf, stderr)
	}()

	encoder := json.NewEncoder(stdin)
	decoder := json.NewDecoder(bufio.NewReader(stdout))

	initReq := map[string]any{
		"id":     1,
		"method": "initialize",
		"params": map[string]any{
			"clientInfo": map[string]any{
				"name":    "agtmux",
				"title":   "AGTMUX",
				"version": "v1",
			},
			"capabilities": map[string]any{
				"experimentalApi": true,
			},
		},
	}
	if err := encoder.Encode(initReq); err != nil {
		return codexThreadHint{}, fmt.Errorf("write initialize: %w", err)
	}
	if _, err := waitCodexAppServerResponse(ctx, decoder, 1); err != nil {
		return codexThreadHint{}, formatCodexAppServerErr("initialize failed", err, stderrBuf.String())
	}
	if err := encoder.Encode(map[string]any{"method": "initialized"}); err != nil {
		return codexThreadHint{}, fmt.Errorf("write initialized: %w", err)
	}

	listReq := map[string]any{
		"id":     2,
		"method": "thread/list",
		"params": map[string]any{
			"limit":       50,
			"sortKey":     "updated_at",
			"sourceKinds": []string{"cli", "vscode", "subAgentThreadSpawn"},
		},
	}
	if err := encoder.Encode(listReq); err != nil {
		return codexThreadHint{}, fmt.Errorf("write thread/list: %w", err)
	}
	resp, err := waitCodexAppServerResponse(ctx, decoder, 2)
	if err != nil {
		return codexThreadHint{}, formatCodexAppServerErr("thread/list failed", err, stderrBuf.String())
	}

	hint, ok := parseCodexThreadListHint(resp, workspacePath)
	if !ok {
		return codexThreadHint{}, nil
	}
	return hint, nil
}

func waitCodexAppServerResponse(ctx context.Context, decoder *json.Decoder, requestID int64) (map[string]any, error) {
	for {
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		var msg map[string]any
		if err := decoder.Decode(&msg); err != nil {
			return nil, err
		}
		rawID, ok := msg["id"]
		if !ok {
			continue
		}
		id, ok := jsonIDToInt64(rawID)
		if !ok || id != requestID {
			continue
		}
		if rawErr, hasErr := msg["error"]; hasErr && rawErr != nil {
			return nil, fmt.Errorf("app-server error: %v", rawErr)
		}
		return msg, nil
	}
}

func parseCodexThreadListHint(resp map[string]any, workspacePath string) (codexThreadHint, bool) {
	result, _ := resp["result"].(map[string]any)
	if result == nil {
		return codexThreadHint{}, false
	}
	items := codexThreadItems(result)
	if len(items) == 0 {
		return codexThreadHint{}, false
	}

	pathKey := normalizeCodexWorkspacePath(workspacePath)
	bestHint := codexThreadHint{}
	bestFound := false
	bestRank := int64(-1)
	for _, item := range items {
		thread, _ := item.(map[string]any)
		if thread == nil {
			continue
		}
		threadPath := normalizeCodexWorkspacePath(asString(thread["cwd"]))
		if pathKey != "" && threadPath != "" && threadPath != pathKey {
			continue
		}
		if pathKey != "" && threadPath == "" {
			continue
		}

		label := strings.TrimSpace(firstNonEmpty(
			asString(thread["preview"]),
			asString(thread["name"]),
			asString(thread["title"]),
			asString(thread["thread_name"]),
			asString(thread["id"]),
		))
		if label == "" {
			continue
		}

		hint := codexThreadHint{label: compactPreview(label, 72)}
		if ts, ok := parseCodexThreadTimestamp(thread["updatedAt"]); ok {
			hint.at = ts.UTC()
		} else if ts, ok := parseCodexThreadTimestamp(thread["updated_at"]); ok {
			hint.at = ts.UTC()
		} else if ts, ok := parseCodexThreadTimestamp(thread["createdAt"]); ok {
			hint.at = ts.UTC()
		} else if ts, ok := parseCodexThreadTimestamp(thread["created_at"]); ok {
			hint.at = ts.UTC()
		}
		rank := int64(0)
		if !hint.at.IsZero() {
			rank = hint.at.Unix()
		}
		if !bestFound || rank > bestRank {
			bestFound = true
			bestRank = rank
			bestHint = hint
		}
	}
	if bestFound {
		return bestHint, true
	}
	return codexThreadHint{}, false
}

func codexThreadItems(result map[string]any) []any {
	merged := make([]any, 0, 32)
	if data, ok := result["data"].([]any); ok {
		merged = append(merged, data...)
	}
	if threads, ok := result["threads"].([]any); ok {
		merged = append(merged, threads...)
	}
	return merged
}

func parseCodexThreadTimestamp(raw any) (time.Time, bool) {
	switch v := raw.(type) {
	case float64:
		return parseEpochNumber(v)
	case int64:
		return parseEpochNumber(float64(v))
	case int:
		return parseEpochNumber(float64(v))
	case json.Number:
		n, err := v.Float64()
		if err != nil {
			return time.Time{}, false
		}
		return parseEpochNumber(n)
	case string:
		s := strings.TrimSpace(v)
		if s == "" {
			return time.Time{}, false
		}
		if n, err := strconv.ParseFloat(s, 64); err == nil {
			return parseEpochNumber(n)
		}
		if parsed, err := time.Parse(time.RFC3339Nano, s); err == nil {
			return parsed, true
		}
		if parsed, err := time.Parse(time.RFC3339, s); err == nil {
			return parsed, true
		}
		return time.Time{}, false
	default:
		return time.Time{}, false
	}
}

func parseEpochNumber(raw float64) (time.Time, bool) {
	if raw <= 0 {
		return time.Time{}, false
	}
	sec := raw
	if raw > 1_000_000_000_000 {
		sec = raw / 1000
	}
	return time.Unix(int64(sec), 0).UTC(), true
}

func jsonIDToInt64(raw any) (int64, bool) {
	switch v := raw.(type) {
	case float64:
		return int64(v), true
	case int64:
		return v, true
	case int:
		return int64(v), true
	case string:
		n, err := strconv.ParseInt(strings.TrimSpace(v), 10, 64)
		if err != nil {
			return 0, false
		}
		return n, true
	case json.Number:
		n, err := v.Int64()
		if err != nil {
			return 0, false
		}
		return n, true
	default:
		return 0, false
	}
}

func normalizeCodexWorkspacePath(path string) string {
	trimmed := strings.TrimSpace(path)
	if trimmed == "" {
		return ""
	}
	if abs, err := filepath.Abs(trimmed); err == nil {
		trimmed = abs
	}
	return filepath.Clean(trimmed)
}

func firstNonEmpty(values ...string) string {
	for _, value := range values {
		if strings.TrimSpace(value) != "" {
			return value
		}
	}
	return ""
}

func asString(raw any) string {
	switch v := raw.(type) {
	case string:
		return v
	case fmt.Stringer:
		return v.String()
	default:
		return ""
	}
}

func formatCodexAppServerErr(prefix string, err error, stderr string) error {
	stderr = strings.TrimSpace(stderr)
	if stderr == "" {
		return fmt.Errorf("%s: %w", prefix, err)
	}
	if errors.Is(err, context.DeadlineExceeded) || errors.Is(err, context.Canceled) {
		return fmt.Errorf("%s: %w (%s)", prefix, err, compactPreview(stderr, 120))
	}
	return fmt.Errorf("%s: %w (%s)", prefix, err, compactPreview(stderr, 120))
}
