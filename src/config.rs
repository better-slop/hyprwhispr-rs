use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_primary_shortcut")]
    pub primary_shortcut: String,

    #[serde(default = "default_model")]
    pub model: String,

    #[serde(default)]
    pub fallback_cli: bool,

    #[serde(default = "default_threads")]
    pub threads: usize,

    #[serde(default)]
    pub word_overrides: HashMap<String, String>,

    #[serde(default = "default_whisper_prompt")]
    pub whisper_prompt: String,

    #[serde(default)]
    pub audio_feedback: bool,

    #[serde(default = "default_volume")]
    pub start_sound_volume: f32,

    #[serde(default = "default_volume")]
    pub stop_sound_volume: f32,

    #[serde(default)]
    pub start_sound_path: Option<String>,

    #[serde(default)]
    pub stop_sound_path: Option<String>,

    #[serde(default = "default_auto_copy_clipboard")]
    pub auto_copy_clipboard: bool,

    #[serde(default = "default_shift_paste")]
    pub shift_paste: bool,

    #[serde(default)]
    pub audio_device: Option<usize>,

    #[serde(default = "default_gpu_layers")]
    pub gpu_layers: i32,
}

fn default_gpu_layers() -> i32 {
    999 // Offload all layers to GPU by default
}

fn default_primary_shortcut() -> String {
    "SUPER+ALT+R".to_string() // R for Rust version (Python uses D)
}

fn default_model() -> String {
    "base".to_string()
}

fn default_threads() -> usize {
    4
}

fn default_whisper_prompt() -> String {
    "Transcribe with proper capitalization, including sentence beginnings, proper nouns, titles, and standard English capitalization rules.".to_string()
}

fn default_volume() -> f32 {
    0.3
}

fn default_auto_copy_clipboard() -> bool {
    true
}

fn default_shift_paste() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            primary_shortcut: default_primary_shortcut(),
            model: default_model(),
            fallback_cli: false,
            threads: default_threads(),
            word_overrides: HashMap::new(),
            whisper_prompt: default_whisper_prompt(),
            audio_feedback: false,
            start_sound_volume: default_volume(),
            stop_sound_volume: default_volume(),
            start_sound_path: None,
            stop_sound_path: None,
            auto_copy_clipboard: default_auto_copy_clipboard(),
            shift_paste: default_shift_paste(),
            audio_device: None,
            gpu_layers: default_gpu_layers(),
        }
    }
}

pub struct ConfigManager {
    config: Config,
    config_path: PathBuf,
}

impl ConfigManager {
    pub fn load() -> Result<Self> {
        let config_dir = directories::ProjectDirs::from("", "", "hyprwhspr-rs")
            .context("Failed to get config directory")?
            .config_dir()
            .to_path_buf();

        fs::create_dir_all(&config_dir).context("Failed to create config directory")?;

        let config_path = config_dir.join("config.json");

        let config = if config_path.exists() {
            let content = fs::read_to_string(&config_path).context("Failed to read config file")?;
            serde_json::from_str(&content).context("Failed to parse config file")?
        } else {
            let default_config = Config::default();
            // Save default config
            let json = serde_json::to_string_pretty(&default_config)
                .context("Failed to serialize default config")?;
            fs::write(&config_path, json).context("Failed to write default config")?;
            tracing::info!("Created default config at: {:?}", config_path);
            default_config
        };

        tracing::info!("Loaded config from: {:?}", config_path);

        Ok(Self {
            config,
            config_path,
        })
    }

    pub fn get(&self) -> &Config {
        &self.config
    }

    pub fn save(&self) -> Result<()> {
        let json =
            serde_json::to_string_pretty(&self.config).context("Failed to serialize config")?;
        fs::write(&self.config_path, json).context("Failed to write config file")?;
        tracing::info!("Saved config to: {:?}", self.config_path);
        Ok(())
    }

    pub fn get_model_path(&self) -> PathBuf {
        // Try system models first, then fall back to local
        let system_models = PathBuf::from("/usr/share/whisper/models");

        let models_dir = if system_models.exists() {
            system_models
        } else {
            // Fall back to shared Python version's models
            let home = std::env::var("HOME").expect("HOME not set");
            PathBuf::from(home).join(".local/share/hyprwhspr/whisper.cpp/models")
        };

        // Handle model naming conventions
        let model_name = &self.config.model;
        let model_filename = if model_name.ends_with(".en") {
            format!("ggml-{}.bin", model_name)
        } else {
            // Try .en.bin first, then .bin
            let en_path = models_dir.join(format!("ggml-{}.en.bin", model_name));
            if en_path.exists() {
                return en_path;
            }
            format!("ggml-{}.bin", model_name)
        };

        models_dir.join(model_filename)
    }

    pub fn get_whisper_binary_path(&self) -> PathBuf {
        // Priority order:
        // 1. System-wide installation (AUR package)
        // 2. User's local build

        let system_binary = PathBuf::from("/usr/bin/whisper-cli");
        if system_binary.exists() {
            return system_binary;
        }

        // Fall back to local build
        let home = std::env::var("HOME").expect("HOME not set");
        let local_dir = PathBuf::from(home).join(".local/share/hyprwhspr/whisper.cpp");

        let candidates = vec![
            local_dir.join("build/bin/whisper-cli"),
            local_dir.join("main"),
            local_dir.join("whisper"),
        ];

        for path in candidates {
            if path.exists() {
                return path;
            }
        }

        // Return system path as default
        system_binary
    }

    pub fn get_temp_dir(&self) -> PathBuf {
        let data_dir = directories::ProjectDirs::from("", "", "hyprwhspr-rs")
            .expect("Failed to get data directory")
            .data_dir()
            .to_path_buf();

        let temp_dir = data_dir.join("temp");
        fs::create_dir_all(&temp_dir).ok();
        temp_dir
    }

    pub fn get_assets_dir(&self) -> PathBuf {
        // Try installation directory first
        let install_path = PathBuf::from("/usr/lib/hyprwhspr-rs/share/assets");
        if install_path.exists() {
            return install_path;
        }

        // Fallback to relative path for development
        PathBuf::from("assets")
    }
}
