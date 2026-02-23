// ---------------------------------------------------------------------------
// Minimal Tauri shell — the frontend connects directly to the daemon via
// WebSocket (ws://127.0.0.1:9780), so no Rust-side bridging is needed.
// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
