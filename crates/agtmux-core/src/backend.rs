use crate::types::RawPane;

/// Abstraction over terminal backends (tmux, native PTY, Zellij, etc.)
///
/// Defined in agtmux-core (pure, no async) as a synchronous trait.
/// Async wrappers live in the backend implementation crates.
pub trait TerminalBackend: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn list_panes(&self) -> Result<Vec<RawPane>, Self::Error>;
    fn capture_pane(&self, pane_id: &str) -> Result<String, Self::Error>;
    fn select_pane(&self, pane_id: &str) -> Result<(), Self::Error>;
}
