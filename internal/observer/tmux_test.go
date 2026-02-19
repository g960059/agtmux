package observer

import (
	"context"
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/target"
	"github.com/g960059/agtmux/internal/testutil"
)

type sequenceRunner struct {
	outputs []string
	idx     int
}

func (r *sequenceRunner) Run(_ context.Context, _ string, _ ...string) ([]byte, error) {
	if len(r.outputs) == 0 {
		return []byte{}, nil
	}
	if r.idx >= len(r.outputs) {
		return []byte(r.outputs[len(r.outputs)-1]), nil
	}
	out := r.outputs[r.idx]
	r.idx++
	return []byte(out), nil
}

func TestMultiTargetTopologyConvergence(t *testing.T) {
	store, ctx := testutil.NewStore(t)
	now := time.Now().UTC()
	for _, tg := range []model.Target{
		{TargetID: "host", TargetName: "host", Kind: model.TargetKindLocal, Health: model.TargetHealthOK, UpdatedAt: now},
		{TargetID: "vm1", TargetName: "vm1", Kind: model.TargetKindSSH, ConnectionRef: "vm1", Health: model.TargetHealthOK, UpdatedAt: now},
	} {
		if err := store.UpsertTarget(ctx, tg); err != nil {
			t.Fatalf("upsert target: %v", err)
		}
	}

	hostRunner := &sequenceRunner{outputs: []string{
		"%1\ts1\t@1\tw1\tcodex\t123\n%2\ts1\t@1\tw1\tzsh\t124\n",
		"%2\ts1\t@1\tw1\tzsh\t124\n",
	}}
	vmRunner := &sequenceRunner{outputs: []string{
		"%9\ts2\t@3\tw3\tclaude\t900\n",
	}}

	cfg := config.DefaultConfig()
	hostObs := NewTmuxObserver(target.NewExecutorWithRunner(cfg, hostRunner), store)
	vmObs := NewTmuxObserver(target.NewExecutorWithRunner(cfg, vmRunner), store)

	if _, err := hostObs.Collect(ctx, model.Target{TargetID: "host", Kind: model.TargetKindLocal}, now); err != nil {
		t.Fatalf("collect host first: %v", err)
	}
	if _, err := vmObs.Collect(ctx, model.Target{TargetID: "vm1", Kind: model.TargetKindSSH, ConnectionRef: "vm1"}, now); err != nil {
		t.Fatalf("collect vm first: %v", err)
	}
	if _, err := hostObs.Collect(ctx, model.Target{TargetID: "host", Kind: model.TargetKindLocal}, now.Add(time.Second)); err != nil {
		t.Fatalf("collect host second: %v", err)
	}

	var hostCount int
	if err := store.DB().QueryRowContext(ctx, `SELECT COUNT(*) FROM panes WHERE target_id = 'host'`).Scan(&hostCount); err != nil {
		t.Fatalf("count host panes: %v", err)
	}
	if hostCount != 1 {
		t.Fatalf("expected host pane convergence without stale bleed, got %d panes", hostCount)
	}

	var vmCount int
	if err := store.DB().QueryRowContext(ctx, `SELECT COUNT(*) FROM panes WHERE target_id = 'vm1'`).Scan(&vmCount); err != nil {
		t.Fatalf("count vm panes: %v", err)
	}
	if vmCount != 1 {
		t.Fatalf("expected vm panes unchanged, got %d", vmCount)
	}
}

func TestParseListPanesOutputParsesCommandAndPID(t *testing.T) {
	now := time.Now().UTC()
	out := "%1\ts1\t@1\tw1\tcodex\t123\t/dev/ttys001\n"
	panes, err := parseListPanesOutput("t1", out, now)
	if err != nil {
		t.Fatalf("parse list panes: %v", err)
	}
	if len(panes) != 1 {
		t.Fatalf("expected one pane, got %d", len(panes))
	}
	if panes[0].CurrentCmd != "codex" {
		t.Fatalf("expected current cmd codex, got %q", panes[0].CurrentCmd)
	}
	if panes[0].CurrentPID == nil || *panes[0].CurrentPID != 123 {
		t.Fatalf("expected current pid 123, got %+v", panes[0].CurrentPID)
	}
	if panes[0].TTY != "/dev/ttys001" {
		t.Fatalf("expected pane tty to be parsed, got %q", panes[0].TTY)
	}
}

func TestParseListPanesOutputParsesHistoryAndPaneTitle(t *testing.T) {
	now := time.Now().UTC()
	out := "%1\ts1\t@1\tw1\tclaude\t123\t/dev/ttys001\t4567\tReview results output\n"
	panes, err := parseListPanesOutput("t1", out, now)
	if err != nil {
		t.Fatalf("parse list panes: %v", err)
	}
	if len(panes) != 1 {
		t.Fatalf("expected one pane, got %d", len(panes))
	}
	if panes[0].HistoryBytes != 4567 {
		t.Fatalf("expected history bytes 4567, got %d", panes[0].HistoryBytes)
	}
	if panes[0].PaneTitle != "Review results output" {
		t.Fatalf("expected pane title parsed, got %q", panes[0].PaneTitle)
	}
	if panes[0].CurrentPath != "" {
		t.Fatalf("expected empty current path for legacy format, got %q", panes[0].CurrentPath)
	}
}

