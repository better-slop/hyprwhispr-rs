use async_trait::async_trait;

#[async_trait]
pub trait Transcriber: Send + Sync {
    async fn transcribe(&self, audio: Vec<f32>, sample_rate_hz: u32) -> anyhow::Result<String>;
}
