//! Turn a spoken enumeration into dot points.
//!
//! Conservative by design: ordinary prose must come out untouched, so this
//! only fires on two unmistakable shapes.
//!
//! 1. **Ordinal markers**, three or more in strict order, each starting a
//!    clause: "First, call Mitch. Second, send the invoice. Third, book it."
//!    Ordinals ("first", "secondly") or counting words ("one", "two") work,
//!    but counting words need a comma or colon after them, because "one" in
//!    the middle of prose is far more often a number than a marker.
//! 2. **A list cue followed by a comma list** of three or more short items:
//!    "My shopping list is eggs, milk, bread and butter."
//!
//! Anything else returns `None` and the caller pastes the text as it was.

const ORDINALS: &[&[&str]] = &[
    &["first", "firstly"],
    &["second", "secondly"],
    &["third", "thirdly"],
    &["fourth", "fourthly"],
    &["fifth", "fifthly"],
    &["sixth"],
    &["seventh"],
    &["eighth"],
    &["ninth"],
    &["tenth"],
];
const COUNTING: &[&str] = &["one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"];

const CUES: &[&str] = &[
    "the list is",
    "my list is",
    "shopping list is",
    "to do list is",
    "to-do list is",
    "todo list is",
    "the items are",
    "items are",
    "things to do are",
    "things to do today are",
    "things i need to do are",
    "here's the list",
    "here is the list",
];

/// Longest item allowed, in words. A "list" of paragraphs is prose.
const MAX_ORDINAL_ITEM_WORDS: usize = 30;
const MAX_CUE_ITEM_WORDS: usize = 6;
const MIN_ITEMS: usize = 3;

pub const BULLET: &str = "- ";

struct Word {
    start: usize,
    end: usize,
    bare: String,
}

fn words(text: &str) -> Vec<Word> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                out.push(make_word(text, s, i));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        out.push(make_word(text, s, text.len()));
    }
    out
}

fn make_word(text: &str, start: usize, end: usize) -> Word {
    let bare = text[start..end]
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    Word { start, end, bare }
}

/// True when the word at `i` begins a clause: start of text, or the previous
/// word ends in clause punctuation, or the previous word is a bare "and".
fn starts_clause(text: &str, ws: &[Word], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let prev = &text[ws[i - 1].start..ws[i - 1].end];
    if prev.ends_with(['.', '!', '?', ';', ':', ',']) {
        return true;
    }
    ws[i - 1].bare == "and"
}

fn tidy_item(raw: &str) -> String {
    let mut s = raw.trim();
    s = s.trim_start_matches(|c: char| matches!(c, ',' | ':' | '-' | ' '));
    // "first of all, ..." -> drop "of all".
    if let Some(r) = s.strip_prefix("of all") {
        s = r.trim_start_matches(|c: char| matches!(c, ',' | ':' | ' '));
    }
    let mut s = s.trim_end_matches(|c: char| matches!(c, ',' | ';' | '.' | ' ')).to_string();
    for tail in [" and", " then", " and then"] {
        if s.to_lowercase().ends_with(tail) {
            s.truncate(s.len() - tail.len());
            s = s.trim_end_matches(|c: char| matches!(c, ',' | ';' | ' ')).to_string();
        }
    }
    let mut chars = s.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn render(intro: &str, items: &[String], outro: &str) -> String {
    let mut out = String::new();
    let intro = intro.trim();
    if !intro.is_empty() {
        let intro = intro.trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | ' '));
        out.push_str(intro);
        if !intro.ends_with(':') {
            out.push(':');
        }
        out.push('\n');
    }
    let lines: Vec<String> = items.iter().map(|i| format!("{}{}", BULLET, i)).collect();
    out.push_str(&lines.join("\n"));
    let outro = outro.trim();
    if !outro.is_empty() {
        out.push_str("\n\n");
        out.push_str(outro);
    }
    out
}

fn ordinal_rank(bare: &str) -> Option<(usize, bool)> {
    if let Some(r) = ORDINALS.iter().position(|forms| forms.contains(&bare)) {
        return Some((r, false));
    }
    COUNTING.iter().position(|w| *w == bare).map(|r| (r, true))
}

fn format_ordinals(text: &str) -> Option<String> {
    let ws = words(text);
    // (word index, rank) for every clause-starting marker in strict sequence.
    let mut markers: Vec<usize> = Vec::new();
    let mut counting: Option<bool> = None;
    for (i, w) in ws.iter().enumerate() {
        let Some((rank, is_counting)) = ordinal_rank(&w.bare) else { continue };
        if rank != markers.len() || !starts_clause(text, &ws, i) {
            continue;
        }
        if counting.is_some_and(|c| c != is_counting) {
            continue;
        }
        // Counting words must be followed directly by a comma or colon.
        if is_counting && !text[w.start..w.end].ends_with([',', ':']) {
            continue;
        }
        counting = Some(is_counting);
        markers.push(i);
    }
    if markers.len() < MIN_ITEMS {
        return None;
    }

    let intro = &text[..ws[markers[0]].start];
    let mut items = Vec::new();
    let mut outro = "";
    for (k, &m) in markers.iter().enumerate() {
        let body_start = ws[m].end;
        let body_end = match markers.get(k + 1) {
            Some(&next) => ws[next].start,
            None => {
                // The last item ends at the first sentence end after it;
                // anything after that is a closing remark, not an item.
                let tail = &text[body_start..];
                match tail.find(['.', '!', '?']) {
                    Some(p) if tail[p + 1..].trim().len() > 0 => {
                        outro = &tail[p + 1..];
                        body_start + p + 1
                    }
                    _ => text.len(),
                }
            }
        };
        let item = tidy_item(&text[body_start..body_end]);
        if item.is_empty() || item.split_whitespace().count() > MAX_ORDINAL_ITEM_WORDS {
            return None;
        }
        items.push(item);
    }
    Some(render(intro, &items, outro))
}

