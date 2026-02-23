use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use crate::label::RecordedEvent;

#[derive(Debug, Deserialize)]
pub struct ExpectedLabel {
    pub ts: String,
    pub state: String,
    pub pane_id: Option<String>,
}

#[derive(Debug)]
pub struct ExpectedInterval {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub state: String,
    pub pane_id: Option<String>,
}

pub struct AutoLabelReport {
    pub total_events: usize,
    pub labeled_events: usize,
    pub unlabeled_events: usize,
}

pub fn run_auto_label(
    recording_path: &Path,
    expected_path: &Path,
    window: Duration,
    output_path: &Path,
) -> Result<AutoLabelReport, Box<dyn std::error::Error>> {
    let labels = parse_expected_labels(expected_path)?;
    let intervals = labels_to_intervals(&labels);

    let file = std::fs::File::open(recording_path)?;
    let reader = BufReader::new(file);

    let mut events: Vec<RecordedEvent> = Vec::new();
    for (line_num, line_result) in reader.lines().enumerate() {
        let line = line_result?;
        if line.trim().is_empty() {
            continue;
        }
        let event: RecordedEvent = serde_json::from_str(&line)
            .map_err(|e| format!("line {}: {}", line_num + 1, e))?;
        events.push(event);
    }

    let mut labeled_events = 0usize;
    let mut unlabeled_events = 0usize;

    for event in &mut events {
        if event.event_type != "StateChanged" {
            continue;
        }

        let pane_id = event.data.get("pane_id").and_then(|v| v.as_str());
        let event_ts = DateTime::parse_from_rfc3339(&event.ts)
            .map(|dt| dt.with_timezone(&Utc));

        match (event_ts, pane_id) {
            (Ok(ts), Some(pid)) => {
                if let Some(state) = find_expected_state(&intervals, ts, pid, window) {
                    event.label = Some(state);
                    labeled_events += 1;
                } else {
                    unlabeled_events += 1;
                }
            }
            _ => {
                unlabeled_events += 1;
            }
        }
    }

    let total_events = events.len();
    let mut out_file = std::fs::File::create(output_path)?;
    for event in &events {
        let line = serde_json::to_string(event)?;
        writeln!(out_file, "{}", line)?;
    }

    Ok(AutoLabelReport {
        total_events,
        labeled_events,
        unlabeled_events,
    })
}

fn parse_expected_labels(path: &Path) -> Result<Vec<ExpectedLabel>, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut labels = Vec::new();

    for (line_num, line_result) in reader.lines().enumerate() {
        let line = line_result?;
        if line.trim().is_empty() {
            continue;
        }
        let label: ExpectedLabel = serde_json::from_str(&line)
            .map_err(|e| format!("expected labels line {}: {}", line_num + 1, e))?;
        labels.push(label);
    }

    Ok(labels)
}

fn labels_to_intervals(labels: &[ExpectedLabel]) -> Vec<ExpectedInterval> {
    let mut by_pane: std::collections::HashMap<Option<&str>, Vec<(DateTime<Utc>, &str)>> =
        std::collections::HashMap::new();

    for label in labels {
        let ts = DateTime::parse_from_rfc3339(&label.ts)
            .expect("expected label timestamp must be valid RFC 3339")
            .with_timezone(&Utc);
        let key = label.pane_id.as_deref();
        by_pane.entry(key).or_default().push((ts, &label.state));
    }

    let mut intervals = Vec::new();

    for (pane_id, mut entries) in by_pane {
        entries.sort_by_key(|(ts, _)| *ts);

        for i in 0..entries.len() {
            let (start, state) = entries[i];
            let end = if i + 1 < entries.len() {
                entries[i + 1].0
            } else {
                DateTime::<Utc>::MAX_UTC
            };

            intervals.push(ExpectedInterval {
                start,
                end,
                state: state.to_string(),
                pane_id: pane_id.map(|s| s.to_string()),
            });
        }
    }

    intervals
}

