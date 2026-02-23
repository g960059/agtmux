use std::path::Path;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::server::PaneInfo;

/// Minimal client for the agtmux daemon JSON-RPC Unix socket API.
pub struct DaemonClient {
    stream: BufReader<UnixStream>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    #[allow(dead_code)]
    id: Option<u64>,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    #[allow(dead_code)]
    code: i32,
    message: String,
}

#[derive(Debug, Deserialize)]
struct ListPanesResult {
    panes: Vec<PaneInfo>,
}

/// Parse a raw JSON-RPC response line into a `Vec<PaneInfo>`.
///
/// This is extracted from `DaemonClient::list_panes` so it can be unit-tested
/// without a live socket connection.
fn parse_list_panes_response(line: &str) -> Result<Vec<PaneInfo>, Box<dyn std::error::Error>> {
    let resp: JsonRpcResponse = serde_json::from_str(line)?;
    if let Some(err) = resp.error {
        return Err(format!("daemon error: {}", err.message).into());
    }

    let result_value = resp
        .result
        .ok_or("missing result in response")?;
    let list: ListPanesResult = serde_json::from_value(result_value)?;
    Ok(list.panes)
}

impl DaemonClient {
    /// Connect to the daemon at the given Unix socket path.
    pub async fn connect(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let stream = UnixStream::connect(path).await?;
        Ok(Self {
            stream: BufReader::new(stream),
        })
    }

    /// Call `list_panes` and return the current pane state snapshot.
    pub async fn list_panes(&mut self) -> Result<Vec<PaneInfo>, Box<dyn std::error::Error>> {
        let request = r#"{"jsonrpc":"2.0","id":1,"method":"list_panes","params":{}}"#;

        // Write the request as a newline-delimited JSON line.
        let writer = self.stream.get_mut();
        writer.write_all(request.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;

        // Read the response line.
        let mut line = String::new();
        self.stream.read_line(&mut line).await?;

        parse_list_panes_response(&line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_panes_response_success() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"panes":[{"pane_id":"%1","session_name":"s","window_id":"@1","pane_title":"","current_cmd":"claude","provider":"claude","provider_confidence":0.95,"activity_state":"running","activity_confidence":0.9,"activity_source":"hook","attention_state":"none","attention_reason":"","attention_since":null,"updated_at":"2026-01-01T00:00:00Z"}]}}"#;
        let panes = parse_list_panes_response(json).expect("should parse successfully");
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].pane_id, "%1");
        assert_eq!(panes[0].session_name, "s");
        assert_eq!(panes[0].activity_state, "running");
        assert_eq!(panes[0].provider, Some("claude".into()));
        assert!((panes[0].activity_confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_list_panes_response_error() {
        let json = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"method not found"}}"#;
        let result = parse_list_panes_response(json);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("method not found"),
            "error message should contain the daemon error: {}",
            err_msg,
        );
    }

    #[test]
    fn parse_list_panes_response_empty_panes() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"panes":[]}}"#;
        let panes = parse_list_panes_response(json).expect("should parse successfully");
        assert!(panes.is_empty());
    }

    #[test]
    fn parse_list_panes_response_missing_result() {
        let json = r#"{"jsonrpc":"2.0","id":1}"#;
        let result = parse_list_panes_response(json);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("missing result"),
            "should report missing result: {}",
            err_msg,
        );
    }

    #[test]
    fn parse_list_panes_response_invalid_json() {
        let result = parse_list_panes_response("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn parse_list_panes_response_multiple_panes() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"panes":[
            {"pane_id":"%1","session_name":"s","window_id":"@1","pane_title":"","current_cmd":"claude","provider":"claude","provider_confidence":0.95,"activity_state":"running","activity_confidence":0.9,"activity_source":"hook","attention_state":"none","attention_reason":"","attention_since":null,"updated_at":"2026-01-01T00:00:00Z"},
            {"pane_id":"%2","session_name":"dev","window_id":"@2","pane_title":"test","current_cmd":"codex","provider":"codex","provider_confidence":0.8,"activity_state":"idle","activity_confidence":0.7,"activity_source":"poller","attention_state":"stale","attention_reason":"no output","attention_since":"2026-01-01T00:05:00Z","updated_at":"2026-01-01T00:05:00Z"}
        ]}}"#;
        let panes = parse_list_panes_response(json).expect("should parse successfully");
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].pane_id, "%1");
        assert_eq!(panes[1].pane_id, "%2");
        assert_eq!(panes[1].provider, Some("codex".into()));
        assert_eq!(panes[1].attention_state, "stale");
        assert_eq!(panes[1].attention_since, Some("2026-01-01T00:05:00Z".into()));
    }

    #[test]
    fn parse_list_panes_response_without_jsonrpc_still_works() {
        // Backward compatibility: responses without jsonrpc field should still parse.
        let json = r#"{"id":1,"result":{"panes":[]}}"#;
        let panes = parse_list_panes_response(json).expect("should parse successfully");
        assert!(panes.is_empty());
    }
}
