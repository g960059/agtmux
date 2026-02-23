use agtmux_core::engine::{Engine, EngineConfig};
use agtmux_core::types::{ActivityState, Evidence};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct Fixture {
    name: String,
    #[allow(dead_code)]
    description: String,
    evidence: Vec<Evidence>,
    now: DateTime<Utc>,
    expected: Expected,
}

#[derive(Debug, Deserialize)]
struct Expected {
    activity_state: ActivityState,
    confidence_min: f64,
}

fn fixtures_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.join("../../fixtures")
}

fn load_fixtures_from_dir(dir: &std::path::Path) -> Vec<Fixture> {
    let mut fixtures = Vec::new();
    if !dir.exists() {
        return fixtures;
    }
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let content = fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!("failed to read fixture {:?}: {}", path, e);
            });
            let fixture: Fixture = serde_json::from_str(&content).unwrap_or_else(|e| {
                panic!("failed to parse fixture {:?}: {}", path, e);
            });
            fixtures.push(fixture);
        }
    }
    fixtures
}

fn load_all_unit_fixtures() -> Vec<Fixture> {
    let base = fixtures_dir();
    let mut all = Vec::new();
    for subdir in &[
        "claude",
        "codex",
        "gemini",
        "copilot",
        "attention",
        "edge_cases",
    ] {
        all.extend(load_fixtures_from_dir(&base.join(subdir)));
    }
    all
}

#[test]
fn test_all_fixtures() {
    let fixtures = load_all_unit_fixtures();
    assert!(
        !fixtures.is_empty(),
        "no fixtures found in {:?}",
        fixtures_dir()
    );

    let engine = Engine::new(EngineConfig::default());
    let mut passed = 0;
    let mut failed = 0;

    for fixture in &fixtures {
        let result = engine.resolve(&fixture.evidence, fixture.now);

        let state_ok = result.state == fixture.expected.activity_state;
        let confidence_ok = result.confidence >= fixture.expected.confidence_min;

        if !state_ok || !confidence_ok {
            failed += 1;
            eprintln!(
                "FAIL: fixture '{}': expected state={:?} got={:?}, expected confidence>={:.2} got={:.2}",
                fixture.name,
                fixture.expected.activity_state,
                result.state,
                fixture.expected.confidence_min,
                result.confidence
            );
        } else {
            passed += 1;
        }
    }

    eprintln!(
        "\nFixture results: {} passed, {} failed, {} total",
        passed,
        failed,
        passed + failed
    );

    assert_eq!(failed, 0, "{} fixtures failed", failed);
}