fn format_cue_list(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    // Only an ASCII lowercasing keeps byte offsets aligned with `text`.
    if lower.len() != text.len() {
        return None;
    }
    let (cue_end, _) = CUES
        .iter()
        .filter_map(|c| {
            lower.find(c).and_then(|p| {
                let end = p + c.len();
                let boundary_before = p == 0 || !lower.as_bytes()[p - 1].is_ascii_alphanumeric();
                let boundary_after = end == lower.len() || !lower.as_bytes()[end].is_ascii_alphanumeric();
                (boundary_before && boundary_after).then_some((end, p))
            })
        })
        .min_by_key(|(_, p)| *p)?;

    let rest = &text[cue_end..];
    let rest = rest.trim_start_matches(|c: char| matches!(c, ':' | ',' | ' '));
    let (body, outro) = match rest.find(['.', '!', '?']) {
        Some(p) => (&rest[..p], &rest[p + 1..]),
        None => (rest, ""),
    };

    let mut parts: Vec<String> = body.split(',').map(|s| s.trim().to_string()).collect();
    // "bread and butter" in last place: "..., bread and butter" -> two items
    // only when the list already used commas (Oxford or not).
    if parts.len() >= 2 {
        let last = parts.pop().unwrap();
        let last = last.strip_prefix("and ").map(str::to_string).unwrap_or(last);
        match last.rsplit_once(" and ") {
            Some((a, b)) => {
                parts.push(a.trim().to_string());
                parts.push(b.trim().to_string());
            }
            None => parts.push(last),
        }
    }
    if parts.len() < MIN_ITEMS {
        return None;
    }
    let mut items = Vec::new();
    for p in parts {
        let item = tidy_item(&p);
        let n = item.split_whitespace().count();
        if n == 0 || n > MAX_CUE_ITEM_WORDS {
            return None;
        }
        items.push(item);
    }
    Some(render(&text[..cue_end], &items, outro))
}

/// Format `text` as dot points if it is clearly an enumeration.
pub fn format_list(text: &str) -> Option<String> {
    let t = text.trim();
    // Already structured: leave it alone.
    if t.is_empty() || t.contains('\n') {
        return None;
    }
    format_ordinals(t).or_else(|| format_cue_list(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinal_sentences_become_dot_points() {
        let got = format_list("I need three things. First, call Mitch. Second, send the invoice. Third, book the onboarding call.").unwrap();
        assert_eq!(got, "I need three things:\n- Call Mitch\n- Send the invoice\n- Book the onboarding call");
    }

    #[test]
    fn ordinals_in_one_run_on_sentence() {
        let got = format_list("First check the reviews, second reply to them and third post the update.").unwrap();
        assert_eq!(got, "- Check the reviews\n- Reply to them\n- Post the update");
    }

    #[test]
    fn counting_words_with_commas() {
        let got = format_list("One, eggs. Two, milk. Three, bread.").unwrap();
        assert_eq!(got, "- Eggs\n- Milk\n- Bread");
    }

    #[test]
    fn a_closing_remark_after_the_last_item_stays_prose() {
        let got = format_list("Firstly, fix the hotkey. Secondly, fix the paste. Thirdly, ship it. Let me know when done.").unwrap();
        assert_eq!(got, "- Fix the hotkey\n- Fix the paste\n- Ship it\n\nLet me know when done.");
    }

    #[test]
    fn cue_and_comma_list() {
        let got = format_list("My shopping list is eggs, milk, bread and butter.").unwrap();
        assert_eq!(got, "My shopping list is:\n- Eggs\n- Milk\n- Bread\n- Butter");
        let got = format_list("The things to do are: call Mitch, email Sarah, and fix the site.").unwrap();
        assert_eq!(got, "The things to do are:\n- Call Mitch\n- Email Sarah\n- Fix the site");
    }

    #[test]
    fn ordinary_prose_is_untouched() {
        for s in [
            "I need eggs, milk, bread and butter.",
            "First, I want to say thanks for the call yesterday.",
            "First we fix it, then we ship it.",
            "One of the two options is fine, three is too many.",
            "I came second in the race and third in the next one.",
            "The first time I saw it, the second guy left, and the third stayed.",
            "We have one, maybe two, clients who could refer.",
            "The list is long and boring.",
            "Items are on the table, mostly.",
            "Can you send me that file when you get a sec?",
            "",
        ] {
            assert_eq!(format_list(s), None, "prose was reformatted: {:?}", s);
        }
    }

    #[test]
    fn two_items_is_not_enough() {
        assert_eq!(format_list("First, call Mitch. Second, send the invoice."), None);
        assert_eq!(format_list("The list is eggs and milk."), None);
    }

    #[test]
    fn already_formatted_text_is_left_alone() {
        assert_eq!(format_list("Todo:\n- one\n- two\n- three"), None);
    }

    #[test]
    fn long_cue_items_mean_it_is_prose() {
        assert_eq!(
            format_list("The items are the thing we talked about at the meeting last week, the invoice, and the call."),
            None
        );
    }
}
