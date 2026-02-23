use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::{ActivityState, Provider, SourceType};

/// A single observation contributing to state inference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub provider: Provider,
    pub kind: EvidenceKind,
    pub signal: ActivityState,
    pub weight: f64,
    pub confidence: f64,
    pub timestamp: DateTime<Utc>,
    #[serde(with = "humantime_serde_compat")]
    pub ttl: Duration,
    pub source: SourceType,
    pub reason_code: String,
}

/// Distinguishes how the evidence was obtained.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum EvidenceKind {
    HookEvent(String),
    ApiNotification(String),
    FileChange(String),
    PollerMatch(String),
}

// Simple Duration serde helper (seconds as f64)
mod humantime_serde_compat {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(d.as_secs_f64())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = f64::deserialize(d)?;
        Ok(Duration::from_secs_f64(secs))
    }
}
