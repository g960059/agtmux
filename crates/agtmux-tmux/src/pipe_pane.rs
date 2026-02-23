//! FIFO-based pane output capture via `tmux pipe-pane`.
//!
//! Each [`PaneTap`] manages the lifecycle of a named FIFO that receives
//! the raw byte stream from a single tmux pane.  The flow is:
//!
//! 1. `start()` — creates `/tmp/agtmux/pane-tap-{pid}-{pane_id}.fifo`,
//!    then runs `tmux pipe-pane -t {pane_id} -O "exec cat > {fifo}"`.
//! 2. `read()` — async, non-blocking read of up to 16 KiB from the FIFO.
//! 3. `stop()` — detaches pipe-pane, removes the FIFO.
//!
//! `Drop` performs best-effort FIFO cleanup.

use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

const FIFO_DIR: &str = "/tmp/agtmux";
const READ_BUF_SIZE: usize = 16 * 1024; // 16 KiB

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum PaneTapError {
    #[error("fifo creation failed: {0}")]
    FifoCreation(String),
    #[error("tmux pipe-pane failed: {0}")]
    PipePaneSetup(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// PaneTap
// ---------------------------------------------------------------------------

pub struct PaneTap {
    pane_id: String,
    fifo_path: PathBuf,
    tmux_bin: String,
    active: bool,
    /// Lazily-opened read handle for the FIFO.
    reader: Option<File>,
}

impl PaneTap {
    /// Create a new `PaneTap` for the given tmux pane, using the default
    /// `"tmux"` binary on `$PATH`.
    ///
    /// Does **not** start capturing yet — call [`start()`](Self::start) for
    /// that.
    pub fn new(pane_id: &str) -> Self {
        Self::with_tmux_bin(pane_id, "tmux")
    }

    /// Create a new `PaneTap` with a custom tmux binary path.
    ///
    /// This is consistent with [`TmuxExecutor::with_bin`] and useful when
    /// the tmux binary is not on `$PATH` or a specific version is needed.
    pub fn with_tmux_bin(pane_id: &str, tmux_bin: impl Into<String>) -> Self {
        let pid = std::process::id();
        // Sanitize pane_id: tmux pane ids look like %0, %1 etc.
        // We strip the leading '%' for the filename so the path is clean.
        let safe_id = pane_id.replace('%', "");
        let fifo_path = PathBuf::from(FIFO_DIR)
            .join(format!("pane-tap-{pid}-{safe_id}.fifo"));

        Self {
            pane_id: pane_id.to_owned(),
            fifo_path,
            tmux_bin: tmux_bin.into(),
            active: false,
            reader: None,
        }
    }

    /// Start capturing output from the pane.
    ///
    /// 1. Ensures `/tmp/agtmux/` exists.
    /// 2. Creates the FIFO via `mkfifo`.
    /// 3. Runs `tmux pipe-pane -t <pane> -O "exec cat > <fifo>"`.
    ///
    /// Returns the FIFO path on success.
    pub async fn start(&mut self) -> Result<PathBuf, PaneTapError> {
        // Ensure the directory exists.
        tokio::fs::create_dir_all(FIFO_DIR).await?;

        // Remove stale FIFO if it exists (ignore errors).
        let _ = tokio::fs::remove_file(&self.fifo_path).await;

        // Create the FIFO.
        self.mkfifo().await?;

        // Attach tmux pipe-pane.
        self.attach_pipe_pane().await?;

        self.active = true;
        Ok(self.fifo_path.clone())
    }

    /// Stop capturing: detaches `pipe-pane` and removes the FIFO.
    pub async fn stop(&mut self) -> Result<(), PaneTapError> {
        if !self.active {
            return Ok(());
        }

        // Drop the reader before removing the FIFO so the fd is closed.
        self.reader.take();

        // Detach pipe-pane (empty command = detach).
        self.detach_pipe_pane().await?;

        // Remove the FIFO file.
        if self.fifo_path.exists() {
            tokio::fs::remove_file(&self.fifo_path).await?;
        }

        self.active = false;
        Ok(())
    }

    /// Read available bytes from the FIFO (non-blocking, up to 16 KiB).
    ///
    /// Returns `Ok(None)` if no data is currently available.
    pub async fn read(&mut self) -> Result<Option<Vec<u8>>, PaneTapError> {
        if !self.active {
            return Ok(None);
        }

        // Lazily open the FIFO for reading.
        if self.reader.is_none() {
            let file = tokio::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&self.fifo_path)
                .await?;
            self.reader = Some(file);
        }

        let reader = self.reader.as_mut().unwrap();
        let mut buf = vec![0u8; READ_BUF_SIZE];

        match reader.read(&mut buf).await {
            Ok(0) => Ok(None),
            Ok(n) => {
                buf.truncate(n);
                Ok(Some(buf))
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(PaneTapError::Io(e)),
        }
    }

    /// Path to the FIFO file.
    pub fn fifo_path(&self) -> &PathBuf {
        &self.fifo_path
    }

    /// Whether this tap is currently active (capturing).
    pub fn is_active(&self) -> bool {
        self.active
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Create the named pipe via the `mkfifo` command.
    async fn mkfifo(&self) -> Result<(), PaneTapError> {
        let output = tokio::process::Command::new("mkfifo")
            .arg(&self.fifo_path)
            .output()
            .await
            .map_err(|e| PaneTapError::FifoCreation(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(PaneTapError::FifoCreation(format!(
                "mkfifo exited {}: {}",
                output.status.code().unwrap_or(-1),
                stderr.trim(),
            )));
        }

        Ok(())
    }

    /// Run `tmux pipe-pane -t <pane> -O "exec cat > <fifo>"`.
    async fn attach_pipe_pane(&self) -> Result<(), PaneTapError> {
        let cat_cmd = format!(
            "exec cat > '{}'",
            self.fifo_path.display().to_string().replace('\'', "'\\''"),
        );

        let output = tokio::process::Command::new(&self.tmux_bin)
            .args(["pipe-pane", "-t", &self.pane_id, "-O", &cat_cmd])
            .output()
            .await
            .map_err(|e| PaneTapError::PipePaneSetup(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(PaneTapError::PipePaneSetup(format!(
                "tmux pipe-pane exited {}: {}",
                output.status.code().unwrap_or(-1),
                stderr.trim(),
            )));
        }

        Ok(())
    }

    /// Run `tmux pipe-pane -t <pane>` (no command = detach).
    async fn detach_pipe_pane(&self) -> Result<(), PaneTapError> {
        let output = tokio::process::Command::new(&self.tmux_bin)
            .args(["pipe-pane", "-t", &self.pane_id])
            .output()
            .await
            .map_err(|e| PaneTapError::PipePaneSetup(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(PaneTapError::PipePaneSetup(format!(
                "tmux pipe-pane detach exited {}: {}",
                output.status.code().unwrap_or(-1),
                stderr.trim(),
            )));
        }

        Ok(())
    }
}

impl Drop for PaneTap {
    /// Best-effort cleanup: remove the FIFO file.
    ///
    /// We intentionally do NOT attempt to detach pipe-pane in Drop because
    /// spawning a child process in a destructor is unreliable and can panic
    /// in certain shutdown scenarios.  The caller should call `stop()` for
    /// a clean teardown.
    fn drop(&mut self) {
        self.reader.take();
        let _ = std::fs::remove_file(&self.fifo_path);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_correct_fifo_path() {
        let tap = PaneTap::new("%42");
        let pid = std::process::id();
        let expected = PathBuf::from(format!("/tmp/agtmux/pane-tap-{pid}-42.fifo"));
        assert_eq!(tap.fifo_path(), &expected);
        assert!(!tap.is_active());
    }

    #[test]
    fn new_strips_percent_from_pane_id() {
        let tap = PaneTap::new("%0");
        assert!(
            !tap.fifo_path()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .contains('%'),
            "FIFO filename should not contain '%'",
        );
    }

    #[test]
    fn new_without_percent() {
        let tap = PaneTap::new("7");
        let pid = std::process::id();
        let expected = PathBuf::from(format!("/tmp/agtmux/pane-tap-{pid}-7.fifo"));
        assert_eq!(tap.fifo_path(), &expected);
    }

    #[test]
    fn with_tmux_bin_stores_custom_path() {
        let tap = PaneTap::with_tmux_bin("%1", "/usr/local/bin/tmux");
        assert_eq!(tap.tmux_bin, "/usr/local/bin/tmux");
    }

    #[test]
    fn new_uses_default_tmux_bin() {
        let tap = PaneTap::new("%1");
        assert_eq!(tap.tmux_bin, "tmux");
    }

    #[test]
    fn is_active_default_false() {
        let tap = PaneTap::new("%1");
        assert!(!tap.is_active());
    }

    #[test]
    fn drop_cleans_up_fifo() {
        // Create a real temp file to prove Drop removes it.
        let dir = std::env::temp_dir().join("agtmux-test-drop");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test-drop.fifo");
        std::fs::write(&path, b"").unwrap();
        assert!(path.exists());

        // Build a PaneTap whose fifo_path points to this file, then drop it.
        let tap = PaneTap {
            pane_id: "test".into(),
            fifo_path: path.clone(),
            tmux_bin: "tmux".into(),
            active: false,
            reader: None,
        };
        drop(tap);

        assert!(!path.exists(), "FIFO should be removed on drop");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Integration tests that require a running tmux server are intentionally
    // omitted here.  They belong in `tests/integration/` and should be gated
    // behind a `#[cfg(feature = "integration")]` or similar.
}
