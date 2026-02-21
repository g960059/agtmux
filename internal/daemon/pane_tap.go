package daemon

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"golang.org/x/sys/unix"

	"github.com/g960059/agtmux/internal/model"
	"github.com/g960059/agtmux/internal/target"
	"github.com/g960059/agtmux/internal/ttyv2"
)

const paneTapDirName = "agtmux-pane-tap"

type paneTapEvent struct {
	PaneID string
	Bytes  []byte
}

type paneTapHandle struct {
	targetName  string
	sessionName string
	paneID      string
	fifoPath    string

	ctx    context.Context
	cancel context.CancelFunc

	mu       sync.Mutex
	fifo     *os.File
	attached bool

	events chan paneTapEvent
	errs   chan error
	done   chan struct{}
}

func (h *paneTapHandle) matches(pref ttyv2.PaneRef) bool {
	if h == nil {
		return false
	}
	return h.targetName == strings.TrimSpace(pref.Target) &&
		h.sessionName == strings.TrimSpace(pref.SessionName) &&
		h.paneID == strings.TrimSpace(pref.PaneID)
}

func (h *paneTapHandle) stop(executor *target.Executor, tg model.Target) {
	if h == nil {
		return
	}
	h.mu.Lock()
	attached := h.attached
	h.attached = false
	fifo := h.fifo
	h.fifo = nil
	h.mu.Unlock()

	h.cancel()

	if attached {
		_, _ = executor.Run(
			context.Background(),
			tg,
			target.BuildTmuxCommand("pipe-pane", "-t", h.paneID),
		)
	}
	if fifo != nil {
		_ = fifo.Close()
	}

	select {
	case <-h.done:
	case <-time.After(500 * time.Millisecond):
	}
	_ = os.Remove(h.fifoPath)
}

func startPaneTapForPane(tg model.Target, pref ttyv2.PaneRef, executor *target.Executor) (*paneTapHandle, error) {
	if tg.Kind != model.TargetKindLocal {
		return nil, errors.New("pane tap supports local targets only")
	}
	paneID := strings.TrimSpace(pref.PaneID)
	if paneID == "" {
		return nil, errors.New("pane id is required")
	}
	fifoPath, err := newPaneTapFIFOPath()
	if err != nil {
		return nil, err
	}
	if err := unix.Mkfifo(fifoPath, 0o600); err != nil {
		return nil, fmt.Errorf("mkfifo: %w", err)
	}

	fifo, err := os.OpenFile(fifoPath, os.O_RDWR, 0)
	if err != nil {
		_ = os.Remove(fifoPath)
		return nil, fmt.Errorf("open fifo: %w", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	h := &paneTapHandle{
		targetName:  strings.TrimSpace(pref.Target),
		sessionName: strings.TrimSpace(pref.SessionName),
		paneID:      paneID,
		fifoPath:    fifoPath,
		ctx:         ctx,
		cancel:      cancel,
		fifo:        fifo,
		events:      make(chan paneTapEvent, 512),
		errs:        make(chan error, 1),
		done:        make(chan struct{}),
	}
	go h.readLoop()

	shellCmd := buildPaneTapShellCommand(fifoPath)
	if _, runErr := executor.Run(
		context.Background(),
		tg,
		target.BuildTmuxCommand("pipe-pane", "-t", paneID, "-O", shellCmd),
	); runErr != nil {
		h.stop(executor, tg)
		return nil, fmt.Errorf("pipe-pane attach: %w", runErr)
	}

	h.mu.Lock()
	h.attached = true
	h.mu.Unlock()
	return h, nil
}

func (h *paneTapHandle) readLoop() {
	defer close(h.done)
	defer close(h.events)
	buf := make([]byte, 16*1024)
	for {
		if h.ctx.Err() != nil {
			return
		}
		n, err := h.fifo.Read(buf)
		if n > 0 {
			chunk := make([]byte, n)
			copy(chunk, buf[:n])
			select {
			case h.events <- paneTapEvent{PaneID: h.paneID, Bytes: chunk}:
			default:
			}
		}
		if err == nil {
			continue
		}
		if errors.Is(err, os.ErrClosed) || errors.Is(err, io.EOF) || h.ctx.Err() != nil {
			return
		}
		select {
		case h.errs <- err:
		default:
		}
		return
	}
}

func newPaneTapFIFOPath() (string, error) {
	dir := filepath.Join(os.TempDir(), paneTapDirName)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return "", fmt.Errorf("mkdir pane tap dir: %w", err)
	}
	name := fmt.Sprintf("pane-tap-%d-%d.fifo", os.Getpid(), time.Now().UTC().UnixNano())
	return filepath.Join(dir, name), nil
}

func buildPaneTapShellCommand(fifoPath string) string {
	quoted := shellSingleQuote(fifoPath)
	return "exec cat > " + quoted
}

func shellSingleQuote(raw string) string {
	if raw == "" {
		return "''"
	}
	return "'" + strings.ReplaceAll(raw, "'", "'\\''") + "'"
}
