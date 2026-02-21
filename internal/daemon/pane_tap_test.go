package daemon

import (
	"strings"
	"testing"

	"github.com/g960059/agtmux/internal/ttyv2"
)

func TestPaneTapHandleMatches(t *testing.T) {
	handle := &paneTapHandle{
		targetName:  "local",
		sessionName: "exp",
		paneID:      "%3",
	}
	if !handle.matches(ttyv2.PaneRef{
		Target:      " local ",
		SessionName: "exp",
		PaneID:      " %3",
	}) {
		t.Fatalf("expected pane tap handle to match canonical pane ref")
	}
	if handle.matches(ttyv2.PaneRef{
		Target:      "local",
		SessionName: "exp",
		PaneID:      "%4",
	}) {
		t.Fatalf("expected pane tap handle mismatch on pane id")
	}
}

func TestBuildPaneTapShellCommandQuotesFifoPath(t *testing.T) {
	path := "/tmp/agtmux path/it's fifo"
	command := buildPaneTapShellCommand(path)
	if !strings.HasPrefix(command, "exec cat > ") {
		t.Fatalf("unexpected pane tap command prefix: %q", command)
	}
	if strings.Contains(command, path) {
		t.Fatalf("expected raw fifo path to be quoted, command=%q", command)
	}
	if !strings.Contains(command, "'\\''") {
		t.Fatalf("expected single quote escaping in shell command, command=%q", command)
	}
}

func TestShellSingleQuote(t *testing.T) {
	if got := shellSingleQuote(""); got != "''" {
		t.Fatalf("expected empty quote, got %q", got)
	}
	if got := shellSingleQuote("abc"); got != "'abc'" {
		t.Fatalf("expected simple quote, got %q", got)
	}
	if got := shellSingleQuote("a'b"); got != "'a'\\''b'" {
		t.Fatalf("expected escaped single quote, got %q", got)
	}
}
