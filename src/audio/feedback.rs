use anyhow::{Context, Result};
use rodio::{Decoder, OutputStream, Sink};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use tracing::{debug, error, warn};

pub struct AudioFeedback {
    enabled: bool,
    start_sound: PathBuf,
    stop_sound: PathBuf,
    start_volume: f32,
    stop_volume: f32,
}

impl AudioFeedback {
    pub fn new(
        enabled: bool,
        assets_dir: PathBuf,
        start_sound_path: Option<String>,
        stop_sound_path: Option<String>,
        start_volume: f32,
        stop_volume: f32,
    ) -> Self {
        // Resolve start sound path
        let start_sound = if let Some(ref path) = start_sound_path {
            let custom_path = PathBuf::from(path);
            if custom_path.exists() {
                custom_path
            } else {
                let relative_path = assets_dir.join(path);
                if relative_path.exists() {
                    relative_path
                } else {
                    assets_dir.join("ping-up.ogg")
                }
            }
        } else {
            assets_dir.join("ping-up.ogg")
        };

        // Resolve stop sound path
        let stop_sound = if let Some(ref path) = stop_sound_path {
            let custom_path = PathBuf::from(path);
            if custom_path.exists() {
                custom_path
            } else {
                let relative_path = assets_dir.join(path);
                if relative_path.exists() {
                    relative_path
                } else {
                    assets_dir.join("ping-down.ogg")
                }
            }
        } else {
            assets_dir.join("ping-down.ogg")
        };

        // Validate volumes
        let start_volume = start_volume.clamp(0.1, 1.0);
        let stop_volume = stop_volume.clamp(0.1, 1.0);

        // Check if sound files exist
        if !start_sound.exists() {
            warn!("Start sound not found: {:?}", start_sound);
        }
        if !stop_sound.exists() {
            warn!("Stop sound not found: {:?}", stop_sound);
        }

        debug!(
            "Audio feedback initialized - enabled: {}, start: {:?}, stop: {:?}",
            enabled, start_sound, stop_sound
        );

        Self {
            enabled,
            start_sound,
            stop_sound,
            start_volume,
            stop_volume,
        }
    }

    pub fn play_start_sound(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        debug!("Playing start sound: {:?}", self.start_sound);
        self.play_sound(&self.start_sound, self.start_volume)
    }

    pub fn play_stop_sound(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        debug!("Playing stop sound: {:?}", self.stop_sound);
        self.play_sound(&self.stop_sound, self.stop_volume)
    }

    fn play_sound(&self, path: &PathBuf, volume: f32) -> Result<()> {
        if !path.exists() {
            warn!("Sound file not found: {:?}", path);
            return Ok(());
        }

        // Spawn in a separate thread to avoid blocking
        let path = path.clone();
        std::thread::spawn(move || {
            if let Err(e) = Self::play_sound_blocking(&path, volume) {
                error!("Failed to play sound {:?}: {}", path, e);
            }
        });

        Ok(())
    }

    fn play_sound_blocking(path: &PathBuf, volume: f32) -> Result<()> {
        // Create output stream
        let (_stream, stream_handle) =
            OutputStream::try_default().context("Failed to open audio output")?;

        // Create sink
        let sink = Sink::try_new(&stream_handle).context("Failed to create audio sink")?;

        // Load and decode audio file
        let file =
            File::open(path).with_context(|| format!("Failed to open audio file: {:?}", path))?;
        let source = Decoder::new(BufReader::new(file)).context("Failed to decode audio file")?;

        // Set volume and play
        sink.set_volume(volume);
        sink.append(source);

        // Wait for playback to complete
        sink.sleep_until_end();

        Ok(())
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        debug!("Audio feedback enabled: {}", enabled);
    }
}
