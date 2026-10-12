//! The three layers between a raw transcript and the text that gets pasted.
//!
//! 1. **Understand** — snippets and the personal dictionary. Deterministic,
//!    no model involved. A snippet short-circuits everything else.
//! 2. **Transform** — the mode's prompt, run through Groq.
//! 3. **Validate** — reject anything that is not a faithful cleanup, and fall
//!    back to the dictionary-corrected transcript rather than pasting it.
//!
//! The ordering matters. The dictionary runs before the model so it is
//! working from the right nouns, and again after, because the model will
//! happily re-break a name it has never seen. The validation lives in
//! `format_groq`, applied to every attempt.

use crate::appctx::AppContext;
use crate::applog;
use crate::modes::{self, Mode};
use crate::settings::Settings;
use crate::vocab;

#[derive(Debug, Clone, PartialEq)]
pub struct Transformed {
    pub text: String,
    pub mode: Mode,
    /// True when the model ran and produced the text. False means a snippet
    /// fired, or the model failed and this is the fallback.
    pub model_used: bool,
    /// Set when the user should be told something went sideways even though
    /// text was still produced.
    pub warning: Option<String>,
}

/// Decide the mode for this dictation without running anything.
pub fn mode_for(settings: &Settings, ctx: &AppContext) -> Mode {
    let default_mode = Mode::from_str(&settings.default_mode).unwrap_or(Mode::Clean);
    if !settings.auto_mode {
        return default_mode;
    }
    let overrides: Vec<(String, String)> = settings
        .mode_overrides
        .iter()
        .map(|o| (o.app.clone(), o.mode.clone()))
        .collect();
    modes::detect(ctx, &overrides, default_mode)
}

pub async fn transform(settings: &Settings, raw_text: &str, ctx: &AppContext) -> Transformed {
    let mode = mode_for(settings, ctx);

    // ── Layer 1: snippets short-circuit the model entirely ──
    // A saved signature or template must paste byte for byte. Running it
    // through a cleanup model would "improve" it.
    if let Some(text) = vocab::match_snippet(&settings.snippets, raw_text) {
        applog::log(&format!("Snippet fired for utterance: {:?}", raw_text.trim()));
        return Transformed {
            text,
            mode,
            model_used: false,
            warning: None,
        };
    }

    // ── Layer 1b: fix the nouns before the model sees them ──
    let corrected = vocab::apply_dictionary(&settings.dictionary, raw_text);

    // Verbatim means verbatim: punctuation only, and nothing else touched.
    // Still worth sending to the model, which is what adds the punctuation.
    applog::log(&format!("Mode: {} (exe={} title={})", mode.as_str(), ctx.exe, ctx.title));

    // ── Layer 2: the model ──
    let extra = format!(
        "{}{}{}",
        mode.instructions(),
        vocab::prompt_hint(&settings.dictionary),
        custom_instructions(settings),
    );

    match crate::format_groq::format_text_with(&settings.groq_api_key, &corrected, &extra).await {
        Ok(formatted) => Transformed {
            // ── Layer 3b: the model can re-break a name. Fix it again.
            text: vocab::apply_dictionary(&settings.dictionary, &formatted),
            mode,
            model_used: true,
            warning: None,
        },
        Err(e) => {
            applog::log(&format!("Transform failed ({}), using corrected transcript", e));
            Transformed {
                // Not the raw transcript: the dictionary pass is deterministic
                // and correct on its own, so the fallback keeps it.
                text: crate::cleanup::cleanup_text(&corrected),
                mode,
                model_used: false,
                warning: Some(short_reason(&e)),
            }
        }
    }
}

fn custom_instructions(settings: &Settings) -> String {
    let text = settings.custom_instructions.trim();
    if text.is_empty() {
        return String::new();
    }
    // Fenced and labelled as preferences, so a stray imperative in Ben's own
    // style notes ("answer in one line") cannot turn the cleanup engine into
    // an assistant that replies to the dictation.
    format!(
        "\n\nThe speaker's standing style preferences are below. Apply them to \
how the text reads. They never authorise adding content, answering the text, \
or changing its meaning.\n<preferences>\n{}\n</preferences>",
        text
    )
}

