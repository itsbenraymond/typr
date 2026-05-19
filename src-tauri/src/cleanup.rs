pub fn cleanup_text(text: &str) -> String {
    let s = remove_noise_labels(text);
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Normalize multiple spaces to single space
    let normalized: String = trimmed
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ");

    // Capitalize first letter of each sentence
    let mut result = String::new();
    let mut capitalize_next = true;

    for ch in normalized.chars() {
        if capitalize_next && ch.is_alphabetic() {
            result.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
            if ch == '.' || ch == '!' || ch == '?' {
                capitalize_next = true;
            }
        }
    }

    // Ensure ending punctuation
    if let Some(last) = result.chars().last() {
        if !matches!(last, '.' | '!' | '?') {
            result.push('.');
        }
    }

    result
}

/// Steps 1–3 only: remove noise labels + collapse whitespace. No capitalize/punctuate.
pub fn strip_noise_only(text: &str) -> String {
    let s = remove_noise_labels(text);
    s.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Remove Whisper noise artifacts before formatting.
/// Handles: silence/placeholder tokens, 26 noise-label categories in [brackets] or (parens).
fn remove_noise_labels(text: &str) -> String {
    use std::sync::OnceLock;
    use regex::Regex;

    static SILENCE_RE: OnceLock<Regex> = OnceLock::new();
    static NOISE_RE: OnceLock<Regex>   = OnceLock::new();

    let silence_re = SILENCE_RE.get_or_init(|| {
        Regex::new(r"(?i)\[(?:BLANK[_ ]AUDIO|SILENCE|S)\]|<\|nospeech\|>|\[\s*S\s*\]")
            .unwrap()
    });

    let noise_labels = [
        "applause", "background noise", "blank audio", "breathing",
        "cough", "coughing", "exhale", "heartbeat", "indistinct",
        "inaudible", "inhale", "laughing", "laughter", "loud noise",
        "muffled speech", "music", "noise", "silence", "sigh", "sighs",
        "sniffing", "static", "unclear speech", "unintelligible",
        "wind", "wind blowing", "wind noise",
    ];
    let labels_pattern = noise_labels.join("|");
    let noise_re = NOISE_RE.get_or_init(|| {
        Regex::new(&format!(r"(?i)[\[\(]\s*(?:{})\s*[\]\)]", labels_pattern)).unwrap()
    });

    let s = silence_re.replace_all(text, " ");
    let s = noise_re.replace_all(&s, " ");
    s.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim_whitespace() {
        assert_eq!(cleanup_text("  hello world  "), "Hello world.");
    }

    #[test]
    fn test_normalize_spaces() {
        assert_eq!(cleanup_text("hello    world"), "Hello world.");
    }

    #[test]
    fn test_capitalize_first_letter() {
        assert_eq!(cleanup_text("hello world"), "Hello world.");
    }

    #[test]
    fn test_capitalize_after_period() {
        assert_eq!(cleanup_text("hello. world"), "Hello. World.");
    }

    #[test]
    fn test_capitalize_after_question_mark() {
        assert_eq!(cleanup_text("hello? world"), "Hello? World.");
    }

    #[test]
    fn test_capitalize_after_exclamation() {
        assert_eq!(cleanup_text("hello! world"), "Hello! World.");
    }

    #[test]
    fn test_ensure_ending_punctuation() {
        assert_eq!(cleanup_text("hello world"), "Hello world.");
    }

    #[test]
    fn test_preserve_existing_ending_punctuation() {
        assert_eq!(cleanup_text("hello world."), "Hello world.");
        assert_eq!(cleanup_text("hello world!"), "Hello world!");
        assert_eq!(cleanup_text("hello world?"), "Hello world?");
    }

    #[test]
    fn test_empty_string() {
        assert_eq!(cleanup_text(""), "");
        assert_eq!(cleanup_text("   "), "");
    }

    #[test]
    fn test_already_clean() {
        assert_eq!(cleanup_text("Hello world."), "Hello world.");
    }

    #[test]
    fn test_removes_blank_audio() {
        assert_eq!(cleanup_text("[BLANK_AUDIO]"), "");
        assert_eq!(cleanup_text("hello [BLANK_AUDIO] world"), "Hello world.");
    }

    #[test]
    fn test_removes_silence_marker() {
        assert_eq!(cleanup_text("[SILENCE]"), "");
        assert_eq!(cleanup_text("hello [SILENCE] world"), "Hello world.");
    }

    #[test]
    fn test_removes_nospeech_token() {
        assert_eq!(cleanup_text("<|nospeech|>"), "");
        assert_eq!(cleanup_text("hello <|nospeech|> world"), "Hello world.");
    }

    #[test]
    fn test_removes_noise_labels_in_brackets() {
        assert_eq!(cleanup_text("[background noise]"), "");
        assert_eq!(cleanup_text("hello [laughter] world"), "Hello world.");
    }

    #[test]
    fn test_removes_noise_labels_in_parens() {
        assert_eq!(cleanup_text("(applause)"), "");
        assert_eq!(cleanup_text("hello (coughing) world"), "Hello world.");
    }

    #[test]
    fn test_noise_removal_case_insensitive() {
        assert_eq!(cleanup_text("[Background Noise]"), "");
        assert_eq!(cleanup_text("[LAUGHTER]"), "");
    }

    #[test]
    fn test_strip_noise_only_no_capitalization() {
        let result = strip_noise_only("hello [BLANK_AUDIO] world");
        assert_eq!(result, "hello world");
        assert!(!result.ends_with('.'));
        assert_eq!(&result[..1], "h");
    }
}
