//! Claude as the last step of the cleanup chain, after both Groq models.
//!
//! Text only. The Anthropic API does not transcribe audio, so this can never
//! stand in for Whisper; see `transcribe_groq` for the speech-to-text chain.

use std::time::Duration;

use reqwest::Client;
use serde_json::json;

use crate::failover::Failure;

/// Fastest current Claude model. Cleanup is a light rewrite, it does not need
/// a bigger one, and Ben is waiting on the paste.
pub const CLAUDE_MODEL: &str = "claude-haiku-5-5";

/// Measured 1.4s for a two-word round trip on 2026-11-16. Generous enough for
/// a long ramble, short enough that a hung call does not eat the dictation.
const TIMEOUT_SECS: u64 = 12;

#[derive(Debug, Clone, Default)]
pub struct ClaudeCreds {
    pub api_key: String,
    /// Needed when the key is an org-level key that is not scoped to a
    /// workspace (Ben's is: without it the API answers 400 "must include the
    /// anthropic-workspace-id header"). Empty for an ordinary workspace key.
    pub workspace_id: String,
}

impl ClaudeCreds {
    pub fn is_configured(&self) -> bool {
        !self.api_key.trim().is_empty()
    }
}

/// One cleanup call. Returns the model's text, unsanitised and unvalidated;
/// the caller runs the same checks it runs on Groq's output.
pub async fn complete(
    creds: &ClaudeCreds,
    system_prompt: &str,
    user_msg: &str,
    max_tokens: u64,
) -> Result<String, Failure> {
    let client = Client::builder()
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(4))
        .build()
        .map_err(|e| Failure::fatal(e.to_string()))?;

    let body = json!({
        "model": CLAUDE_MODEL,
        "max_tokens": max_tokens,
        // No `temperature`: claude-haiku-5-5 answers 400 "`temperature` is
        // deprecated for this model" (caught by the live test, 2026-11-16).
        "system": system_prompt,
        "messages": [{ "role": "user", "content": user_msg }]
    });

    let mut req = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", creds.api_key.trim())
        .header("anthropic-version", "2023-06-01")
        .json(&body);
    if !creds.workspace_id.trim().is_empty() {
        req = req.header("anthropic-workspace-id", creds.workspace_id.trim());
    }

    let resp = req.send().await.map_err(|e| Failure::from_reqwest(&e))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        return Err(Failure::from_status(status, &text));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| Failure::fatal(format!("JSON parse failed: {}", e)))?;

    if json["stop_reason"].as_str() == Some("max_tokens") {
        return Err(Failure::fatal("response truncated (stop_reason=max_tokens)"));
    }

    let text: String = json["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    if text.trim().is_empty() {
        return Err(Failure::fatal("empty response"));
    }
    Ok(text)
}