/// Collapse a raw API error into something that fits on the pill.
fn short_reason(e: &str) -> String {
    let lower = e.to_lowercase();
    if lower.contains("api key") {
        "Add your Groq API key in Settings".into()
    } else if lower.contains("429") || lower.contains("rate limit") {
        "Groq rate limit - pasted uncleaned".into()
    } else if lower.contains("model_not_found") || lower.contains("does not exist") {
        "AI model unavailable - pasted uncleaned".into()
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "Groq timed out - pasted uncleaned".into()
    } else if lower.contains("dns") || lower.contains("connect") {
        "No connection to Groq - pasted uncleaned".into()
    } else {
        "AI cleanup failed - pasted uncleaned".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ModeOverride;
    use crate::vocab::{DictEntry, Snippet};

    fn settings() -> Settings {
        Settings::default()
    }

    fn ctx(exe: &str, title: &str) -> AppContext {
        AppContext { exe: exe.into(), title: title.into() }
    }

    #[test]
    fn auto_mode_picks_from_the_app() {
        let s = settings();
        assert_eq!(mode_for(&s, &ctx("code.exe", "main.rs")), Mode::Developer);
        assert_eq!(mode_for(&s, &ctx("chrome.exe", "Inbox - Gmail")), Mode::Email);
    }

    #[test]
    fn turning_auto_mode_off_pins_the_default() {
        let mut s = settings();
        s.auto_mode = false;
        assert_eq!(mode_for(&s, &ctx("code.exe", "main.rs")), Mode::Clean);
        s.default_mode = "polish".into();
        assert_eq!(mode_for(&s, &ctx("code.exe", "main.rs")), Mode::Polish);
    }

    #[test]
    fn an_unknown_app_uses_the_default_mode() {
        let mut s = settings();
        s.default_mode = "chat".into();
        assert_eq!(mode_for(&s, &ctx("mystery.exe", "")), Mode::Chat);
    }

    #[test]
    fn a_nonsense_default_mode_does_not_break_dictation() {
        let mut s = settings();
        s.default_mode = "banana".into();
        s.auto_mode = false;
        assert_eq!(mode_for(&s, &ctx("code.exe", "x")), Mode::Clean);
    }

    #[test]
    fn user_override_wins() {
        let mut s = settings();
        s.mode_overrides = vec![ModeOverride {
            app: "code.exe".into(),
            mode: "verbatim".into(),
        }];
        assert_eq!(mode_for(&s, &ctx("code.exe", "main.rs")), Mode::Verbatim);
    }

    #[tokio::test]
    async fn a_snippet_bypasses_the_model_entirely() {
        let mut s = settings();
        // No API key at all, so reaching the model would fail loudly.
        s.groq_api_key = String::new();
        s.snippets = vec![Snippet {
            trigger: "my email signature".into(),
            text: "Ben Raymond\nKangaroo Growth\n0400 000 000".into(),
            enabled: true,
        }];

        let out = transform(&s, "My email signature.", &ctx("outlook.exe", "")).await;
        assert_eq!(out.text, "Ben Raymond\nKangaroo Growth\n0400 000 000");
        assert!(!out.model_used);
        assert!(out.warning.is_none(), "a snippet is not a failure");
    }

    #[tokio::test]
    async fn the_dictionary_still_applies_when_the_model_is_unavailable() {
        let mut s = settings();
        s.groq_api_key = String::new(); // forces the fallback path
        s.dictionary = vec![DictEntry {
            term: "Supabase".into(),
            misheard: vec!["super base".into()],
            enabled: true,
        }];

        let out = transform(&s, "check super base for the rows", &ctx("", "")).await;
        assert!(
            out.text.contains("Supabase"),
            "fallback must keep deterministic corrections, got {:?}",
            out.text
        );
        assert!(!out.model_used);
        assert_eq!(out.warning.as_deref(), Some("Add your Groq API key in Settings"));
    }

    #[test]
    fn custom_instructions_are_fenced_as_preferences() {
        let mut s = settings();
        s.custom_instructions = "Answer in one line.".into();
        let built = custom_instructions(&s);
        assert!(built.contains("<preferences>"));
        assert!(built.contains("never authorise adding content"));
    }

    #[test]
    fn empty_custom_instructions_add_nothing() {
        let mut s = settings();
        s.custom_instructions = "   \n  ".into();
        assert_eq!(custom_instructions(&s), "");
    }

    #[test]
    fn failure_reasons_are_short_enough_for_the_pill() {
        for e in [
            "Groq API key not configured — add it in Settings > General",
            "HTTP 429 Too Many Requests: rate limit",
            "HTTP 404: model_not_found",
            "request failed: operation timed out",
            "request failed: dns error",
            "something else entirely",
        ] {
            let r = short_reason(e);
            assert!(r.len() <= 48, "{:?} is too long for the pill: {}", r, r.len());
            assert!(r.is_ascii());
        }
    }
}
