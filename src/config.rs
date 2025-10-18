use crate::transcription::DEFAULT_PROMPT;
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
#[serde(default)]
pub struct ShortcutsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub press: Option<String>,
}

impl Default for ShortcutsConfig {
    fn default() -> Self {
        Self {
            hold: None,
            press: Some(default_primary_shortcut()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default = "default_primary_shortcut", skip_serializing)]
    pub primary_shortcut: String,

    #[serde(default)]
    pub shortcuts: ShortcutsConfig,

    #[serde(default)]
    pub word_overrides: HashMap<String, String>,

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

    #[serde(default)]
    pub vad: VadConfig,

    #[serde(default)]
    pub transcription: TranscriptionConfig,

    #[serde(default, rename = "model", skip_serializing)]
    legacy_model: Option<String>,

    #[serde(default, rename = "threads", skip_serializing)]
    legacy_threads: Option<usize>,

    #[serde(default, rename = "gpu_layers", skip_serializing)]
    legacy_gpu_layers: Option<i32>,

    #[serde(default, rename = "whisper_prompt", skip_serializing)]
    legacy_whisper_prompt: Option<String>,

    #[serde(default, rename = "models_dirs", skip_serializing)]
    legacy_models_dirs: Option<Vec<String>>,

    #[serde(default, rename = "no_speech_threshold", skip_serializing)]
    legacy_no_speech_threshold: Option<f32>,

    #[serde(default, rename = "fallback_cli", skip_serializing)]
    legacy_fallback_cli: Option<bool>,
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
    DEFAULT_PROMPT.to_string()
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

fn default_no_speech_threshold() -> f32 {
    0.60
}

fn default_vad_model() -> String {
    "ggml-silero-v5.1.2.bin".to_string()
}

fn default_vad_threshold() -> f32 {
    0.50
}

fn default_vad_min_speech_ms() -> u32 {
    250
}

fn default_vad_min_silence_ms() -> u32 {
    100
}

fn default_vad_max_speech_s() -> f32 {
    f32::INFINITY
}

fn default_vad_speech_pad_ms() -> u32 {
    30
}

fn default_vad_samples_overlap() -> f32 {
    0.10
}

fn default_transcription_request_timeout_secs() -> u64 {
    45
}

fn default_transcription_max_retries() -> u32 {
    2
}

fn default_groq_model() -> String {
    "whisper-large-v3-turbo".to_string()
}

fn default_groq_endpoint() -> String {
    "https://api.groq.com/openai/v1/audio/transcriptions".to_string()
}

fn default_gemini_model() -> String {
    "gemini-2.5-pro-exp-0827".to_string()
}

fn default_gemini_endpoint() -> String {
    "https://generativelanguage.googleapis.com/v1beta/models".to_string()
}

fn default_gemini_temperature() -> f32 {
    0.0
}

fn default_gemini_max_output_tokens() -> u32 {
    1024
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VadConfig {
    pub enabled: bool,
    pub model: String,
    pub threshold: f32,
    pub min_speech_ms: u32,
    pub min_silence_ms: u32,
    pub max_speech_s: f32,
    pub speech_pad_ms: u32,
    pub samples_overlap: f32,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: default_vad_model(),
            threshold: default_vad_threshold(),
            min_speech_ms: default_vad_min_speech_ms(),
            min_silence_ms: default_vad_min_silence_ms(),
            max_speech_s: default_vad_max_speech_s(),
            speech_pad_ms: default_vad_speech_pad_ms(),
            samples_overlap: default_vad_samples_overlap(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionProvider {
    WhisperCpp,
    Groq,
    Gemini,
}

impl Default for TranscriptionProvider {
    fn default() -> Self {
        TranscriptionProvider::WhisperCpp
    }
}

impl TranscriptionProvider {
    pub fn label(&self) -> &'static str {
        match self {
            TranscriptionProvider::WhisperCpp => "whisper.cpp (local)",
            TranscriptionProvider::Groq => "Groq Whisper API",
            TranscriptionProvider::Gemini => "Gemini 2.5 Pro Flash",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WhisperCppConfig {
    pub prompt: String,
    pub model: String,
    pub threads: usize,
    pub gpu_layers: i32,
    pub fallback_cli: bool,
    pub no_speech_threshold: f32,
    pub models_dirs: Vec<String>,
}

impl Default for WhisperCppConfig {
    fn default() -> Self {
        Self {
            prompt: default_whisper_prompt(),
            model: default_model(),
            threads: default_threads(),
            gpu_layers: default_gpu_layers(),
            fallback_cli: false,
            no_speech_threshold: default_no_speech_threshold(),
            models_dirs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GroqConfig {
    pub model: String,
    pub endpoint: String,
    pub prompt: String,
}

impl Default for GroqConfig {
    fn default() -> Self {
        Self {
            model: default_groq_model(),
            endpoint: default_groq_endpoint(),
            prompt: default_whisper_prompt(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GeminiConfig {
    pub model: String,
    pub endpoint: String,
    pub temperature: f32,
    pub max_output_tokens: u32,
    pub prompt: String,
}

impl Default for GeminiConfig {
    fn default() -> Self {
        Self {
            model: default_gemini_model(),
            endpoint: default_gemini_endpoint(),
            temperature: default_gemini_temperature(),
            max_output_tokens: default_gemini_max_output_tokens(),
            prompt: default_whisper_prompt(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TranscriptionConfig {
    pub provider: TranscriptionProvider,
    pub request_timeout_secs: u64,
    pub max_retries: u32,
    pub whisper_cpp: WhisperCppConfig,
    pub groq: GroqConfig,
    pub gemini: GeminiConfig,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            provider: TranscriptionProvider::default(),
            request_timeout_secs: default_transcription_request_timeout_secs(),
            max_retries: default_transcription_max_retries(),
            whisper_cpp: WhisperCppConfig::default(),
            groq: GroqConfig::default(),
            gemini: GeminiConfig::default(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut config = Self {
            primary_shortcut: default_primary_shortcut(),
            shortcuts: ShortcutsConfig::default(),
            word_overrides: HashMap::new(),
            audio_feedback: false,
            start_sound_volume: default_volume(),
            stop_sound_volume: default_volume(),
            start_sound_path: None,
            stop_sound_path: None,
            auto_copy_clipboard: default_auto_copy_clipboard(),
            shift_paste: default_shift_paste(),
            audio_device: None,
            vad: VadConfig::default(),
            transcription: TranscriptionConfig::default(),
            legacy_model: None,
            legacy_threads: None,
            legacy_gpu_layers: None,
            legacy_whisper_prompt: None,
            legacy_models_dirs: None,
            legacy_no_speech_threshold: None,
            legacy_fallback_cli: None,
        };
        config.normalize_shortcuts();
        config
    }
}

impl Config {
    pub fn normalize_shortcuts(&mut self) {
        let legacy_primary = Self::sanitize_shortcut(&self.primary_shortcut);

        self.shortcuts.press = self
            .shortcuts
            .press
            .as_ref()
            .and_then(|value| Self::sanitize_shortcut(value));
        self.shortcuts.hold = self
            .shortcuts
            .hold
            .as_ref()
            .and_then(|value| Self::sanitize_shortcut(value));

        if let (Some(current), Some(legacy)) = (&self.shortcuts.press, &legacy_primary) {
            if current != legacy {
                self.shortcuts.press = Some(legacy.clone());
            }
        } else if self.shortcuts.press.is_none() {
            if let Some(legacy) = &legacy_primary {
                self.shortcuts.press = Some(legacy.clone());
            }
        }

        if let Some(press) = self.shortcuts.press.clone() {
            self.primary_shortcut = press;
        } else {
            let fallback = legacy_primary.unwrap_or_else(default_primary_shortcut);
            self.primary_shortcut = fallback.clone();
            self.shortcuts.press = Some(fallback);
        }
    }

    pub fn migrate_legacy_transcription_settings(&mut self) {
        if let Some(model) = self.legacy_model.take() {
            self.transcription.whisper_cpp.model = model;
        }

        if let Some(threads) = self.legacy_threads.take() {
            self.transcription.whisper_cpp.threads = threads;
        }

        if let Some(gpu_layers) = self.legacy_gpu_layers.take() {
            self.transcription.whisper_cpp.gpu_layers = gpu_layers;
        }

        if let Some(prompt) = self.legacy_whisper_prompt.take() {
            self.transcription.whisper_cpp.prompt = prompt.clone();
            self.transcription.groq.prompt = prompt.clone();
            self.transcription.gemini.prompt = prompt;
        }

        if let Some(dirs) = self.legacy_models_dirs.take() {
            self.transcription.whisper_cpp.models_dirs = dirs;
        }

        if let Some(threshold) = self.legacy_no_speech_threshold.take() {
            self.transcription.whisper_cpp.no_speech_threshold = threshold;
        }

        if let Some(fallback_cli) = self.legacy_fallback_cli.take() {
            self.transcription.whisper_cpp.fallback_cli = fallback_cli;
        }
    }

    pub fn press_shortcut(&self) -> Option<&str> {
        self.shortcuts.press.as_deref()
    }

    pub fn hold_shortcut(&self) -> Option<&str> {
        self.shortcuts.hold.as_deref()
    }

    fn sanitize_shortcut(value: &str) -> Option<String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
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

    pub fn get_vad_model_path(&self, config: &Config) -> Option<PathBuf> {
        Self::resolve_vad_model_path(config, Some(&self.inner.config_path))
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
        let mut config = config.clone();
        config.normalize_shortcuts();
        let json = serde_json::to_string_pretty(&config).context("Failed to serialize config")?;
        fs::write(path, json).with_context(|| format!("Failed to write config file at {:?}", path))
    }

    fn parse_config(content: &str) -> Result<Config> {
        let value = parse_to_serde_value(content, &ParseOptions::default())
            .context("Failed to parse config as JSONC")?
            .ok_or_else(|| anyhow!("Config file did not contain a JSON value"))?;
        let mut config: Config =
            serde_json::from_value(value).context("Failed to deserialize config")?;
        config.migrate_legacy_transcription_settings();
        config.normalize_shortcuts();
        Ok(config)
    }

    fn file_state(path: &Path) -> Option<(SystemTime, u64)> {
        let metadata = fs::metadata(path).ok()?;
        let modified = metadata.modified().ok()?;
        Some((modified, metadata.len()))
    }

    fn resolve_model_path(config: &Config) -> PathBuf {
        let models_dir = Self::model_search_dirs(config)
            .into_iter()
            .next()
            .unwrap_or_else(|| PathBuf::from("."));

        let model_name = &config.transcription.whisper_cpp.model;
        if model_name.ends_with(".en") {
            return models_dir.join(format!("ggml-{}.bin", model_name));
        }

        let en_path = models_dir.join(format!("ggml-{}.en.bin", model_name));
        if en_path.exists() {
            return en_path;
        }

        models_dir.join(format!("ggml-{}.bin", model_name))
    }

    fn resolve_vad_model_path(config: &Config, config_path: Option<&Path>) -> Option<PathBuf> {
        if !config.vad.enabled {
            return None;
        }

        let model_ref = config.vad.model.trim();
        if model_ref.is_empty() {
            return None;
        }

        let candidate = PathBuf::from(model_ref);
        if candidate.is_absolute() && candidate.exists() {
            return Some(candidate);
        }
        if candidate.exists() {
            return Some(candidate);
        }

        if let Some(base) = config_path.and_then(|p| p.parent()) {
            let candidate = base.join(model_ref);
            if candidate.exists() {
                return Some(candidate);
            }
        }

        if let Some(project_dirs) = directories::ProjectDirs::from("", "", "hyprwhspr-rs") {
            let cfg_candidate = project_dirs.config_dir().join(model_ref);
            if cfg_candidate.exists() {
                return Some(cfg_candidate);
            }
        }

        for dir in Self::model_search_dirs(config) {
            let candidate = dir.join(model_ref);
            if candidate.exists() {
                return Some(candidate);
            }
        }

        None
    }

    fn model_search_dirs(config: &Config) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        // Add custom models directories from config (with path expansion)
        for dir_str in &config.transcription.whisper_cpp.models_dirs {
            let expanded = if dir_str.starts_with("~/") {
                if let Ok(home) = env::var("HOME") {
                    PathBuf::from(home).join(&dir_str[2..])
                } else {
                    PathBuf::from(dir_str)
                }
            } else {
                PathBuf::from(dir_str)
            };

            if expanded.exists() {
                dirs.push(expanded);
            }
        }

        // Add system default paths as fallback
        let system_models = PathBuf::from("/usr/share/whisper/models");
        if system_models.exists() {
            dirs.push(system_models);
        }
        if let Ok(home) = env::var("HOME") {
            let legacy_path = PathBuf::from(home).join(".local/share/hyprwhspr/whisper.cpp/models");
            if legacy_path.exists() {
                dirs.push(legacy_path);
            }
        }

        dirs
    }
}
