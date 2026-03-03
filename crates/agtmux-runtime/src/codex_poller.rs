//! Codex App Server integration.
//!
//! Three evidence paths for Codex:
//!
//! 1. **App Server client** (primary for historical sessions): Spawns `codex app-server`
//!    in stdio mode, performs JSON-RPC 2.0 handshake, and polls `thread/list`.
//!    Note: in Codex v0.106.0 the App Server only returns *historical* sessions
//!    (those already committed to the SQLite DB).  Newly started `codex --full-auto`
//!    sessions are NOT visible via `thread/list` while they are running.
//!    See <https://developers.openai.com/codex/app-server/>.
//!
//! 2. **JSONL file scanner** (primary for *currently running* sessions): Scans
//!    `~/.codex/sessions/YYYY/MM/DD/*.jsonl` for recently modified rollout files.
//!    Codex writes the session JSONL at startup (T+5-6 s) and keeps updating it
//!    while running, so the file mtime is a reliable proxy for activity.  The CWD
//!    is read from the `session_meta.payload.cwd` field of the first line.
//!    This path produces `EvidenceTier::Deterministic` events.
//!
//! 3. **Capture-based extraction** (fallback): When `codex exec --json` outputs
//!    NDJSON events to stdout, they appear in tmux capture text and can be parsed
//!    as deterministic evidence without an app-server connection.
//!
//! The poll_loop tries the app-server first. If spawning fails (no `codex` binary,
//! auth issues, etc.), the capture-based fallback is used automatically.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use agtmux_source_codex_appserver::translate::CodexRawEvent;
use agtmux_tmux_v5::ProcessMap;
use chrono::{DateTime, Utc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

// ─── Pane CWD info for cwd correlation (T-119) ─────────────────────

/// Pane metadata used for cwd ↔ thread correlation.
#[derive(Debug, Clone)]
pub struct PaneCwdInfo {
    /// tmux pane ID (e.g. "%5").
    pub pane_id: String,
    /// Current working directory of the pane.
    pub cwd: String,
    /// Pane generation from PaneGenerationTracker.
    pub generation: Option<u64>,
    /// Pane birth timestamp from PaneGenerationTracker.
    pub birth_ts: Option<chrono::DateTime<Utc>>,
    /// The agent CLI detected in this pane's current command, if any.
    /// Examples: `Some("codex")`, `Some("claude")`, `None` for neutral shells.
    /// Used for 3-tier assignment priority: codex(0) > neutral(1) > competing-agent(2).
    pub process_hint: Option<String>,
    /// PID of the shell started in this pane (tmux `#{pane_pid}`).
    /// Used for process-first codex JSONL detection: find child codex processes,
    /// then find their open JSONL files via lsof — bypasses CWD ambiguity and date limits.
    pub pane_pid: Option<u32>,
}

/// Correlated pane identity for a Codex thread.
#[derive(Debug, Clone)]
struct ThreadPaneBinding {
    pane_id: String,
    pane_generation: Option<u64>,
    pane_birth_ts: Option<chrono::DateTime<Utc>>,
}

/// Last emitted thread state (used for deduplication + heartbeat).
#[derive(Debug, Clone)]
struct LastThreadState {
    status: String,
    pane_id: Option<String>,
    /// When this state was last emitted (for heartbeat re-emission).
    emitted_at: DateTime<Utc>,
}

/// Heartbeat interval: re-emit unchanged thread state to keep
/// deterministic freshness alive. Must be shorter than
/// `agtmux_core_v5::resolver::FRESH_THRESHOLD_SECS` (3s).
const HEARTBEAT_INTERVAL_SECS: i64 = 2;

/// Maximum JSONL session file age (seconds) to classify a `notLoaded` thread as "active".
/// Must exceed the longest expected tool operation (e.g., `sleep 15` → 30 s margin).
const NOTLOADED_ACTIVE_THRESHOLD_SECS: u64 = 30;

/// Maximum JSONL session file age (seconds) to classify a `notLoaded` thread as "idle".
/// Beyond this, the thread is treated as a historical session and skipped.
const NOTLOADED_IDLE_THRESHOLD_SECS: u64 = 120;

/// Classify a `notLoaded` Codex thread by checking how recently its session JSONL
/// file was modified.
///
/// `codex app-server` (v0.106.0) reports every `codex --full-auto` session as
/// `status.type = "notLoaded"` regardless of whether it is currently executing.
/// We use the JSONL file's mtime as a proxy for activity:
///
/// - mtime < [`NOTLOADED_ACTIVE_THRESHOLD_SECS`] → `"active"` (session recently writing)
/// - mtime < [`NOTLOADED_IDLE_THRESHOLD_SECS`]   → `"idle"`   (session just finished)
/// - mtime ≥ [`NOTLOADED_IDLE_THRESHOLD_SECS`]   → `None`     (historical, skip)
///
/// Returns `None` if the path is empty, the file cannot be stat'd, or the file
/// is too old to be relevant.
fn classify_notloaded_status(session_path: &str) -> Option<&'static str> {
    if session_path.is_empty() {
        return None;
    }
    let age_secs = std::fs::metadata(session_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|mtime| mtime.elapsed().ok())
        .map(|age| age.as_secs())?;

    if age_secs <= NOTLOADED_ACTIVE_THRESHOLD_SECS {
        Some("active")
    } else if age_secs <= NOTLOADED_IDLE_THRESHOLD_SECS {
        Some("idle")
    } else {
        None // Historical session — skip
    }
}

// ─── Direct JSONL file scanner (T-CODEX-JSONL) ─────────────────────
//
// codex app-server v0.106.0 does NOT expose currently running sessions via
// thread/list — it only shows historical (completed) sessions from the SQLite
// database.  As a workaround, we scan today's JSONL rollout files directly.
//
// Layout: ~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl
// First line: {"type":"session_meta","payload":{"cwd":"<pane-cwd>",...},...}
//
// Rules:
//   mtime < JSONL_ACTIVE_THRESHOLD_SECS → thread.active  (codex currently writing)
//   mtime < JSONL_IDLE_THRESHOLD_SECS   → thread.idle    (recently finished)
//   mtime ≥ JSONL_IDLE_THRESHOLD_SECS   → skip           (historical, not relevant)

/// Active threshold: must exceed the longest expected write gap (e.g., `sleep 15` = 15 s gap).
const JSONL_ACTIVE_THRESHOLD_SECS: u64 = 25;
/// Idle threshold: emit thread.idle for recently completed sessions.
const JSONL_IDLE_THRESHOLD_SECS: u64 = 120;

/// Return true when the `args` string belongs to a Codex CLI process.
///
/// Matches both the compiled binary (`codex`) and Node.js invocations
/// (`node /path/to/codex/dist/cli.mjs`).  Explicitly excludes agtmux
/// processes whose paths happen to contain the string "codex".
fn is_codex_process_args(args: &str) -> bool {
    if args.contains("/agtmux") {
        return false;
    }
    // Binary: "codex" or "codex --full-auto ..."
    let first_token = args.split_whitespace().next().unwrap_or("");
    let binary_name = std::path::Path::new(first_token)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(first_token);
    if binary_name == "codex" {
        return true;
    }
    // Node.js wrapper: "node .../codex/dist/cli.mjs ..."
    args.contains("codex") && (binary_name == "node" || binary_name == "node_modules")
}

/// Scan the open file descriptors of `pid` for a Codex JSONL session file.
///
/// Uses `lsof -p <pid> -Fan` (field output mode: access-mode + name).
/// Returns `(path, is_write_open)` for the first matching file found, or
/// `None` if no JSONL under `sessions_base` is currently open by `pid`.
fn find_open_jsonl_for_pid(pid: u32, sessions_base: &str) -> Option<(std::path::PathBuf, bool)> {
    let output = std::process::Command::new("lsof")
        .args(["-p", &pid.to_string(), "-Fan"])
        .output()
        .ok()?;

    if output.stdout.is_empty() {
        return None;
    }

    // lsof -Fan field output format (per open-file record):
    //   f<fd>   — file descriptor number (starts a new record)
    //   a<mode> — access mode: r=read, w=write-only, u=read/write
    //   n<path> — file name / path
    let text = String::from_utf8_lossy(&output.stdout);
    let mut current_write_open = false;

    for line in text.lines() {
        match line.as_bytes().first() {
            Some(b'f') => {
                // Start of a new fd record — reset access mode.
                current_write_open = false;
            }
            Some(b'a') => {
                let mode = &line[1..];
                current_write_open = mode == "w" || mode == "u";
            }
            Some(b'n') => {
                let path_str = &line[1..];
                if path_str.starts_with(sessions_base) && path_str.ends_with(".jsonl") {
                    return Some((std::path::PathBuf::from(path_str), current_write_open));
                }
                // Reset for the next record.
                current_write_open = false;
            }
            _ => {}
        }
    }
    None
}

/// Process-first Codex JSONL detection.
///
/// Given a pane's shell PID, searches `process_map` for direct child
/// processes that look like Codex, then probes each child's open file
/// descriptors for a JSONL session file under `sessions_base`.
///
/// Returns `(jsonl_path, is_write_open)` for the first match found.
/// Returns `None` when no running Codex child owns an open session file.
fn find_codex_child_jsonl(
    pane_pid: u32,
    process_map: &ProcessMap,
    sessions_base: &str,
) -> Option<(std::path::PathBuf, bool)> {
    // Collect direct children of this pane's shell that look like Codex.
    let codex_children: Vec<u32> = process_map
        .values()
        .filter(|p| p.ppid == pane_pid && is_codex_process_args(&p.args))
        .map(|p| p.pid)
        .collect();

    for child_pid in codex_children {
        if let Some(result) = find_open_jsonl_for_pid(child_pid, sessions_base) {
            return Some(result);
        }
    }
    None
}

