//! "Typr, ..." voice commands.
//!
//! A dictation that starts with the app's name and then an imperative verb is
//! an instruction to Typr, not text to paste. Whisper rarely hears the name
//! right, so the usual mis-hearings count too ("typer", "tipper", "type r",
//! "typo").
//!
//! Two gates, both required, so ordinary sentences keep dictating:
//! 1. the first word (or two) is the name or a known mis-hearing, and
//! 2. the next word is an imperative verb from a fixed list.
//!
//! "Typer is my favourite app" fails gate 2 and pastes as written. A command
//! that passes both gates but that Typr cannot carry out ([`Action::Unsupported`])
//! is also pasted as written by the caller, so a false positive never loses
//! what Ben said.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// Lowercased verb, e.g. "add".
    pub verb: String,
    /// Everything after the verb, original casing, trimmed.
    pub rest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Dictate,
    Command(Command),
}

const IMPERATIVES: &[&str] = &[
    "add", "remove", "delete", "make", "format", "undo", "list", "show", "open", "clear",
    "approve", "reject", "learn", "forget", "spell", "set", "turn", "switch", "change", "hide",
];

fn bare(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase()
}

/// How many leading words are the wake name, if any.
fn prefix_len(words: &[&str]) -> Option<usize> {
    let first = bare(words.first()?);
    match first.as_str() {
        "typr" | "typer" | "tipper" | "typo" | "typa" => Some(1),
        "type" if words.get(1).map(|w| bare(w)) == Some("r".into()) => Some(2),
        _ => None,
    }
}

pub fn classify(transcript: &str) -> Intent {
    let words: Vec<&str> = transcript.split_whitespace().collect();
    let Some(mut i) = prefix_len(&words) else {
        return Intent::Dictate;
    };
    // "Typr, please add ..." is still a command.
    if words.get(i).map(|w| bare(w)) == Some("please".into()) {
        i += 1;
    }
    let Some(verb_word) = words.get(i) else {
        return Intent::Dictate;
    };
    let verb = bare(verb_word);
    if !IMPERATIVES.contains(&verb.as_str()) {
        return Intent::Dictate;
    }
    let rest = words[i + 1..].join(" ");
    let rest = rest.trim_end_matches(|c: char| matches!(c, '.' | '!' | '?' | ',')).trim().to_string();
    Intent::Command(Command { verb, rest })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    UndoLastChange,
    ShowSettings,
    AddWord(String),
    RemoveWord(String),
    /// Parsed as a command but not one Typr can do. Caller dictates instead.
    Unsupported,
}

/// Strip a trailing "to/from the dictionary" and return what came before it.
fn before_dictionary_suffix(rest: &str, prepositions: &[&str]) -> Option<String> {
    let words: Vec<&str> = rest.split_whitespace().collect();
    let lower: Vec<String> = words.iter().map(|w| bare(w)).collect();
    let n = lower.len();
    let target = |w: &str| matches!(w, "dictionary" | "vocabulary" | "vocab" | "words");
    if n < 2 || !target(&lower[n - 1]) {
        return None;
    }
    let mut cut = n - 1;
    if cut > 0 && matches!(lower[cut - 1].as_str(), "the" | "my") {
        cut -= 1;
    }
    if cut == 0 || !prepositions.contains(&lower[cut - 1].as_str()) {
        return None;
    }
    let term: String = words[..cut - 1].join(" ");
    let term = term.trim_matches(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | '.' | ':'));
    if term.is_empty() {
        None
    } else {
        Some(term.to_string())
    }
}

pub fn parse_action(cmd: &Command) -> Action {
    match cmd.verb.as_str() {
        "undo" => Action::UndoLastChange,
        "show" | "open" | "list" => Action::ShowSettings,
        "add" | "learn" => before_dictionary_suffix(&cmd.rest, &["to", "into"])
            .map(Action::AddWord)
            .unwrap_or(Action::Unsupported),
        "remove" | "delete" | "forget" => before_dictionary_suffix(&cmd.rest, &["from"])
            .map(Action::RemoveWord)
            .unwrap_or(Action::Unsupported),
        _ => Action::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_command(s: &str) -> bool {
        matches!(classify(s), Intent::Command(_))
    }

    fn action(s: &str) -> Action {
        match classify(s) {
            Intent::Command(c) => parse_action(&c),
            Intent::Dictate => panic!("{:?} should be a command", s),
        }
    }

    #[test]
    fn the_name_and_its_mishearings_trigger_a_command() {
        for s in [
            "Typr, add Mitch to the dictionary.",
            "typer add Mitch to the dictionary",
            "Tipper, undo.",
            "Type R show my corrections",
            "Typo remove grok from the dictionary",
            "Typr please undo that",
            "Typer. Format this as a list.",
        ] {
            assert!(is_command(s), "{:?} should be a command", s);
        }
    }

    #[test]
    fn ordinary_sentences_still_dictate() {
        for s in [
            "Typer is my favourite app",
            "typr is my favourite app",
            "Typo in the second paragraph, fix it later",
            "Tipper trucks are expensive",
            "Type something into the box",
            "type r is how you spell it",
            "Add Mitch to the dictionary",
            "I said typr add Mitch to the dictionary",
            "Typr",
            "",
        ] {
            assert_eq!(classify(s), Intent::Dictate, "{:?} should dictate", s);
        }
    }

    #[test]
    fn verb_and_rest_are_split_out() {
        assert_eq!(
            classify("Typer, add Kangaroo Growth to the dictionary."),
            Intent::Command(Command { verb: "add".into(), rest: "Kangaroo Growth to the dictionary".into() })
        );
    }

    #[test]
    fn actions_parse() {
        assert_eq!(action("Typr undo"), Action::UndoLastChange);
        assert_eq!(action("Typr undo the last change."), Action::UndoLastChange);
        assert_eq!(action("typer show corrections"), Action::ShowSettings);
        assert_eq!(action("Typr open settings"), Action::ShowSettings);
        assert_eq!(action("Typr add Kangaroo Growth to the dictionary."), Action::AddWord("Kangaroo Growth".into()));
        assert_eq!(action("Typr add \"ServiceM8\" to my dictionary"), Action::AddWord("ServiceM8".into()));
        assert_eq!(action("Typr learn Simplicity into the vocabulary"), Action::AddWord("Simplicity".into()));
        assert_eq!(action("tipper remove grok from the dictionary"), Action::RemoveWord("grok".into()));
    }

    #[test]
    fn commands_typr_cannot_do_are_unsupported_so_they_dictate() {
        // Imperative but not something Typr acts on.
        assert_eq!(action("Typer, make sure you check the invoices"), Action::Unsupported);
        assert_eq!(action("Typr add a note about Mitch"), Action::Unsupported);
        assert_eq!(action("Typr add to the dictionary"), Action::Unsupported);
        assert_eq!(action("Typr remove the last line"), Action::Unsupported);
        assert_eq!(action("Typr format this as a list"), Action::Unsupported);
    }
}
