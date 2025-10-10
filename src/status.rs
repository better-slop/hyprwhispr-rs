use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// Writes recording status for Waybar tray script to read
pub struct StatusWriter {
    status_file: PathBuf,
}

impl StatusWriter {
    pub fn new() -> Result<Self> {
        let config_dir = directories::ProjectDirs::from("", "", "hyprwhspr-rs")
            .context("Failed to get config directory")?
            .config_dir()
            .to_path_buf();

        fs::create_dir_all(&config_dir).context("Failed to create config directory")?;

        Ok(Self {
            status_file: config_dir.join("recording_status"),
        })
    }

    /// Set recording status
    /// - recording=true: writes "true" to file
    /// - recording=false: removes the file (matches Python behavior)
    pub fn set_recording(&self, recording: bool) -> Result<()> {
        if recording {
            fs::write(&self.status_file, "true").context("Failed to write recording status")?;
            tracing::debug!("Set recording status: true");
        } else {
            // Remove file when not recording to avoid stale state
            if self.status_file.exists() {
                fs::remove_file(&self.status_file)
                    .context("Failed to remove recording status file")?;
                tracing::debug!("Removed recording status file");
            }
        }
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        if let Ok(content) = fs::read_to_string(&self.status_file) {
            content.trim() == "true"
        } else {
            false
        }
    }
}

impl Default for StatusWriter {
    fn default() -> Self {
        Self::new().expect("Failed to create StatusWriter")
    }
}