/// Metadata read from the `session_meta` first line of a Codex JSONL rollout file.
struct JsonlSessionMeta {
    cwd: String,
    /// First user task text — extracted from the first `response_item` with role=user
    /// whose content does not start with `#` (AGENTS.md injection) or `<` (XML-style
    /// environment context).  Truncated to 120 chars.  `None` if not found within the
    /// first 30 lines after `session_meta`.
    instructions: Option<String>,
}

/// Read metadata from the `session_meta` first line of a Codex JSONL rollout file.
/// Returns `None` if the file cannot be read or the first line is not `session_meta`.
///
/// After reading the CWD from `session_meta`, scans forward up to 30 lines to find
/// the first genuine user task message (skipping AGENTS.md context injections and
/// XML-style environment blocks).
fn read_jsonl_session_meta(path: &std::path::Path) -> Option<JsonlSessionMeta> {
    use std::io::BufRead as _;
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let obj: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if obj.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
        return None;
    }
    let payload = obj.get("payload")?;
    let cwd = payload
        .get("cwd")
        .and_then(|c| c.as_str())
        .map(String::from)?;

    // Scan forward through subsequent lines to find the first genuine user task.
    // Codex JSONL layout (typical):
    //   line 1: session_meta        (cwd, base_instructions — no task text)
    //   line 2: response_item       role=developer  (permissions/sandbox)
    //   line 3: response_item       role=user       (AGENTS.md context injection → skip)
    //   line 4: event_msg           task_started
    //   line 5: response_item       role=developer  (collaboration mode)
    //   line 6: response_item       role=user       ← actual task text ✓
    //
    // Heuristic: the first `response_item` role=user content item that doesn't start
    // with `#` (AGENTS.md header) or `<` (XML-style context block) is the task.
    let mut instructions: Option<String> = None;
    for _ in 0..30 {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break; // EOF
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(evt) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        if evt.get("type").and_then(|t| t.as_str()) != Some("response_item") {
            continue;
        }
        let evt_payload = match evt.get("payload").and_then(|p| p.as_object()) {
            Some(p) => p,
            None => continue,
        };
        if evt_payload.get("role").and_then(|r| r.as_str()) != Some("user") {
            continue;
        }
        let content = match evt_payload.get("content").and_then(|c| c.as_array()) {
            Some(c) => c,
            None => continue,
        };
        for item in content {
            let text = match item.get("text").and_then(|t| t.as_str()) {
                Some(t) if !t.is_empty() => t,
                _ => continue,
            };
            // Skip injected context blocks.
            if text.starts_with('#') || text.starts_with('<') {
                continue;
            }
            // Found the task. Truncate to 120 chars for a compact title.
            instructions = Some(text.chars().take(120).collect());
            break;
        }
        if instructions.is_some() {
            break;
        }
    }

    Some(JsonlSessionMeta { cwd, instructions })
}

/// Scan Codex JSONL session files and emit deterministic events for panes with
/// a currently running Codex process.
///
/// ## Detection strategy
///
/// **Pass 1 — Process-first** (preferred): For each pane with a known shell
/// PID (`pane_pid`), find child Codex processes in `process_map` and probe
/// their open file descriptors via `lsof`.  This approach:
/// - Ties the JSONL directly to the exact pane (no CWD ambiguity)
/// - Works for session files in any date directory (bypasses today/yesterday limit)
/// - Is unaffected by stale background Codex processes sharing the same CWD
///
/// **Pass 2 — CWD-based** (fallback): For panes not yet matched, scan
/// today's and yesterday's JSONL directories and match by CWD.  Used when
/// `pane_pid` is unavailable or the child process search yields nothing.
///
/// **Pass 3 — Historical enrichment**: For tier-0/1 panes still unmatched,
/// search the last 7 days of JSONL files to populate a correct last-activity
/// timestamp (`actual_activity_at`).
pub(crate) fn scan_jsonl_sessions(
    pane_cwds: &[PaneCwdInfo],
    now: DateTime<Utc>,
    process_map: &ProcessMap,
) -> Vec<CodexRawEvent> {
    let home = match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => h,
        _ => return Vec::new(),
    };

    let sessions_base = format!("{home}/.codex/sessions");

    // Pre-sort panes by tier so that when multiple panes share a CWD, the highest-priority
    // pane (tier 0 = explicit codex, tier 1 = neutral runtime) wins the assignment.
    let mut sorted_panes: Vec<&PaneCwdInfo> = pane_cwds
        .iter()
        .filter(|p| pane_tier(p) < 3) // exclude shell / runtime_unknown upfront
        .collect();
    sorted_panes.sort_by(|a, b| {
        pane_tier(a)
            .cmp(&pane_tier(b))
            .then_with(|| a.pane_id.cmp(&b.pane_id))
    });

    let mut events = Vec::new();
    // Track which panes have already been matched to avoid double-assignment.
    let mut matched_panes: HashSet<String> = HashSet::new();

    // ── Pass 1: Process-first detection ──────────────────────────────────────
    //
    // For panes with a known shell PID, look for child Codex processes in the
    // process map and probe their open file descriptors.  This correctly handles:
    //   - Sessions whose JSONL lives in an old date directory (false negatives)
    //   - Background Codex processes sharing the same CWD (false positives)
    for pane in &sorted_panes {
        let Some(pane_pid) = pane.pane_pid else {
            continue;
        };
        let Some((path, is_write_open)) =
            find_codex_child_jsonl(pane_pid, process_map, &sessions_base)
        else {
            continue;
        };

        let Some(meta) = read_jsonl_session_meta(&path) else {
            tracing::debug!(
                "jsonl-process-first: pane={} pane_pid={} skip {:?} (no session_meta)",
                pane.pane_id,
                pane_pid,
                path.file_name()
            );
            continue;
        };

        let age_secs = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|mtime| mtime.elapsed().ok())
            .map(|d| d.as_secs())
            .unwrap_or(u64::MAX);

        tracing::debug!(
            "jsonl-process-first: pane={} pane_pid={} {:?} age={}s write_open={}",
            pane.pane_id,
            pane_pid,
            path.file_name(),
            age_secs,
            is_write_open
        );

        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| format!("jsonl-{s}"))
            .unwrap_or_else(|| format!("jsonl-{}", pane.pane_id));

        events.push(CodexRawEvent {
            id: format!("jsonlproc-{}-{}", pane.pane_id, now.timestamp_millis()),
            event_type: "thread.active".to_string(),
            session_id,
            timestamp: now,
            pane_id: Some(pane.pane_id.clone()),
            pane_generation: pane.generation,
            pane_birth_ts: pane.birth_ts,
            payload: serde_json::json!({
                "cwd": meta.cwd,
                "jsonl_age_secs": age_secs,
                "jsonl_write_open": is_write_open,
                "source": "process_first",
                "instructions": meta.instructions,
            }),
            is_heartbeat: false,
            actual_activity_at: None,
        });

        matched_panes.insert(pane.pane_id.clone());
    }

    // ── Pass 2: CWD-based scan (fallback for panes without pane_pid) ──────────
    //
    // Scan today's + yesterday's JSONL directories and match by CWD.
    // `is_file_write_open` is intentionally NOT used here: it would flag any
    // process that has the file open, including background processes from other
    // panes that happen to share the same CWD — the root cause of false positives.

    let today = now.format("%Y/%m/%d").to_string();
    let yesterday = (now - chrono::Duration::days(1))
        .format("%Y/%m/%d")
        .to_string();

    let collect_candidates = |date: &str| -> Vec<(std::path::PathBuf, u64)> {
        let dir = std::path::PathBuf::from(format!("{sessions_base}/{date}"));
        let Ok(iter) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        iter.flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    return None;
                }
                let age_secs = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|mtime| mtime.elapsed().ok())
                    .map(|d| d.as_secs())?;

                if age_secs <= JSONL_IDLE_THRESHOLD_SECS {
                    Some((path, age_secs))
                } else {
                    None // Historical — skip (process-first handles still-running cases)
                }
            })
            .collect()
    };

    let mut candidates = collect_candidates(&today);
    candidates.extend(collect_candidates(&yesterday));

    // Sort: active (mtime-fresh) sessions first, then newer first.
    candidates.sort_by_key(|(_, age)| *age);

    tracing::debug!(
        "jsonl-scan: {} candidates, {} eligible panes (today={}, yesterday={}), {} already matched",
        candidates.len(),
        sorted_panes.len(),
        today,
        yesterday,
        matched_panes.len(),
    );

    for (path, age_secs) in candidates {
        let meta = match read_jsonl_session_meta(&path) {
            Some(m) => m,
            None => {
                tracing::debug!("jsonl-scan: skip {:?} (no session_meta)", path.file_name());
                continue;
            }
        };

        let is_active = age_secs <= JSONL_ACTIVE_THRESHOLD_SECS;
        let event_type = if is_active {
            "thread.active"
        } else {
            "thread.idle"
        };

        tracing::debug!(
            "jsonl-scan: {:?} age={}s cwd={} type={}",
            path.file_name(),
            age_secs,
            meta.cwd,
            event_type
        );

        // Match the CWD to the highest-priority eligible pane (tier-sorted above).
        for pane in &sorted_panes {
            if matched_panes.contains(&pane.pane_id) {
                continue;
            }

            // Canonicalize for /tmp → /private/tmp comparison on macOS.
            let pane_canonical = std::fs::canonicalize(&pane.cwd)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| pane.cwd.clone());

            if pane_canonical != meta.cwd {
                tracing::trace!(
                    "jsonl-scan: no match pane={} cwd={} vs meta.cwd={}",
                    pane.pane_id,
                    pane_canonical,
                    meta.cwd
                );
                continue;
            }

            // Derive a stable session key from the filename (uuid part).
            let session_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| format!("jsonl-{s}"))
                .unwrap_or_else(|| format!("jsonl-{}", pane.pane_id));

            events.push(CodexRawEvent {
                id: format!("jsonlscan-{}-{}", pane.pane_id, now.timestamp_millis()),
                event_type: event_type.to_string(),
                session_id,
                timestamp: now,
                pane_id: Some(pane.pane_id.clone()),
                pane_generation: pane.generation,
                pane_birth_ts: pane.birth_ts,
                payload: serde_json::json!({
                    "cwd": meta.cwd,
                    "jsonl_age_secs": age_secs,
                    "source": "jsonl_scan",
                    // T-135a-bis: task instructions used as conversation title.
                    "instructions": meta.instructions,
                }),
                // Idle events are heartbeats: they maintain state continuity without
                // advancing last_real_activity or biasing cross-provider arbitration.
                is_heartbeat: !is_active,
                actual_activity_at: None,
            });

            matched_panes.insert(pane.pane_id.clone());
            break; // One match per file.
        }
    }

    // ── Pass 3: Historical enrichment ────────────────────────────────────────────
    //
    // For heuristic codex panes not matched by the fresh scan (all their JSONL files are
    // older than JSONL_IDLE_THRESHOLD_SECS), search the past 7 days for the most-recently
    // modified JSONL file whose session_meta.cwd matches the pane.  Always emits
    // `thread.idle` with `actual_activity_at = file_mtime` so the projection records
    // the correct last-activity time (instead of "just now" from the daemon start).
    //
    // Design note: we do NOT emit thread.active here even when the file mtime is fresh.
    // Codex writes "keepalive" lines to the JSONL while waiting for user input (burst-write
    // pattern, ~1 write/15 s).  Treating a fresh mtime as "active" causes updatedAt to
    // oscillate every burst, making long-idle sessions flicker as "Running just now".
    // Sessions that are truly running a task are detected by Pass 1 (lsof while writing)
    // or Pass 2 (today/yesterday CWD scan during a burst).
    //
    // `is_heartbeat = false` is intentional: the first emission updates `updated_at` in
    // the projection; subsequent identical emissions are idempotent (same `actual_activity_at`).

    /// Max age (seconds) to consider for historical enrichment (7 days).
    const HISTORICAL_ENRICHMENT_SECS: u64 = 7 * 24 * 3600;

    for pane in &sorted_panes {
        if matched_panes.contains(&pane.pane_id) {
            continue; // Already matched in the fresh scan
        }
        if pane_tier(pane) > 1 {
            continue; // Only enrich tier-0 (codex) and tier-1 (neutral runtime like node)
        }

        let pane_canonical = std::fs::canonicalize(&pane.cwd)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| pane.cwd.clone());

        // Scan recent date directories for a matching JSONL file.
        let mut best: Option<(std::path::PathBuf, u64, JsonlSessionMeta)> = None;

        'outer: for days_ago in 0..=7_i64 {
            let date = (now - chrono::Duration::days(days_ago))
                .format("%Y/%m/%d")
                .to_string();
            let dir = std::path::PathBuf::from(format!("{sessions_base}/{date}"));
            let Ok(iter) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in iter.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let age_secs = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|mtime| mtime.elapsed().ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(u64::MAX);
                // Skip files already handled by the fresh scan (Pass 2).
                // Also skip keepalive-write sessions: a fresh mtime in an old date
                // dir most likely means Codex is alive but *waiting for user input*,
                // not actively executing a task.  Treating those as "active" causes
                // updatedAt to oscillate as long as Codex emits heartbeat writes.
                if age_secs <= JSONL_IDLE_THRESHOLD_SECS {
                    continue;
                }
                if age_secs > HISTORICAL_ENRICHMENT_SECS {
                    continue;
                }
                let Some(meta) = read_jsonl_session_meta(&path) else {
                    continue;
                };
                if meta.cwd != pane_canonical {
                    continue;
                }
                // Keep the most-recently-modified match.
                if best.as_ref().is_none_or(|(_, a, _)| age_secs < *a) {
                    best = Some((path, age_secs, meta));
                    // Oldest files are at the start of earlier days; once we found a match
                    // on a given day we can still look earlier — but stop once the best
                    // match is from today or yesterday (most recent possible anyway).
                    if days_ago == 0 {
                        break 'outer;
                    }
                }
            }
        }

        let Some((path, age_secs, meta)) = best else {
            continue;
        };

        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| format!("jsonl-{s}"))
            .unwrap_or_else(|| format!("jsonl-{}", pane.pane_id));

        let actual_activity_at = now - chrono::Duration::seconds(age_secs as i64);

        tracing::debug!(
            "jsonl-history: pane={} session={} age={}s cwd={}",
            pane.pane_id,
            session_id,
            age_secs,
            meta.cwd
        );

        events.push(CodexRawEvent {
            id: format!(
                "jsonlhist-{}-{}",
                pane.pane_id,
                path.file_stem().and_then(|s| s.to_str()).unwrap_or("?")
            ),
            event_type: "thread.idle".to_string(),
            session_id,
            timestamp: now,
            pane_id: Some(pane.pane_id.clone()),
            pane_generation: pane.generation,
            pane_birth_ts: pane.birth_ts,
            payload: serde_json::json!({
                "cwd": meta.cwd,
                "jsonl_age_secs": age_secs,
                "source": "jsonl_history",
                "instructions": meta.instructions,
            }),
            is_heartbeat: false,
            actual_activity_at: Some(actual_activity_at),
        });
        matched_panes.insert(pane.pane_id.clone());
    }

    events
}

