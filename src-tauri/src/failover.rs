//! How a failed provider call decides what happens next.
//!
//! Groq is the primary for both speech-to-text and the cleanup pass, and it
//! fails in a few distinct ways that call for different moves:
//!
//! * **429** is per model. On 2026-11-16 `whisper-large-v3-turbo` hit its
//!   7,200 audio-seconds-per-hour cap and every dictation failed with "try
//!   again in 2m". Retrying the same model is pointless; the next model has
//!   its own bucket, so move on immediately.
//! * **5xx / connection reset** is usually a blip. One quick retry, then move on.
//! * **Timeout** means the provider is struggling as a whole. Skip the rest of
//!   that provider's models and go straight to the next provider.
//! * Anything else (401, 404, bad output) is not going to change on retry.

use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailKind {
    RateLimited,
    Timeout,
    Transient,
    Fatal,
}

#[derive(Debug, Clone)]
pub struct Failure {
    pub kind: FailKind,
    pub msg: String,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl Failure {
    pub fn fatal(msg: impl Into<String>) -> Self {
        Self { kind: FailKind::Fatal, msg: msg.into() }
    }

    pub fn from_status(status: u16, body: &str) -> Self {
        let kind = match status {
            429 => FailKind::RateLimited,
            408 | 500..=599 => FailKind::Transient,
            _ => FailKind::Fatal,
        };
        Self { kind, msg: format!("HTTP {}: {}", status, body.trim()) }
    }

    pub fn from_reqwest(e: &reqwest::Error) -> Self {
        let kind = if e.is_timeout() {
            FailKind::Timeout
        } else {
            FailKind::Transient
        };
        // reqwest's Display hides the cause ("error sending request for url");
        // the source chain says whether it was a timeout, reset or TLS error.
        let mut msg = format!("request failed: {}", e);
        let mut src = std::error::Error::source(e);
        while let Some(inner) = src {
            msg.push_str(&format!(": {}", inner));
            src = inner.source();
        }
        Self { kind, msg }
    }
}

/// Pause before the single retry on a transient failure. Short on purpose:
/// Ben is waiting on the paste.
pub const RETRY_DELAY: Duration = Duration::from_millis(300);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_the_right_move() {
        assert_eq!(Failure::from_status(429, "").kind, FailKind::RateLimited);
        assert_eq!(Failure::from_status(500, "").kind, FailKind::Transient);
        assert_eq!(Failure::from_status(503, "").kind, FailKind::Transient);
        assert_eq!(Failure::from_status(408, "").kind, FailKind::Transient);
        assert_eq!(Failure::from_status(401, "").kind, FailKind::Fatal);
        assert_eq!(Failure::from_status(404, "").kind, FailKind::Fatal);
    }

    #[test]
    fn message_keeps_status_and_body_for_the_log() {
        let f = Failure::from_status(429, " rate_limit_exceeded \n");
        assert_eq!(f.to_string(), "HTTP 429: rate_limit_exceeded");
    }
}
