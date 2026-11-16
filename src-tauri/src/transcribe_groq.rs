//! Speech-to-text, with failover.
//!
//! The Anthropic API cannot transcribe audio, so Claude is not in this chain.
//! What is:
//!
//! 1. Groq `whisper-large-v3-turbo`, the primary.
//! 2. Groq `whisper-large-v3`. Same key, but Groq's audio-seconds-per-hour
//!    limit is **per model**: on 2026-11-16 turbo sat at its 7,200 s/hr cap
//!    returning "try again in 2m" while large-v3 answered in 0.6s. Slightly
//!    slower and keeps more fillers, which the cleanup pass strips anyway.
//! 3. Mistral `voxtral-mini-latest`, only if `mistralApiKey` is set. A
//!    different provider, so it survives a Groq-wide outage.
//!
//! Move-on rules are in `failover`: 429 goes to the next model at once, a
//! 5xx or dropped connection gets one quick retry, a timeout skips the rest
//! of that provider.

use std::path::PathBuf;
use std::time::Duration;

use reqwest::multipart;

use crate::failover::{FailKind, Failure, RETRY_DELAY};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Groq,
    Mistral,
}

#[derive(Debug, Clone, Copy)]
pub struct SttStep {
    pub provider: Provider,
    pub model: &'static str,
}

pub const STT_CHAIN: &[SttStep] = &[
    SttStep { provider: Provider::Groq, model: "whisper-large-v3-turbo" },
    SttStep { provider: Provider::Groq, model: "whisper-large-v3" },
    SttStep { provider: Provider::Mistral, model: "voxtral-mini-latest" },
];

impl Provider {
    fn url(self) -> &'static str {
        match self {
            Provider::Groq => "https://api.groq.com/openai/v1/audio/transcriptions",
            Provider::Mistral => "https://api.mistral.ai/v1/audio/transcriptions",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SttKeys {
    pub groq: String,
    pub mistral: String,
}

impl SttKeys {
    fn for_provider(&self, p: Provider) -> &str {
        match p {
            Provider::Groq => self.groq.trim(),
            Provider::Mistral => self.mistral.trim(),
        }
    }
}

/// Upload time grows with the recording (16 kHz mono WAV is ~32 KB/s, so a
/// five-minute ramble is ~10 MB). A flat short timeout would fail every long
/// dictation; a flat long one would make a hung Groq cost half a minute.
fn timeout_for(bytes: usize) -> Duration {
    let secs = 8 + (bytes as u64 / 250_000);
    Duration::from_secs(secs.min(60))
}

pub async fn transcribe_groq(api_key: &str, audio_path: &PathBuf) -> Result<String, String> {
    let keys = SttKeys { groq: api_key.to_string(), mistral: String::new() };
    transcribe(&keys, audio_path).await
}

pub async fn transcribe(keys: &SttKeys, audio_path: &PathBuf) -> Result<String, String> {
    transcribe_with_chain(keys, audio_path, STT_CHAIN).await
}

pub async fn transcribe_with_chain(
    keys: &SttKeys,
    audio_path: &PathBuf,
    chain: &[SttStep],
) -> Result<String, String> {
    if keys.groq.trim().is_empty() && keys.mistral.trim().is_empty() {
        return Err("Groq API key not set. Please enter your API key in settings.".to_string());
    }

    let audio_bytes = std::fs::read(audio_path)
        .map_err(|e| format!("Failed to read audio file: {}", e))?;

    let client = reqwest::Client::builder()
        .timeout(timeout_for(audio_bytes.len()))
        .connect_timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| e.to_string())?;

    let mut errors: Vec<String> = Vec::new();
    let mut skip_provider: Option<Provider> = None;

    'chain: for step in chain {
        let key = keys.for_provider(step.provider);
        if key.is_empty() || skip_provider == Some(step.provider) {
            continue;
        }
        for try_no in 0..2 {
            match attempt(&client, key, step, &audio_bytes).await {
                Ok(text) => {
                    if !errors.is_empty() {
                        crate::applog::log(&format!(
                            "Transcribed with fallback {:?} {} after: {}",
                            step.provider,
                            step.model,
                            errors.join(" | ")
                        ));
                    }
                    return Ok(text);
                }
                Err(e) => {
                    crate::applog::log(&format!(
                        "Transcribe attempt failed on {:?} {}: {}",
                        step.provider, step.model, e
                    ));
                    errors.push(format!("{}: {}", step.model, e));
                    match e.kind {
                        FailKind::Transient if try_no == 0 => {
                            tokio::time::sleep(RETRY_DELAY).await;
                            continue;
                        }
                        FailKind::Timeout => {
                            skip_provider = Some(step.provider);
                            continue 'chain;
                        }
                        _ => continue 'chain,
                    }
                }
            }
        }
    }