/// Assignment priority tier for a pane based on its process hint.
/// Lower value = higher priority for Codex thread assignment.
/// Tier 3 panes are **never** assigned a Codex thread.
///
/// - 0: Running Codex CLI explicitly → highest priority
/// - 1: Neutral runtime (node, python, …) → may host Codex/Claude, eligible
/// - 2: Running a competing agent (claude, gemini, …) → deprioritized
///   (those panes have their own deterministic sources; T-123 arbitration resolves conflicts)
/// - 3: Plain interactive shell (zsh, bash, fish, …) → ineligible; never a Codex runtime
fn pane_tier(p: &PaneCwdInfo) -> u8 {
    match p.process_hint.as_deref() {
        Some("codex") => 0,
        None => 1,
        Some("shell") | Some("runtime_unknown") => 3, // never assigned: shell or ambiguous neutral
        _ => 2,
    }
}

/// Build a cwd → pane-group map for multi-pane Codex thread assignment.
///
/// All panes sharing the same cwd are collected into a `Vec`, sorted by
/// assignment priority (tier 0→2) then `pane_id` for deterministic ordering.
/// Every pane in the group is eligible for assignment; threads are round-robined
/// across the group in `process_thread_list_response`.
fn build_cwd_pane_groups(pane_cwds: &[PaneCwdInfo]) -> HashMap<String, Vec<PaneCwdInfo>> {
    let mut map: HashMap<String, Vec<PaneCwdInfo>> = HashMap::new();
    for info in pane_cwds {
        if !info.cwd.is_empty() {
            // Canonicalize to resolve symlinks (e.g. macOS /tmp → /private/tmp).
            // Codex records the canonical path in thread.cwd, so the query CWD
            // must also be canonical for the App Server's filter to match.
            let canonical = std::fs::canonicalize(&info.cwd)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| info.cwd.clone());
            map.entry(canonical).or_default().push(info.clone());
        }
    }
    for panes in map.values_mut() {
        panes.sort_by(|a, b| {
            pane_tier(a)
                .cmp(&pane_tier(b))
                .then_with(|| a.pane_id.cmp(&b.pane_id))
        });
    }
    map
}

/// Maximum per-CWD thread/list queries per poll tick.
///
/// With multiple CWDs (e.g. vm-agtmux + test-session), capping at 1 causes
/// starvation: only the highest-priority CWD is ever queried, so sessions in
/// other CWDs never get an App Server update.  Set to 3 to cover typical
/// multi-session scenarios while staying within the outer timeout budget.
const MAX_CWD_QUERIES_PER_TICK: usize = 3;

/// Per-request timeout for thread/list RPC calls.
///
/// Codex app-server v0.106.0 takes ~3.4 s to respond to CWD-filtered
/// queries (SQLite DB load on first query after initialize).  We set this
/// to 4 s so a single per-CWD query succeeds, while leaving budget for the
/// global query that follows it.
const THREAD_LIST_REQUEST_TIMEOUT: Duration = Duration::from_millis(4000);

// ─── App Server client (JSON-RPC 2.0 over stdio) ────────────────────

