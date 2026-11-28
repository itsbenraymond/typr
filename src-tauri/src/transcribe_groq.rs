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
//! 3. Cloudflare Workers AI `whisper-large-v3-turbo`, only if
//!    `cloudflareAccountId` + `cloudflareApiToken` are set. A different
//!    provider on its own free quota, so it survives a Groq-wide outage
//!    (added 2026-11-28, measured 2.5s on a 20s clip). The older
//!    `@cf/openai/whisper` was tried and dropped: 39-78s per 3-minute chunk,
//!    timing out when chunks ran in parallel.
//! 4. Mistral `voxtral-mini-latest`, only if `mistralApiKey` is set (no
//!    budget on the key since 2026-11-16, so it is last).
//!
//! **Long recordings are split before any upload (2026-11-28).** Groq rejects
//! anything over 25 MB with a 413, and 16 kHz mono WAV is ~32 KB/s, so every
//! dictation over ~13 minutes failed on EVERY model in the chain at once (a
//! 17-minute brief was lost that way; failover cannot help when the file
//! itself is the problem). `split_wav` cuts at the quietest 100 ms inside the
//! last 20 s before each 3-minute mark, the chunks go through the full chain
//! in parallel, and the text is joined in order.
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
    Cloudflare,
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
    SttStep { provider: Provider::Cloudflare, model: "@cf/openai/whisper-large-v3-turbo" },
    SttStep { provider: Provider::Mistral, model: "voxtral-mini-latest" },
];

/// Longest piece sent in one request. 3 minutes of 16 kHz mono 16-bit WAV is
/// 5.8 MB (7.7 MB as base64 for Cloudflare), well under every provider's cap,
/// and smaller pieces in parallel finish sooner than a few big ones.
pub const CHUNK_SECS: usize = 180;
/// How far back from each chunk boundary to look for a pause to cut on.
const CUT_SEARCH_SECS: usize = 20;
/// Chunks transcribed at once. Groq free tier allows 20-30 requests a minute.
const PARALLEL_CHUNKS: usize = 4;

fn url(step: &SttStep, keys: &SttKeys) -> String {
    match step.provider {
        Provider::Groq => "https://api.groq.com/openai/v1/audio/transcriptions".to_string(),
        Provider::Mistral => "https://api.mistral.ai/v1/audio/transcriptions".to_string(),
        Provider::Cloudflare => format!(
            "https://api.cloudflare.com/client/v4/accounts/{}/ai/run/{}",
            keys.cloudflare_account.trim(),
            step.model
        ),
    }
}

#[derive(Debug, Clone, Default)]
pub struct SttKeys {
    pub groq: String,
    pub mistral: String,
    pub cloudflare_account: String,
    pub cloudflare_token: String,
}

impl SttKeys {
    fn for_provider(&self, p: Provider) -> &str {
        match p {
            Provider::Groq => self.groq.trim(),
            Provider::Mistral => self.mistral.trim(),
            Provider::Cloudflare if self.cloudflare_account.trim().is_empty() => "",
            Provider::Cloudflare => self.cloudflare_token.trim(),
        }
    }

