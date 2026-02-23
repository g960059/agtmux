use crate::types::{ActivityState, Evidence, EvidenceKind, PaneMeta, Provider, SourceType};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

use super::{detect_by_agent_or_cmd, EvidenceBuilder, ProviderDetector};

/// Error type for provider loader operations.
#[derive(Debug)]
pub enum LoaderError {
    /// I/O error reading a file or directory.
    Io(std::io::Error),
    /// TOML parse error.
    Parse(toml::de::Error),
    /// The agent_type in the TOML does not map to a known Provider enum variant.
    InvalidProvider(String),
}

impl std::fmt::Display for LoaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoaderError::Io(e) => write!(f, "I/O error: {e}"),
            LoaderError::Parse(e) => write!(f, "TOML parse error: {e}"),
            LoaderError::InvalidProvider(name) => {
                write!(f, "unknown provider agent_type: {name:?}")
            }
        }
    }
}

impl std::error::Error for LoaderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoaderError::Io(e) => Some(e),
            LoaderError::Parse(e) => Some(e),
            LoaderError::InvalidProvider(_) => None,
        }
    }
}

impl From<std::io::Error> for LoaderError {
    fn from(e: std::io::Error) -> Self {
        LoaderError::Io(e)
    }
}

impl From<toml::de::Error> for LoaderError {
    fn from(e: toml::de::Error) -> Self {
        LoaderError::Parse(e)
    }
}

/// Declarative provider definition loaded from TOML.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderDef {
    pub detection: DetectionDef,
    pub signals: Vec<SignalDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DetectionDef {
    pub agent_type: String,
    pub cmd_tokens: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SignalDef {
    pub pattern: Vec<String>,
    pub activity: ActivityState,
    pub weight: f64,
    pub confidence: f64,
    #[serde(default = "default_ttl_secs")]
    pub ttl_secs: u64,
}

fn default_ttl_secs() -> u64 {
    90
}

impl ProviderDef {
    /// Parse a TOML string into a ProviderDef.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Resolve the Provider enum from the detection.agent_type field.
    pub fn provider(&self) -> Option<Provider> {
        match self.detection.agent_type.as_str() {
            "claude" => Some(Provider::Claude),
            "codex" => Some(Provider::Codex),
            "gemini" => Some(Provider::Gemini),
            "copilot" => Some(Provider::Copilot),
            _ => None,
        }
    }
}

/// TOML-driven ProviderDetector.
pub struct TomlDetector {
    provider: Provider,
    def: ProviderDef,
}

impl TomlDetector {
    pub fn new(provider: Provider, def: ProviderDef) -> Self {
        Self { provider, def }
    }
}

impl ProviderDetector for TomlDetector {
    fn id(&self) -> Provider {
        self.provider
    }

    fn detect(&self, meta: &PaneMeta) -> Option<f64> {
        let cmd_tokens: Vec<&str> = self.def.detection.cmd_tokens.iter().map(|s| s.as_str()).collect();
        detect_by_agent_or_cmd(meta, &self.def.detection.agent_type, &cmd_tokens)
    }
}

/// TOML-driven EvidenceBuilder.
pub struct TomlEvidenceBuilder {
    provider: Provider,
    def: ProviderDef,
}

impl TomlEvidenceBuilder {
    pub fn new(provider: Provider, def: ProviderDef) -> Self {
        Self { provider, def }
    }
}

impl EvidenceBuilder for TomlEvidenceBuilder {
    fn provider(&self) -> Provider {
        self.provider
    }

    fn build_evidence(&self, meta: &PaneMeta, now: DateTime<Utc>) -> Vec<Evidence> {
        let combined = format!(
            "{} {} {}",
            meta.raw_state, meta.raw_reason_code, meta.last_event_type
        )
        .to_lowercase();

        let mut evidence = Vec::new();

        for signal_def in &self.def.signals {
            let matched = signal_def.pattern.iter().any(|p| combined.contains(p));
            if matched {
                evidence.push(Evidence {
                    provider: self.provider,
                    kind: EvidenceKind::PollerMatch(
                        signal_def.pattern.first().cloned().unwrap_or_default(),
                    ),
                    signal: signal_def.activity,
                    weight: signal_def.weight,
                    confidence: signal_def.confidence,
                    timestamp: now,
                    ttl: Duration::from_secs(signal_def.ttl_secs),
                    source: SourceType::Poller,
                    reason_code: meta.raw_reason_code.clone(),
                });
            }
        }

        evidence
    }
}

// Compile-time embedded provider definitions.
pub const CLAUDE_TOML: &str = include_str!("../../../../providers/claude.toml");
pub const CODEX_TOML: &str = include_str!("../../../../providers/codex.toml");
pub const GEMINI_TOML: &str = include_str!("../../../../providers/gemini.toml");
pub const COPILOT_TOML: &str = include_str!("../../../../providers/copilot.toml");

/// Load a single provider from a TOML string.
///
/// Parses the TOML, resolves the `Provider` enum from `detection.agent_type`,
/// and returns a `(detector, builder)` pair.
///
/// This is the shared code path used by both `builtin_adapters()` and
/// `load_adapters_from_dir()`.
pub fn load_provider_from_toml(
    toml_str: &str,
) -> Result<(Box<dyn ProviderDetector>, Box<dyn EvidenceBuilder>), LoaderError> {
    let def = ProviderDef::from_toml(toml_str)?;
    let provider = def.provider().ok_or_else(|| {
        LoaderError::InvalidProvider(def.detection.agent_type.clone())
    })?;
    let detector: Box<dyn ProviderDetector> =
        Box::new(TomlDetector::new(provider, def.clone()));
    let builder: Box<dyn EvidenceBuilder> =
        Box::new(TomlEvidenceBuilder::new(provider, def));
    Ok((detector, builder))
}

/// Load provider adapters from a directory of TOML files at runtime.
///
/// All files matching `*.toml` in the given directory are loaded. Each file
/// must contain a valid `ProviderDef` whose `detection.agent_type` maps to a
/// known `Provider` variant.
pub fn load_adapters_from_dir(
    dir: &Path,
) -> Result<Vec<(Box<dyn ProviderDetector>, Box<dyn EvidenceBuilder>)>, LoaderError> {
    let mut adapters = Vec::new();
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(dir)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    for path in paths {
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            let content = std::fs::read_to_string(&path)?;
            let pair = load_provider_from_toml(&content)?;
            adapters.push(pair);
        }
    }
    Ok(adapters)
}