/// Codex App Server client connected via stdio JSON-RPC 2.0.
///
/// Protocol reference: <https://developers.openai.com/codex/app-server/>
///
/// Lifecycle:
/// 1. Spawn `codex app-server` with stdio transport
/// 2. Send `initialize` request, receive response
/// 3. Send `initialized` notification
/// 4. Issue `thread/list` requests to discover active threads
pub struct CodexAppServerClient {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    /// Tracks which thread states we've already emitted.
    last_thread_states: HashMap<String, LastThreadState>,
    /// Best-known thread -> pane binding, reused for pane-less events.
    thread_pane_bindings: HashMap<String, ThreadPaneBinding>,
    /// Time after which thread/list queries may be sent.
    ///
    /// codex app-server v0.106.0 takes ~3 s after `initialize` to open its SQLite DB
    /// and become ready to serve `thread/list` requests.  Sending queries before this
    /// causes timeouts that corrupt the socket read buffer and make all future queries
    /// fail too (cascade problem).  We skip polling until this instant has passed.
    ready_at: Instant,
}

/// Result of attempting to spawn and connect to the Codex App Server.
#[derive(Debug)]
pub enum AppServerSpawnResult {
    Connected,
    Failed(String),
}

impl CodexAppServerClient {
    /// Spawn `codex app-server` and perform the JSON-RPC initialize handshake.
    ///
    /// Returns `None` if:
    /// - `codex` binary is not found
    /// - The app-server process fails to start
    /// - The initialize handshake times out or fails
    pub async fn spawn() -> Option<Self> {
        let mut child = match Command::new("codex")
            .arg("app-server")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::info!("codex app-server not available: {e}");
                return None;
            }
        };

        let stdin = child.stdin.take()?;
        let stdout = BufReader::new(child.stdout.take()?);

        let mut client = Self {
            _child: child,
            stdin,
            stdout,
            next_id: 1,
            last_thread_states: HashMap::new(),
            thread_pane_bindings: HashMap::new(),
            // Populated after handshake succeeds; starts as "already ready" placeholder.
            ready_at: Instant::now(),
        };

        // Perform JSON-RPC initialize handshake with 10s timeout
        match tokio::time::timeout(std::time::Duration::from_secs(10), client.initialize()).await {
            Ok(Ok(())) => {
                // Skip the first 2 polls after initialization.  Even though our
                // THREAD_LIST_REQUEST_TIMEOUT (4 s) now accommodates the ~3.4 s
                // per-CWD query latency, skipping the very first tick avoids sending
                // a query before the initialize handshake has fully settled and any
                // internal notifications have been flushed.
                client.ready_at = Instant::now() + Duration::from_secs(2);
                tracing::info!("codex app-server connected (stdio transport)");
                Some(client)
            }
            Ok(Err(e)) => {
                tracing::warn!("codex app-server handshake failed: {e}");
                None
            }
            Err(_) => {
                tracing::warn!("codex app-server handshake timed out");
                None
            }
        }
    }

    /// Send the JSON-RPC `initialize` request and `initialized` notification.
    async fn initialize(&mut self) -> anyhow::Result<()> {
        // Step 1: Send initialize request
        let init_request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialize",
            "id": self.alloc_id(),
            "params": {
                "clientInfo": {
                    "name": "agtmux",
                    "title": "agtmux-v5",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {}
            }
        });
        self.send(&init_request).await?;

        // Step 2: Read response
        let response = self.recv().await?;
        if response.get("error").is_some() {
            anyhow::bail!(
                "initialize error: {}",
                response["error"]["message"].as_str().unwrap_or("unknown")
            );
        }

        // Step 3: Send initialized notification (no id = notification)
        let initialized =
            serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}});
        self.send(&initialized).await?;

        Ok(())
    }

    /// Poll the app-server for active threads and return new events.
    ///
    /// Calls `thread/list` (with per-pane `cwd` filter when pane info is available)
    /// and translates thread status changes into [`CodexRawEvent`] objects.
    ///
    /// `pane_cwds` maps `(pane_id, cwd, generation, birth_ts)` for cwd correlation (T-119).
    /// When provided, each unique cwd gets its own `thread/list` request, and matched
    /// threads have `pane_id`, `pane_generation`, and `pane_birth_ts` set on their events.
    /// Returns `(events, timed_out)`.
    ///
    /// `timed_out = true` means the outer 5-second timeout fired.  The caller should
    /// drop and reconnect the client because the socket read buffer may be corrupt
    /// (stale responses from unfinished CWD queries accumulate and cause every future
    /// query to also time out — the cascade problem).
    pub async fn poll_threads(&mut self, pane_cwds: &[PaneCwdInfo]) -> (Vec<CodexRawEvent>, bool) {
        // Warm-up guard: skip polling until the server has had time to open its DB.
        if Instant::now() < self.ready_at {
            tracing::debug!("codex app-server: warming up, skipping poll");
            return (Vec::new(), false);
        }

        match tokio::time::timeout(
            // 16 s: up to 3 CWD queries (first ≤4 s, subsequent ≤1 s each)
            //       + 1 global query (≤1 s) + slack.
            std::time::Duration::from_secs(16),
            self.poll_threads_inner(pane_cwds),
        )
        .await
        {
            Ok(Ok(events)) => (events, false),
            Ok(Err(e)) => {
                tracing::debug!("codex app-server poll failed: {e}");
                (Vec::new(), false)
            }
            Err(_) => {
                tracing::debug!("codex app-server poll timed out");
                (Vec::new(), true) // timed_out=true → caller should reconnect
            }
        }
    }

    async fn poll_threads_inner(
        &mut self,
        pane_cwds: &[PaneCwdInfo],
    ) -> anyhow::Result<Vec<CodexRawEvent>> {
        // Drain any pending notifications first
        let notifications = self.drain_notifications().await;

        let now = Utc::now();
        let mut events = Vec::new();

        // Process notifications (turn/started, turn/completed, thread/status/changed).
        // Notifications don't carry cwd directly; when possible we enrich them via
        // cached thread -> pane correlation from thread/list.
        for notif in &notifications {
            let pane_binding = notification_thread_id(notif)
                .and_then(|thread_id| self.thread_pane_bindings.get(thread_id));
            if let Some(event) = notification_to_event_with_pane(notif, now, pane_binding) {
                events.push(event);
            }
        }

        // Build cwd → Vec<PaneCwdInfo> groups (all panes per CWD, sorted by tier).
        let cwd_pane_groups = build_cwd_pane_groups(pane_cwds);

        // Step 1: Per-cwd queries. CWDs with actual Codex panes are queried first.
        let mut query_plan: Vec<(String, Vec<PaneCwdInfo>)> = cwd_pane_groups.into_iter().collect();
        query_plan.sort_by(|(cwd_a, panes_a), (cwd_b, panes_b)| {
            let has_codex_a = panes_a
                .iter()
                .any(|p| p.process_hint.as_deref() == Some("codex"));
            let has_codex_b = panes_b
                .iter()
                .any(|p| p.process_hint.as_deref() == Some("codex"));
            has_codex_b.cmp(&has_codex_a).then_with(|| cwd_a.cmp(cwd_b))
        });
        if query_plan.len() > MAX_CWD_QUERIES_PER_TICK {
            tracing::debug!(
                "codex thread/list cwd queries capped: {} -> {}",
                query_plan.len(),
                MAX_CWD_QUERIES_PER_TICK
            );
        }

        // H2: tick-scope dedup — prevents same thread being assigned in multiple CWD queries
        // (guards against cwd filter returning broader results than expected).
        let mut assigned_in_tick: HashSet<String> = HashSet::new();

        for (cwd, pane_group) in query_plan.into_iter().take(MAX_CWD_QUERIES_PER_TICK) {
            match self
                .send_thread_list_timed(Some(&cwd), THREAD_LIST_REQUEST_TIMEOUT)
                .await
            {
                Ok(response) => {
                    self.process_thread_list_response(
                        &response,
                        &pane_group,
                        &mut assigned_in_tick,
                        now,
                        &mut events,
                    );
                }
                Err(e) => {
                    tracing::debug!("codex thread/list cwd query failed (cwd={cwd}): {e}");
                }
            }
        }

        // Step 2: Global query — empty pane slice means no new assignments.
        // Existing bindings are reused for heartbeat continuity.
        match self
            .send_thread_list_timed(None, THREAD_LIST_REQUEST_TIMEOUT)
            .await
        {
            Ok(response) => {
                self.process_thread_list_response(
                    &response,
                    &[],
                    &mut assigned_in_tick,
                    now,
                    &mut events,
                );
            }
            Err(e) => {
                tracing::debug!("codex thread/list global query failed: {e}");
            }
        }

        Ok(events)
    }

    async fn send_thread_list_timed(
        &mut self,
        cwd: Option<&str>,
        timeout: Duration,
    ) -> anyhow::Result<serde_json::Value> {
        match tokio::time::timeout(timeout, self.send_thread_list(cwd)).await {
            Ok(result) => result,
            Err(_) => anyhow::bail!("thread/list timeout"),
        }
    }

    /// Send a `thread/list` request, optionally filtered by `cwd`.
    /// Returns the response JSON, collecting interleaved notifications into `self`.
    async fn send_thread_list(&mut self, cwd: Option<&str>) -> anyhow::Result<serde_json::Value> {
        let id = self.alloc_id();
        let mut params = serde_json::json!({
            "limit": 50,
            "sortKey": "updated_at"
        });
        if let Some(cwd) = cwd {
            params["cwd"] = serde_json::Value::String(cwd.to_string());
        }
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "thread/list",
            "id": id,
            "params": params
        });
        self.send(&request).await?;

        // Read response (skip notifications until we get our response)
        loop {
            let msg = self.recv().await?;
            if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                return Ok(msg);
            }
            // Interleaved notification — ignored (already drained above)
        }
    }

    /// Process a `thread/list` response, emitting events for status changes.
    ///
    /// `pane_infos` is the sorted pane group for this CWD (tier 0→2, pane_id asc).
    /// Pass `&[]` for the global query — no new pane assignments are made.
    ///
    /// `assigned_in_tick` prevents the same thread from being re-assigned by a
    /// subsequent CWD query in the same tick (cwd filter anomaly guard).
    fn process_thread_list_response(
        &mut self,
        response: &serde_json::Value,
        pane_infos: &[PaneCwdInfo],
        assigned_in_tick: &mut HashSet<String>,
        now: chrono::DateTime<Utc>,
        events: &mut Vec<CodexRawEvent>,
    ) {
        // API returns { result: { data: [...] } } per docs/codex-appserver-api-reference.md
        let Some(result) = response.get("result") else {
            return;
        };
        let Some(threads) = result.get("data").and_then(|t| t.as_array()) else {
            return;
        };

        // Sort threads by thread_id for deterministic, stable assignment across ticks.
        let mut sorted_threads: Vec<&serde_json::Value> = threads.iter().collect();
        sorted_threads.sort_by_key(|t| t["id"].as_str().unwrap_or(""));

        // Panes already claimed by cache entries IN THIS RESPONSE.
        // Only threads present in this response count — exited threads release their pane.
        // Additionally, only count a binding as "claimed" if the pane generation still matches
        // the current pane group; a stale binding must not block the pane from being unclaimed.
        let cached_pane_ids: HashSet<String> = sorted_threads
            .iter()
            .filter_map(|t| {
                let tid = t["id"].as_str()?;
                let binding = self.thread_pane_bindings.get(tid)?;
                // For global query (pane_infos empty): trust all existing bindings.
                let generation_valid = pane_infos.is_empty()
                    || pane_infos.iter().any(|p| {
                        p.pane_id == binding.pane_id
                            && p.generation == binding.pane_generation
                            && p.birth_ts == binding.pane_birth_ts
                    });
                if generation_valid {
                    Some(binding.pane_id.clone())
                } else {
                    None // stale binding — leave pane available as unclaimed
                }
            })
            .collect();

        // Unclaimed panes: group members not held by any live thread's cache.
        // Tier-3 panes (plain shells) are permanently ineligible for Codex thread assignment.
        let mut unclaimed: VecDeque<ThreadPaneBinding> = pane_infos
            .iter()
            .filter(|p| !cached_pane_ids.contains(&p.pane_id) && pane_tier(p) < 3)
            .map(|p| ThreadPaneBinding {
                pane_id: p.pane_id.clone(),
                pane_generation: p.generation,
                pane_birth_ts: p.birth_ts,
            })
            .collect();

        for thread in sorted_threads {
            let thread_id = thread["id"].as_str().unwrap_or("");
            // Status is an object { type: "idle" } per the API reference.
            // The real App Server (v0.104.0+) may omit `status` from `thread/list` —
            // default to "idle" (a listed thread is at least available/loaded).
            let raw_status = thread
                .get("status")
                .and_then(|s| s.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("idle");

            // `codex app-server` (v0.106.0) reports ALL `codex --full-auto` sessions as
            // `notLoaded`, even while they are actively executing.  For CWD-specific
            // queries we reclassify by checking the session JSONL file's mtime:
            //   mtime < 30 s  → "active"  (session is writing, i.e. currently running)
            //   mtime < 120 s → "idle"    (session just finished)
            //   mtime ≥ 120 s → skip      (historical session, irrelevant)
            // For the global query (pane_infos empty) we skip notLoaded as before —
            // no pane assignment can be made there anyway.
            let status: &str = if raw_status == "notLoaded" {
                if pane_infos.is_empty() {
                    continue;
                }
                let session_path = thread.get("path").and_then(|p| p.as_str()).unwrap_or("");
                match classify_notloaded_status(session_path) {
                    Some(s) => s,
                    None => continue,
                }
            } else {
                raw_status
            };
            if thread_id.is_empty() {
                continue;
            }

            // H2: tick-scope dedup — skip threads already assigned earlier in this tick.
            if assigned_in_tick.contains(thread_id) {
                continue;
            }

            // Determine pane binding for this thread:
            // H1: validate cached binding by pane generation + birth_ts to detect reuse.
            let assigned_binding =
                if let Some(cached) = self.thread_pane_bindings.get(thread_id).cloned() {
                    // For global query (pane_infos empty): trust existing binding as-is.
                    // For CWD queries: verify the pane instance hasn't been recycled.
                    let pane_still_valid = pane_infos.is_empty()
                        || pane_infos.iter().any(|p| {
                            p.pane_id == cached.pane_id
                                && p.generation == cached.pane_generation
                                && p.birth_ts == cached.pane_birth_ts
                        });
                    if pane_still_valid {
                        Some(cached)
                    } else {
                        // Pane was recycled — invalidate stale binding, try unclaimed.
                        self.thread_pane_bindings.remove(thread_id);
                        let b = unclaimed.pop_front();
                        if let Some(ref binding) = b {
                            self.thread_pane_bindings
                                .insert(thread_id.to_string(), binding.clone());
                        }
                        b
                    }
                } else if !pane_infos.is_empty() {
                    // No cache: assign next available pane in group order.
                    let b = unclaimed.pop_front();
                    if let Some(ref binding) = b {
                        self.thread_pane_bindings
                            .insert(thread_id.to_string(), binding.clone());
                    }
                    b
                } else {
                    // Global query: no new assignments.
                    None
                };

            assigned_in_tick.insert(thread_id.to_string());

            // effective_pane_id: new assignment, or fall back to last known state.
            let effective_pane_id = assigned_binding
                .as_ref()
                .map(|b| b.pane_id.clone())
                .or_else(|| {
                    self.last_thread_states
                        .get(thread_id)
                        .and_then(|s| s.pane_id.clone())
                });

            // Emit when status/pane changed, or heartbeat interval elapsed.
            let should_emit = match self.last_thread_states.get(thread_id) {
                None => true,
                Some(prev) => {
                    prev.status != status
                        || prev.pane_id != effective_pane_id
                        || (now - prev.emitted_at).num_seconds() >= HEARTBEAT_INTERVAL_SECS
                }
            };

            if should_emit {
                // Heartbeat: emit triggered by elapsed time only (no status/pane change).
                let is_heartbeat = match self.last_thread_states.get(thread_id) {
                    None => false,
                    Some(prev) => prev.status == status && prev.pane_id == effective_pane_id,
                };

                self.last_thread_states.insert(
                    thread_id.to_string(),
                    LastThreadState {
                        status: status.to_string(),
                        pane_id: effective_pane_id,
                        emitted_at: now,
                    },
                );

                let event_binding =
                    assigned_binding.or_else(|| self.thread_pane_bindings.get(thread_id).cloned());

                let event_type = match status {
                    "active" => "thread.active",
                    "idle" => "thread.idle",
                    "systemError" => "thread.error",
                    _ => "thread.status_changed",
                };

                events.push(CodexRawEvent {
                    id: format!("appserver-{thread_id}-{}", now.timestamp_millis()),
                    event_type: event_type.to_string(),
                    session_id: thread_id.to_string(),
                    timestamp: now,
                    pane_id: event_binding.as_ref().map(|b| b.pane_id.clone()),
                    pane_generation: event_binding.as_ref().and_then(|b| b.pane_generation),
                    pane_birth_ts: event_binding.as_ref().and_then(|b| b.pane_birth_ts),
                    payload: thread.clone(),
                    is_heartbeat,
                    actual_activity_at: None,
                });
            }
        }
    }

    /// Try to read any pending notifications (non-blocking).
    async fn drain_notifications(&mut self) -> Vec<serde_json::Value> {
        let mut notifications = Vec::new();
        while let Ok(Ok(msg)) =
            tokio::time::timeout(std::time::Duration::from_millis(10), self.recv()).await
        {
            if msg.get("id").is_none() {
                notifications.push(msg);
            }
        }
        notifications
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    async fn send(&mut self, msg: &serde_json::Value) -> anyhow::Result<()> {
        let mut line = serde_json::to_string(msg)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn recv(&mut self) -> anyhow::Result<serde_json::Value> {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).await?;
        if n == 0 {
            anyhow::bail!("codex app-server closed stdout");
        }
        Ok(serde_json::from_str(line.trim())?)
    }

    /// Check if the app-server process is still alive.
    pub fn is_alive(&mut self) -> bool {
        match self._child.try_wait() {
            Ok(None) => true,     // still running
            Ok(Some(_)) => false, // exited
            Err(_) => false,
        }
    }
}

/// Convert a Codex App Server notification to a [`CodexRawEvent`].
///
/// Handles: `turn/started`, `turn/completed`, `thread/status/changed`.
/// Returns `None` for unrecognized notification methods.
fn notification_to_event(
    notif: &serde_json::Value,
    now: chrono::DateTime<Utc>,
) -> Option<CodexRawEvent> {
    notification_to_event_with_pane(notif, now, None)
}

fn notification_thread_id(notif: &serde_json::Value) -> Option<&str> {
    let method = notif["method"].as_str()?;
    let params = notif.get("params")?;
    match method {
        "turn/started" | "turn/completed" | "thread/status/changed" => params["threadId"].as_str(),
        _ => None,
    }
}

fn notification_to_event_with_pane(
    notif: &serde_json::Value,
    now: chrono::DateTime<Utc>,
    pane_binding: Option<&ThreadPaneBinding>,
) -> Option<CodexRawEvent> {
    let method = notif["method"].as_str()?;
    let params = notif.get("params")?;

    match method {
        "turn/started" => {
            let turn_id = params["turn"]["id"].as_str().unwrap_or("unknown");
            Some(CodexRawEvent {
                id: format!("notif-turn-started-{turn_id}"),
                event_type: "turn.started".to_string(),
                session_id: params["threadId"].as_str().unwrap_or("unknown").to_string(),
                timestamp: now,
                pane_id: pane_binding.map(|b| b.pane_id.clone()),
                pane_generation: pane_binding.and_then(|b| b.pane_generation),
                pane_birth_ts: pane_binding.and_then(|b| b.pane_birth_ts),
                payload: params.clone(),
                is_heartbeat: false, // notifications = real activity
                actual_activity_at: None,
            })
        }
        "turn/completed" => {
            let turn_id = params["turn"]["id"].as_str().unwrap_or("unknown");
            let status = params["turn"]["status"].as_str().unwrap_or("completed");
            Some(CodexRawEvent {
                id: format!("notif-turn-completed-{turn_id}"),
                event_type: format!("turn.{status}"),
                session_id: params["threadId"].as_str().unwrap_or("unknown").to_string(),
                timestamp: now,
                pane_id: pane_binding.map(|b| b.pane_id.clone()),
                pane_generation: pane_binding.and_then(|b| b.pane_generation),
                pane_birth_ts: pane_binding.and_then(|b| b.pane_birth_ts),
                payload: params.clone(),
                is_heartbeat: false, // notifications = real activity
                actual_activity_at: None,
            })
        }
        "thread/status/changed" => {
            let thread_id = params["threadId"].as_str().unwrap_or("unknown");
            // Status can be object { type: "active" } or string "active"
            let status = params
                .get("status")
                .and_then(|s| {
                    s.get("type")
                        .and_then(|t| t.as_str())
                        .or_else(|| s.as_str())
                })
                .unwrap_or("unknown");
            Some(CodexRawEvent {
                id: format!("notif-status-{thread_id}-{}", now.timestamp_millis()),
                event_type: format!("thread.{status}"),
                session_id: thread_id.to_string(),
                timestamp: now,
                pane_id: pane_binding.map(|b| b.pane_id.clone()),
                pane_generation: pane_binding.and_then(|b| b.pane_generation),
                pane_birth_ts: pane_binding.and_then(|b| b.pane_birth_ts),
                payload: params.clone(),
                is_heartbeat: false, // notifications = real activity
                actual_activity_at: None,
            })
        }
        _ => None,
    }
}

// ─── Capture-based JSON extraction (fallback) ──────────────────────────

/// Tracks which Codex JSON events have been ingested from tmux capture,
/// preventing re-ingestion of the same event across poll ticks.
///
/// Uses content-based fingerprinting: each JSON line is hashed, and the
/// hash is stored per-pane. This is not cryptographic — it's a fast
/// dedup mechanism for the same terminal output appearing in consecutive
/// captures.
#[derive(Debug, Default)]
pub struct CodexCaptureTracker {
    /// Per-pane set of seen event fingerprints.
    seen: HashMap<String, HashSet<u64>>,
}

impl CodexCaptureTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if this fingerprint is new for the given pane.
    /// Returns `true` if the fingerprint was not seen before (and records it).
    fn is_new(&mut self, pane_id: &str, fingerprint: u64) -> bool {
        self.seen
            .entry(pane_id.to_string())
            .or_default()
            .insert(fingerprint)
    }

    /// Remove tracking for panes that no longer exist.
    pub fn retain_panes(&mut self, active_pane_ids: &[&str]) {
        let active: HashSet<&str> = active_pane_ids.iter().copied().collect();
        self.seen.retain(|k, _| active.contains(k.as_str()));
    }
}