    fn any(&self) -> bool {
        [Provider::Groq, Provider::Cloudflare, Provider::Mistral]
            .iter()
            .any(|p| !self.for_provider(*p).is_empty())
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
    let keys = SttKeys { groq: api_key.to_string(), ..Default::default() };
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
    if !keys.any() {
        return Err("Groq API key not set. Please enter your API key in settings.".to_string());
    }

    let audio_bytes = std::fs::read(audio_path)
        .map_err(|e| format!("Failed to read audio file: {}", e))?;

    let chunks = match split_wav(&audio_bytes) {
        Ok(c) => c,
        Err(e) => {
            crate::applog::log(&format!("Could not split recording ({}), sending it whole", e));
            vec![audio_bytes]
        }
    };
    if chunks.len() == 1 {
        return transcribe_bytes(keys, &chunks[0], chain).await;
    }

    let n = chunks.len();
    crate::applog::log(&format!(
        "Long recording: split into {} chunks of up to {} min",
        n,
        CHUNK_SECS / 60
    ));
    let mut texts = vec![String::new(); n];
    let mut chunks: Vec<Option<Vec<u8>>> = chunks.into_iter().map(Some).collect();
    let mut next = 0;
    while next < n {
        let end = (next + PARALLEL_CHUNKS).min(n);
        let mut set = tokio::task::JoinSet::new();
        for i in next..end {
            let keys = keys.clone();
            let chain = chain.to_vec();
            let bytes = chunks[i].take().unwrap_or_default();
            set.spawn(async move {
                // A chunk that fails the whole chain gets one more pass: a
                // single dropped connection on one of seven chunks must not
                // throw away the other six (seen live 2026-11-28).
                let mut res = transcribe_bytes(&keys, &bytes, &chain).await;
                if res.is_err() {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    res = transcribe_bytes(&keys, &bytes, &chain).await;
                }
                (i, res)
            });
        }
        next = end;
        while let Some(joined) = set.join_next().await {
            let (i, res) = joined.map_err(|e| format!("chunk task failed: {}", e))?;
            texts[i] = res.map_err(|e| format!("chunk {} of {}: {}", i + 1, n, e))?;
        }
    }
    Ok(texts.iter().map(|t| t.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" "))
}

/// Split a WAV into pieces of at most `CHUNK_SECS`, each cut on the quietest
/// 100 ms in the `CUT_SEARCH_SECS` before the limit so a word is not sliced.
/// A recording that already fits comes back as the original bytes, untouched.
pub fn split_wav(bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut reader =
        hound::WavReader::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let per_sec = (spec.sample_rate as usize) * (spec.channels as usize);
    let max = CHUNK_SECS * per_sec;
    if (reader.len() as usize) <= max {
        return Ok(vec![bytes.to_vec()]);
    }
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
        return Err(format!("unexpected WAV format {:?}", spec));
    }
    let samples: Vec<i16> = reader
        .samples::<i16>()
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    let mut start = 0;
    for cut in cut_points(&samples, per_sec, max, spec.channels as usize) {
        out.push(write_wav(&samples[start..cut], spec)?);
        start = cut;
    }
    out.push(write_wav(&samples[start..], spec)?);
    Ok(out)
}

fn cut_points(samples: &[i16], per_sec: usize, max: usize, channels: usize) -> Vec<usize> {
    let channels = channels.max(1);
    let frame = (per_sec / 10).max(channels);
    let search = CUT_SEARCH_SECS * per_sec;
    let mut cuts = Vec::new();
    let mut start = 0;
    while samples.len() - start > max {
        let hi = start + max;
        let lo = hi.saturating_sub(search).max(start + frame);
        let (mut best, mut best_energy) = (hi, u64::MAX);
        let mut i = lo;
        while i + frame <= hi {
            let energy: u64 = samples[i..i + frame].iter().map(|s| s.unsigned_abs() as u64).sum();
            if energy < best_energy {
                best_energy = energy;
                best = i + frame / 2;
            }
            i += frame;
        }
        best -= best % channels;
        cuts.push(best);
        start = best;
    }
    cuts
}

fn write_wav(samples: &[i16], spec: hound::WavSpec) -> Result<Vec<u8>, String> {
    let mut cur = std::io::Cursor::new(Vec::with_capacity(samples.len() * 2 + 44));
    {
        let mut w = hound::WavWriter::new(&mut cur, spec).map_err(|e| e.to_string())?;
        for &s in samples {
            w.write_sample(s).map_err(|e| e.to_string())?;
        }
        w.finalize().map_err(|e| e.to_string())?;
    }
    Ok(cur.into_inner())
}

