package tmuxfmt

import "strings"

// FieldSeparator is the canonical tmux list format delimiter used by agtmux.
// ASCII Unit Separator avoids collision with common pane title/content text.
const FieldSeparator = "\x1f"

// Join builds a tmux format string with the canonical delimiter.
func Join(fields ...string) string {
	return strings.Join(fields, FieldSeparator)
}

// SplitLine splits a tmux formatted line with compatibility fallbacks.
// It accepts canonical separator, real tabs, escaped "\t", and legacy "_".
func SplitLine(line string, maxParts int) []string {
	if maxParts <= 0 {
		return nil
	}
	if strings.Contains(line, FieldSeparator) {
		return strings.SplitN(line, FieldSeparator, maxParts)
	}
	if strings.Contains(line, "\t") {
		return strings.SplitN(line, "\t", maxParts)
	}
	if strings.Contains(line, `\t`) {
		return strings.SplitN(line, `\t`, maxParts)
	}
	if strings.Contains(line, "_") {
		return strings.SplitN(line, "_", maxParts)
	}
	return []string{line}
}
