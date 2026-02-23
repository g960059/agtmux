use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;

use agtmux_tmux::pipe_pane::PaneTap;

/// Validate that a pane_id matches the tmux format `%<digits>`.
pub fn validate_pane_id(pane_id: &str) -> bool {
    pane_id.len() >= 2
        && pane_id.starts_with('%')
        && pane_id[1..].bytes().all(|b| b.is_ascii_digit())
}

const MAX_PANE_DIMENSION: u16 = 500;

// ---------------------------------------------------------------------------
// Binary frame format
// ---------------------------------------------------------------------------

/// Encode a binary frame: `[pane_id_len: u8][pane_id: bytes][output_data: bytes]`
pub fn encode_output_frame(pane_id: &str, data: &[u8]) -> Vec<u8> {
    let id_bytes = pane_id.as_bytes();
    let id_len = id_bytes.len().min(255) as u8;
    let mut frame = Vec::with_capacity(1 + id_len as usize + data.len());
    frame.push(id_len);
    frame.extend_from_slice(&id_bytes[..id_len as usize]);
    frame.extend_from_slice(data);
    frame
}

/// Decode a binary frame. Returns `(pane_id, output_data)`.
pub fn decode_output_frame(frame: &[u8]) -> Option<(&str, &[u8])> {
    if frame.is_empty() {
        return None;
    }
    let id_len = frame[0] as usize;
    if frame.len() < 1 + id_len {
        return None;
    }
    let pane_id = std::str::from_utf8(&frame[1..1 + id_len]).ok()?;
    let data = &frame[1 + id_len..];
    Some((pane_id, data))
}

// ---------------------------------------------------------------------------
// OutputBroadcaster
// ---------------------------------------------------------------------------

/// Payload sent over the output broadcast channel.
#[derive(Debug, Clone)]
pub struct PaneOutput {
    pub pane_id: String,
    pub data: Vec<u8>,
}

/// Manages PaneTap instances and broadcasts terminal output to subscribers.
///
/// Each pane gets at most one PaneTap. When the first client subscribes to a
/// pane, a PaneTap is started and a read loop spawned. When the last client
/// unsubscribes, the PaneTap is stopped.
pub struct OutputBroadcaster {
    output_tx: broadcast::Sender<PaneOutput>,
    inner: Arc<Mutex<BroadcasterInner>>,
}

struct BroadcasterInner {
    /// Reference counts: pane_id -> number of subscribers.
    subscribers: HashMap<String, usize>,
    /// Active read-loop handles and their cancellation tokens.
    tasks: HashMap<String, (tokio::task::JoinHandle<()>, CancellationToken)>,
}

impl OutputBroadcaster {
    pub fn new() -> (Self, broadcast::Receiver<PaneOutput>) {
        let (output_tx, output_rx) = broadcast::channel(256);
        let inner = Arc::new(Mutex::new(BroadcasterInner {
            subscribers: HashMap::new(),
            tasks: HashMap::new(),
        }));
        (Self { output_tx, inner }, output_rx)
    }

    pub fn subscribe_receiver(&self) -> broadcast::Receiver<PaneOutput> {
        self.output_tx.subscribe()
    }

    pub async fn subscribe_pane(&self, pane_id: &str) {
        let mut inner = self.inner.lock().await;
        let count = inner.subscribers.entry(pane_id.to_string()).or_insert(0);
        *count += 1;
        if *count == 1 {
            let tx = self.output_tx.clone();
            let pane_id_owned = pane_id.to_string();
            let token = CancellationToken::new();
            let token_clone = token.clone();
            let handle = tokio::spawn(async move {
                run_pane_tap(&pane_id_owned, tx, token_clone).await;
            });
            inner.tasks.insert(pane_id.to_string(), (handle, token));
        }
    }

    pub async fn unsubscribe_pane(&self, pane_id: &str) {
        let mut inner = self.inner.lock().await;
        if let Some(count) = inner.subscribers.get_mut(pane_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                inner.subscribers.remove(pane_id);
                if let Some((handle, token)) = inner.tasks.remove(pane_id) {
                    token.cancel();
                    let _ = handle.await;
                }
            }
        }
    }

    pub async fn unsubscribe_all(&self, pane_ids: &HashSet<String>) {
        for pane_id in pane_ids {
            self.unsubscribe_pane(pane_id).await;
        }
    }
}