async fn transcribe_bytes(
    keys: &SttKeys,
    audio_bytes: &[u8],
    chain: &[SttStep],
) -> Result<String, String> {
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
            match attempt(&client, keys, key, step, audio_bytes).await {
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
    keys: &SttKeys,
    key: &str,
    step: &SttStep,
    audio_bytes: &[u8],
) -> Result<String, Failure> {
    if step.provider == Provider::Cloudflare {
        return attempt_cloudflare(client, keys, key, step, audio_bytes).await;
    }
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
        .post(url(step, keys))
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

/// Workers AI: turbo takes base64 audio in JSON, the older `whisper` takes the
/// raw bytes. Both answer `{"result":{"text":...},"success":true}`.
async fn attempt_cloudflare(
    client: &reqwest::Client,
    keys: &SttKeys,
    key: &str,
    step: &SttStep,
    audio_bytes: &[u8],
) -> Result<String, Failure> {
    use base64::Engine;
    let req = client.post(url(step, keys)).bearer_auth(key);
    // The older `whisper` is slow (39s for a 3-minute chunk, 2026-11-28), so it
    // gets more time than the shared byte-based timeout allows.
    let req = if step.model.contains("turbo") {
        req
    } else {
        req.timeout(Duration::from_secs((20 + audio_bytes.len() as u64 / 100_000).min(120)))
    };
    let req = if step.model.contains("turbo") {
        let audio = base64::engine::general_purpose::STANDARD.encode(audio_bytes);
        req.json(&serde_json::json!({ "audio": audio, "language": "en" }))
    } else {
        req.header("Content-Type", "application/octet-stream").body(audio_bytes.to_vec())
    };
    let response = req.send().await.map_err(|e| Failure::from_reqwest(&e))?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(Failure::from_status(status, &body));
    }
    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| Failure::fatal(format!("Failed to parse response: {}", e)))?;
    json["result"]["text"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| Failure::fatal("No 'result.text' field in response"))
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

    fn wav_of(secs: usize, quiet_at: Option<usize>) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let samples: Vec<i16> = (0..secs * 16000)
            .map(|i| match quiet_at {
                Some(q) if i >= q * 16000 && i < q * 16000 + 3200 => 0,
                _ => if i % 2 == 0 { 9000 } else { -9000 },
            })
            .collect();
        write_wav(&samples, spec).unwrap()
    }

    #[test]
    fn a_short_recording_is_sent_untouched() {
        let wav = wav_of(30, None);
        let chunks = split_wav(&wav).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], wav);
    }

    #[test]
    fn a_17_minute_recording_splits_under_the_upload_cap_and_keeps_every_sample() {
        let wav = wav_of(17 * 60, None);
        let chunks = split_wav(&wav).unwrap();
        assert!((6..=7).contains(&chunks.len()), "{} chunks", chunks.len());
        let mut total = 0;
        for c in &chunks {
            assert!(c.len() < 20_000_000, "chunk {} bytes", c.len());
            total += hound::WavReader::new(std::io::Cursor::new(c)).unwrap().len();
        }
        assert_eq!(total as usize, 17 * 60 * 16000);
    }

    #[test]
    fn the_cut_lands_on_the_pause() {
        let wav = wav_of(CHUNK_SECS + 60, Some(CHUNK_SECS - 7));
        let chunks = split_wav(&wav).unwrap();
        assert_eq!(chunks.len(), 2);
        let first = hound::WavReader::new(std::io::Cursor::new(&chunks[0])).unwrap().len() as usize;
        let pause = (CHUNK_SECS - 7) * 16000;
        assert!(first >= pause && first <= pause + 3200, "cut at {} not in pause {}", first, pause);
    }

    #[test]
    fn cloudflare_needs_both_account_and_token() {
        let only_token = SttKeys { cloudflare_token: "t".into(), ..Default::default() };
        assert!(!only_token.any());
        let both = SttKeys {
            cloudflare_token: "t".into(),
            cloudflare_account: "a".into(),
            ..Default::default()
        };
        assert!(both.any());
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
        let keys = SttKeys { groq: key(), ..Default::default() };
        let text = transcribe_with_chain(&keys, &wav(), &chain).await.expect("fallback failed");
        println!("fallback transcript: {}", text);
        assert!(text.split_whitespace().count() >= 3, "too little text: {:?}", text);
    }

    #[tokio::test]
    #[ignore = "hits the real Groq API; needs TYPR_LIVE_GROQ_KEY + TYPR_LIVE_WAV"]
    async fn every_groq_whisper_model_in_the_chain_still_exists() {
        let keys = SttKeys { groq: key(), ..Default::default() };
        for step in STT_CHAIN.iter().filter(|s| s.provider == Provider::Groq) {
            let text = transcribe_with_chain(&keys, &wav(), std::slice::from_ref(step))
                .await
                .unwrap_or_else(|e| panic!("{} is broken: {}", step.model, e));
            println!("{:<24} ok: {}", step.model, &text[..text.len().min(60)]);
        }
    }

    /// TYPR_LIVE_CF_ACCOUNT + TYPR_LIVE_CF_TOKEN: both Workers AI models answer.
    #[tokio::test]
    #[ignore = "hits Cloudflare Workers AI; needs TYPR_LIVE_CF_ACCOUNT, TYPR_LIVE_CF_TOKEN, TYPR_LIVE_WAV"]
    async fn every_cloudflare_whisper_model_answers() {
        let keys = SttKeys {
            cloudflare_account: std::env::var("TYPR_LIVE_CF_ACCOUNT").unwrap(),
            cloudflare_token: std::env::var("TYPR_LIVE_CF_TOKEN").unwrap(),
            ..Default::default()
        };
        for step in STT_CHAIN.iter().filter(|s| s.provider == Provider::Cloudflare) {
            let text = transcribe_with_chain(&keys, &wav(), std::slice::from_ref(step))
                .await
                .unwrap_or_else(|e| panic!("{} is broken: {}", step.model, e));
            println!("{:<36} ok: {}", step.model, &text[..text.len().min(60)]);
        }
    }
}
