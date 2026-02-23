/// State indicator symbols used across TUI and status output.
pub const INDICATOR_RUNNING: &str = "●";
pub const INDICATOR_APPROVAL: &str = "◉";
pub const INDICATOR_INPUT: &str = "◈";
pub const INDICATOR_IDLE: &str = "○";
pub const INDICATOR_UNKNOWN: &str = "◌";
pub const INDICATOR_ERROR: &str = "✖";

/// Map an activity state string to its indicator symbol.
pub fn state_indicator(activity_state: &str) -> &'static str {
    match activity_state {
        "running" => INDICATOR_RUNNING,
        "waiting_approval" => INDICATOR_APPROVAL,
        "waiting_input" => INDICATOR_INPUT,
        "idle" => INDICATOR_IDLE,
        "error" => INDICATOR_ERROR,
        _ => INDICATOR_UNKNOWN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_indicator_all_states() {
        assert_eq!(state_indicator("running"), "●");
        assert_eq!(state_indicator("waiting_approval"), "◉");
        assert_eq!(state_indicator("waiting_input"), "◈");
        assert_eq!(state_indicator("idle"), "○");
        assert_eq!(state_indicator("error"), "✖");
        assert_eq!(state_indicator("unknown"), "◌");
        assert_eq!(state_indicator("anything_else"), "◌");
    }

    #[test]
    fn state_indicator_empty_string_falls_through() {
        assert_eq!(state_indicator(""), "◌");
    }

    #[test]
    fn indicator_constants_match_expected_unicode() {
        assert_eq!(INDICATOR_RUNNING, "\u{25CF}");
        assert_eq!(INDICATOR_APPROVAL, "\u{25C9}");
        assert_eq!(INDICATOR_INPUT, "\u{25C8}");
        assert_eq!(INDICATOR_IDLE, "\u{25CB}");
        assert_eq!(INDICATOR_UNKNOWN, "\u{25CC}");
        assert_eq!(INDICATOR_ERROR, "\u{2716}");
    }
}
