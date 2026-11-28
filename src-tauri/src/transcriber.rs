use std::path::PathBuf;
use async_trait::async_trait;

pub struct TranscribeOpts {
    pub api_key: String,
    /// Optional second-provider key for when Groq is down; empty = skip.
    pub mistral_api_key: String,
    /// Cloudflare Workers AI account + token; either empty = skip.
    pub cloudflare_account_id: String,
    pub cloudflare_api_token: String,
    pub language: String,
}

#[async_trait]
pub trait Transcriber: Send + Sync {
    async fn transcribe(&self, audio_path: &PathBuf, opts: &TranscribeOpts) -> Result<String, String>;
}
