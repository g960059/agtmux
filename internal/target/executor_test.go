package target

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	"github.com/g960059/agtmux/internal/config"
	"github.com/g960059/agtmux/internal/model"
)

type fakeRunner struct {
	calls   []runnerCall
	results []runnerResult
}

type runnerCall struct {
	name string
	args []string
}

type runnerResult struct {
	out []byte
	err error
}

func (f *fakeRunner) Run(_ context.Context, name string, args ...string) ([]byte, error) {
	f.calls = append(f.calls, runnerCall{name: name, args: append([]string(nil), args...)})
	if len(f.results) == 0 {
		return []byte("ok"), nil
	}
	r := f.results[0]
	f.results = f.results[1:]
	return r.out, r.err
}

func TestExecutorLocalCommandPath(t *testing.T) {
	cfg := config.DefaultConfig()
	cfg.RetryBackoff = nil
	r := &fakeRunner{}
	ex := NewExecutorWithRunner(cfg, r)

	result, err := ex.Run(context.Background(), model.Target{Kind: model.TargetKindLocal}, []string{"tmux", "list-panes", "-a"})
	if err != nil {
		t.Fatalf("run local command: %v", err)
	}
	if strings.TrimSpace(result.Output) != "ok" {
		t.Fatalf("unexpected output: %q", result.Output)
	}
	if len(r.calls) != 1 {
		t.Fatalf("expected one runner call, got %d", len(r.calls))
	}
	if r.calls[0].name != "tmux" {
		t.Fatalf("expected binary tmux, got %s", r.calls[0].name)
	}
	if len(r.calls[0].args) != 2 || r.calls[0].args[0] != "list-panes" {
		t.Fatalf("unexpected args: %#v", r.calls[0].args)
	}
}

func TestExecutorSSHCommandPath(t *testing.T) {
	cfg := config.DefaultConfig()
	cfg.RetryBackoff = nil
	r := &fakeRunner{}
	ex := NewExecutorWithRunner(cfg, r)

	_, err := ex.Run(context.Background(), model.Target{Kind: model.TargetKindSSH, ConnectionRef: "vm1"}, []string{"tmux", "list-panes", "-a"})
	if err != nil {
		t.Fatalf("run ssh command: %v", err)
	}
	if len(r.calls) != 1 {
		t.Fatalf("expected one call, got %d", len(r.calls))
	}
	if r.calls[0].name != "ssh" {
		t.Fatalf("expected ssh binary, got %s", r.calls[0].name)
	}
	joined := strings.Join(r.calls[0].args, " ")
	if strings.Contains(joined, " -- ") {
		t.Fatalf("unexpected standalone -- in ssh args: %s", joined)
	}
	if !strings.Contains(joined, "tmux list-panes -a") {
		t.Fatalf("expected ssh argv-safe command args, got %s", joined)
	}
}

func TestExecutorRejectsOptionLikeSSHConnectionRef(t *testing.T) {
	cfg := config.DefaultConfig()
	cfg.RetryBackoff = nil
	r := &fakeRunner{}
	ex := NewExecutorWithRunner(cfg, r)

	_, err := ex.Run(context.Background(), model.Target{Kind: model.TargetKindSSH, ConnectionRef: "-Fmalicious"}, []string{"tmux", "list-panes"})
	if err == nil {
		t.Fatalf("expected invalid ssh connection ref error")
	}
	if len(r.calls) != 0 {
		t.Fatalf("runner should not be called for invalid connection ref")
	}
}

func TestExecutorRetries(t *testing.T) {
	cfg := config.DefaultConfig()
	cfg.RetryBackoff = []time.Duration{1 * time.Millisecond, 1 * time.Millisecond}
	r := &fakeRunner{results: []runnerResult{
		{err: errors.New("temporary")},
		{err: errors.New("temporary")},
		{out: []byte("ok"), err: nil},
	}}
	ex := NewExecutorWithRunner(cfg, r)
	_, err := ex.Run(context.Background(), model.Target{Kind: model.TargetKindLocal}, []string{"tmux", "list-panes", "-a"})
	if err != nil {
		t.Fatalf("expected retry success: %v", err)
	}
	if len(r.calls) != 3 {
		t.Fatalf("expected 3 attempts, got %d", len(r.calls))
	}
}

func TestExecutorRetryWithZeroBackoffDoesNotPanic(t *testing.T) {
	cfg := config.DefaultConfig()
	cfg.RetryBackoff = []time.Duration{0}
	r := &fakeRunner{results: []runnerResult{
		{err: errors.New("temporary")},
		{out: []byte("ok"), err: nil},
	}}
	ex := NewExecutorWithRunner(cfg, r)
	if _, err := ex.Run(context.Background(), model.Target{Kind: model.TargetKindLocal}, []string{"tmux", "list-panes"}); err != nil {
		t.Fatalf("expected retry success: %v", err)
	}
}

func TestExecutorWriteCommandDoesNotRetry(t *testing.T) {
	cfg := config.DefaultConfig()
	cfg.RetryBackoff = []time.Duration{1 * time.Millisecond, 1 * time.Millisecond}
	r := &fakeRunner{results: []runnerResult{
		{err: errors.New("write failed")},
		{out: []byte("unexpected"), err: nil},
	}}
	ex := NewExecutorWithRunner(cfg, r)

	_, err := ex.Run(context.Background(), model.Target{Kind: model.TargetKindLocal}, []string{"tmux", "send-keys", "hello"})
	if err == nil {
		t.Fatalf("expected write command error")
	}
	if len(r.calls) != 1 {
		t.Fatalf("write command should not retry, got %d calls", len(r.calls))
	}
}
