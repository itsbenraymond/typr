//! Deterministic backstop for the two things Ben asked for on 2026-11-22:
//! "clean up the text I say and format it nicely into paragraphs", and stop
//! keeping "okay" literally.
//!
//! The prompt asks for both, but Groq is not deterministic at temperature 0
//! (see CLAUDE.md), so a prompt rule alone lands most of the time, not every
//! time. This runs after the model and makes both properties hold whatever the
//! model did. It only ever removes an opener or inserts a blank line, never
//! changes a word, so it cannot alter meaning.

use crate::modes::Mode;

/// Below this, one paragraph is fine.
const MIN_WORDS_FOR_BREAKS: usize = 60;
/// Sentences per paragraph before a break is forced.
const MAX_SENTENCES: usize = 4;

/// Openers that are pure padding when they start a sentence with more after
/// them. Deliberately short: "yeah" and "so" can carry meaning ("yeah, do
/// that" answering Claude), so those are left to the model.
const PADDING_OPENERS: &[&str] = &["okay", "ok", "alright", "all right"];

/// Words that usually start a new point when they open a sentence.
const TRANSITIONS: &[&str] = &[
    "also", "second", "secondly", "third", "thirdly", "another", "anyway", "next",
    "then", "finally", "lastly", "plus", "step", "something", "other", "first",
    "firstly", "now", "oh",
];

pub fn tidy(text: &str, mode: Mode) -> String {
    match mode {
        // Code and commands: never touch line structure. Verbatim: the words
        // are the point. Chat: short messages, the model's shape is right.
        Mode::Developer | Mode::Verbatim | Mode::Chat => text.to_string(),
        _ => ensure_paragraphs(&strip_padding_openers(text)),
    }
}

/// Drop "Okay," / "Alright," etc. at the start of any sentence that has at
/// least three more words after it, and recapitalise what follows.
pub fn strip_padding_openers(text: &str) -> String {
    let sentences = split_keep_breaks(text);
    let mut out = String::with_capacity(text.len());
    for piece in sentences {
        match piece {
            Piece::Break(b) => out.push_str(&b),
            Piece::Sentence(s) => out.push_str(&strip_one(&s)),
        }
    }
    out
}

fn strip_one(sentence: &str) -> String {
    let mut s = sentence.to_string();
    // Loop so "Okay, alright, the header..." loses both.
    loop {
        let lower = s.to_lowercase();
        let mut stripped = false;
        for op in PADDING_OPENERS {
            let with_comma = format!("{},", op);
            if lower.starts_with(&with_comma) {
                let Some(rest) = s.get(with_comma.len()..) else { continue };
                let rest = rest.trim_start();
                if rest.split_whitespace().count() >= 3 {
                    s = capitalise_first(rest);
                    stripped = true;
                    break;
                }
            }
        }
        if !stripped {
            return s;
        }
    }
}

fn capitalise_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// If a long text came back as one block, break it into paragraphs at
/// sentence boundaries: on a transition word once a paragraph has two
/// sentences, and always after four.
pub fn ensure_paragraphs(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.contains('\n') || trimmed.split_whitespace().count() < MIN_WORDS_FOR_BREAKS {
        return text.to_string();
    }
    let sentences = sentences(trimmed);
    if sentences.len() < 3 {
        return text.to_string();
    }

    let mut paragraphs: Vec<Vec<&str>> = vec![Vec::new()];
    for s in &sentences {
        let current = paragraphs.last().unwrap();
        let first_word = s
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches(|c: char| !c.is_alphabetic())
            .to_lowercase();
        let starts_point = TRANSITIONS.contains(&first_word.as_str());
        if current.len() >= MAX_SENTENCES || (current.len() >= 2 && starts_point) {
            paragraphs.push(Vec::new());
        }
        paragraphs.last_mut().unwrap().push(s);
    }
    paragraphs
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| p.join(" "))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Split into sentences on . ? ! followed by a space and an uppercase letter
/// or digit. Abbreviations like "e.g." and "vs." are not followed by one in
/// normal prose often enough to matter, and a missed split only means a
/// slightly longer paragraph.
fn sentences(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if (b == b'.' || b == b'?' || b == b'!')
            && i + 2 < bytes.len()
            && bytes[i + 1] == b' '
            && (bytes[i + 2].is_ascii_uppercase() || bytes[i + 2].is_ascii_digit() || bytes[i + 2] == b'"')
            && !ends_with_abbrev(&text[start..=i])
        {
            out.push(text[start..=i].trim());
            start = i + 2;
            i += 2;
            continue;
        }
        i += 1;
    }
    if start < text.len() {
        let tail = text[start..].trim();
        if !tail.is_empty() {
            out.push(tail);
        }
    }
    out
}