/// Compute a content fingerprint for dedup (not cryptographic).
fn fingerprint(line: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    line.hash(&mut hasher);
    hasher.finish()
}

/// Parse Codex NDJSON events from tmux capture lines (fallback path).
///
/// When Codex CLI runs with `--json`, it outputs newline-delimited JSON events
/// to stdout (e.g., `{"type": "turn.completed", ...}`). This function extracts
/// those events from captured terminal output and converts them to [`CodexRawEvent`].
///
/// Only lines that parse as JSON objects with a `"type"` field are considered
/// Codex events. Other lines (prompts, plain text, partial JSON) are ignored.
///
/// The `tracker` parameter provides cross-tick deduplication: events whose
/// content fingerprint has already been seen for this pane are skipped.
pub fn parse_codex_capture_events(
    capture_lines: &[String],
    pane_id: &str,
    tracker: &mut CodexCaptureTracker,
) -> Vec<CodexRawEvent> {
    let now = Utc::now();
    let mut events = Vec::new();

    for line in capture_lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.starts_with('{') {
            continue;
        }

        let fp = fingerprint(trimmed);
        if !tracker.is_new(pane_id, fp) {
            continue; // Already ingested in a previous tick
        }

        let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Must have a "type" field to be a Codex event
        let event_type = match parsed["type"].as_str() {
            Some(t) => t.to_string(),
            None => continue,
        };

        // Generate deterministic event ID from fingerprint
        let id = format!("cap-{fp:016x}");

        // Extract session_id if available, otherwise derive from pane_id
        let session_id = parsed["session_id"]
            .as_str()
            .or_else(|| parsed["id"].as_str())
            .map(String::from)
            .unwrap_or_else(|| format!("codex-{pane_id}"));

        events.push(CodexRawEvent {
            id,
            event_type,
            session_id,
            timestamp: now,
            pane_id: Some(pane_id.to_string()),
            pane_generation: None,
            pane_birth_ts: None,
            payload: parsed,
            is_heartbeat: false, // captured NDJSON events = real activity
            actual_activity_at: None,
        });
    }

    events
}