    // Keeps the "Groq API error (429 ..." shape the pill and log already use.
    Err(format!("Groq API error (all transcription models failed): {}", errors.join(" | ")))
}

async fn attempt(
    client: &reqwest::Client,
    key: &str,
    step: &SttStep,
    audio_bytes: &[u8],
) -> Result<String, Failure> {
    let file_part = multipart::Part::bytes(audio_bytes.to_vec())
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| Failure::fatal(e.to_string()))?;

    let mut form = multipart::Form::new()
        .text("model", step.model)
        .text("language", "en")
        .part("file", file_part);
    if step.provider == Provider::Groq {
        form = form.text("response_format", "json");
    }

    let response = client
        .post(step.provider.url())
        .bearer_auth(key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| Failure::from_reqwest(&e))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(Failure::from_status(status, &body));
    }

    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| Failure::fatal(format!("Failed to parse response: {}", e)))?;

    json["text"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| Failure::fatal("No 'text' field in response"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_empty_api_key() {
        let path = PathBuf::from("/tmp/test.wav");
        let result = transcribe_groq("", &path).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key not set"));
    }

    #[test]
    fn chain_starts_on_turbo_and_has_a_second_groq_bucket() {
        assert_eq!(STT_CHAIN[0].model, "whisper-large-v3-turbo");
        assert!(STT_CHAIN
            .iter()
            .any(|s| s.provider == Provider::Groq && s.model == "whisper-large-v3"));
    }

    #[test]
    fn timeout_scales_with_the_recording_and_is_capped() {
        assert_eq!(timeout_for(0), Duration::from_secs(8));
        assert!(timeout_for(10_000_000) > timeout_for(800_000));
        assert_eq!(timeout_for(1_000_000_000), Duration::from_secs(60));
    }
}

/// Live checks against the real Groq API. Ignored by default.
///
/// ```text
/// TYPR_LIVE_GROQ_KEY=gsk_... TYPR_LIVE_WAV=path\to\speech.wav \
///   cargo test --release -- --ignored --test-threads=1
/// ```
#[cfg(test)]
mod live {
    use super::*;

    fn key() -> String {
        std::env::var("TYPR_LIVE_GROQ_KEY").expect("set TYPR_LIVE_GROQ_KEY")
    }
    fn wav() -> PathBuf {
        PathBuf::from(std::env::var("TYPR_LIVE_WAV").expect("set TYPR_LIVE_WAV to a speech wav"))
    }

    /// Forces the primary to fail with a model id that does not exist and
    /// proves the second Groq bucket returns text.
    #[tokio::test]
    #[ignore = "hits the real Groq API; needs TYPR_LIVE_GROQ_KEY + TYPR_LIVE_WAV"]
    async fn falls_over_to_the_next_whisper_model() {
        let chain = [
            SttStep { provider: Provider::Groq, model: "whisper-does-not-exist" },
            SttStep { provider: Provider::Groq, model: "whisper-large-v3" },
        ];
        let keys = SttKeys { groq: key(), mistral: String::new() };
        let text = transcribe_with_chain(&keys, &wav(), &chain).await.expect("fallback failed");
        println!("fallback transcript: {}", text);
        assert!(text.split_whitespace().count() >= 3, "too little text: {:?}", text);
    }

    #[tokio::test]
    #[ignore = "hits the real Groq API; needs TYPR_LIVE_GROQ_KEY + TYPR_LIVE_WAV"]
    async fn every_groq_whisper_model_in_the_chain_still_exists() {
        let keys = SttKeys { groq: key(), mistral: String::new() };
        for step in STT_CHAIN.iter().filter(|s| s.provider == Provider::Groq) {
            let text = transcribe_with_chain(&keys, &wav(), std::slice::from_ref(step))
                .await
                .unwrap_or_else(|e| panic!("{} is broken: {}", step.model, e));
            println!("{:<24} ok: {}", step.model, &text[..text.len().min(60)]);
        }
    }
}