/// Merge runtime adapters with builtins.
///
/// Runtime adapters whose detector returns the same `Provider` ID as a builtin
/// will replace that builtin. Runtime adapters for new providers are appended.
pub fn merge_adapters(
    builtins: Vec<(Box<dyn ProviderDetector>, Box<dyn EvidenceBuilder>)>,
    runtime: Vec<(Box<dyn ProviderDetector>, Box<dyn EvidenceBuilder>)>,
) -> Vec<(Box<dyn ProviderDetector>, Box<dyn EvidenceBuilder>)> {
    // Collect provider IDs present in the runtime set.
    let runtime_providers: std::collections::HashSet<Provider> =
        runtime.iter().map(|(d, _)| d.id()).collect();

    // Keep builtins that are NOT overridden by a runtime adapter.
    let mut merged: Vec<(Box<dyn ProviderDetector>, Box<dyn EvidenceBuilder>)> = builtins
        .into_iter()
        .filter(|(d, _)| !runtime_providers.contains(&d.id()))
        .collect();

    // Append all runtime adapters (overrides + new providers).
    merged.extend(runtime);
    merged
}

/// Load all built-in provider definitions.
pub fn builtin_provider_defs() -> Vec<(Provider, ProviderDef)> {
    let defs = [
        (Provider::Claude, CLAUDE_TOML),
        (Provider::Codex, CODEX_TOML),
        (Provider::Gemini, GEMINI_TOML),
        (Provider::Copilot, COPILOT_TOML),
    ];

    defs.into_iter()
        .map(|(p, toml_str)| {
            let def = ProviderDef::from_toml(toml_str)
                .unwrap_or_else(|e| panic!("failed to parse {:?} provider TOML: {}", p, e));
            (p, def)
        })
        .collect()
}

