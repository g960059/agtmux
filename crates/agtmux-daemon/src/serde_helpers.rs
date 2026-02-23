use serde::{Deserialize, Serialize};

/// Serialize an enum variant to its serde snake_case string representation.
///
/// serde_json serializes a unit enum variant as a JSON string, e.g. `"waiting_input"`.
/// We strip the surrounding quotes to get the raw string.
pub fn serde_variant_name<T: Serialize>(value: &T) -> String {
    let json = serde_json::to_string(value).unwrap_or_default();
    json.trim_matches('"').to_string()
}

/// Parse a serde snake_case string back into an enum variant.
pub fn parse_enum<T: for<'de> Deserialize<'de>>(s: &str) -> Option<T> {
    let json = format!("\"{}\"", s);
    serde_json::from_str(&json).ok()
}
