use std::path::PathBuf;
use async_trait::async_trait;

pub struct TranscribeOpts {
    pub api_key: String,
    pub language: String,
}

#[async_trait]
pub trait Transcriber: Send + Sync {
    async fn transcribe(&self, audio_path: &PathBuf, opts: &TranscribeOpts) -> Result<String, String>;
}