/// Create detector + builder pairs from built-in TOML definitions.
///
/// Internally uses `load_provider_from_toml()` so the same parsing path is
/// exercised for both compile-time and runtime provider loading.
pub fn builtin_adapters() -> Vec<(Box<dyn ProviderDetector>, Box<dyn EvidenceBuilder>)> {
    let tomls = [CLAUDE_TOML, CODEX_TOML, GEMINI_TOML, COPILOT_TOML];

    tomls
        .into_iter()
        .map(|toml_str| {
            load_provider_from_toml(toml_str)
                .unwrap_or_else(|e| panic!("failed to load builtin provider TOML: {e}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_claude_toml() {
        let def = ProviderDef::from_toml(CLAUDE_TOML).unwrap();
        assert_eq!(def.detection.agent_type, "claude");
        assert_eq!(def.signals.len(), 5);
        assert_eq!(def.signals[0].activity, ActivityState::WaitingApproval);
        assert!((def.signals[0].weight - 0.95).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_all_builtin_providers() {
        let defs = builtin_provider_defs();
        assert_eq!(defs.len(), 4);
        assert_eq!(defs[0].0, Provider::Claude);
        assert_eq!(defs[1].0, Provider::Codex);
        assert_eq!(defs[2].0, Provider::Gemini);
        assert_eq!(defs[3].0, Provider::Copilot);
    }

    #[test]
    fn toml_evidence_builder_matches() {
        let def = ProviderDef::from_toml(CLAUDE_TOML).unwrap();
        let builder = TomlEvidenceBuilder::new(Provider::Claude, def);

        let meta = PaneMeta {
            pane_id: "%1".into(),
            agent_type: "claude".into(),
            current_cmd: "claude".into(),
            pane_title: "".into(),
            session_label: "".into(),
            raw_state: "waiting_approval".into(),
            raw_reason_code: "needs_approval".into(),
            last_event_type: "approval".into(),
        };

        let evidence = builder.build_evidence(&meta, Utc::now());
        assert!(!evidence.is_empty());
        assert!(evidence.iter().any(|e| e.signal == ActivityState::WaitingApproval));
    }

    #[test]
    fn toml_detector_matches() {
        let def = ProviderDef::from_toml(CLAUDE_TOML).unwrap();
        let detector = TomlDetector::new(Provider::Claude, def);

        let meta = PaneMeta {
            pane_id: "%1".into(),
            agent_type: "claude".into(),
            current_cmd: "".into(),
            pane_title: "".into(),
            session_label: "".into(),
            raw_state: "".into(),
            raw_reason_code: "".into(),
            last_event_type: "".into(),
        };

        assert_eq!(detector.detect(&meta), Some(1.0));
    }

    // -----------------------------------------------------------------
    // Tests for load_provider_from_toml
    // -----------------------------------------------------------------

    #[test]
    fn test_load_provider_from_toml_valid() {
        let toml_str = r#"
[detection]
agent_type = "claude"
cmd_tokens = ["claude"]

[[signals]]
pattern = ["running"]
activity = "running"
weight = 0.90
confidence = 0.85
ttl_secs = 60
"#;
        let (detector, builder) = load_provider_from_toml(toml_str).unwrap();
        assert_eq!(detector.id(), Provider::Claude);
        assert_eq!(builder.provider(), Provider::Claude);
    }

    #[test]
    fn test_load_provider_from_toml_invalid_toml() {
        let bad_toml = "this is not valid toml [][[]";
        let result = load_provider_from_toml(bad_toml);
        match result {
            Err(LoaderError::Parse(_)) => {} // expected
            Err(other) => panic!("expected LoaderError::Parse, got: {other:?}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn test_load_provider_from_toml_unknown_provider() {
        let toml_str = r#"
[detection]
agent_type = "unknown_agent"
cmd_tokens = ["ua"]

[[signals]]
pattern = ["running"]
activity = "running"
weight = 0.90
confidence = 0.85
"#;
        let result = load_provider_from_toml(toml_str);
        match result {
            Err(LoaderError::InvalidProvider(name)) => assert_eq!(name, "unknown_agent"),
            Err(other) => panic!("expected LoaderError::InvalidProvider, got: {other:?}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    // -----------------------------------------------------------------
    // Tests for load_adapters_from_dir
    // -----------------------------------------------------------------

    #[test]
    fn test_load_adapters_from_dir() {
        let tmp = std::env::temp_dir().join("agtmux_test_load_dir");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Write two valid TOML files
        let claude_toml = r#"
[detection]
agent_type = "claude"
cmd_tokens = ["claude"]

[[signals]]
pattern = ["running"]
activity = "running"
weight = 0.90
confidence = 0.85
"#;
        let codex_toml = r#"
[detection]
agent_type = "codex"
cmd_tokens = ["codex"]

[[signals]]
pattern = ["idle"]
activity = "idle"
weight = 0.80
confidence = 0.80
"#;
        std::fs::write(tmp.join("claude.toml"), claude_toml).unwrap();
        std::fs::write(tmp.join("codex.toml"), codex_toml).unwrap();
        // A non-toml file should be ignored
        std::fs::write(tmp.join("readme.txt"), "not a provider").unwrap();

        let adapters = load_adapters_from_dir(&tmp).unwrap();
        assert_eq!(adapters.len(), 2);

        let providers: std::collections::HashSet<Provider> =
            adapters.iter().map(|(d, _)| d.id()).collect();
        assert!(providers.contains(&Provider::Claude));
        assert!(providers.contains(&Provider::Codex));

        // Cleanup
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_load_adapters_from_dir_nonexistent() {
        let result = load_adapters_from_dir(Path::new("/nonexistent/path/agtmux_test_xxx"));
        match result {
            Err(LoaderError::Io(_)) => {} // expected
            Err(other) => panic!("expected LoaderError::Io, got: {other:?}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    // -----------------------------------------------------------------
    // Tests for merge_adapters
    // -----------------------------------------------------------------

    #[test]
    fn test_merge_adapters_override() {
        // Create a builtin Claude adapter with weight 0.90
        let builtin_toml = r#"
[detection]
agent_type = "claude"
cmd_tokens = ["claude"]

[[signals]]
pattern = ["running"]
activity = "running"
weight = 0.90
confidence = 0.85
"#;
        // Create a runtime Claude adapter with different weight 0.99
        let runtime_toml = r#"
[detection]
agent_type = "claude"
cmd_tokens = ["claude", "claude-custom"]

[[signals]]
pattern = ["running"]
activity = "running"
weight = 0.99
confidence = 0.95
"#;
        let builtins = vec![load_provider_from_toml(builtin_toml).unwrap()];
        let runtime = vec![load_provider_from_toml(runtime_toml).unwrap()];

        let merged = merge_adapters(builtins, runtime);

        // Should have exactly one Claude adapter (the runtime one).
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].0.id(), Provider::Claude);

        // Verify it is the runtime version by checking evidence weight.
        let meta = PaneMeta {
            pane_id: "%1".into(),
            agent_type: "claude".into(),
            current_cmd: "claude".into(),
            pane_title: "".into(),
            session_label: "".into(),
            raw_state: "running".into(),
            raw_reason_code: "".into(),
            last_event_type: "".into(),
        };
        let evidence = merged[0].1.build_evidence(&meta, Utc::now());
        assert!(!evidence.is_empty());
        assert!((evidence[0].weight - 0.99).abs() < f64::EPSILON);
    }

    #[test]
    fn test_merge_adapters_additive() {
        // Builtin has Claude only
        let claude_toml = r#"
[detection]
agent_type = "claude"
cmd_tokens = ["claude"]

[[signals]]
pattern = ["running"]
activity = "running"
weight = 0.90
confidence = 0.85
"#;
        // Runtime adds Codex (no overlap)
        let codex_toml = r#"
[detection]
agent_type = "codex"
cmd_tokens = ["codex"]

[[signals]]
pattern = ["idle"]
activity = "idle"
weight = 0.80
confidence = 0.80
"#;
        let builtins = vec![load_provider_from_toml(claude_toml).unwrap()];
        let runtime = vec![load_provider_from_toml(codex_toml).unwrap()];

        let merged = merge_adapters(builtins, runtime);

        // Should have both providers.
        assert_eq!(merged.len(), 2);

        let providers: std::collections::HashSet<Provider> =
            merged.iter().map(|(d, _)| d.id()).collect();
        assert!(providers.contains(&Provider::Claude));
        assert!(providers.contains(&Provider::Codex));
    }
}
