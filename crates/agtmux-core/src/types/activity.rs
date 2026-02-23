use serde::{Deserialize, Serialize};

/// Agent activity state, ordered by precedence.
/// Derive Ord so higher variants win in resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ActivityState {
    Unknown = 0,
    Idle = 1,
    Running = 2,
    WaitingInput = 3,
    WaitingApproval = 4,
    Error = 5,
}
