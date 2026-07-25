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
}