async fn run_pane_tap(pane_id: &str, tx: broadcast::Sender<PaneOutput>, token: CancellationToken) {
    let mut tap = PaneTap::new(pane_id);
    if let Err(e) = tap.start().await {
        tracing::error!(pane_id = %pane_id, error = %e, "failed to start PaneTap");
        return;
    }

    loop {
        tokio::select! {
            _ = token.cancelled() => {
                break;
            }
            result = tap.read() => {
                match result {
                    Ok(Some(data)) => {
                        let _ = tx.send(PaneOutput {
                            pane_id: pane_id.to_string(),
                            data,
                        });
                    }
                    Ok(None) => {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                    Err(e) => {
                        tracing::warn!(pane_id = %pane_id, error = %e, "PaneTap read error");
                        break;
                    }
                }
            }
        }
    }

    let _ = tap.stop().await;
}

// ---------------------------------------------------------------------------
// Tmux input / resize helpers
// ---------------------------------------------------------------------------

/// Send keys to a tmux pane via `tmux send-keys -t <pane_id> -l <data>`.
pub async fn send_keys(pane_id: &str, data: &str) -> Result<(), String> {
    let output = tokio::process::Command::new("tmux")
        .args(["send-keys", "-t", pane_id, "-l", data])
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "tmux send-keys exited {}: {}",
            output.status.code().unwrap_or(-1),
            stderr.trim(),
        ));
    }
    Ok(())
}

