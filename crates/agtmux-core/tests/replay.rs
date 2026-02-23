use agtmux_core::engine::{Engine, EngineConfig};
use agtmux_core::types::{ActivityState, Evidence, EvidenceKind, Provider, SourceType};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    #[allow(dead_code)]
    description: String,
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
struct Step {
    timestamp: DateTime<Utc>,
    new_evidence: Vec<StepEvidence>,
    expected_activity: ActivityState,
}

/// Evidence within a step — timestamp is inherited from the step.
#[derive(Debug, Deserialize)]
struct StepEvidence {
    provider: Provider,
    kind: EvidenceKind,
    signal: ActivityState,
    weight: f64,
    confidence: f64,
    ttl: f64,
    source: SourceType,
    reason_code: String,
}

fn scenarios_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.join("../../fixtures/scenarios")
}

fn load_scenarios() -> Vec<Scenario> {
    let dir = scenarios_dir();
    let mut scenarios = Vec::new();
    if !dir.exists() {
        return scenarios;
    }
    for entry in fs::read_dir(&dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let content = fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!("failed to read scenario {:?}: {}", path, e);
            });
            let scenario: Scenario = serde_json::from_str(&content).unwrap_or_else(|e| {
                panic!("failed to parse scenario {:?}: {}", path, e);
            });
            scenarios.push(scenario);
        }
    }
    scenarios
}

/// Key for evidence supersession: same provider + source = replace.
/// This simulates the orchestrator's evidence management where a new event
/// from the same provider/source supersedes previous evidence.
fn evidence_key(ev: &Evidence) -> (Provider, SourceType) {
    (ev.provider, ev.source)
}

#[test]
fn test_all_scenarios() {
    let scenarios = load_scenarios();
    assert!(
        !scenarios.is_empty(),
        "no scenarios found in {:?}",
        scenarios_dir()
    );

    let engine = Engine::new(EngineConfig::default());
    let mut total_steps = 0;
    let mut passed_steps = 0;
    let mut failed_scenarios = Vec::new();

    for scenario in &scenarios {
        // Evidence window with supersession: new evidence from the same
        // (provider, source) replaces the old one, simulating the orchestrator.
        let mut evidence_window: Vec<Evidence> = Vec::new();
        let mut scenario_ok = true;

        for (i, step) in scenario.steps.iter().enumerate() {
            // Add new evidence, superseding old evidence from the same provider+source
            for ev in &step.new_evidence {
                let new_ev = Evidence {
                    provider: ev.provider,
                    kind: ev.kind.clone(),
                    signal: ev.signal,
                    weight: ev.weight,
                    confidence: ev.confidence,
                    timestamp: step.timestamp,
                    ttl: std::time::Duration::from_secs_f64(ev.ttl),
                    source: ev.source,
                    reason_code: ev.reason_code.clone(),
                };

                let key = evidence_key(&new_ev);
                evidence_window.retain(|old| evidence_key(old) != key);
                evidence_window.push(new_ev);
            }

            let result = engine.resolve(&evidence_window, step.timestamp);
            total_steps += 1;

            if result.state != step.expected_activity {
                scenario_ok = false;
                eprintln!(
                    "FAIL: scenario '{}' step {}: expected {:?} got {:?} (confidence={:.3})",
                    scenario.name, i, step.expected_activity, result.state, result.confidence
                );
            } else {
                passed_steps += 1;
            }
        }

        if !scenario_ok {
            failed_scenarios.push(scenario.name.clone());
        }
    }

    eprintln!(
        "\nReplay results: {}/{} steps passed, {}/{} scenarios passed",
        passed_steps,
        total_steps,
        scenarios.len() - failed_scenarios.len(),
        scenarios.len()
    );

    assert!(
        failed_scenarios.is_empty(),
        "failed scenarios: {:?}",
        failed_scenarios
    );
}
