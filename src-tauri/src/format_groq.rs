use std::time::Duration;
use reqwest::Client;
use serde_json::json;

const FORMAT_PROMPT: &str = "\
You are a plain-text formatter for speech transcriptions. Rules:\n\
1. Fix punctuation and capitalize the start of every sentence.\n\
2. If the speaker lists items, put each item on its own line with a dash prefix.\n\
3. Add a blank line between distinct topics or paragraphs.\n\
4. Output ONLY the corrected text. No intro, no explanation, no markdown (no **, no *, no #, no backticks).";

pub async fn format_text(api_key: &str, raw_text: &str) -> Result<String, String> {
    if api_key.is_empty() {
        return Err("Groq API key not configured — add it in Settings > Engine".to_string());
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let body = json!({
        "model": "llama-3.1-8b-instant",
        "messages": [
            { "role": "system", "content": FORMAT_PROMPT },
            { "role": "user",   "content": raw_text }
        ],
        "max_tokens": 1024,
        "temperature": 0.0
    });

    let resp = client
        .post("https://api.groq.com/openai/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Groq request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Groq error {}: {}", status, text));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Groq JSON parse failed: {}", e))?;

    let raw = json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim();

    if raw.is_empty() {
        return Err("Groq returned empty response".to_string());
    }

    Ok(sanitize_output(raw))
}

/// Strip markdown artifacts that the model occasionally emits despite instructions.
fn sanitize_output(text: &str) -> String {
    // Remove code fences
    let text = text.trim_matches('`');
    // Strip leading/trailing quotes
    let text = text.trim_matches('"').trim_matches('\'');
    // Remove bold/italic markers
    let text = text.replace("**", "").replace("__", "");
    // Collapse runs of spaces produced by stripping markers
    let text: String = text
        .lines()
        .map(|line| {
            line.split_whitespace().collect::<Vec<_>>().join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n");
    text.trim().to_string()
}
