// tmux control mode (-C) parser.
//
// Parses output from `tmux -C attach-session`. Control mode outputs lines
// starting with `%` prefix. This module provides:
//
// - `ControlEvent` enum for parsed event types
// - `decode_octal_escaped()` for tmux octal escape sequences
// - `parse_line()` for parsing a single control mode output line

use bytes::Bytes;

/// Parsed event from tmux control mode output.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlEvent {
    /// Terminal output from a pane.
    /// Format: `%output %<pane-id> <octal-escaped-bytes>`
    Output { pane_id: String, data: Bytes },

    /// Extended output from a pane (includes age/latency).
    /// Format: `%extended-output %<pane-id> <age> : <octal-escaped-bytes>`
    ExtendedOutput {
        pane_id: String,
        age: u64,
        data: Bytes,
    },

    /// Window layout changed.
    /// Format: `%layout-change @<window-id> <layout-string>`
    LayoutChange {
        window_id: String,
        layout: String,
    },

    /// Session changed.
    /// Format: `%session-changed $<id> <name>`
    SessionChanged {
        session_id: String,
        name: String,
    },

    /// Window added.
    /// Format: `%window-add @<id>`
    WindowAdd { window_id: String },

    /// Window closed.
    /// Format: `%window-close @<id>`
    WindowClose { window_id: String },

    /// Pane mode changed.
    /// Format: `%pane-mode-changed %<pane-id>`
    PaneModeChanged { pane_id: String },

    /// Control mode exit.
    /// Format: `%exit [reason]`
    Exit { reason: String },

    /// Unrecognized control mode line (starts with `%` but not a known event).
    Unknown(String),
}

