package target

import (
	"context"
	"errors"
	"fmt"
	"os/exec"
	"strings"
	"time"

	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/model"
)

type RunResult struct {
	Output   string
	Duration time.Duration
}

type Runner interface {
	Run(ctx context.Context, name string, args ...string) ([]byte, error)
}

type OSRunner struct{}

func (OSRunner) Run(ctx context.Context, name string, args ...string) ([]byte, error) {
	cmd := exec.CommandContext(ctx, name, args...)
	return cmd.CombinedOutput()
}

type Executor struct {
	cfg    config.Config
	runner Runner
}

func NewExecutor(cfg config.Config) *Executor {
	return &Executor{
		cfg:    cfg,
		runner: OSRunner{},
	}
}

func NewExecutorWithRunner(cfg config.Config, runner Runner) *Executor {
	e := NewExecutor(cfg)
	e.runner = runner
	return e
}

func (e *Executor) Run(ctx context.Context, target model.Target, command []string) (RunResult, error) {
	if len(command) == 0 {
		return RunResult{}, fmt.Errorf("empty command")
	}

	maxAttempts := 1
	if isRetryableCommand(command) {
		maxAttempts += len(e.cfg.RetryBackoff)
	}
	var lastErr error
	for attempt := 1; attempt <= maxAttempts; attempt++ {
		start := time.Now()
		runCtx, cancel := context.WithTimeout(ctx, e.cfg.CommandTimeout)
		var (
			out []byte
			err error
		)
		switch target.Kind {
		case model.TargetKindLocal:
			out, err = e.runner.Run(runCtx, command[0], command[1:]...)
		case model.TargetKindSSH:
			args, argErr := e.buildSSHArgs(target.ConnectionRef, command)
			if argErr != nil {
				cancel()
				return RunResult{}, argErr
			}
			out, err = e.runner.Run(runCtx, "ssh", args...)
		default:
			cancel()
			return RunResult{}, fmt.Errorf("unsupported target kind: %s", target.Kind)
		}
		cancel()
		if err == nil {
			return RunResult{Output: string(out), Duration: time.Since(start)}, nil
		}
		lastErr = err

		if attempt < maxAttempts {
			backoff := e.cfg.RetryBackoff[attempt-1]
			jitter := time.Duration(0)
			maxJitter := int64(backoff / 4)
			if maxJitter > 0 {
				jitter = time.Duration(time.Now().UTC().UnixNano() % maxJitter)
			}
			select {
			case <-ctx.Done():
				return RunResult{}, ctx.Err()
			case <-time.After(backoff + jitter):
			}
		}
	}

	if errors.Is(lastErr, context.DeadlineExceeded) || errors.Is(lastErr, context.Canceled) {
		return RunResult{}, fmt.Errorf("%s: %w", model.ErrTargetUnreachable, lastErr)
	}
	return RunResult{}, fmt.Errorf("%s: %w", model.ErrTargetUnreachable, lastErr)
}

func (e *Executor) buildSSHArgs(connectionRef string, command []string) ([]string, error) {
	if strings.TrimSpace(connectionRef) == "" {
		return nil, fmt.Errorf("ssh target connection_ref is required")
	}
	if strings.HasPrefix(strings.TrimSpace(connectionRef), "-") {
		return nil, fmt.Errorf("invalid ssh target connection_ref")
	}
	args := []string{
		"-o", "BatchMode=yes",
		"-o", fmt.Sprintf("ConnectTimeout=%d", int(e.cfg.ConnectTimeout.Seconds())),
		"-o", "ControlMaster=auto",
		"-o", "ControlPersist=60",
		connectionRef,
	}
	args = append(args, command...)
	return args, nil
}

func BuildTmuxCommand(args ...string) []string {
	cmd := make([]string, 0, len(args)+1)
	cmd = append(cmd, "tmux")
	cmd = append(cmd, args...)
	return cmd
}

func isRetryableCommand(command []string) bool {
	if len(command) < 2 {
		return false
	}
	if command[0] != "tmux" {
		return false
	}
	switch strings.ToLower(command[1]) {
	case "list-panes", "list-windows", "list-sessions", "display-message", "capture-pane", "show-options", "show-environment":
		return true
	default:
		return false
	}
}
