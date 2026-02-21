package daemon

import (
	"bytes"
	"strings"
)

type tmuxControlEventType string

const (
	tmuxControlEventOutput         tmuxControlEventType = "output"
	tmuxControlEventExtendedOutput tmuxControlEventType = "extended_output"
	tmuxControlEventLayoutChange   tmuxControlEventType = "layout_change"
	tmuxControlEventSessionChanged tmuxControlEventType = "session_changed"
	tmuxControlEventWindowAdd      tmuxControlEventType = "window_add"
	tmuxControlEventExit           tmuxControlEventType = "exit"
)

type tmuxControlEvent struct {
	Type        tmuxControlEventType
	PaneID      string
	Bytes       []byte
	WindowID    string
	LayoutRaw   string
	LayoutCols  int
	LayoutRows  int
	LayoutKnown bool
	SessionID   string
	SessionName string
	Raw         string
}

type tmuxLayoutGeometry struct {
	Cols int
	Rows int
}

type tmuxControlOutput struct {
	PaneID string
	Bytes  []byte
}

func parseTmuxControlEventLine(raw string) (tmuxControlEvent, bool) {
	line := strings.TrimRight(raw, "\r\n")
	if line == "" {
		return tmuxControlEvent{}, false
	}
	if out, ok := parseTmuxControlOutputLine(line); ok {
		eventType := tmuxControlEventOutput
		if strings.HasPrefix(line, "%extended-output ") {
			eventType = tmuxControlEventExtendedOutput
		}
		return tmuxControlEvent{
			Type:   eventType,
			PaneID: out.PaneID,
			Bytes:  out.Bytes,
			Raw:    line,
		}, true
	}
	switch {
	case strings.HasPrefix(line, "%layout-change "):
		rest := strings.TrimPrefix(line, "%layout-change ")
		windowID, tail, ok := cutToken(rest)
		if !ok || windowID == "" {
			return tmuxControlEvent{}, false
		}
		layoutRaw := strings.TrimSpace(tail)
		layoutCols, layoutRows, layoutKnown := parseTmuxLayoutGeometry(layoutRaw)
		return tmuxControlEvent{
			Type:        tmuxControlEventLayoutChange,
			WindowID:    windowID,
			LayoutRaw:   layoutRaw,
			LayoutCols:  layoutCols,
			LayoutRows:  layoutRows,
			LayoutKnown: layoutKnown,
			Raw:         line,
		}, true
	case strings.HasPrefix(line, "%session-changed "):
		rest := strings.TrimPrefix(line, "%session-changed ")
		sessionID, tail, ok := cutToken(rest)
		if !ok || sessionID == "" {
			return tmuxControlEvent{}, false
		}
		return tmuxControlEvent{
			Type:        tmuxControlEventSessionChanged,
			SessionID:   sessionID,
			SessionName: strings.TrimSpace(tail),
			Raw:         line,
		}, true
	case strings.HasPrefix(line, "%window-add "):
		rest := strings.TrimPrefix(line, "%window-add ")
		windowID, _, ok := cutToken(rest)
		if !ok || windowID == "" {
			return tmuxControlEvent{}, false
		}
		return tmuxControlEvent{
			Type:     tmuxControlEventWindowAdd,
			WindowID: windowID,
			Raw:      line,
		}, true
	case strings.HasPrefix(line, "%exit"):
		return tmuxControlEvent{
			Type: tmuxControlEventExit,
			Raw:  line,
		}, true
	default:
		return tmuxControlEvent{}, false
	}
}

func parseTmuxLayoutGeometry(layoutRaw string) (cols int, rows int, ok bool) {
	trimmed := strings.TrimSpace(layoutRaw)
	if trimmed == "" {
		return 0, 0, false
	}
	parts := strings.Split(trimmed, ",")
	if len(parts) < 2 {
		return 0, 0, false
	}
	dim := strings.TrimSpace(parts[1])
	if dim == "" {
		return 0, 0, false
	}
	x := strings.IndexByte(dim, 'x')
	if x <= 0 || x >= len(dim)-1 {
		return 0, 0, false
	}
	left := strings.TrimSpace(dim[:x])
	right := strings.TrimSpace(dim[x+1:])
	if !isASCIIInt(left) || !isASCIIInt(right) {
		return 0, 0, false
	}
	c := atoiSmall(left)
	r := atoiSmall(right)
	if c <= 0 || r <= 0 {
		return 0, 0, false
	}
	return c, r, true
}

func shouldHandleLayoutGeometryChange(prev tmuxLayoutGeometry, next tmuxLayoutGeometry) bool {
	if next.Cols <= 0 || next.Rows <= 0 {
		return false
	}
	if prev.Cols <= 0 || prev.Rows <= 0 {
		return true
	}
	return prev.Cols != next.Cols || prev.Rows != next.Rows
}

