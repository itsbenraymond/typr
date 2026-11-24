//! Deterministic backstop for paragraph shape and the opening "Okay," (Ben,
//! 2026-11-22: "format it nicely into paragraphs", stop keeping "okay").
//!
//! Turned around on 2026-11-24 (Ben: "it's breaking my sentences into a
//! thousand paragraphs when it should be two or three. Some things don't need
//! to be broken into paragraphs at all."). Real dictations showed two shapes:
//! one sentence per line (#2076, 49 words over 10 lines) and five paragraphs
//! for 108 words (#2074). So this now joins single line breaks back into their
//! paragraph, caps a message at three paragraphs, makes a short message one
//! paragraph, and only splits a genuine wall of text. Groq is not deterministic
//! (see CLAUDE.md), which is why the shape is held here and not only asked for.
//! It still only removes an opener or changes whitespace, never a word.

use crate::modes::Mode;

/// Below this, a message is one paragraph with no breaks at all.
const ONE_PARAGRAPH_UNDER: usize = 80;
/// A single block this long, with no break from the model, gets split.
const MIN_WORDS_FOR_BREAKS: usize = 140;
/// Sentences per paragraph before a split is forced, in a wall of text.
const MAX_SENTENCES: usize = 7;
/// Most paragraphs a dictation ends up with.
const MAX_PARAGRAPHS: usize = 3;

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

/// `spoken` is the transcript before the model. If the speaker said "new line"
/// or "new paragraph", their breaks are theirs and the layout is left alone.
pub fn tidy(text: &str, mode: Mode, spoken: &str) -> String {
    match mode {
        // Code and commands: never touch line structure. Verbatim: the words
        // are the point. Email and notes keep greetings, sign-offs and lists
        // on their own lines, so only the opener is stripped there.
        Mode::Developer | Mode::Verbatim => text.to_string(),
        Mode::Email | Mode::Notes => strip_padding_openers(text),
        _ if said_a_break(spoken) => strip_padding_openers(text),
        Mode::Chat => join_soft_breaks(text),
        _ => cap_paragraphs(&ensure_paragraphs(&join_soft_breaks(&strip_padding_openers(text)))),
    }
}

fn said_a_break(spoken: &str) -> bool {
    let s = spoken.to_lowercase();
    s.contains("new line") || s.contains("new paragraph") || s.contains("newline")
}

/// A line that is a list item stays on its own line.
fn is_list_line(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("- ") || t.starts_with("* ") || t.starts_with('\u{2022}') {
        return true;
    }
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    !digits.is_empty() && (t[digits.len()..].starts_with(". ") || t[digits.len()..].starts_with(") "))
}

