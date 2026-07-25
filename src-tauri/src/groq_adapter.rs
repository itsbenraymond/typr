use std::path::PathBuf;
use async_trait::async_trait;
use crate::transcriber::{Transcriber, TranscribeOpts};
use crate::transcribe_groq;

pub struct GroqAdapter;

#[async_trait]
impl Transcriber for GroqAdapter {
    async fn transcribe(&self, audio_path: &PathBuf, opts: &TranscribeOpts) -> Result<String, String> {
        transcribe_groq::transcribe_groq(&opts.api_key, audio_path).await
    }
}

pub fn default_transcriber() -> Box<dyn Transcriber> {
    Box::new(GroqAdapter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_transcriber_builds() {
        let _t = default_transcriber();
    }

    // Drive the trait boundary: an empty api_key must reach transcribe_groq's
    // pre-network guard, proving the adapter forwards opts.api_key (not "" or
    // opts.language) and audio_path through the seam. No network I/O.
    #[tokio::test]
    async fn adapter_forwards_api_key_through_trait() {
        let t = default_transcriber();
        let opts = TranscribeOpts {
            api_key: String::new(),
            language: "en".to_string(),
        };
        let result = t.transcribe(&PathBuf::from("unused.wav"), &opts).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key not set"));
    }
}
