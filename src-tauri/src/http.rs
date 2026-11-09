//! One shared HTTP client for every Groq call.
//!
//! Before this, the transcription call and the cleanup call each built a new
//! `reqwest::Client`, so every dictation paid for two fresh TCP + TLS
//! handshakes to api.groq.com, back to back. From Melbourne that is a few
//! round trips to the US each time, spent before the paste can happen.
//! A shared client keeps the connection from the transcription request open
//! for the cleanup request a moment later.
//!
//! [`prewarm`] goes one step further: it opens that connection while Ben is
//! still talking, so the transcription request does not pay for it either.

use std::sync::OnceLock;
use std::time::Duration;

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Idle pooled connections are dropped after this. Long enough to span one
/// recording plus its two requests; short enough that a connection is not
/// left to go stale across a sleep/resume. hyper also retries a request whose
/// pooled connection turns out to be closed before anything was sent.
const POOL_IDLE: Duration = Duration::from_secs(90);

pub fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .pool_idle_timeout(POOL_IDLE)
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            // Builder only fails on TLS backend init; the default client
            // would fail identically, so there is nothing better to fall to.
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Open (or refresh) the pooled connection to Groq without sending anything
/// that needs the API key or counts against the rate limit. Fire and forget.
pub fn prewarm() {
    tauri::async_runtime::spawn(async {
        let started = std::time::Instant::now();
        let result = client()
            .head("https://api.groq.com/")
            .timeout(Duration::from_secs(5))
            .send()
            .await;
        crate::applog::log(&format!(
            "Timing: groq prewarm {} in {}ms",
            if result.is_ok() { "ok" } else { "failed" },
            started.elapsed().as_millis()
        ));
    });
}
