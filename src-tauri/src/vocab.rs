//! Personal dictionary and snippets.
//!
//! Both are deterministic on purpose.
//!
//! Whisper reliably mangles the words Ben says most: "high level" for
//! HighLevel, "grok" for Groq, "typer" for Typr, "super base" for Supabase.
//! Asking the language model to fix those in the prompt works most of the
//! time, and "most of the time" on a client's business name is not good
//! enough. A substitution pass does it every time, and it also runs *after*
//! the model, because the model will happily re-break a term it has never
//! seen.
//!
//! Snippets skip the model entirely. A saved signature must paste exactly as
//! saved, byte for byte, or it is not a snippet.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DictEntry {
    /// How it should be written.
    pub term: String,
    /// What the transcriber tends to hear instead, comma-free, one per entry.
    #[serde(default)]
    pub misheard: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snippet {
    /// Spoken trigger, matched against the whole utterance.
    pub trigger: String,
    /// Inserted exactly as written.
    pub text: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Reduce an utterance to comparable form: lowercase, no punctuation, single
/// spaces. Speech has no punctuation of its own, so "my email signature." and
/// "My email signature" have to be the same trigger.
fn normalize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Return a snippet's text if the utterance *is* the trigger.
///
/// Whole-utterance only, never a substring: expanding "bug report template"
/// in the middle of "remind me to write the bug report template" would replace
/// a sentence the speaker was still in the middle of.
pub fn match_snippet(snippets: &[Snippet], raw_text: &str) -> Option<String> {
    let spoken = normalize(raw_text);
    if spoken.is_empty() {
        return None;
    }
    snippets
        .iter()
        .filter(|s| s.enabled && !s.trigger.trim().is_empty())
        .find(|s| normalize(&s.trigger) == spoken)
        .map(|s| s.text.clone())
}

/// Apply dictionary corrections to `text`.
///
/// Whole-word, case-insensitive, and it preserves the capitalisation the
/// entry declares — the point of the dictionary is that "Supabase" comes out
/// as "Supabase", not "supabase", wherever it lands in the sentence.
pub fn apply_dictionary(entries: &[DictEntry], text: &str) -> String {
    // One flat list of (what to look for, what to write), from every entry.
    //
    // It has to be flat and globally sorted, not per-entry: applying entries
    // one at a time let "high level" -> HighLevel consume the text before
    // "go high level" -> GoHighLevel ever got a chance, so the longer, more
    // specific name could never win.
    let mut targets: Vec<(String, &str)> = Vec::new();
    for entry in entries.iter().filter(|e| e.enabled) {
        let term = entry.term.trim();
        if term.is_empty() {
            continue;
        }
        // The term itself is included, so a correctly-heard but wrongly-cased
        // "supabase" still gets fixed.
        targets.push((term.to_lowercase(), term));
        for m in &entry.misheard {
            let m = m.trim();
            if !m.is_empty() {
                targets.push((m.to_lowercase(), term));
            }
        }
    }
    if targets.is_empty() {
        return text.to_string();
    }
    targets.sort_by_key(|(needle, _)| std::cmp::Reverse(needle.len()));

    replace_longest_first(text, &targets)
}

/// Single left-to-right pass, preferring the longest match at each position.
///
/// Hand-rolled rather than a regex so an entry containing regex
/// metacharacters (`C++`, `.NET`, `node.js`) is treated as literal text
/// instead of silently becoming a pattern — or failing to compile and
/// dropping the correction entirely. A single pass also means replaced text
/// is never re-examined, so one correction cannot cascade into another.
fn replace_longest_first(haystack: &str, targets: &[(String, &str)]) -> String {
    // Work in chars, not bytes. Byte offsets into a lowercased copy only line
    // up with the original while every character lowercases to the same
    // length, which is true for ASCII and not guaranteed in general. Bailing
    // out on a non-ASCII transcript would silently switch the whole
    // dictionary off for exactly the names most likely to need it.
    let original: Vec<char> = haystack.chars().collect();
    let lower: Vec<char> = original
        .iter()
        .flat_map(|c| c.to_lowercase())
        .collect::<Vec<char>>();

    // flat_map can change the character count (e.g. 'İ'); when it does, fall
    // back to a simple per-char lowercase so the two vectors stay aligned.
    let lower: Vec<char> = if lower.len() == original.len() {
        lower
    } else {
        original
            .iter()
            .map(|c| c.to_lowercase().next().unwrap_or(*c))
            .collect()
    };

    let needles: Vec<(Vec<char>, &str)> = targets
        .iter()
        .map(|(n, r)| (n.chars().collect::<Vec<char>>(), *r))
        .collect();

    let mut out = String::with_capacity(haystack.len());
    let mut i = 0usize;

    while i < original.len() {
        let boundary_before = i == 0 || !is_word_char(lower[i - 1]);
        let mut matched: Option<(usize, &str)> = None;

        if boundary_before {
            for (needle, replacement) in &needles {
                let end = i + needle.len();
                if end <= lower.len()
                    && lower[i..end] == needle[..]
                    && (end == lower.len() || !is_word_char(lower[end]))
                {
                    matched = Some((end, replacement));
                    break; // needles are sorted longest-first
                }
            }
        }

        match matched {
            Some((end, replacement)) => {
                out.push_str(replacement);
                i = end;
            }
            None => {
                out.push(original[i]);
                i += 1;
            }
        }
    }

    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A short prompt fragment naming the dictionary terms, so the model is at
/// least aware of them while rewriting. The substitution pass is what
/// guarantees the result; this only reduces how often it has to fix things.
pub fn prompt_hint(entries: &[DictEntry]) -> String {
    let terms: Vec<&str> = entries
        .iter()
        .filter(|e| e.enabled && !e.term.trim().is_empty())
        .map(|e| e.term.trim())
        .collect();
    if terms.is_empty() {
        return String::new();
    }
    format!(
        "\n\nThese names are spelled exactly like this and are never to be \
changed, translated or split into separate words: {}.",
        terms.join(", ")
    )
}

/// The terms Typr ships with, because they are the ones Ben says daily and
/// Whisper gets wrong every time.
pub fn default_dictionary() -> Vec<DictEntry> {
    let d = |term: &str, misheard: &[&str]| DictEntry {
        term: term.to_string(),
        misheard: misheard.iter().map(|s| s.to_string()).collect(),
        enabled: true,
    };
    vec![
        d("Typr", &["typer", "tipper", "type r"]),
        d("Groq", &["grok", "grock", "grog"]),
        d("Supabase", &["super base", "supa base", "superbase"]),
        d("Cloudflare", &["cloud flare", "cloudflair"]),
        d("HighLevel", &["high level", "highlevel", "go high level"]),
        d("GoHighLevel", &["go high level", "gohighlevel"]),
        d("Kangaroo Growth", &["kangaroo growth"]),
        d("GBP", &["gbp", "g b p"]),
        d("SEO", &["seo", "s e o"]),
        d("Wrangler", &["rangler"]),
        d("Syncthing", &["sync thing"]),
    ]
}

pub fn default_snippets() -> Vec<Snippet> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> Vec<DictEntry> {
        default_dictionary()
    }

    #[test]
    fn fixes_the_terms_whisper_always_mangles() {
        let got = apply_dictionary(&dict(), "push it to cloud flare and check super base");
        assert_eq!(got, "push it to Cloudflare and check Supabase");
    }

    #[test]
    fn fixes_casing_even_when_the_word_was_heard_correctly() {
        assert_eq!(apply_dictionary(&dict(), "open supabase"), "open Supabase");
    }

    #[test]
    fn does_not_touch_the_inside_of_other_words() {
        // "grok" must not be rewritten inside a longer word.
        let entries = vec![DictEntry {
            term: "Groq".into(),
            misheard: vec!["grok".into()],
            enabled: false,
        }];
        let mut enabled = entries.clone();
        enabled[0].enabled = true;
        assert_eq!(apply_dictionary(&enabled, "grokking the docs"), "grokking the docs");
        assert_eq!(apply_dictionary(&enabled, "use grok for this"), "use Groq for this");
    }

    #[test]
    fn a_disabled_entry_does_nothing() {
        let entries = vec![DictEntry {
            term: "Typr".into(),
            misheard: vec!["typer".into()],
            enabled: false,
        }];
        assert_eq!(apply_dictionary(&entries, "open typer"), "open typer");
    }

    #[test]
    fn longer_phrases_win_over_shorter_ones() {
        let entries = vec![
            DictEntry { term: "HighLevel".into(), misheard: vec!["high level".into()], enabled: true },
            DictEntry { term: "GoHighLevel".into(), misheard: vec!["go high level".into()], enabled: true },
        ];
        assert_eq!(
            apply_dictionary(&entries, "set it up in go high level today"),
            "set it up in GoHighLevel today"
        );
    }

    #[test]
    fn entries_with_regex_metacharacters_are_literal() {
        // A naive regex implementation would treat "C++" as a bad pattern and
        // either panic, fail to compile, or match something absurd.
        let entries = vec![
            DictEntry { term: "C++".into(), misheard: vec!["c plus plus".into()], enabled: true },
            DictEntry { term: "Node.js".into(), misheard: vec!["node js".into()], enabled: true },
        ];
        assert_eq!(
            apply_dictionary(&entries, "written in c plus plus and node js"),
            "written in C++ and Node.js"
        );
        // The dot in "Node.js" must not match an arbitrary character.
        assert_eq!(apply_dictionary(&entries, "nodexjs is not a thing"), "nodexjs is not a thing");
    }

    #[test]
    fn snippet_matches_the_whole_utterance_only() {
        let snips = vec![Snippet {
            trigger: "my email signature".into(),
            text: "Ben Raymond\nKangaroo Growth".into(),
            enabled: true,
        }];
        assert_eq!(
            match_snippet(&snips, "My email signature."),
            Some("Ben Raymond\nKangaroo Growth".to_string())
        );
        // Mid-sentence must NOT expand.
        assert_eq!(
            match_snippet(&snips, "remind me to update my email signature next week"),
            None
        );
    }

    #[test]
    fn snippet_matching_ignores_punctuation_and_case() {
        let snips = vec![Snippet {
            trigger: "bug report template".into(),
            text: "Steps:\nExpected:\nActual:".into(),
            enabled: true,
        }];
        for spoken in ["Bug report template", "bug report template.", "  BUG REPORT TEMPLATE!  "] {
            assert!(match_snippet(&snips, spoken).is_some(), "failed on {:?}", spoken);
        }
    }

    #[test]
    fn disabled_and_blank_snippets_never_fire() {
        let snips = vec![
            Snippet { trigger: "sig".into(), text: "x".into(), enabled: false },
            Snippet { trigger: "   ".into(), text: "y".into(), enabled: true },
        ];
        assert_eq!(match_snippet(&snips, "sig"), None);
        assert_eq!(match_snippet(&snips, ""), None);
        assert_eq!(match_snippet(&snips, "   "), None);
    }

    #[test]
    fn prompt_hint_is_empty_when_there_is_nothing_to_say() {
        assert_eq!(prompt_hint(&[]), "");
        let disabled = vec![DictEntry { term: "Typr".into(), misheard: vec![], enabled: false }];
        assert_eq!(prompt_hint(&disabled), "");
    }

    #[test]
    fn prompt_hint_lists_enabled_terms() {
        let hint = prompt_hint(&dict());
        assert!(hint.contains("Typr"));
        assert!(hint.contains("Supabase"));
    }
}