fn ends_with_abbrev(s: &str) -> bool {
    let last = s.split_whitespace().last().unwrap_or("").to_lowercase();
    matches!(last.as_str(), "e.g." | "i.e." | "vs." | "mr." | "mrs." | "dr." | "st." | "etc.")
}

enum Piece {
    Sentence(String),
    Break(String),
}

/// Sentences plus the exact whitespace between them, so stripping an opener
/// never disturbs the paragraph layout the model produced.
fn split_keep_breaks(text: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    for (pi, para) in text.split("\n\n").enumerate() {
        if pi > 0 {
            pieces.push(Piece::Break("\n\n".into()));
        }
        for (li, line) in para.split('\n').enumerate() {
            if li > 0 {
                pieces.push(Piece::Break("\n".into()));
            }
            let ss = sentences(line);
            for (si, s) in ss.iter().enumerate() {
                if si > 0 {
                    pieces.push(Piece::Break(" ".into()));
                }
                pieces.push(Piece::Sentence((*s).to_string()));
            }
        }
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAMBLE: &str = "The scrolling speed should be normalized between all of them. \
As soon as I start hovering over it, that's when it should start scrolling. I think \
that's really good. All of the websites are looking really good. Also, some of the \
Google review stuff needs work. For the Kangaroo logo, can you make that the same \
orange as the Kangaroo word in the top left? That's perfect. Basically just fixing \
that scrolling so it stays smooth on every page and every device we test.";

    #[test]
    fn a_long_single_block_gets_paragraphs() {
        let out = ensure_paragraphs(RAMBLE);
        assert!(out.contains("\n\n"), "no paragraph break: {}", out);
        // No words lost or changed: only whitespace differs.
        let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(norm(&out), norm(RAMBLE));
    }

    #[test]
    fn a_transition_word_starts_the_new_paragraph() {
        let out = ensure_paragraphs(RAMBLE);
        assert!(out.contains("\n\nAlso, some"), "{}", out);
    }

    #[test]
    fn short_text_is_left_alone() {
        let s = "Can you check why the build failed? It was fine yesterday.";
        assert_eq!(ensure_paragraphs(s), s);
    }

    #[test]
    fn text_the_model_already_broke_up_is_left_alone() {
        let s = format!("{}\n\nSecond point here.", RAMBLE);
        assert_eq!(ensure_paragraphs(&s), s);
    }

    #[test]
    fn opening_okay_is_dropped() {
        assert_eq!(
            strip_padding_openers("Okay, cool, but don't leak what platform they use."),
            "Cool, but don't leak what platform they use."
        );
        assert_eq!(
            strip_padding_openers("Alright, the scrolling speed should match."),
            "The scrolling speed should match."
        );
        assert_eq!(
            strip_padding_openers("All right, let's use a giga prompt. Okay, step one is Typr."),
            "Let's use a giga prompt. Step one is Typr."
        );
    }

    #[test]
    fn a_short_okay_reply_is_kept() {
        // "Okay, do it." is the whole message: it is an answer, not padding.
        assert_eq!(strip_padding_openers("Okay, do it."), "Okay, do it.");
        assert_eq!(strip_padding_openers("Is that okay?"), "Is that okay?");
    }

    #[test]
    fn stripping_keeps_the_models_paragraphs() {
        let s = "Okay, the header is too big.\n\nOkay, can we make the footer orange?";
        assert_eq!(
            strip_padding_openers(s),
            "The header is too big.\n\nCan we make the footer orange?"
        );
    }

    #[test]
    fn developer_and_verbatim_are_never_touched() {
        let s = format!("Okay, {}", RAMBLE);
        assert_eq!(tidy(&s, Mode::Developer), s);
        assert_eq!(tidy(&s, Mode::Verbatim), s);
        let prompt = tidy(&s, Mode::Prompt);
        assert!(prompt.starts_with("The scrolling"));
        assert!(prompt.contains("\n\n"));
    }

    #[test]
    fn abbreviations_do_not_split_a_sentence() {
        let s = sentences("Use a tool e.g. Figma for this. Then ship it.");
        assert_eq!(s, vec!["Use a tool e.g. Figma for this.", "Then ship it."]);
    }
}
