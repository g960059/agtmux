package observer

import (
	"bufio"
	"context"
	"fmt"
	"strconv"
	"strings"
	"time"

	"github.com/g960059/agtmux/internal/db"
	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/target"
	"github.com/g960059/agtmux/internal/tmuxfmt"
)

type TmuxObserver struct {
	executor *target.Executor
	store    *db.Store
}

func NewTmuxObserver(executor *target.Executor, store *db.Store) *TmuxObserver {
	return &TmuxObserver{executor: executor, store: store}
}

func (o *TmuxObserver) Collect(ctx context.Context, tg model.Target, at time.Time) ([]model.Pane, error) {
	res, err := o.executor.Run(ctx, tg, target.BuildTmuxCommand(
		"list-panes",
		"-a",
		"-F",
		tmuxfmt.Join(
			"#{pane_id}",
			"#{session_name}",
			"#{window_id}",
			"#{window_name}",
			"#{pane_current_command}",
			"#{pane_pid}",
			"#{pane_tty}",
			"#{pane_current_path}",
			"#{history_bytes}",
			"#{pane_title}",
		),
	))
	if err != nil {
		return nil, err
	}

	panes, err := parseListPanesOutput(tg.TargetID, res.Output, at)
	if err != nil {
		return nil, err
	}
	if err := o.store.SyncTargetPanes(ctx, tg.TargetID, panes); err != nil {
		return nil, err
	}
	return panes, nil
}

func parseListPanesOutput(targetID, output string, updatedAt time.Time) ([]model.Pane, error) {
	s := bufio.NewScanner(strings.NewReader(output))
	panes := make([]model.Pane, 0)
	for s.Scan() {
		line := strings.TrimSpace(s.Text())
		if line == "" {
			continue
		}
		parts := tmuxfmt.SplitLine(line, 10)
		if len(parts) != 4 && len(parts) != 6 && len(parts) != 7 && len(parts) != 8 && len(parts) != 9 && len(parts) != 10 {
			return nil, fmt.Errorf("invalid tmux list-panes line: %q", line)
		}
		if !strings.HasPrefix(strings.TrimSpace(parts[0]), "%") {
			return nil, fmt.Errorf("invalid tmux list-panes line: %q", line)
		}
		if len(parts) >= 3 && !strings.HasPrefix(strings.TrimSpace(parts[2]), "@") {
			return nil, fmt.Errorf("invalid tmux list-panes line: %q", line)
		}
		var pidPtr *int64
		cmd := ""
		tty := ""
		currentPath := ""
		historyBytes := int64(0)
		paneTitle := ""
		if len(parts) == 6 {
			cmd = strings.TrimSpace(parts[4])
			pidStr := strings.TrimSpace(parts[5])
			if pidStr != "" {
				if pid, err := strconv.ParseInt(pidStr, 10, 64); err == nil && pid > 0 {
					pidPtr = &pid
				}
			}
		}
		if len(parts) >= 7 {
			cmd = strings.TrimSpace(parts[4])
			pidStr := strings.TrimSpace(parts[5])
			if pidStr != "" {
				if pid, err := strconv.ParseInt(pidStr, 10, 64); err == nil && pid > 0 {
					pidPtr = &pid
				}
			}
			tty = strings.TrimSpace(parts[6])
		}
		if len(parts) == 8 {
			currentPath = strings.TrimSpace(parts[7])
		}
		if len(parts) == 9 {
			col7 := strings.TrimSpace(parts[7])
			col8 := strings.TrimSpace(parts[8])
			if parsed, err := strconv.ParseInt(col7, 10, 64); err == nil && parsed >= 0 {
				// Backward-compatible old format: ... tty, history_bytes, pane_title
				historyBytes = parsed
				paneTitle = col8
			} else {
				// New format without pane_title support: ... tty, pane_current_path, history_bytes|title
				currentPath = col7
				if parsed, err := strconv.ParseInt(col8, 10, 64); err == nil && parsed >= 0 {
					historyBytes = parsed
				} else {
					paneTitle = col8
				}
			}
		}
		if len(parts) == 10 {
			currentPath = strings.TrimSpace(parts[7])
			historyRaw := strings.TrimSpace(parts[8])
			if historyRaw != "" {
				if parsed, err := strconv.ParseInt(historyRaw, 10, 64); err == nil && parsed >= 0 {
					historyBytes = parsed
				}
			}
			paneTitle = strings.TrimSpace(parts[9])
		}
		panes = append(panes, model.Pane{
			TargetID:     targetID,
			PaneID:       parts[0],
			SessionName:  parts[1],
			WindowID:     parts[2],
			WindowName:   parts[3],
			CurrentCmd:   cmd,
			CurrentPath:  currentPath,
			PaneTitle:    paneTitle,
			HistoryBytes: historyBytes,
			CurrentPID:   pidPtr,
			TTY:          tty,
			UpdatedAt:    updatedAt.UTC(),
		})
	}
	if err := s.Err(); err != nil {
		return nil, fmt.Errorf("scan tmux output: %w", err)
	}
	return panes, nil
}