func parsePaneSize(raw string) (cols int, rows int, ok bool) {
	trimmed := strings.TrimSpace(raw)
	if trimmed == "" {
		return 0, 0, false
	}
	parts := strings.Split(trimmed, ",")
	if len(parts) != 2 {
		return 0, 0, false
	}
	left := strings.TrimSpace(parts[0])
	right := strings.TrimSpace(parts[1])
	if !isASCIIInt(left) || !isASCIIInt(right) {
		return 0, 0, false
	}
	c := atoiSmall(left)
	r := atoiSmall(right)
	if c <= 0 || r <= 0 {
		return 0, 0, false
	}
	return c, r, true
}

func paneSizeChanged(prevCols *int, prevRows *int, nextCols int, nextRows int) bool {
	if nextCols <= 0 || nextRows <= 0 {
		return false
	}
	if prevCols == nil || prevRows == nil || *prevCols <= 0 || *prevRows <= 0 {
		return true
	}
	return *prevCols != nextCols || *prevRows != nextRows
}

func isASCIIInt(raw string) bool {
	if raw == "" {
		return false
	}
	for i := 0; i < len(raw); i++ {
		if raw[i] < '0' || raw[i] > '9' {
			return false
		}
	}
	return true
}

func atoiSmall(raw string) int {
	v := 0
	for i := 0; i < len(raw); i++ {
		v = (v * 10) + int(raw[i]-'0')
	}
	return v
}

func parseTmuxControlOutputLine(raw string) (tmuxControlOutput, bool) {
	line := strings.TrimRight(raw, "\r\n")
	switch {
	case strings.HasPrefix(line, "%output "):
		return parseControlOutputWithPrefix(line, "%output ")
	case strings.HasPrefix(line, "%extended-output "):
		// tmux can emit: %extended-output <pane-id> <age> <escaped-bytes>
		rest := strings.TrimPrefix(line, "%extended-output ")
		paneID, tail, ok := cutToken(rest)
		if !ok || paneID == "" {
			return tmuxControlOutput{}, false
		}
		_, tail, ok = cutToken(tail) // age
		if !ok {
			return tmuxControlOutput{}, false
		}
		decoded, ok := decodeTmuxControlEscaped(tail)
		if !ok {
			return tmuxControlOutput{}, false
		}
		return tmuxControlOutput{PaneID: paneID, Bytes: decoded}, true
	default:
		return tmuxControlOutput{}, false
	}
}

func parseControlOutputWithPrefix(line string, prefix string) (tmuxControlOutput, bool) {
	rest := strings.TrimPrefix(line, prefix)
	paneID, payload, ok := cutToken(rest)
	if !ok || paneID == "" {
		return tmuxControlOutput{}, false
	}
	decoded, ok := decodeTmuxControlEscaped(payload)
	if !ok {
		return tmuxControlOutput{}, false
	}
	return tmuxControlOutput{PaneID: paneID, Bytes: decoded}, true
}

func cutToken(raw string) (token string, tail string, ok bool) {
	trimmed := strings.TrimSpace(raw)
	if trimmed == "" {
		return "", "", false
	}
	idx := strings.IndexAny(trimmed, " \t")
	if idx < 0 {
		return trimmed, "", true
	}
	return trimmed[:idx], strings.TrimLeft(trimmed[idx:], " \t"), true
}

func decodeTmuxControlEscaped(raw string) ([]byte, bool) {
	if raw == "" {
		return []byte{}, true
	}
	out := bytes.NewBuffer(make([]byte, 0, len(raw)))
	for i := 0; i < len(raw); i++ {
		ch := raw[i]
		if ch != '\\' {
			out.WriteByte(ch)
			continue
		}
		if i+1 >= len(raw) {
			return nil, false
		}
		next := raw[i+1]
		if next >= '0' && next <= '7' {
			// tmux uses octal escapes like \033.
			if i+3 >= len(raw) {
				return nil, false
			}
			oct := raw[i+1 : i+4]
			if !isOctal3(oct) {
				return nil, false
			}
			value := ((oct[0] - '0') << 6) | ((oct[1] - '0') << 3) | (oct[2] - '0')
			out.WriteByte(value)
			i += 3
			continue
		}
		switch next {
		case '\\':
			out.WriteByte('\\')
		case 'n':
			out.WriteByte('\n')
		case 'r':
			out.WriteByte('\r')
		case 't':
			out.WriteByte('\t')
		default:
			// Keep unknown escaped byte as-is (best-effort compatibility).
			out.WriteByte(next)
		}
		i++
	}
	return out.Bytes(), true
}

func isOctal3(raw string) bool {
	if len(raw) != 3 {
		return false
	}
	for i := 0; i < len(raw); i++ {
		if raw[i] < '0' || raw[i] > '7' {
			return false
		}
	}
	return true
}
