use anyhow::{anyhow, Context, Result};
use jsonc_parser::{parse_to_serde_value, ParseOptions};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};
use tokio::sync::watch;
use tokio::time;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

#[derive(Clone)]
pub struct ConfigManager {
    inner: Arc<ConfigManagerInner>,
}

struct ConfigManagerInner {
    config: RwLock<Config>,
    config_path: PathBuf,
    change_tx: watch::Sender<Config>,
    watcher_active: AtomicBool,
}

impl ConfigManager {
    pub fn load() -> Result<Self> {
        let config_dir = directories::ProjectDirs::from("", "", "hyprwhspr-rs")
            .context("Failed to get config directory")?
            .config_dir()
            .to_path_buf();

        fs::create_dir_all(&config_dir).context("Failed to create config directory")?;

        let jsonc_path = config_dir.join("config.jsonc");
        let legacy_path = config_dir.join("config.json");

        let (config_path, config) = if jsonc_path.exists() {
            let config = Self::read_config_from_disk(&jsonc_path)?;
            (jsonc_path, config)
        } else if legacy_path.exists() {
            let config = Self::read_config_from_disk(&legacy_path)?;
            Self::write_config_file(&jsonc_path, &config)?;
            tracing::info!(
                "Migrated legacy config to JSONC: {:?} -> {:?}",
                legacy_path,
                jsonc_path
            );
            (jsonc_path, config)
        } else {
            let default_config = Config::default();
            Self::write_config_file(&jsonc_path, &default_config)?;
            tracing::info!("Created default config at: {:?}", jsonc_path);
            (jsonc_path, default_config)
        };

        tracing::info!("Loaded config from: {:?}", config_path);

        let (change_tx, _) = watch::channel(config.clone());

        Ok(Self {
            inner: Arc::new(ConfigManagerInner {
                config: RwLock::new(config),
                config_path,
                change_tx,
                watcher_active: AtomicBool::new(false),
            }),
        })
    }

    pub fn start_watching(&self) {
        if self.inner.watcher_active.swap(true, Ordering::SeqCst) {
            return;
        }

        let inner = Arc::clone(&self.inner);

        tokio::spawn(async move {
            let mut last_state = Self::file_state(&inner.config_path);
            let mut ticker = time::interval(Duration::from_millis(500));

            loop {
                ticker.tick().await;

                let current_state = Self::file_state(&inner.config_path);
                if current_state == last_state {
                    continue;
                }

                last_state = current_state;

                match Self::read_config_from_disk(&inner.config_path) {
                    Ok(new_config) => {
                        let mut guard = inner.config.write().expect("config lock poisoned");
                        if *guard != new_config {
                            let old_config = guard.clone();
                            *guard = new_config.clone();
                            drop(guard);

                            if inner.change_tx.send(new_config.clone()).is_ok() {
                                tracing::info!("Reloaded config from: {:?}", inner.config_path);
                                tracing::debug!(
                                    ?old_config,
                                    ?new_config,
                                    "Config watcher applied update"
                                );
                            }
                        }
                    }
                    Err(err) => {
                        tracing::warn!("Failed to reload config: {err}");
                    }
                }
            }
        });
    }

    pub fn subscribe(&self) -> watch::Receiver<Config> {
        self.inner.change_tx.subscribe()
    }

    pub fn get(&self) -> Config {
        self.inner
            .config
            .read()
            .expect("config lock poisoned")
            .clone()
    }

    pub fn save(&self) -> Result<()> {
        let config = self.get();
        Self::write_config_file(&self.inner.config_path, &config)?;

        {
            let mut guard = self.inner.config.write().expect("config lock poisoned");
            *guard = config.clone();
        }

        let _ = self.inner.change_tx.send(config);

        tracing::info!("Saved config to: {:?}", self.inner.config_path);
        Ok(())
    }

    pub fn get_model_path(&self) -> PathBuf {
        let config = self.get();
        Self::resolve_model_path(&config)
    }

    pub fn get_whisper_binary_path(&self) -> PathBuf {
        let system_binary = PathBuf::from("/usr/bin/whisper-cli");
        if system_binary.exists() {
            return system_binary;
        }

        let home = env::var("HOME").expect("HOME not set");
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
        let install_path = PathBuf::from("/usr/lib/hyprwhspr-rs/share/assets");
        if install_path.exists() {
            return install_path;
        }

        PathBuf::from("assets")
    }

    fn read_config_from_disk(path: &Path) -> Result<Config> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file at {:?}", path))?;
        Self::parse_config(&content)
    }

    fn write_config_file(path: &Path, config: &Config) -> Result<()> {
        let json = serde_json::to_string_pretty(config).context("Failed to serialize config")?;
        fs::write(path, json).with_context(|| format!("Failed to write config file at {:?}", path))
    }

    fn parse_config(content: &str) -> Result<Config> {
        let value = parse_to_serde_value(content, &ParseOptions::default())
            .context("Failed to parse config as JSONC")?
            .ok_or_else(|| anyhow!("Config file did not contain a JSON value"))?;
        serde_json::from_value(value).context("Failed to deserialize config")
    }

    fn file_state(path: &Path) -> Option<(SystemTime, u64)> {
        let metadata = fs::metadata(path).ok()?;
        let modified = metadata.modified().ok()?;
        Some((modified, metadata.len()))
    }

    fn resolve_model_path(config: &Config) -> PathBuf {
        let system_models = PathBuf::from("/usr/share/whisper/models");

        let models_dir = if system_models.exists() {
            system_models
        } else {
            let home = env::var("HOME").expect("HOME not set");
            PathBuf::from(home).join(".local/share/hyprwhspr/whisper.cpp/models")
        };

        let model_name = &config.model;
        if model_name.ends_with(".en") {
            return models_dir.join(format!("ggml-{}.bin", model_name));
        }

        let en_path = models_dir.join(format!("ggml-{}.en.bin", model_name));
        if en_path.exists() {
            return en_path;
        }

        models_dir.join(format!("ggml-{}.bin", model_name))
    }
}