func TestParseListPanesOutputParsesCurrentPathHistoryAndPaneTitle(t *testing.T) {
	now := time.Now().UTC()
	out := "%1\ts1\t@1\tw1\tcodex\t321\t/dev/ttys001\t/Users/virtualmachine/worktree\t6543\tImplement panel layout\n"
	panes, err := parseListPanesOutput("t1", out, now)
	if err != nil {
		t.Fatalf("parse list panes: %v", err)
	}
	if len(panes) != 1 {
		t.Fatalf("expected one pane, got %d", len(panes))
	}
	if panes[0].CurrentPath != "/Users/virtualmachine/worktree" {
		t.Fatalf("expected current path parsed, got %q", panes[0].CurrentPath)
	}
	if panes[0].HistoryBytes != 6543 {
		t.Fatalf("expected history bytes parsed, got %d", panes[0].HistoryBytes)
	}
	if panes[0].PaneTitle != "Implement panel layout" {
		t.Fatalf("expected pane title parsed, got %q", panes[0].PaneTitle)
	}
}

func TestParseListPanesOutputParsesUnitSeparatorFormat(t *testing.T) {
	now := time.Now().UTC()
	out := "%1\x1fs1\x1f@1\x1fw1\x1fcodex\x1f321\x1f/dev/ttys001\x1f/Users/virtualmachine/worktree\x1f6543\x1fImplement panel layout\n"
	panes, err := parseListPanesOutput("t1", out, now)
	if err != nil {
		t.Fatalf("parse list panes (unit separator): %v", err)
	}
	if len(panes) != 1 {
		t.Fatalf("expected one pane, got %d", len(panes))
	}
	if panes[0].PaneID != "%1" || panes[0].SessionName != "s1" || panes[0].WindowID != "@1" {
		t.Fatalf("unexpected identity fields: %+v", panes[0])
	}
	if panes[0].CurrentCmd != "codex" || panes[0].CurrentPath != "/Users/virtualmachine/worktree" {
		t.Fatalf("unexpected parsed fields: %+v", panes[0])
	}
	if panes[0].HistoryBytes != 6543 || panes[0].PaneTitle != "Implement panel layout" {
		t.Fatalf("unexpected history/title: %+v", panes[0])
	}
}

func TestParseListPanesOutputParsesEscapedTabFormat(t *testing.T) {
	now := time.Now().UTC()
	out := "%1\\ts1\\t@1\\tw1\\tcodex\\t321\\t/dev/ttys001\\t/Users/virtualmachine/worktree\\t6543\\tImplement panel layout\n"
	panes, err := parseListPanesOutput("t1", out, now)
	if err != nil {
		t.Fatalf("parse list panes (escaped tab): %v", err)
	}
	if len(panes) != 1 {
		t.Fatalf("expected one pane, got %d", len(panes))
	}
	if panes[0].PaneID != "%1" || panes[0].SessionName != "s1" || panes[0].WindowID != "@1" {
		t.Fatalf("unexpected identity fields: %+v", panes[0])
	}
}

func TestParseListPanesOutputParsesLegacyUnderscoreFormat(t *testing.T) {
	now := time.Now().UTC()
	out := "%1_s1_@1_w1_codex_321_/dev/ttys001_/Users/virtualmachine/worktree_6543_Implement panel layout\n"
	panes, err := parseListPanesOutput("t1", out, now)
	if err != nil {
		t.Fatalf("parse list panes (legacy underscore): %v", err)
	}
	if len(panes) != 1 {
		t.Fatalf("expected one pane, got %d", len(panes))
	}
	if panes[0].PaneTitle != "Implement panel layout" {
		t.Fatalf("unexpected pane title: %q", panes[0].PaneTitle)
	}
}

func TestParseListPanesOutputBackwardCompatibleFourColumns(t *testing.T) {
	now := time.Now().UTC()
	out := "%1\ts1\t@1\tw1\n"
	panes, err := parseListPanesOutput("t1", out, now)
	if err != nil {
		t.Fatalf("parse list panes 4-col: %v", err)
	}
	if len(panes) != 1 {
		t.Fatalf("expected one pane, got %d", len(panes))
	}
	if panes[0].CurrentCmd != "" {
		t.Fatalf("expected empty current cmd, got %q", panes[0].CurrentCmd)
	}
	if panes[0].CurrentPID != nil {
		t.Fatalf("expected nil current pid, got %+v", panes[0].CurrentPID)
	}
}

func TestParseListPanesOutputBackwardCompatibleSixColumns(t *testing.T) {
	now := time.Now().UTC()
	out := "%1\ts1\t@1\tw1\tnode\t999\n"
	panes, err := parseListPanesOutput("t1", out, now)
	if err != nil {
		t.Fatalf("parse list panes 6-col: %v", err)
	}
	if len(panes) != 1 {
		t.Fatalf("expected one pane, got %d", len(panes))
	}
	if panes[0].CurrentCmd != "node" {
		t.Fatalf("expected current cmd node, got %q", panes[0].CurrentCmd)
	}
	if panes[0].CurrentPID == nil || *panes[0].CurrentPID != 999 {
		t.Fatalf("expected current pid 999, got %+v", panes[0].CurrentPID)
	}
	if panes[0].TTY != "" {
		t.Fatalf("expected empty tty for 6-col output, got %q", panes[0].TTY)
	}
}
