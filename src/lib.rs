pub mod app;
pub mod app_test;
pub mod audio;
pub mod config;
pub mod input;
pub mod status;
pub mod whisper;

pub use app::HyprwhsprApp;
pub use config::{Config, ConfigManager};
pub use status::StatusWriter;
