use std::time::Duration;
use reqwest::Client;
use serde_json::json;

const FORMAT_PROMPT: &str = "\
You are a text formatter. The input is a raw speech transcription. \
Clean it up: fix punctuation, capitalize sentences. \
If the content is a list, format it with bullet points and newlines. \
If there are natural paragraph breaks, add them. \
Return ONLY the formatted text — no commentary, no explanation.";

pub async fn format_text(api_key: &str, raw_text: &str) -> Result<String, String> {
    if api_key.is_empty() {
        return Err("Groq API key not set".to_string());
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;

    let body = json!({
        "model": "llama-3.1-8b-instant",
        "messages": [
            { "role": "system", "content": FORMAT_PROMPT },
            { "role": "user",   "content": raw_text }
        ],
        "max_tokens": 1024,
        "temperature": 0.1
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

    let formatted = json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();

    if formatted.is_empty() {
        return Err("Groq returned empty response".to_string());
    }

    Ok(formatted)
}