/// Decode tmux octal-escaped byte string into raw bytes.
///
/// Tmux control mode encodes non-printable and non-ASCII bytes using octal
/// escape sequences:
/// - `\NNN` where NNN is exactly 3 octal digits maps to a single byte
/// - `\\` maps to a literal backslash (`\`)
/// - All other characters pass through as their UTF-8 bytes
///
/// Multi-byte UTF-8 characters (CJK, emoji, etc.) appear as consecutive
/// octal escapes for each byte of the UTF-8 encoding.
///
/// After decoding, the resulting `Vec<u8>` can be interpreted as UTF-8
/// using `String::from_utf8_lossy()` or kept as raw bytes.
pub fn decode_octal_escaped(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut out = Vec::with_capacity(len);
    let mut i = 0;

    while i < len {
        if bytes[i] == b'\\' && i + 1 < len {
            // Check for octal escape: exactly 3 octal digits following '\'
            if i + 3 < len
                && is_octal_digit(bytes[i + 1])
                && is_octal_digit(bytes[i + 2])
                && is_octal_digit(bytes[i + 3])
            {
                let val = (bytes[i + 1] - b'0') as u16 * 64
                    + (bytes[i + 2] - b'0') as u16 * 8
                    + (bytes[i + 3] - b'0') as u16;
                out.push(val as u8);
                i += 4;
            } else if bytes[i + 1] == b'\\' {
                // Escaped backslash
                out.push(b'\\');
                i += 2;
            } else {
                // Not a recognized escape — pass through the backslash literally
                out.push(b'\\');
                i += 1;
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }

    out
}

/// Parse a single control mode line into a `ControlEvent`.
///
/// Returns `None` if the line does not start with `%` (i.e., it is not a
/// control mode event line — could be a command response or empty line).
pub fn parse_line(line: &str) -> Option<ControlEvent> {
    let line = line.trim_end_matches(['\r', '\n']);

    if !line.starts_with('%') {
        return None;
    }

    // Split into the event keyword and the rest
    let (keyword, rest) = split_first_word(line);

    match keyword {
        "%output" => parse_output(rest),
        "%extended-output" => parse_extended_output(rest),
        "%layout-change" => parse_layout_change(rest),
        "%session-changed" => parse_session_changed(rest),
        "%window-add" => parse_window_add(rest),
        "%window-close" => parse_window_close(rest),
        "%pane-mode-changed" => parse_pane_mode_changed(rest),
        "%exit" => Some(ControlEvent::Exit {
            reason: rest.to_string(),
        }),
        _ => Some(ControlEvent::Unknown(line.to_string())),
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

#[inline]
fn is_octal_digit(b: u8) -> bool {
    b >= b'0' && b <= b'7'
}

/// Split a string into the first whitespace-delimited word and the remainder.
/// The remainder is trimmed of leading whitespace.
fn split_first_word(s: &str) -> (&str, &str) {
    match s.find(char::is_whitespace) {
        Some(pos) => (&s[..pos], s[pos..].trim_start()),
        None => (s, ""),
    }
}

/// `%output %<pane-id> <octal-escaped-data>`
fn parse_output(rest: &str) -> Option<ControlEvent> {
    let (pane_id, data_str) = split_first_word(rest);
    if pane_id.is_empty() {
        return Some(ControlEvent::Unknown(format!("%output {rest}")));
    }
    let data = decode_octal_escaped(data_str);
    Some(ControlEvent::Output {
        pane_id: pane_id.to_string(),
        data: Bytes::from(data),
    })
}

/// `%extended-output %<pane-id> <age> : <octal-escaped-data>`
fn parse_extended_output(rest: &str) -> Option<ControlEvent> {
    // pane_id
    let (pane_id, rest) = split_first_word(rest);
    if pane_id.is_empty() {
        return Some(ControlEvent::Unknown(format!("%extended-output {rest}")));
    }
    // age
    let (age_str, rest) = split_first_word(rest);
    let age: u64 = match age_str.parse() {
        Ok(v) => v,
        Err(_) => {
            return Some(ControlEvent::Unknown(format!(
                "%extended-output {pane_id} {age_str} {rest}"
            )));
        }
    };
    // colon separator
    let rest = rest.strip_prefix(": ").or_else(|| rest.strip_prefix(":"))?;
    let data = decode_octal_escaped(rest);
    Some(ControlEvent::ExtendedOutput {
        pane_id: pane_id.to_string(),
        age,
        data: Bytes::from(data),
    })
}

/// `%layout-change @<window-id> <layout-string>`
fn parse_layout_change(rest: &str) -> Option<ControlEvent> {
    let (window_id, layout) = split_first_word(rest);
    if window_id.is_empty() {
        return Some(ControlEvent::Unknown(format!("%layout-change {rest}")));
    }
    Some(ControlEvent::LayoutChange {
        window_id: window_id.to_string(),
        layout: layout.to_string(),
    })
}

/// `%session-changed $<id> <name>`
fn parse_session_changed(rest: &str) -> Option<ControlEvent> {
    let (session_id, name) = split_first_word(rest);
    if session_id.is_empty() {
        return Some(ControlEvent::Unknown(format!("%session-changed {rest}")));
    }
    Some(ControlEvent::SessionChanged {
        session_id: session_id.to_string(),
        name: name.to_string(),
    })
}

/// `%window-add @<id>`
fn parse_window_add(rest: &str) -> Option<ControlEvent> {
    let (window_id, _) = split_first_word(rest);
    if window_id.is_empty() {
        return Some(ControlEvent::Unknown(format!("%window-add {rest}")));
    }
    Some(ControlEvent::WindowAdd {
        window_id: window_id.to_string(),
    })
}

/// `%window-close @<id>`
fn parse_window_close(rest: &str) -> Option<ControlEvent> {
    let (window_id, _) = split_first_word(rest);
    if window_id.is_empty() {
        return Some(ControlEvent::Unknown(format!("%window-close {rest}")));
    }
    Some(ControlEvent::WindowClose {
        window_id: window_id.to_string(),
    })
}

/// `%pane-mode-changed %<pane-id>`
fn parse_pane_mode_changed(rest: &str) -> Option<ControlEvent> {
    let (pane_id, _) = split_first_word(rest);
    if pane_id.is_empty() {
        return Some(ControlEvent::Unknown(format!("%pane-mode-changed {rest}")));
    }
    Some(ControlEvent::PaneModeChanged {
        pane_id: pane_id.to_string(),
    })
}

// ===========================================================================
// Tests
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // decode_octal_escaped tests
    // -----------------------------------------------------------------------

    #[test]
    fn decode_plain_ascii() {
        // 1. ASCII text without escapes
        let input = "hello world";
        let decoded = decode_octal_escaped(input);
        assert_eq!(decoded, b"hello world");
        assert_eq!(
            String::from_utf8_lossy(&decoded),
            "hello world"
        );
    }

    #[test]
    fn decode_simple_escape_esc_sequence() {
        // 2. Simple escape: \033[1m  → ESC [ 1 m
        let input = r"\033[1m";
        let decoded = decode_octal_escaped(input);
        assert_eq!(decoded, vec![0x1B, b'[', b'1', b'm']);
    }

    #[test]
    fn decode_cjk_3byte_hiragana() {
        // 3. CJK 3-byte: \343\201\202 → "あ" (U+3042)
        // UTF-8 encoding of U+3042 is [0xE3, 0x81, 0x82]
        let input = r"\343\201\202";
        let decoded = decode_octal_escaped(input);
        assert_eq!(decoded, vec![0xE3, 0x81, 0x82]);
        assert_eq!(String::from_utf8_lossy(&decoded), "あ");
    }

    #[test]
    fn decode_emoji_4byte() {
        // 4. Emoji 4-byte: \360\237\230\200 → "😀" (U+1F600)
        // UTF-8 encoding of U+1F600 is [0xF0, 0x9F, 0x98, 0x80]
        let input = r"\360\237\230\200";
        let decoded = decode_octal_escaped(input);
        assert_eq!(decoded, vec![0xF0, 0x9F, 0x98, 0x80]);
        assert_eq!(String::from_utf8_lossy(&decoded), "😀");
    }

    #[test]
    fn decode_mixed_ascii_cjk_emoji() {
        // 5. Mixed: ASCII + CJK + emoji in one string
        // "Hi\343\201\202\360\237\230\200!" → "Hiあ😀!"
        let input = r"Hi\343\201\202\360\237\230\200!";
        let decoded = decode_octal_escaped(input);
        let text = String::from_utf8_lossy(&decoded);
        assert_eq!(text, "Hiあ😀!");
    }

    #[test]
    fn decode_backslash_escape() {
        // 6. Backslash escape: \\ → \
        let input = r"foo\\bar";
        let decoded = decode_octal_escaped(input);
        assert_eq!(decoded, b"foo\\bar");
        assert_eq!(String::from_utf8_lossy(&decoded), "foo\\bar");
    }

    #[test]
    fn decode_multiple_cjk_chars() {
        // Multiple CJK: "日本語" = U+65E5 U+672C U+8A9E
        // UTF-8: [0xE6,0x97,0xA5] [0xE6,0x9C,0xAC] [0xE8,0xAA,0x9E]
        let input = r"\346\227\245\346\234\254\350\252\236";
        let decoded = decode_octal_escaped(input);
        assert_eq!(String::from_utf8_lossy(&decoded), "日本語");
    }

    #[test]
    fn decode_empty_string() {
        let decoded = decode_octal_escaped("");
        assert!(decoded.is_empty());
    }

    #[test]
    fn decode_lone_backslash() {
        // A trailing backslash with nothing after it passes through
        let input = r"abc\";
        let decoded = decode_octal_escaped(input);
        assert_eq!(decoded, b"abc\\");
    }

    #[test]
    fn decode_backslash_with_non_octal() {
        // \n is NOT an octal escape (n is not an octal digit) — pass through literally
        let input = r"\n";
        let decoded = decode_octal_escaped(input);
        // '\' followed by 'n' — the backslash is kept and 'n' follows
        assert_eq!(decoded, b"\\n");
    }

    #[test]
    fn decode_color_escape_sequence() {
        // Typical tmux output with ANSI color: ESC[31m hello ESC[0m
        let input = r"\033[31mhello\033[0m";
        let decoded = decode_octal_escaped(input);
        assert_eq!(
            decoded,
            vec![0x1B, b'[', b'3', b'1', b'm', b'h', b'e', b'l', b'l', b'o', 0x1B, b'[', b'0', b'm']
        );
    }

    #[test]
    fn decode_octal_377_max_byte() {
        // \377 = 3*64 + 7*8 + 7 = 255 (0xFF), the maximum valid single-byte value.
        // This previously overflowed with u8 arithmetic because 3*64 = 192
        // which, when added to 7*8 + 7 = 63, gives 255 — but the intermediate
        // multiplication `3u8 * 64` = 192 is fine, it was the full expression
        // that could overflow in debug mode for values like \400+.
        let input = r"\377";
        let decoded = decode_octal_escaped(input);
        assert_eq!(decoded, vec![0xFF]);
    }

    #[test]
    fn decode_octal_300_high_value() {
        // \300 = 3*64 + 0*8 + 0 = 192 (0xC0)
        let input = r"\300";
        let decoded = decode_octal_escaped(input);
        assert_eq!(decoded, vec![0xC0]);
    }

    // -----------------------------------------------------------------------
    // parse_line tests — individual events
    // -----------------------------------------------------------------------

    #[test]
    fn parse_output_basic() {
        // 7. Real tmux output line parsing
        let line = r"%output %0 hello\033[1m world";
        let event = parse_line(line).unwrap();
        match event {
            ControlEvent::Output { pane_id, data } => {
                assert_eq!(pane_id, "%0");
                // "hello" + ESC + "[1m world"
                let decoded = data.to_vec();
                assert_eq!(decoded[0..5], *b"hello");
                assert_eq!(decoded[5], 0x1B); // ESC
                assert_eq!(&decoded[6..], b"[1m world");
            }
            other => panic!("expected Output, got {:?}", other),
        }
    }

    #[test]
    fn parse_output_cjk() {
        let line = r"%output %5 \343\201\202\343\201\204\343\201\206";
        let event = parse_line(line).unwrap();
        match event {
            ControlEvent::Output { pane_id, data } => {
                assert_eq!(pane_id, "%5");
                assert_eq!(String::from_utf8_lossy(&data), "あいう");
            }
            other => panic!("expected Output, got {:?}", other),
        }
    }

    #[test]
    fn parse_output_empty_data() {
        let line = "%output %0 ";
        let event = parse_line(line).unwrap();
        match event {
            ControlEvent::Output { pane_id, data } => {
                assert_eq!(pane_id, "%0");
                assert!(data.is_empty());
            }
            other => panic!("expected Output, got {:?}", other),
        }
    }

    #[test]
    fn parse_extended_output() {
        let line = r"%extended-output %3 1234 : hello\033[0m";
        let event = parse_line(line).unwrap();
        match event {
            ControlEvent::ExtendedOutput { pane_id, age, data } => {
                assert_eq!(pane_id, "%3");
                assert_eq!(age, 1234);
                let decoded = data.to_vec();
                assert_eq!(&decoded[..5], b"hello");
                assert_eq!(decoded[5], 0x1B);
            }
            other => panic!("expected ExtendedOutput, got {:?}", other),
        }
    }

    #[test]
    fn parse_layout_change() {
        let line = "%layout-change @1 abc1,200x50,0,0";
        let event = parse_line(line).unwrap();
        assert_eq!(
            event,
            ControlEvent::LayoutChange {
                window_id: "@1".to_string(),
                layout: "abc1,200x50,0,0".to_string(),
            }
        );
    }

    #[test]
    fn parse_session_changed() {
        let line = "%session-changed $2 my-session";
        let event = parse_line(line).unwrap();
        assert_eq!(
            event,
            ControlEvent::SessionChanged {
                session_id: "$2".to_string(),
                name: "my-session".to_string(),
            }
        );
    }

    #[test]
    fn parse_window_add() {
        let line = "%window-add @3";
        let event = parse_line(line).unwrap();
        assert_eq!(
            event,
            ControlEvent::WindowAdd {
                window_id: "@3".to_string(),
            }
        );
    }

    #[test]
    fn parse_window_close() {
        let line = "%window-close @7";
        let event = parse_line(line).unwrap();
        assert_eq!(
            event,
            ControlEvent::WindowClose {
                window_id: "@7".to_string(),
            }
        );
    }

    #[test]
    fn parse_pane_mode_changed() {
        let line = "%pane-mode-changed %2";
        let event = parse_line(line).unwrap();
        assert_eq!(
            event,
            ControlEvent::PaneModeChanged {
                pane_id: "%2".to_string(),
            }
        );
    }

    #[test]
    fn parse_exit_with_reason() {
        let line = "%exit server exited";
        let event = parse_line(line).unwrap();
        assert_eq!(
            event,
            ControlEvent::Exit {
                reason: "server exited".to_string(),
            }
        );
    }

    #[test]
    fn parse_exit_no_reason() {
        let line = "%exit";
        let event = parse_line(line).unwrap();
        assert_eq!(
            event,
            ControlEvent::Exit {
                reason: "".to_string(),
            }
        );
    }

    #[test]
    fn parse_unknown_event() {
        let line = "%something-new @1 data";
        let event = parse_line(line).unwrap();
        assert_eq!(
            event,
            ControlEvent::Unknown("%something-new @1 data".to_string())
        );
    }

    #[test]
    fn parse_non_control_line_returns_none() {
        assert!(parse_line("not a control line").is_none());
        assert!(parse_line("").is_none());
        assert!(parse_line("1234").is_none());
    }

    #[test]
    fn parse_line_strips_trailing_newlines() {
        let line = "%window-add @1\r\n";
        let event = parse_line(line).unwrap();
        assert_eq!(
            event,
            ControlEvent::WindowAdd {
                window_id: "@1".to_string(),
            }
        );
    }

    #[test]
    fn parse_output_with_emoji() {
        let line = r"%output %0 \360\237\230\200";
        let event = parse_line(line).unwrap();
        match event {
            ControlEvent::Output { pane_id, data } => {
                assert_eq!(pane_id, "%0");
                assert_eq!(String::from_utf8_lossy(&data), "😀");
            }
            other => panic!("expected Output, got {:?}", other),
        }
    }

    #[test]
    fn parse_output_real_tmux_prompt() {
        // Simulate a realistic tmux control mode output with a shell prompt:
        // ESC]0;user@host: ~ BEL user@host:~$
        let line = r"%output %0 \033]0;user@host: ~\007user@host:~$ ";
        let event = parse_line(line).unwrap();
        match event {
            ControlEvent::Output { pane_id, data } => {
                assert_eq!(pane_id, "%0");
                let decoded = data.to_vec();
                // Starts with ESC ] 0 ;
                assert_eq!(decoded[0], 0x1B);
                assert_eq!(decoded[1], b']');
                assert_eq!(decoded[2], b'0');
                assert_eq!(decoded[3], b';');
                // Contains BEL (0x07)
                assert!(decoded.contains(&0x07));
            }
            other => panic!("expected Output, got {:?}", other),
        }
    }

    #[test]
    fn parse_session_changed_with_spaces_in_name() {
        // Session names can technically contain spaces
        let line = "%session-changed $0 my session name";
        let event = parse_line(line).unwrap();
        assert_eq!(
            event,
            ControlEvent::SessionChanged {
                session_id: "$0".to_string(),
                name: "my session name".to_string(),
            }
        );
    }

    #[test]
    fn parse_layout_change_complex_layout() {
        // Real tmux layout string
        let line = "%layout-change @0 d3da,211x50,0,0{105x50,0,0,0,105x50,106,0,3}";
        let event = parse_line(line).unwrap();
        assert_eq!(
            event,
            ControlEvent::LayoutChange {
                window_id: "@0".to_string(),
                layout: "d3da,211x50,0,0{105x50,0,0,0,105x50,106,0,3}".to_string(),
            }
        );
    }
}
