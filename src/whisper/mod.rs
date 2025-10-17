pub mod groq;
pub mod local;
pub mod transcriber;
pub mod wav;

pub use groq::GroqClient;
pub use local::{LocalWhisper, WhisperVadOptions};
pub use transcriber::Transcriber;