// ─── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[tokio::test]
    async fn app_server_spawn_fails_gracefully_when_codex_missing() {
        // If codex is not installed, spawn should return None (not panic)
        // We can't guarantee codex is installed in CI, so this test just
        // verifies no panic/crash on attempt.
        let _result = CodexAppServerClient::spawn().await;
        // Result is either Some (codex installed) or None (not installed) — both OK
    }

    #[test]
    fn notification_to_event_turn_started() {
        let notif = serde_json::json!({
            "method": "turn/started",
            "params": {
                "threadId": "thr_abc",
                "turn": {"id": "turn_1", "status": "inProgress", "items": []}
            }
        });
        let event = notification_to_event(&notif, Utc::now()).expect("should parse turn/started");
        assert_eq!(event.event_type, "turn.started");
        assert_eq!(event.session_id, "thr_abc");
    }

    #[test]
    fn notification_to_event_with_cached_pane_binding() {
        let notif = serde_json::json!({
            "method": "turn/started",
            "params": {
                "threadId": "thr_abc",
                "turn": {"id": "turn_1", "status": "inProgress", "items": []}
            }
        });
        let binding = ThreadPaneBinding {
            pane_id: "%9".to_string(),
            pane_generation: Some(3),
            pane_birth_ts: None,
        };
        let event = notification_to_event_with_pane(&notif, Utc::now(), Some(&binding))
            .expect("should parse turn/started");
        assert_eq!(event.pane_id, Some("%9".to_string()));
        assert_eq!(event.pane_generation, Some(3));
    }

    #[test]
    fn notification_to_event_turn_completed() {
        let notif = serde_json::json!({
            "method": "turn/completed",
            "params": {
                "threadId": "thr_abc",
                "turn": {"id": "turn_1", "status": "completed"}
            }
        });
        let event = notification_to_event(&notif, Utc::now()).expect("should parse turn/completed");
        assert_eq!(event.event_type, "turn.completed");
    }

    #[test]
    fn notification_to_event_thread_status_changed_object() {
        // Official API format: status is { type: "active", activeFlags: [...] }
        let notif = serde_json::json!({
            "method": "thread/status/changed",
            "params": {
                "threadId": "thr_xyz",
                "status": { "type": "active", "activeFlags": ["waitingOnApproval"] }
            }
        });
        let event =
            notification_to_event(&notif, Utc::now()).expect("should parse thread/status/changed");
        assert_eq!(event.event_type, "thread.active");
        assert_eq!(event.session_id, "thr_xyz");
    }

    #[test]
    fn notification_to_event_thread_status_changed_string() {
        // Backwards compat: status as plain string
        let notif = serde_json::json!({
            "method": "thread/status/changed",
            "params": {
                "threadId": "thr_xyz",
                "status": "idle"
            }
        });
        let event = notification_to_event(&notif, Utc::now()).expect("should parse string status");
        assert_eq!(event.event_type, "thread.idle");
    }

    #[test]
    fn notification_to_event_ignores_unknown_methods() {
        let notif = serde_json::json!({
            "method": "item/agentMessage/delta",
            "params": {"text": "hello"}
        });
        assert!(
            notification_to_event(&notif, Utc::now()).is_none(),
            "should ignore non-lifecycle notifications"
        );
    }

    // ── Capture-based JSON extraction tests ─────────────────────────

    #[test]
    fn parse_codex_capture_extracts_json_events() {
        let lines = vec![
            "$ codex exec --json \"do something\"".to_string(),
            "{\"type\":\"message.created\",\"id\":\"msg-1\"}".to_string(),
            "".to_string(),
            "{\"type\":\"turn.completed\",\"id\":\"turn-1\"}".to_string(),
            "wait_result=idle".to_string(),
        ];
        let mut tracker = CodexCaptureTracker::new();
        let events = parse_codex_capture_events(&lines, "%0", &mut tracker);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "message.created");
        assert_eq!(events[1].event_type, "turn.completed");
        assert_eq!(events[0].pane_id, Some("%0".to_string()));
        assert_eq!(events[1].pane_id, Some("%0".to_string()));
    }

    #[test]
    fn parse_codex_capture_skips_non_json_and_typeless() {
        let lines = vec![
            "$ ls".to_string(),
            "file.txt".to_string(),
            "not json at all".to_string(),
            "{invalid json}".to_string(),
            "{\"key\":\"value\"}".to_string(),
        ];
        let mut tracker = CodexCaptureTracker::new();
        let events = parse_codex_capture_events(&lines, "%0", &mut tracker);

        assert!(events.is_empty());
    }

    #[test]
    fn codex_capture_tracker_deduplicates_across_ticks() {
        let lines = vec!["{\"type\":\"turn.completed\",\"id\":\"t1\"}".to_string()];
        let mut tracker = CodexCaptureTracker::new();

        let events1 = parse_codex_capture_events(&lines, "%0", &mut tracker);
        assert_eq!(events1.len(), 1);

        let events2 = parse_codex_capture_events(&lines, "%0", &mut tracker);
        assert!(
            events2.is_empty(),
            "same event should be deduplicated across ticks"
        );
    }

    #[test]
    fn codex_capture_tracker_retains_only_active_panes() {
        let mut tracker = CodexCaptureTracker::new();
        let lines = vec!["{\"type\":\"turn.completed\"}".to_string()];

        parse_codex_capture_events(&lines, "%0", &mut tracker);
        parse_codex_capture_events(&lines, "%1", &mut tracker);

        tracker.retain_panes(&["%0"]);

        let events = parse_codex_capture_events(&lines, "%1", &mut tracker);
        assert_eq!(
            events.len(),
            1,
            "should see event again after pane tracking cleared"
        );

        let events0 = parse_codex_capture_events(&lines, "%0", &mut tracker);
        assert!(events0.is_empty(), "%0 should still be deduped");
    }

    // ── T-119/T-124: cwd correlation + multi-pane assignment tests ───

    fn make_pane(pane_id: &str, cwd: &str, hint: Option<&str>) -> PaneCwdInfo {
        make_pane_with_gen(pane_id, cwd, hint, Some(1), None)
    }

    fn make_pane_with_gen(
        pane_id: &str,
        cwd: &str,
        hint: Option<&str>,
        generation: Option<u64>,
        birth_ts: Option<chrono::DateTime<Utc>>,
    ) -> PaneCwdInfo {
        PaneCwdInfo {
            pane_id: pane_id.to_string(),
            cwd: cwd.to_string(),
            generation,
            birth_ts,
            process_hint: hint.map(String::from),
            pane_pid: None,
        }
    }

    #[test]
    fn build_cwd_pane_groups_single_pane() {
        let infos = vec![make_pane("%0", "/home/user/project", None)];
        let map = build_cwd_pane_groups(&infos);
        assert_eq!(map.len(), 1);
        assert_eq!(map["/home/user/project"].len(), 1);
        assert_eq!(map["/home/user/project"][0].pane_id, "%0");
    }

    #[test]
    fn build_cwd_pane_groups_different_cwds() {
        let infos = vec![
            make_pane("%0", "/project-a", None),
            make_pane("%1", "/project-b", None),
        ];
        let map = build_cwd_pane_groups(&infos);
        assert_eq!(map.len(), 2);
        assert_eq!(map["/project-a"][0].pane_id, "%0");
        assert_eq!(map["/project-b"][0].pane_id, "%1");
    }

    #[test]
    fn build_cwd_pane_groups_multiple_panes_same_cwd() {
        // 3 panes at same CWD: codex, zsh (None), claude — group should hold all 3
        let infos = vec![
            make_pane("%3", "/proj", Some("claude")),
            make_pane("%1", "/proj", None),
            make_pane("%2", "/proj", Some("codex")),
        ];
        let map = build_cwd_pane_groups(&infos);
        assert_eq!(map.len(), 1);
        let group = &map["/proj"];
        assert_eq!(group.len(), 3, "all 3 panes must be in the group");
        // tier sort: codex(%2,tier0) → neutral(%1,tier1) → claude(%3,tier2)
        assert_eq!(group[0].pane_id, "%2", "codex pane first (tier 0)");
        assert_eq!(group[1].pane_id, "%1", "neutral pane second (tier 1)");
        assert_eq!(group[2].pane_id, "%3", "competing-agent pane last (tier 2)");
    }

    #[test]
    fn build_cwd_pane_groups_tier_sort() {
        // Verify 3-tier: codex(0) < neutral(1) < competing-agent(2), tiebreak by pane_id
        let infos = vec![
            make_pane("%5", "/x", Some("gemini")), // tier 2
            make_pane("%3", "/x", None),           // tier 1
            make_pane("%1", "/x", Some("codex")),  // tier 0
            make_pane("%4", "/x", Some("claude")), // tier 2
            make_pane("%2", "/x", None),           // tier 1
        ];
        let map = build_cwd_pane_groups(&infos);
        let group = &map["/x"];
        assert_eq!(group[0].pane_id, "%1"); // codex, tier 0
        assert_eq!(group[1].pane_id, "%2"); // None, tier 1, pane_id "%2" < "%3"
        assert_eq!(group[2].pane_id, "%3"); // None, tier 1
        assert_eq!(group[3].pane_id, "%4"); // claude, tier 2, pane_id "%4" < "%5"
        assert_eq!(group[4].pane_id, "%5"); // gemini, tier 2
    }

    #[test]
    fn build_cwd_pane_groups_tier_sort_with_shell() {
        // shell (tier 3) sorts after all other tiers
        let infos = vec![
            make_pane("%4", "/x", Some("shell")),  // tier 3
            make_pane("%2", "/x", Some("claude")), // tier 2
            make_pane("%3", "/x", None),           // tier 1
            make_pane("%1", "/x", Some("codex")),  // tier 0
        ];
        let map = build_cwd_pane_groups(&infos);
        let group = &map["/x"];
        assert_eq!(group[0].pane_id, "%1"); // codex
        assert_eq!(group[1].pane_id, "%3"); // neutral
        assert_eq!(group[2].pane_id, "%2"); // claude
        assert_eq!(group[3].pane_id, "%4"); // shell — last
    }

    #[test]
    fn pane_tier_runtime_unknown_is_tier3() {
        // runtime_unknown must be tier 3 (same as shell) so it is never assigned a Codex thread
        let p = make_pane("%1", "/proj", Some("runtime_unknown"));
        assert_eq!(
            pane_tier(&p),
            3,
            "runtime_unknown should be tier 3 (ineligible for Codex assignment)"
        );
    }

    #[tokio::test]
    async fn process_thread_list_runtime_unknown_panes_never_assigned() {
        // 2 threads, 2 panes: runtime_unknown (tier3) and codex (tier0)
        // Only the codex pane should be assigned.
        let mut client = make_test_client().await;
        let now = Utc::now();
        let panes = vec![
            make_pane("%1", "/proj", Some("codex")), // eligible (tier 0)
            make_pane("%2", "/proj", Some("runtime_unknown")), // ineligible (tier 3)
        ];
        let resp = make_thread_list_response(&["t-a", "t-b"]);
        let mut tick = HashSet::new();
        let mut events = Vec::new();

        client.process_thread_list_response(&resp, &panes, &mut tick, now, &mut events);

        assert_eq!(client.thread_pane_bindings["t-a"].pane_id, "%1");
        assert!(
            !client.thread_pane_bindings.contains_key("t-b"),
            "runtime_unknown pane must not receive Codex thread assignment"
        );
    }

    #[tokio::test]
    async fn process_thread_list_shell_panes_never_assigned() {
        // 2 threads, 3 panes: node (tier1), zsh (tier3), zsh (tier3)
        // Only the node pane should receive a thread — shells are ineligible.
        let mut client = make_test_client().await;
        let now = Utc::now();
        let panes = vec![
            make_pane("%1", "/proj", None),          // node — eligible (tier 1)
            make_pane("%2", "/proj", Some("shell")), // zsh  — ineligible (tier 3)
            make_pane("%3", "/proj", Some("shell")), // zsh  — ineligible (tier 3)
        ];
        let resp = make_thread_list_response(&["t-a", "t-b"]);
        let mut tick = HashSet::new();
        let mut events = Vec::new();

        client.process_thread_list_response(&resp, &panes, &mut tick, now, &mut events);

        // t-a → %1 (only eligible pane)
        assert_eq!(client.thread_pane_bindings["t-a"].pane_id, "%1");
        // t-b: no eligible pane left — must NOT be assigned
        assert!(
            !client.thread_pane_bindings.contains_key("t-b"),
            "shell panes must not receive Codex thread assignment"
        );
    }

    // ── process_thread_list_response: stable multi-pane assignment ────

    /// Creates a minimal `CodexAppServerClient` backed by a `cat` subprocess.
    /// The subprocess is never actually communicated with in these tests;
    /// we only exercise the pure-Rust assignment logic.
    async fn make_test_client() -> CodexAppServerClient {
        let mut child = Command::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("cat must be available on the test host");
        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout piped"));
        CodexAppServerClient {
            _child: child,
            stdin,
            stdout,
            next_id: 1,
            last_thread_states: HashMap::new(),
            thread_pane_bindings: HashMap::new(),
            ready_at: Instant::now(), // tests start ready immediately
        }
    }

    fn make_thread_list_response(thread_ids: &[&str]) -> serde_json::Value {
        let threads: Vec<serde_json::Value> = thread_ids
            .iter()
            .map(|id| serde_json::json!({"id": id, "status": {"type": "idle"}}))
            .collect();
        serde_json::json!({"result": {"data": threads}})
    }

    #[tokio::test]
    async fn process_thread_list_multiple_threads_stable_assignment() {
        let mut client = make_test_client().await;
        let now = Utc::now();
        let panes = vec![
            make_pane("%1", "/proj", Some("codex")),
            make_pane("%2", "/proj", None),
        ];
        let resp = make_thread_list_response(&["t-a", "t-b"]);
        let mut tick = HashSet::new();
        let mut events = Vec::new();

        client.process_thread_list_response(&resp, &panes, &mut tick, now, &mut events);

        // t-a (first in thread_id sort) → %1 (first in pane group), t-b → %2
        assert_eq!(client.thread_pane_bindings["t-a"].pane_id, "%1");
        assert_eq!(client.thread_pane_bindings["t-b"].pane_id, "%2");
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn process_thread_list_stable_assignment_across_ticks() {
        let mut client = make_test_client().await;
        let now = Utc::now();
        let panes = vec![
            make_pane_with_gen("%1", "/proj", Some("codex"), Some(1), None),
            make_pane_with_gen("%2", "/proj", None, Some(1), None),
        ];
        let resp = make_thread_list_response(&["t-a", "t-b"]);

        // Tick 1
        let mut tick1 = HashSet::new();
        client.process_thread_list_response(&resp, &panes, &mut tick1, now, &mut Vec::new());
        assert_eq!(client.thread_pane_bindings["t-a"].pane_id, "%1");
        assert_eq!(client.thread_pane_bindings["t-b"].pane_id, "%2");

        // Tick 2 — same panes (generation matches) → same assignment
        let now2 = now + chrono::Duration::seconds(3);
        let mut tick2 = HashSet::new();
        let mut events2 = Vec::new();
        client.process_thread_list_response(&resp, &panes, &mut tick2, now2, &mut events2);
        assert_eq!(
            client.thread_pane_bindings["t-a"].pane_id, "%1",
            "assignment must be stable across ticks"
        );
        assert_eq!(client.thread_pane_bindings["t-b"].pane_id, "%2");
    }

    #[tokio::test]
    async fn process_thread_list_freed_pane_reassigned_to_new_thread() {
        let mut client = make_test_client().await;
        let now = Utc::now();
        let panes = vec![
            make_pane_with_gen("%1", "/proj", Some("codex"), Some(1), None),
            make_pane_with_gen("%2", "/proj", None, Some(1), None),
        ];

        // Tick 1: t-a → %1, t-b → %2
        let resp1 = make_thread_list_response(&["t-a", "t-b"]);
        let mut tick1 = HashSet::new();
        client.process_thread_list_response(&resp1, &panes, &mut tick1, now, &mut Vec::new());

        // Tick 2: t-a exits, t-c appears. t-b keeps %2; t-c should get %1 (freed).
        let now2 = now + chrono::Duration::seconds(3);
        let resp2 = make_thread_list_response(&["t-b", "t-c"]);
        let mut tick2 = HashSet::new();
        let mut events2 = Vec::new();
        client.process_thread_list_response(&resp2, &panes, &mut tick2, now2, &mut events2);

        assert_eq!(
            client.thread_pane_bindings["t-b"].pane_id, "%2",
            "t-b keeps its pane"
        );
        assert_eq!(
            client.thread_pane_bindings["t-c"].pane_id, "%1",
            "t-c gets the pane freed by t-a"
        );
    }

    #[tokio::test]
    async fn process_thread_list_generation_mismatch_invalidates_cache() {
        let mut client = make_test_client().await;
        let now = Utc::now();

        // Tick 1: bind t-a → %1 (gen=1)
        let panes_gen1 = vec![make_pane_with_gen(
            "%1",
            "/proj",
            Some("codex"),
            Some(1),
            None,
        )];
        let resp = make_thread_list_response(&["t-a"]);
        let mut tick1 = HashSet::new();
        client.process_thread_list_response(&resp, &panes_gen1, &mut tick1, now, &mut Vec::new());
        assert_eq!(client.thread_pane_bindings["t-a"].pane_id, "%1");

        // Pane %1 is recycled (generation bumped to 2). New pane %2 added.
        let panes_gen2 = vec![
            make_pane_with_gen("%1", "/proj", Some("codex"), Some(2), None), // recycled
            make_pane_with_gen("%2", "/proj", None, Some(1), None),
        ];
        let now2 = now + chrono::Duration::seconds(3);
        let mut tick2 = HashSet::new();
        let mut events2 = Vec::new();
        client.process_thread_list_response(&resp, &panes_gen2, &mut tick2, now2, &mut events2);

        // Old binding (gen=1) is invalid → should get new unclaimed pane (%1, gen=2)
        let binding = &client.thread_pane_bindings["t-a"];
        assert_eq!(binding.pane_id, "%1");
        assert_eq!(
            binding.pane_generation,
            Some(2),
            "binding must use new generation"
        );
    }

    // ── classify_notloaded_status: mtime-based classification ────────

    #[test]
    fn classify_notloaded_empty_path_returns_none() {
        assert!(
            classify_notloaded_status("").is_none(),
            "empty path must return None"
        );
    }

    #[test]
    fn classify_notloaded_nonexistent_path_returns_none() {
        assert!(
            classify_notloaded_status("/nonexistent/path/does-not-exist.jsonl").is_none(),
            "missing file must return None (can't stat)"
        );
    }

    #[test]
    fn classify_notloaded_recent_file_returns_active() {
        // Create a temp file with a very recent mtime (just written)
        let dir = std::env::temp_dir();
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            "agtmux-test-notloaded-active-{}-{}.jsonl",
            std::process::id(),
            n
        ));
        std::fs::write(&path, b"test").expect("can create temp file");

        let result = classify_notloaded_status(path.to_str().expect("temp path is valid UTF-8"));
        let _ = std::fs::remove_file(&path);

        assert_eq!(
            result,
            Some("active"),
            "file written just now must classify as active"
        );
    }

    #[test]
    fn classify_notloaded_old_file_returns_none() {
        // A file that doesn't exist returns None (simulates an old session whose
        // JSONL file we can't reach).  We rely on this behaviour for old sessions.
        let result = classify_notloaded_status("/tmp/agtmux-test-very-old-session.jsonl");
        assert!(
            result.is_none(),
            "old/missing file must return None (historical session)"
        );
    }

    // ── process_thread_list_response: notLoaded CWD-query handling ───

    /// Helper: build a thread-list response with a notLoaded thread pointing
    /// to a *real, just-written* temp file (simulates an active session).
    fn make_notloaded_thread_response_active() -> (serde_json::Value, std::path::PathBuf) {
        let dir = std::env::temp_dir();
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            "agtmux-test-notloaded-thread-{}-{}.jsonl",
            std::process::id(),
            n
        ));
        std::fs::write(&path, b"session data").expect("can create temp file");
        let resp = serde_json::json!({
            "result": {
                "data": [{
                    "id": "t-notloaded",
                    "status": {"type": "notLoaded"},
                    "path": path.to_str().expect("temp path is valid UTF-8"),
                    "cwd": "/proj"
                }]
            }
        });
        (resp, path)
    }

    #[tokio::test]
    async fn process_thread_list_notloaded_recent_file_emits_active() {
        // A notLoaded thread whose session file was just written → classify as "active"
        let mut client = make_test_client().await;
        let now = Utc::now();
        let panes = vec![make_pane("%1", "/proj", Some("codex"))];
        let (resp, path) = make_notloaded_thread_response_active();
        let mut tick = HashSet::new();
        let mut events = Vec::new();

        client.process_thread_list_response(&resp, &panes, &mut tick, now, &mut events);

        let _ = std::fs::remove_file(&path);

        assert!(
            !events.is_empty(),
            "must emit at least one event for recently-active notLoaded thread"
        );
        assert_eq!(
            events[0].event_type, "thread.active",
            "recent notLoaded → thread.active"
        );
        assert_eq!(
            events[0].pane_id,
            Some("%1".to_string()),
            "event must be bound to the codex pane"
        );
    }

    #[tokio::test]
    async fn process_thread_list_notloaded_global_query_always_skipped() {
        // Even with a recent file, notLoaded threads must be skipped in global query
        // (empty pane_infos).
        let mut client = make_test_client().await;
        let now = Utc::now();
        let (resp, path) = make_notloaded_thread_response_active();
        let mut tick = HashSet::new();
        let mut events = Vec::new();

        client.process_thread_list_response(&resp, &[], &mut tick, now, &mut events);

        let _ = std::fs::remove_file(&path);

        assert!(
            events.is_empty(),
            "notLoaded threads must be skipped in global query (no pane assignment possible)"
        );
    }

    #[tokio::test]
    async fn process_thread_list_notloaded_missing_path_skipped() {
        // notLoaded thread with no "path" field → classify_notloaded_status returns None → skip
        let mut client = make_test_client().await;
        let now = Utc::now();
        let panes = vec![make_pane("%1", "/proj", Some("codex"))];
        let resp = serde_json::json!({
            "result": {"data": [{"id": "t-np", "status": {"type": "notLoaded"}}]}
        });
        let mut tick = HashSet::new();
        let mut events = Vec::new();

        client.process_thread_list_response(&resp, &panes, &mut tick, now, &mut events);

        assert!(
            events.is_empty(),
            "notLoaded thread with no path must be skipped"
        );
    }
}