/// Inside a paragraph, a single newline between two ordinary lines becomes a
/// space: "Sentence one.\nSentence two." is one paragraph, not two lines.
/// Blank lines (real paragraph breaks) and list lines are kept.
pub fn join_soft_breaks(text: &str) -> String {
    text.split("\n\n")
        .map(|para| {
            let lines: Vec<&str> = para.split('\n').map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
            let mut out = String::new();
            for (i, line) in lines.iter().enumerate() {
                if i == 0 {
                    out.push_str(line);
                } else if is_list_line(line) || is_list_line(lines[i - 1]) {
                    out.push('\n');
                    out.push_str(line);
                } else {
                    out.push(' ');
                    out.push_str(line.trim_start());
                }
            }
            out
        })
        .filter(|p| !p.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A short message becomes one paragraph; anything left with more than
/// MAX_PARAGRAPHS has its shortest neighbouring pair merged until it fits.
/// A message holding a list is left as the model laid it out.
pub fn cap_paragraphs(text: &str) -> String {
    let mut paras: Vec<String> = text.split("\n\n").map(str::to_string).filter(|p| !p.trim().is_empty()).collect();
    if paras.len() <= 1 || paras.iter().any(|p| p.lines().any(is_list_line)) {
        return text.to_string();
    }
    let words = |s: &str| s.split_whitespace().count();
    if words(text) < ONE_PARAGRAPH_UNDER {
        return paras.join(" ");
    }
    while paras.len() > MAX_PARAGRAPHS {
        let i = (0..paras.len() - 1).min_by_key(|&i| words(&paras[i]) + words(&paras[i + 1])).unwrap();
        let merged = format!("{} {}", paras[i], paras[i + 1]);
        paras.splice(i..=i + 1, [merged]);
    }
    paras.join("\n\n")
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

/// If a genuine wall of text came back as one block, break it at sentence
/// boundaries: on a transition word once a paragraph has three sentences, and
/// always after seven. cap_paragraphs then holds it to three.
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
        if current.len() >= MAX_SENTENCES || (current.len() >= 3 && starts_point) {
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

    fn norm(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }
    fn paras(s: &str) -> usize {
        s.split("\n\n").filter(|p| !p.trim().is_empty()).count()
    }

    #[test]
    fn a_ramble_under_140_words_stays_one_block() {
        // Ben, 2026-11-24: "some things don't need to be broken into paragraphs at all".
        assert_eq!(ensure_paragraphs(RAMBLE), RAMBLE);
    }

    #[test]
    fn a_genuine_wall_of_text_gets_two_or_three_paragraphs_and_no_word_changes() {
        let wall = format!("{} {} {}", RAMBLE, RAMBLE, RAMBLE);
        let out = tidy(&wall, Mode::Prompt, "");
        assert!((2..=MAX_PARAGRAPHS).contains(&paras(&out)), "{} paragraphs: {}", paras(&out), out);
        assert_eq!(norm(&out), norm(&wall));
    }

    // Real dictation #2076 (2026-11-24, Prompt mode in VS Code): 49 words, one sentence per line.
    const ONE_PER_LINE: &str = "They feel really short.\nThey feel really, really short.\nIt feels so short.\n\
I don't think that's right.\nThere's no way that's right.\nIt feels too short.\nThere's no value.\n\
Nothing like that.\nCan you really just look at what he says and write good copy?\nPlease try again.";

    #[test]
    fn one_sentence_per_line_becomes_one_paragraph() {
        let out = tidy(ONE_PER_LINE, Mode::Prompt, "they feel really short");
        assert!(!out.contains('\n'), "{}", out);
        assert_eq!(norm(&out), norm(ONE_PER_LINE));
    }

    // Real dictation #2074: 108 words the model split into five paragraphs.
    const FIVE_PARAS: &str = "Can you have one name column and no required consent checkbox because it's internal.\n\n\
I want to add a few more things. Keep that message but add more stuff.\n\n\
Make sure they know mobile numbers start with either 04 or +61.\n\n\
Send them the onboarding link because they're doing some of the closing themselves for international leads. \
Just say this one is for anyone who's already paid and then give them that link.\n\n\
What else could they need from me assuming they don't know much about the service? Draft some other Google Docs \
that are worth sending them. What are their details? Could I potentially give them that?";

    #[test]
    fn five_paragraphs_come_back_as_at_most_three_with_every_word_kept() {
        let out = tidy(FIVE_PARAS, Mode::Prompt, "");
        assert!(paras(&out) <= MAX_PARAGRAPHS, "{}", out);
        assert_eq!(norm(&out), norm(FIVE_PARAS));
    }

    #[test]
    fn a_short_message_the_model_split_is_one_paragraph() {
        let s = "The header is too big.\n\nCan we make the footer orange?\n\nThe mobile menu doesn't close.";
        assert_eq!(tidy(s, Mode::Prompt, ""), "The header is too big. Can we make the footer orange? The mobile menu doesn't close.");
    }

    #[test]
    fn lists_spoken_breaks_and_email_layout_are_kept() {
        let list = "Three things:\n- the header\n- the footer\n- the menu";
        assert_eq!(tidy(list, Mode::Prompt, ""), list);
        let numbered = "Do these:\n1. Fix the header\n2) Fix the footer";
        assert_eq!(tidy(numbered, Mode::Prompt, ""), numbered);
        let said = "First line.\nSecond line.";
        assert_eq!(tidy(said, Mode::Prompt, "first line new line second line"), said);
        let email = "Hi Sarah,\n\nThe quote is ready.\n\nCheers,\nBen";
        assert_eq!(tidy(email, Mode::Email, ""), email);
    }

    // What the NEW prompt returned for real dictation #2054 (247 words spoken): one 221-word block.
    // The prompt now leans to one paragraph, so the backstop is what splits a genuinely long one.
    #[test]
    fn a_221_word_block_from_the_new_prompt_becomes_two_or_three_paragraphs() {
        let wall = include_str!("fixtures/wall_2054.txt");
        let out = tidy(wall, Mode::Prompt, "");
        assert!((2..=MAX_PARAGRAPHS).contains(&paras(&out)), "{} paragraphs:\n{}", paras(&out), out);
        assert_eq!(norm(&out), norm(wall));
    }

    #[test]
    fn chat_joins_lines_but_keeps_the_models_paragraphs() {
        assert_eq!(tidy("Running late.\nBe there at six.", Mode::Chat, ""), "Running late. Be there at six.");
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
        let s = format!("Okay, {}\nnext line", RAMBLE);
        assert_eq!(tidy(&s, Mode::Developer, ""), s);
        assert_eq!(tidy(&s, Mode::Verbatim, ""), s);
        let prompt = tidy(&s, Mode::Prompt, "");
        assert!(prompt.starts_with("The scrolling"));
        assert!(!prompt.contains('\n'), "{}", prompt);
    }

    #[test]
    fn abbreviations_do_not_split_a_sentence() {
        let s = sentences("Use a tool e.g. Figma for this. Then ship it.");
        assert_eq!(s, vec!["Use a tool e.g. Figma for this.", "Then ship it."]);
    }
}