/// Resize a tmux pane via `tmux resize-pane -t <pane_id> -x <cols> -y <rows>`.
///
/// `cols` and `rows` must be in the range `1..=500`.
pub async fn resize_pane(pane_id: &str, cols: u16, rows: u16) -> Result<(), String> {
    if cols < 1 || cols > MAX_PANE_DIMENSION || rows < 1 || rows > MAX_PANE_DIMENSION {
        return Err(format!(
            "cols and rows must be in 1..={MAX_PANE_DIMENSION}, got cols={cols} rows={rows}",
        ));
    }
    let output = tokio::process::Command::new("tmux")
        .args([
            "resize-pane",
            "-t",
            pane_id,
            "-x",
            &cols.to_string(),
            "-y",
            &rows.to_string(),
        ])
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "tmux resize-pane exited {}: {}",
            output.status.code().unwrap_or(-1),
            stderr.trim(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trip() {
        let pane_id = "%1";
        let data = b"hello world\x1b[31m";
        let frame = encode_output_frame(pane_id, data);

        let (decoded_id, decoded_data) = decode_output_frame(&frame).unwrap();
        assert_eq!(decoded_id, pane_id);
        assert_eq!(decoded_data, data);
    }

    #[test]
    fn encode_decode_empty_data() {
        let frame = encode_output_frame("%42", b"");
        let (id, data) = decode_output_frame(&frame).unwrap();
        assert_eq!(id, "%42");
        assert!(data.is_empty());
    }

    #[test]
    fn encode_decode_long_pane_id() {
        let pane_id = "%123456789";
        let data = b"\x00\x01\x02";
        let frame = encode_output_frame(pane_id, data);
        let (id, d) = decode_output_frame(&frame).unwrap();
        assert_eq!(id, pane_id);
        assert_eq!(d, data);
    }

    #[test]
    fn decode_empty_frame_returns_none() {
        assert!(decode_output_frame(&[]).is_none());
    }

    #[test]
    fn decode_truncated_frame_returns_none() {
        // id_len says 5 bytes but only 2 are available
        assert!(decode_output_frame(&[5, b'a', b'b']).is_none());
    }

    #[test]
    fn frame_format_is_correct() {
        let frame = encode_output_frame("%1", b"AB");
        // [2, '%', '1', 'A', 'B']
        assert_eq!(frame.len(), 5);
        assert_eq!(frame[0], 2); // pane_id_len
        assert_eq!(&frame[1..3], b"%1");
        assert_eq!(&frame[3..5], b"AB");
    }

    #[test]
    fn decode_binary_data_preserved() {
        let data: Vec<u8> = (0..=255).collect();
        let frame = encode_output_frame("%0", &data);
        let (id, decoded) = decode_output_frame(&frame).unwrap();
        assert_eq!(id, "%0");
        assert_eq!(decoded, data.as_slice());
    }

    // ----- JSON-RPC request parsing tests -----

    #[test]
    fn parse_subscribe_output_request() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"subscribe_output","params":{"pane_id":"%1"}}"#;
        let req: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(req["method"], "subscribe_output");
        let pane_id = req["params"]["pane_id"].as_str().unwrap();
        assert_eq!(pane_id, "%1");
    }

    #[test]
    fn parse_unsubscribe_output_request() {
        let json = r#"{"jsonrpc":"2.0","id":5,"method":"unsubscribe_output","params":{"pane_id":"%3"}}"#;
        let req: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(req["method"], "unsubscribe_output");
        assert_eq!(req["params"]["pane_id"], "%3");
    }

    #[test]
    fn parse_write_input_request() {
        let json = r#"{"jsonrpc":"2.0","id":2,"method":"write_input","params":{"pane_id":"%1","data":"ls\n"}}"#;
        let req: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(req["method"], "write_input");
        assert_eq!(req["params"]["pane_id"], "%1");
        assert_eq!(req["params"]["data"], "ls\n");
    }

    #[test]
    fn parse_resize_pane_request() {
        let json = r#"{"jsonrpc":"2.0","id":3,"method":"resize_pane","params":{"pane_id":"%1","cols":80,"rows":24}}"#;
        let req: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(req["method"], "resize_pane");
        assert_eq!(req["params"]["pane_id"], "%1");
        assert_eq!(req["params"]["cols"], 80);
        assert_eq!(req["params"]["rows"], 24);
    }

    #[test]
    fn subscribe_output_params_deserialization() {
        #[derive(serde::Deserialize)]
        struct Params {
            pane_id: String,
        }
        let json = r#"{"pane_id": "%42"}"#;
        let p: Params = serde_json::from_str(json).unwrap();
        assert_eq!(p.pane_id, "%42");
    }

    #[test]
    fn write_input_params_deserialization() {
        #[derive(serde::Deserialize)]
        struct Params {
            pane_id: String,
            data: String,
        }
        let json = r#"{"pane_id": "%1", "data": "echo hello\n"}"#;
        let p: Params = serde_json::from_str(json).unwrap();
        assert_eq!(p.pane_id, "%1");
        assert_eq!(p.data, "echo hello\n");
    }

    #[test]
    fn resize_pane_params_deserialization() {
        #[derive(serde::Deserialize)]
        struct Params {
            pane_id: String,
            cols: u16,
            rows: u16,
        }
        let json = r#"{"pane_id": "%1", "cols": 120, "rows": 40}"#;
        let p: Params = serde_json::from_str(json).unwrap();
        assert_eq!(p.pane_id, "%1");
        assert_eq!(p.cols, 120);
        assert_eq!(p.rows, 40);
    }

    // ----- pane_id validation tests -----

    #[test]
    fn validate_pane_id_accepts_valid() {
        assert!(validate_pane_id("%0"));
        assert!(validate_pane_id("%1"));
        assert!(validate_pane_id("%42"));
        assert!(validate_pane_id("%12345"));
    }

    #[test]
    fn validate_pane_id_rejects_empty() {
        assert!(!validate_pane_id(""));
    }

    #[test]
    fn validate_pane_id_rejects_bare_percent() {
        assert!(!validate_pane_id("%"));
    }

    #[test]
    fn validate_pane_id_rejects_no_percent() {
        assert!(!validate_pane_id("1"));
        assert!(!validate_pane_id("42"));
    }

    #[test]
    fn validate_pane_id_rejects_session_specifier() {
        assert!(!validate_pane_id("mysession:%1"));
        assert!(!validate_pane_id("s:w.1"));
    }

    #[test]
    fn validate_pane_id_rejects_shell_metacharacters() {
        assert!(!validate_pane_id("%1; rm -rf /"));
        assert!(!validate_pane_id("%1$(whoami)"));
        assert!(!validate_pane_id("%1`id`"));
    }

    #[test]
    fn validate_pane_id_rejects_letters_after_percent() {
        assert!(!validate_pane_id("%abc"));
        assert!(!validate_pane_id("%1a"));
    }

    // ----- resize bounds tests -----

    #[tokio::test]
    async fn resize_pane_rejects_zero_cols() {
        let err = resize_pane("%1", 0, 24).await.unwrap_err();
        assert!(err.contains("1..=500"));
    }

    #[tokio::test]
    async fn resize_pane_rejects_zero_rows() {
        let err = resize_pane("%1", 80, 0).await.unwrap_err();
        assert!(err.contains("1..=500"));
    }

    #[tokio::test]
    async fn resize_pane_rejects_oversized_cols() {
        let err = resize_pane("%1", 501, 24).await.unwrap_err();
        assert!(err.contains("1..=500"));
    }

    #[tokio::test]
    async fn resize_pane_rejects_oversized_rows() {
        let err = resize_pane("%1", 80, 501).await.unwrap_err();
        assert!(err.contains("1..=500"));
    }

    #[tokio::test]
    async fn resize_pane_rejects_u16_max() {
        let err = resize_pane("%1", u16::MAX, u16::MAX).await.unwrap_err();
        assert!(err.contains("1..=500"));
    }
}