fn find_expected_state(
    intervals: &[ExpectedInterval],
    event_ts: DateTime<Utc>,
    pane_id: &str,
    window: Duration,
) -> Option<String> {
    let matching: Vec<&ExpectedInterval> = intervals
        .iter()
        .filter(|iv| match &iv.pane_id {
            Some(pid) => pid == pane_id,
            None => true,
        })
        .collect();

    // Direct containment: start <= event_ts < end
    for iv in &matching {
        if event_ts >= iv.start && event_ts < iv.end {
            return Some(iv.state.clone());
        }
    }

    // Window tolerance: event_ts is within `window` before an interval's start.
    // In that case, assign to the previous interval for that pane.
    let mut sorted = matching.clone();
    sorted.sort_by_key(|iv| iv.start);

    for (i, iv) in sorted.iter().enumerate() {
        let window_start = iv.start - window;
        if event_ts >= window_start && event_ts < iv.start {
            // Assign to previous interval if it exists
            if i > 0 {
                return Some(sorted[i - 1].state.clone());
            }
            return None;
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn make_labels(items: &[(&str, &str, &str)]) -> Vec<ExpectedLabel> {
        items
            .iter()
            .map(|(t, state, pane)| ExpectedLabel {
                ts: t.to_string(),
                state: state.to_string(),
                pane_id: if pane.is_empty() {
                    None
                } else {
                    Some(pane.to_string())
                },
            })
            .collect()
    }

    #[test]
    fn labels_to_intervals_basic() {
        let labels = make_labels(&[
            ("2026-02-23T12:00:00Z", "running", "%1"),
            ("2026-02-23T12:01:00Z", "idle", "%1"),
            ("2026-02-23T12:02:00Z", "error", "%1"),
        ]);

        let intervals = labels_to_intervals(&labels);
        assert_eq!(intervals.len(), 3);

        let mut sorted: Vec<_> = intervals.iter().collect();
        sorted.sort_by_key(|iv| iv.start);

        assert_eq!(sorted[0].state, "running");
        assert_eq!(sorted[0].start, ts("2026-02-23T12:00:00Z"));
        assert_eq!(sorted[0].end, ts("2026-02-23T12:01:00Z"));

        assert_eq!(sorted[1].state, "idle");
        assert_eq!(sorted[1].start, ts("2026-02-23T12:01:00Z"));
        assert_eq!(sorted[1].end, ts("2026-02-23T12:02:00Z"));

        assert_eq!(sorted[2].state, "error");
        assert_eq!(sorted[2].start, ts("2026-02-23T12:02:00Z"));
        assert_eq!(sorted[2].end, DateTime::<Utc>::MAX_UTC);
    }

    #[test]
    fn find_expected_state_middle_of_interval() {
        let labels = make_labels(&[
            ("2026-02-23T12:00:00Z", "running", "%1"),
            ("2026-02-23T12:01:00Z", "idle", "%1"),
        ]);
        let intervals = labels_to_intervals(&labels);

        let result = find_expected_state(
            &intervals,
            ts("2026-02-23T12:00:30Z"),
            "%1",
            Duration::seconds(5),
        );
        assert_eq!(result, Some("running".to_string()));
    }

    #[test]
    fn find_expected_state_at_boundary() {
        let labels = make_labels(&[
            ("2026-02-23T12:00:00Z", "running", "%1"),
            ("2026-02-23T12:01:00Z", "idle", "%1"),
        ]);
        let intervals = labels_to_intervals(&labels);

        // At exact boundary of second interval
        let result = find_expected_state(
            &intervals,
            ts("2026-02-23T12:01:00Z"),
            "%1",
            Duration::seconds(5),
        );
        assert_eq!(result, Some("idle".to_string()));

        // At exact start of first interval
        let result = find_expected_state(
            &intervals,
            ts("2026-02-23T12:00:00Z"),
            "%1",
            Duration::seconds(5),
        );
        assert_eq!(result, Some("running".to_string()));
    }

    #[test]
    fn find_expected_state_before_first_label() {
        let labels = make_labels(&[
            ("2026-02-23T12:00:00Z", "running", "%1"),
        ]);
        let intervals = labels_to_intervals(&labels);

        let result = find_expected_state(
            &intervals,
            ts("2026-02-23T11:59:00Z"),
            "%1",
            Duration::seconds(5),
        );
        assert_eq!(result, None);
    }

    #[test]
    fn find_expected_state_after_last_label() {
        let labels = make_labels(&[
            ("2026-02-23T12:00:00Z", "running", "%1"),
            ("2026-02-23T12:01:00Z", "idle", "%1"),
        ]);
        let intervals = labels_to_intervals(&labels);

        let result = find_expected_state(
            &intervals,
            ts("2026-02-23T12:05:00Z"),
            "%1",
            Duration::seconds(5),
        );
        assert_eq!(result, Some("idle".to_string()));
    }

    #[test]
    fn window_tolerance() {
        let labels = make_labels(&[
            ("2026-02-23T12:00:00Z", "running", "%1"),
            ("2026-02-23T12:01:00Z", "idle", "%1"),
        ]);
        let intervals = labels_to_intervals(&labels);

        // 3 seconds before the boundary at 12:01:00 — within 5s window
        let result = find_expected_state(
            &intervals,
            ts("2026-02-23T12:00:57Z"),
            "%1",
            Duration::seconds(5),
        );
        // Direct containment in "running" interval (12:00:00..12:01:00) wins
        assert_eq!(result, Some("running".to_string()));

        // 3 seconds before the first interval at 12:00:00 — within 5s window,
        // but no previous interval exists
        let result = find_expected_state(
            &intervals,
            ts("2026-02-23T11:59:57Z"),
            "%1",
            Duration::seconds(5),
        );
        assert_eq!(result, None);
    }

    #[test]
    fn pane_id_filtering() {
        let labels = make_labels(&[
            ("2026-02-23T12:00:00Z", "running", "%1"),
            ("2026-02-23T12:00:00Z", "idle", "%2"),
        ]);
        let intervals = labels_to_intervals(&labels);

        let result = find_expected_state(
            &intervals,
            ts("2026-02-23T12:00:30Z"),
            "%1",
            Duration::seconds(5),
        );
        assert_eq!(result, Some("running".to_string()));

        let result = find_expected_state(
            &intervals,
            ts("2026-02-23T12:00:30Z"),
            "%2",
            Duration::seconds(5),
        );
        assert_eq!(result, Some("idle".to_string()));

        // Unknown pane
        let result = find_expected_state(
            &intervals,
            ts("2026-02-23T12:00:30Z"),
            "%99",
            Duration::seconds(5),
        );
        assert_eq!(result, None);
    }

    #[test]
    fn end_to_end_auto_label() {
        // Create recording JSONL
        let mut recording = NamedTempFile::new().unwrap();
        let events = [
            r#"{"ts":"2026-02-23T12:00:05.000000000+00:00","type":"PaneAdded","data":{"pane_id":"%1"}}"#,
            r#"{"ts":"2026-02-23T12:00:10.000000000+00:00","type":"StateChanged","data":{"pane_id":"%1","state":{"activity":{"state":"running","confidence":0.9,"source":"hook","reason_code":"tool_running"}}}}"#,
            r#"{"ts":"2026-02-23T12:01:30.000000000+00:00","type":"StateChanged","data":{"pane_id":"%1","state":{"activity":{"state":"idle","confidence":0.8,"source":"poller","reason_code":"no_activity"}}}}"#,
            r#"{"ts":"2026-02-23T12:02:30.000000000+00:00","type":"StateChanged","data":{"pane_id":"%1","state":{"activity":{"state":"error","confidence":0.95,"source":"hook","reason_code":"exit_nonzero"}}}}"#,
        ];
        for line in &events {
            writeln!(recording, "{}", line).unwrap();
        }

        // Create expected labels JSONL
        let mut expected = NamedTempFile::new().unwrap();
        let labels = [
            r#"{"ts":"2026-02-23T12:00:00Z","state":"running","pane_id":"%1"}"#,
            r#"{"ts":"2026-02-23T12:01:00Z","state":"idle","pane_id":"%1"}"#,
            r#"{"ts":"2026-02-23T12:02:00Z","state":"error","pane_id":"%1"}"#,
        ];
        for line in &labels {
            writeln!(expected, "{}", line).unwrap();
        }

        let output = NamedTempFile::new().unwrap();
        let report = run_auto_label(
            recording.path(),
            expected.path(),
            Duration::seconds(5),
            output.path(),
        )
        .unwrap();

        assert_eq!(report.total_events, 4);
        assert_eq!(report.labeled_events, 3);
        assert_eq!(report.unlabeled_events, 0);

        // Verify output is valid label::RecordedEvent JSONL
        let out_content = std::fs::read_to_string(output.path()).unwrap();
        let out_events: Vec<RecordedEvent> = out_content
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();

        assert_eq!(out_events.len(), 4);

        // PaneAdded: no label
        assert_eq!(out_events[0].event_type, "PaneAdded");
        assert!(out_events[0].label.is_none());

        // StateChanged events have labels
        assert_eq!(out_events[1].label, Some("running".to_string()));
        assert_eq!(out_events[2].label, Some("idle".to_string()));
        assert_eq!(out_events[3].label, Some("error".to_string()));

        // Verify the data field is preserved for accuracy extraction
        let predicted = crate::accuracy::extract_predicted_state(&out_events[1].data);
        assert_eq!(predicted, Some("running".to_string()));
    }
}
