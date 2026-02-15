package daemon

import (
	"context"
	"encoding/json"
	"testing"
	"time"
)

func TestParseCodexThreadListHintMatchesWorkspacePath(t *testing.T) {
	resp := map[string]any{
		"result": map[string]any{
			"data": []any{
				map[string]any{
					"id":         "thread-1",
					"cwd":        "/tmp/other",
					"preview":    "other thread",
					"updated_at": "2026-02-15T00:00:00Z",
				},
				map[string]any{
					"id":         "thread-2",
					"cwd":        "/tmp/work",
					"preview":    "fix dashboard labels",
					"updated_at": "2026-02-15T01:00:00Z",
				},
			},
		},
	}

	hint, ok := parseCodexThreadListHint(resp, "/tmp/work")
	if !ok {
		t.Fatalf("expected hint")
	}
	if hint.label != "fix dashboard labels" {
		t.Fatalf("unexpected label: %+v", hint.label)
	}
	if hint.at.IsZero() {
		t.Fatalf("expected updated timestamp")
	}
}

func TestParseCodexThreadListHintPrefersMostRecentAcrossDataAndThreads(t *testing.T) {
	resp := map[string]any{
		"result": map[string]any{
			"data": []any{
				map[string]any{
					"id":         "thread-1",
					"cwd":        "/tmp/work",
					"preview":    "old preview",
					"updated_at": "2026-02-15T00:00:00Z",
				},
			},
			"threads": []any{
				map[string]any{
					"id":         "thread-1",
					"cwd":        "/tmp/work",
					"preview":    "new preview from threads",
					"updated_at": "2026-02-15T01:00:00Z",
				},
			},
		},
	}

	hint, ok := parseCodexThreadListHint(resp, "/tmp/work")
	if !ok {
		t.Fatalf("expected hint")
	}
	if hint.label != "new preview from threads" {
		t.Fatalf("expected latest preview, got %q", hint.label)
	}
	if hint.at.IsZero() {
		t.Fatalf("expected updated timestamp")
	}
}

func TestParseCodexThreadTimestampHandlesStringAndEpoch(t *testing.T) {
	cases := []struct {
		name string
		raw  any
	}{
		{name: "rfc3339", raw: "2026-02-15T01:00:00Z"},
		{name: "seconds", raw: float64(1_761_147_200)},
		{name: "milliseconds", raw: float64(1_761_147_200_000)},
		{name: "json-number", raw: json.Number("1761147200")},
	}
	for _, tc := range cases {
		tc := tc
		t.Run(tc.name, func(t *testing.T) {
			got, ok := parseCodexThreadTimestamp(tc.raw)
			if !ok {
				t.Fatalf("expected parsed timestamp for %v", tc.raw)
			}
			if got.IsZero() {
				t.Fatalf("expected non-zero timestamp for %v", tc.raw)
			}
		})
	}
}

func TestCodexSessionEnricherCachesFetcherResult(t *testing.T) {
	ctx := context.Background()
	callCount := 0
	enricher := newCodexSessionEnricher(func(_ context.Context, workspacePath string) (codexThreadHint, error) {
		callCount++
		return codexThreadHint{
			label: workspacePath + " hint",
			at:    time.Now().UTC(),
		}, nil
	})
	enricher.ttl = 1 * time.Minute

	paths := []string{"/tmp/work", "/tmp/work"}
	hints := enricher.GetMany(ctx, paths)
	if len(hints) != 1 {
		t.Fatalf("expected single deduped hint, got %+v", hints)
	}
	if callCount != 1 {
		t.Fatalf("expected one fetch call, got %d", callCount)
	}

	hints = enricher.GetMany(ctx, paths)
	if len(hints) != 1 {
		t.Fatalf("expected cached hint, got %+v", hints)
	}
	if callCount != 1 {
		t.Fatalf("expected cached call count=1, got %d", callCount)
	}
}
