//! Learning dictionary entries from the corrections Ben makes after a paste.
//!
//! The flow, end to end:
//!
//! 1. Typr pastes text. About 30 seconds later the platform layer reads what
//!    the target field holds now (see `correction_watch`).
//! 2. [`extract`] diffs the pasted text against that field and keeps only edits
//!    that look like a deliberate fix of a misheard word: a short
//!    substitution, anchored by unchanged words, that is not just typing on.
//! 3. [`LearnStore::observe`] files each one as a *proposal* in a review queue,
//!    with the evidence (original, corrected, context, when, how often).
//!    Nothing is ever added to the dictionary automatically.
//! 4. Ben approves or rejects. Approving edits the dictionary, and every
//!    dictionary change goes to an append-only [`ChangeLog`]. [`undo_last`]
//!    reverts the most recent change that has not already been undone.
//!
//! Everything here is pure logic over strings and vectors (plus two small
//! file helpers) so it tests on any OS. The Windows-only part is reading the
//! field, and that lives elsewhere.

use serde::{Deserialize, Serialize};

use crate::vocab::{apply_dictionary, DictEntry};

/// Longest original or replacement phrase considered, in words. A longer
/// rewrite is an edit to the content, not a misheard name.
const MAX_PHRASE_WORDS: usize = 3;
/// More separate substitutions than this in one paste reads as a rewrite.
const MAX_HUNKS: usize = 3;
/// Share of the pasted words that must still be in the field. Below it the
/// paste was undone, the field was cleared, or focus moved elsewhere.
const MIN_SURVIVING: f64 = 0.6;
/// Share of the pasted words that may be changed before it counts as a rewrite.
const MAX_CHANGED: f64 = 0.4;
/// Pastes shorter than this have no unchanged neighbours to anchor an edit.
const MIN_PASTE_WORDS: usize = 3;
/// Case-only and punctuation-only fixes are style noise until seen this often.
pub const STYLE_ONLY_THRESHOLD: u32 = 3;
/// Above this many cells the diff is skipped instead of stalling the thread.
const MAX_DIFF_CELLS: usize = 4_000_000;
/// Letter-level edit distance allowed, as a share of the longer phrase.
/// "grok"->"Groq" and "super base"->"Supabase" pass; "Tuesday"->"Wednesday"
/// or "five"->"six" are content changes and do not.
const MAX_SPELLING_DISTANCE: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CandidateKind {
    Substitution,
    CaseOnly,
    PunctuationOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub original: String,
    pub corrected: String,
    pub context: String,
    pub kind: CandidateKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Equal,
    Delete,
    Insert,
}

fn tokens(s: &str) -> Vec<&str> {
    s.split_whitespace().collect()
}

/// Token-level diff (LCS). Returns the op sequence or None if too large.
fn diff(a: &[&str], b: &[&str]) -> Option<Vec<Op>> {
    let (n, m) = (a.len(), b.len());
    if n.saturating_mul(m) > MAX_DIFF_CELLS {
        return None;
    }
    // lcs[i][j] = LCS length of a[i..] and b[j..]
    let mut lcs = vec![0u32; (n + 1) * (m + 1)];
    let idx = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[idx(i, j)] = if a[i] == b[j] {
                lcs[idx(i + 1, j + 1)] + 1
            } else {
                lcs[idx(i + 1, j)].max(lcs[idx(i, j + 1)])
            };
        }
    }
    let mut ops = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push(Op::Equal);
            i += 1;
            j += 1;
        } else if lcs[idx(i + 1, j)] >= lcs[idx(i, j + 1)] {
            ops.push(Op::Delete);
            i += 1;
        } else {
            ops.push(Op::Insert);
            j += 1;
        }
    }
    ops.extend(std::iter::repeat(Op::Delete).take(n - i));
    ops.extend(std::iter::repeat(Op::Insert).take(m - j));
    Some(ops)
}

/// One run of non-equal ops: `a[a_start..a_end]` became `b[b_start..b_end]`.
struct Hunk {
    a_start: usize,
    a_end: usize,
    b_start: usize,
    b_end: usize,
    anchored_before: bool,
    anchored_after: bool,
}

fn hunks(ops: &[Op]) -> Vec<Hunk> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let mut k = 0;
    while k < ops.len() {
        if ops[k] == Op::Equal {
            i += 1;
            j += 1;
            k += 1;
            continue;
        }
        let (a_start, b_start) = (i, j);
        let anchored_before = k > 0;
        while k < ops.len() && ops[k] != Op::Equal {
            match ops[k] {
                Op::Delete => i += 1,
                Op::Insert => j += 1,
                Op::Equal => unreachable!(),
            }
            k += 1;
        }
        out.push(Hunk {
            a_start,
            a_end: i,
            b_start,
            b_end: j,
            anchored_before,
            anchored_after: k < ops.len(),
        });
    }
    out
}

fn letters(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

fn is_closed_class(letters: &str) -> bool {
    const WORDS: &[&str] = &[
        "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday",
        "january", "february", "march", "april", "may", "june", "july", "august",
        "september", "october", "november", "december",
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        "eleven", "twelve", "twenty", "thirty", "forty", "fifty", "hundred", "thousand",
        "yes", "no", "am", "pm",
    ];
    !letters.is_empty() && (letters.chars().all(|c| c.is_ascii_digit()) || WORDS.contains(&letters))
}

/// Strip punctuation the two phrases share at either edge, so "grok," vs
/// "Groq," compares the words and not the comma.
fn trim_shared_edges<'a>(a: &'a str, b: &'a str) -> (&'a str, &'a str) {
    let (mut a, mut b) = (a, b);
    loop {
        match (a.chars().last(), b.chars().last()) {
            (Some(x), Some(y)) if x == y && !x.is_alphanumeric() => {
                a = &a[..a.len() - x.len_utf8()];
                b = &b[..b.len() - y.len_utf8()];
            }
            _ => break,
        }
    }
    loop {
        match (a.chars().next(), b.chars().next()) {
            (Some(x), Some(y)) if x == y && !x.is_alphanumeric() => {
                a = &a[x.len_utf8()..];
                b = &b[y.len_utf8()..];
            }
            _ => break,
        }
    }
    (a, b)
}

/// Find the deliberate corrections between what Typr pasted and what the
/// field holds at the end of the watch window.
///
/// `field_after` may hold far more than the paste (the rest of the document);
/// unmatched text around the paste is a pure insertion and is ignored.
pub fn extract(pasted: &str, field_after: &str) -> Vec<Candidate> {
    let a = tokens(pasted);
    let b = tokens(field_after);
    if a.len() < MIN_PASTE_WORDS || a == b {
        return Vec::new();
    }
    let Some(ops) = diff(&a, &b) else {
        return Vec::new();
    };

    let surviving = ops.iter().filter(|o| **o == Op::Equal).count();
    if (surviving as f64) < MIN_SURVIVING * a.len() as f64 {
        return Vec::new();
    }

    // Substitutions only: both sides non-empty. Pure insertions (typing more,
    // or surrounding document text) and pure deletions are never evidence.
    let subs: Vec<Hunk> = hunks(&ops)
        .into_iter()
        .filter(|h| h.a_end > h.a_start && h.b_end > h.b_start)
        .collect();
    if subs.is_empty() || subs.len() > MAX_HUNKS {
        return Vec::new();
    }
    let changed: usize = subs.iter().map(|h| h.a_end - h.a_start).sum();
    if changed as f64 > MAX_CHANGED * a.len() as f64 {
        return Vec::new();
    }

    let mut out = Vec::new();
    for h in subs {
        let del = h.a_end - h.a_start;
        let ins = h.b_end - h.b_start;
        if del > MAX_PHRASE_WORDS || ins > MAX_PHRASE_WORDS {
            continue;
        }
        // An edge hunk can swallow neighbouring text (the document before the
        // paste, or words typed after it). Only trust it when the word counts
        // match, i.e. it is a like-for-like swap.
        if !(h.anchored_before && h.anchored_after) && del != ins {
            continue;
        }
        if !h.anchored_before && !h.anchored_after {
            continue;
        }

        let original_full = a[h.a_start..h.a_end].join(" ");
        let corrected_full = b[h.b_start..h.b_end].join(" ");
        let (original, corrected) = trim_shared_edges(&original_full, &corrected_full);
        if original.is_empty() || corrected.is_empty() || original == corrected {
            continue;
        }

        let (lo, lc) = (letters(original), letters(corrected));
        let kind = if lo == lc {
            if original.to_lowercase() == corrected.to_lowercase() {
                CandidateKind::CaseOnly
            } else {
                CandidateKind::PunctuationOnly
            }
        } else {
            // Finishing a word ("gro" -> "groq") is typing, not a correction.
            if lc.starts_with(&lo) {
                continue;
            }
            // Swapping one weekday, month or number for another is changing
            // the facts, however close the spelling.
            if is_closed_class(&lo) && is_closed_class(&lc) {
                continue;
            }
            let longest = lo.chars().count().max(lc.chars().count()).max(1);
            if edit_distance(&lo, &lc) as f64 > MAX_SPELLING_DISTANCE * longest as f64 {
                continue;
            }
            CandidateKind::Substitution
        };

        let lo_ctx = h.a_start.saturating_sub(5);
        let hi_ctx = (h.a_end + 5).min(a.len());
        out.push(Candidate {
            original: original.to_string(),
            corrected: corrected.to_string(),
            context: a[lo_ctx..hi_ctx].join(" "),
            kind,
        });
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub id: u64,
    pub original: String,
    pub corrected: String,
    /// The pasted words around the most recent sighting.
    pub context: String,
    pub kind: CandidateKind,
    pub first_seen: u64,
    pub last_seen: u64,
    pub count: u32,
    pub status: Status,
}

impl Proposal {
    /// Whether this belongs in front of Ben right now.
    pub fn is_reviewable(&self) -> bool {
        self.status == Status::Pending
            && (self.kind == CandidateKind::Substitution || self.count >= STYLE_ONLY_THRESHOLD)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnStore {
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub proposals: Vec<Proposal>,
}

/// What a dictionary change did, precisely enough to reverse it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Effect {
    CreatedEntry { term: String, misheard: Vec<String> },
    AddedMisheard { term: String, misheard: String },
    RemovedEntry { index: usize, entry: DictEntry },
    /// Rejections are logged too, but change nothing.
    NoChange,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub seq: u64,
    pub at: u64,
    /// "approve", "reject", "undo", "command-add", "command-remove".
    pub action: String,
    #[serde(default)]
    pub proposal_id: Option<u64>,
    pub effect: Effect,
    /// For an undo, the seq of the entry it reverted.
    #[serde(default)]
    pub undoes: Option<u64>,
}

/// Append-only. Entries are only ever pushed; an undo is a new entry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChangeLog {
    entries: Vec<LogEntry>,
}

impl ChangeLog {
    pub fn from_entries(entries: Vec<LogEntry>) -> Self {
        Self { entries }
    }
    pub fn entries(&self) -> &[LogEntry] {
        &self.entries
    }
    fn push(&mut self, at: u64, action: &str, proposal_id: Option<u64>, effect: Effect, undoes: Option<u64>) -> LogEntry {
        let seq = self.entries.last().map(|e| e.seq + 1).unwrap_or(1);
        let entry = LogEntry { seq, at, action: action.to_string(), proposal_id, effect, undoes };
        self.entries.push(entry.clone());
        entry
    }
}

impl LearnStore {
    /// File new candidates as proposals, or bump the evidence on known ones.
    /// Returns the ids touched. A rejected proposal keeps counting but is
    /// never resurfaced; one the dictionary already covers is skipped.
    pub fn observe(&mut self, candidates: &[Candidate], dict: &[DictEntry], now: u64) -> Vec<u64> {
        let mut touched = Vec::new();
        for c in candidates {
            if apply_dictionary(dict, &c.original) == c.corrected {
                continue;
            }
            let key = c.original.to_lowercase();
            if let Some(p) = self
                .proposals
                .iter_mut()
                .find(|p| p.original.to_lowercase() == key && p.corrected == c.corrected)
            {
                p.count += 1;
                p.last_seen = now;
                p.context = c.context.clone();
                touched.push(p.id);
                continue;
            }
            self.next_id += 1;
            self.proposals.push(Proposal {
                id: self.next_id,
                original: c.original.clone(),
                corrected: c.corrected.clone(),
                context: c.context.clone(),
                kind: c.kind,
                first_seen: now,
                last_seen: now,
                count: 1,
                status: Status::Pending,
            });
            touched.push(self.next_id);
        }
        touched
    }

    /// The review queue, most-seen first.
    pub fn queue(&self) -> Vec<Proposal> {
        let mut q: Vec<Proposal> = self.proposals.iter().filter(|p| p.is_reviewable()).cloned().collect();
        q.sort_by(|a, b| b.count.cmp(&a.count).then(b.last_seen.cmp(&a.last_seen)));
        q
    }

    fn pending_mut(&mut self, id: u64) -> Result<&mut Proposal, String> {
        let p = self
            .proposals
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("No proposal {}", id))?;
        if p.status != Status::Pending {
            return Err(format!("Proposal {} is already {:?}", id, p.status));
        }
        Ok(p)
    }

    pub fn approve(&mut self, id: u64, dict: &mut Vec<DictEntry>, log: &mut ChangeLog, now: u64) -> Result<LogEntry, String> {
        let p = self.pending_mut(id)?;
        // A case-only fix needs no misheard form: the term itself is matched
        // case-insensitively by the dictionary.
        let misheard = if p.original.to_lowercase() == p.corrected.to_lowercase() {
            None
        } else {
            Some(p.original.to_lowercase())
        };
        let effect = add_to_dictionary(dict, &p.corrected, misheard);
        p.status = Status::Approved;
        Ok(log.push(now, "approve", Some(id), effect, None))
    }

    pub fn reject(&mut self, id: u64, log: &mut ChangeLog, now: u64) -> Result<LogEntry, String> {
        let p = self.pending_mut(id)?;
        p.status = Status::Rejected;
        Ok(log.push(now, "reject", Some(id), Effect::NoChange, None))
    }
}

fn add_to_dictionary(dict: &mut Vec<DictEntry>, term: &str, misheard: Option<String>) -> Effect {
    if let Some(entry) = dict.iter_mut().find(|e| e.term == term) {
        if let Some(m) = misheard {
            if !entry.misheard.iter().any(|x| x.eq_ignore_ascii_case(&m)) {
                entry.misheard.push(m.clone());
                entry.enabled = true;
                return Effect::AddedMisheard { term: term.to_string(), misheard: m };
            }
        }
        if !entry.enabled {
            // Re-enabling is not tracked separately; treat as no change so
            // undo cannot disable something Ben enabled by hand.
            entry.enabled = true;
        }
        return Effect::NoChange;
    }
    let misheard: Vec<String> = misheard.into_iter().collect();
    dict.push(DictEntry { term: term.to_string(), misheard: misheard.clone(), enabled: true });
    Effect::CreatedEntry { term: term.to_string(), misheard }
}

/// Add a term by voice command ("Typr add Kangaroo to the dictionary").
pub fn command_add(dict: &mut Vec<DictEntry>, log: &mut ChangeLog, term: &str, now: u64) -> Result<LogEntry, String> {
    let term = term.trim();
    if term.is_empty() {
        return Err("Nothing to add".into());
    }
    let effect = add_to_dictionary(dict, term, None);
    if effect == Effect::NoChange {
        return Err(format!("{} is already in the dictionary", term));
    }
    Ok(log.push(now, "command-add", None, effect, None))
}

/// Remove a term by voice command. Matches the term case-insensitively.
pub fn command_remove(dict: &mut Vec<DictEntry>, log: &mut ChangeLog, term: &str, now: u64) -> Result<LogEntry, String> {
    let wanted = term.trim().to_lowercase();
    let index = dict
        .iter()
        .position(|e| e.term.to_lowercase() == wanted)
        .ok_or_else(|| format!("{} is not in the dictionary", term.trim()))?;
    let entry = dict.remove(index);
    Ok(log.push(now, "command-remove", None, Effect::RemovedEntry { index, entry }, None))
}

/// Revert the most recent dictionary change that has not been undone yet.
/// The revert is itself appended to the log.
pub fn undo_last(log: &mut ChangeLog, store: &mut LearnStore, dict: &mut Vec<DictEntry>, now: u64) -> Result<LogEntry, String> {
    let undone: Vec<u64> = log.entries.iter().filter_map(|e| e.undoes).collect();
    let target = log
        .entries
        .iter()
        .rev()
        .find(|e| e.action != "undo" && e.effect != Effect::NoChange && !undone.contains(&e.seq))
        .cloned()
        .ok_or("Nothing to undo")?;

    match &target.effect {
        Effect::CreatedEntry { term, .. } => {
            if let Some(i) = dict.iter().position(|e| &e.term == term) {
                dict.remove(i);
            }
        }
        Effect::AddedMisheard { term, misheard } => {
            if let Some(e) = dict.iter_mut().find(|e| &e.term == term) {
                e.misheard.retain(|m| !m.eq_ignore_ascii_case(misheard));
            }
        }
        Effect::RemovedEntry { index, entry } => {
            let at = (*index).min(dict.len());
            dict.insert(at, entry.clone());
        }
        Effect::NoChange => unreachable!(),
    }
    if let Some(pid) = target.proposal_id {
        if let Some(p) = store.proposals.iter_mut().find(|p| p.id == pid) {
            p.status = Status::Pending;
        }
    }
    Ok(log.push(now, "undo", target.proposal_id, target.effect.clone(), Some(target.seq)))
}

// ── Persistence ─────────────────────────────────────────────────────────────
//
// The queue lives in its own file, not config.json: the settings screen saves
// the whole Settings struct, and a queue in there would be clobbered by any
// save made from a screen that loaded before the proposal arrived.

pub const STORE_FILE: &str = "corrections.json";
pub const LOG_FILE: &str = "dictionary-changes.jsonl";

pub fn load_store(dir: &std::path::Path) -> LearnStore {
    std::fs::read_to_string(dir.join(STORE_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_store(dir: &std::path::Path, store: &LearnStore) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    // Write-then-rename so a crash mid-write cannot leave half a file.
    let tmp = dir.join(format!("{}.tmp", STORE_FILE));
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join(STORE_FILE)).map_err(|e| e.to_string())
}

/// Lines that fail to parse are skipped, not fatal: a torn last line from a
/// crash must not hide every change before it.
pub fn load_log(dir: &std::path::Path) -> ChangeLog {
    let entries = std::fs::read_to_string(dir.join(LOG_FILE))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<LogEntry>(l).ok())
        .collect();
    ChangeLog::from_entries(entries)
}

/// Append one entry. The file is only ever opened in append mode.
pub fn append_log(dir: &std::path::Path, entry: &LogEntry) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(LOG_FILE))
        .map_err(|e| e.to_string())?;
    let line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    writeln!(f, "{}", line).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subs(pasted: &str, after: &str) -> Vec<(String, String)> {
        extract(pasted, after).into_iter().map(|c| (c.original, c.corrected)).collect()
    }

    #[test]
    fn a_misheard_name_fixed_in_place_is_proposed() {
        let got = subs("check the rows in super bass before lunch", "check the rows in Supabase before lunch");
        assert_eq!(got, vec![("super bass".into(), "Supabase".into())]);
    }

    #[test]
    fn surrounding_document_text_is_ignored() {
        let got = subs(
            "push it to grok and wait",
            "Notes from today.\npush it to Groq and wait\nThen lunch.",
        );
        assert_eq!(got, vec![("grok".into(), "Groq".into())]);
    }

    #[test]
    fn shared_punctuation_is_not_part_of_the_correction() {
        let got = subs("we use grok, mostly for speed", "we use Groq, mostly for speed");
        assert_eq!(got, vec![("grok".into(), "Groq".into())]);
    }

    #[test]
    fn pure_additions_and_deletions_are_ignored() {
        assert!(extract("send the invoice to Mitch today", "send the invoice to Mitch today and call him").is_empty());
        assert!(extract("send the invoice to Mitch today. Also call him.", "send the invoice to Mitch today.").is_empty());
    }

    #[test]
    fn unchanged_or_undone_edits_produce_nothing() {
        assert!(extract("send the invoice to Mitch", "send the invoice to Mitch").is_empty());
    }

    #[test]
    fn a_wholesale_rewrite_is_ignored() {
        assert!(extract("send the invoice to Mitch today please", "email Sarah the quote tomorrow instead please").is_empty());
    }

    #[test]
    fn field_that_lost_the_paste_is_ignored() {
        assert!(extract("send the invoice to Mitch today", "completely different text here").is_empty());
    }

    #[test]
    fn content_changes_are_not_mishearings() {
        assert!(extract("meet on Tuesday at the office", "meet on Wednesday at the office").is_empty());
        assert!(extract("we need five more leads today", "we need six more leads today").is_empty());
        assert!(extract("the invoice is 250 dollars total", "the invoice is 260 dollars total").is_empty());
        assert!(extract("we ship in March this year", "we ship in May this year").is_empty());
    }

    #[test]
    fn finishing_a_word_is_typing_not_correcting() {
        assert!(extract("open the gro console now", "open the groq console now").is_empty());
    }

    #[test]
    fn long_phrase_swaps_are_ignored() {
        assert!(extract(
            "one two three four five six seven eight nine ten eleven twelve",
            "one two three alpha beta gamma delta eight nine ten eleven twelve",
        )
        .is_empty());
    }

    #[test]
    fn too_many_separate_edits_is_ambiguous() {
        let pasted = "a1 grok b1 b2 typer c1 c2 supa d1 d2 cloudflair e1 e2 e3 e4 e5 e6 e7 e8 e9 e10";
        let after = "a1 Groq b1 b2 Typr c1 c2 Supa d1 d2 Cloudflare e1 e2 e3 e4 e5 e6 e7 e8 e9 e10";
        assert!(extract(pasted, after).is_empty());
    }

    #[test]
    fn edge_edit_with_extra_words_is_ignored_but_like_for_like_is_kept() {
        // Last word replaced and more typed after: could be continuation.
        assert!(extract("I really like grok", "I really like Groq a lot").is_empty());
        // Last word swapped one for one: fine.
        assert_eq!(subs("I really like grok", "I really like Groq"), vec![("grok".into(), "Groq".into())]);
    }

    #[test]
    fn case_and_punctuation_only_are_flagged() {
        let c = extract("ask kangaroo about the leads", "ask Kangaroo about the leads");
        assert_eq!(c[0].kind, CandidateKind::CaseOnly);
        let c = extract("set up high-level today ok", "set up highlevel today ok");
        assert_eq!(c[0].kind, CandidateKind::PunctuationOnly);
    }

    #[test]
    fn short_pastes_have_no_anchor() {
        assert!(extract("grok", "Groq").is_empty());
    }

    fn cand(o: &str, c: &str, kind: CandidateKind) -> Candidate {
        Candidate { original: o.into(), corrected: c.into(), context: format!("... {} ...", o), kind }
    }

    #[test]
    fn proposals_accumulate_evidence_and_never_touch_the_dictionary() {
        let mut store = LearnStore::default();
        let dict: Vec<DictEntry> = vec![];
        store.observe(&[cand("mitchs", "Mitch's", CandidateKind::PunctuationOnly)], &dict, 10);
        store.observe(&[cand("tipr", "Typr", CandidateKind::Substitution)], &dict, 11);
        store.observe(&[cand("Tipr", "Typr", CandidateKind::Substitution)], &dict, 12);
        assert_eq!(store.proposals.len(), 2);
        let q = store.queue();
        // The style-only one is held back until it is consistent.
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].corrected, "Typr");
        assert_eq!(q[0].count, 2);
        assert_eq!(q[0].first_seen, 11);
        assert_eq!(q[0].last_seen, 12);
        assert!(dict.is_empty());
    }

    #[test]
    fn style_only_surfaces_once_consistent() {
        let mut store = LearnStore::default();
        for t in 0..STYLE_ONLY_THRESHOLD {
            store.observe(&[cand("kangaroo", "Kangaroo", CandidateKind::CaseOnly)], &[], t as u64);
        }
        assert_eq!(store.queue().len(), 1);
    }

    #[test]
    fn already_covered_by_dictionary_is_skipped() {
        let dict = vec![DictEntry { term: "Groq".into(), misheard: vec!["grok".into()], enabled: true }];
        let mut store = LearnStore::default();
        store.observe(&[cand("grok", "Groq", CandidateKind::Substitution)], &dict, 1);
        assert!(store.proposals.is_empty());
    }

    #[test]
    fn approve_reject_undo_round_trip() {
        let mut store = LearnStore::default();
        let mut dict = vec![DictEntry { term: "Typr".into(), misheard: vec!["typer".into()], enabled: true }];
        let mut log = ChangeLog::default();
        let ids = store.observe(
            &[
                cand("tipr", "Typr", CandidateKind::Substitution),
                cand("mich", "Mitch", CandidateKind::Substitution),
                cand("hilevel", "HighLevel", CandidateKind::Substitution),
            ],
            &dict,
            5,
        );

        // Existing term gets a new misheard form.
        store.approve(ids[0], &mut dict, &mut log, 6).unwrap();
        assert_eq!(dict[0].misheard, vec!["typer", "tipr"]);
        // New term gets a new entry.
        store.approve(ids[1], &mut dict, &mut log, 7).unwrap();
        assert_eq!(dict[1].term, "Mitch");
        assert_eq!(apply_dictionary(&dict, "call mich"), "call Mitch");
        // Reject is logged but changes nothing.
        store.reject(ids[2], &mut log, 8).unwrap();
        assert!(store.queue().is_empty());
        assert!(store.approve(ids[2], &mut dict, &mut log, 9).is_err(), "rejected cannot be approved");

        // Undo reverts the LAST approved change (Mitch), not the reject.
        let u = undo_last(&mut log, &mut store, &mut dict, 10).unwrap();
        assert_eq!(u.undoes, Some(2));
        assert_eq!(dict.len(), 1);
        assert_eq!(store.queue().len(), 1, "undone proposal goes back to the queue");
        // Undo again reverts the earlier one.
        undo_last(&mut log, &mut store, &mut dict, 11).unwrap();
        assert_eq!(dict[0].misheard, vec!["typer"]);
        assert!(undo_last(&mut log, &mut store, &mut dict, 12).is_err());

        // Append-only: 3 changes + 2 undos, nothing removed, seqs increase.
        let seqs: Vec<u64> = log.entries().iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn case_only_approval_adds_the_term_without_a_misheard_form() {
        let mut store = LearnStore::default();
        let mut dict = vec![];
        let mut log = ChangeLog::default();
        for t in 0..3 {
            store.observe(&[cand("kangaroo growth", "Kangaroo Growth", CandidateKind::CaseOnly)], &dict, t);
        }
        let id = store.queue()[0].id;
        store.approve(id, &mut dict, &mut log, 4).unwrap();
        assert!(dict[0].misheard.is_empty());
        assert_eq!(apply_dictionary(&dict, "ask kangaroo growth"), "ask Kangaroo Growth");
    }

    #[test]
    fn voice_command_add_and_remove_are_undoable() {
        let mut store = LearnStore::default();
        let mut dict = vec![DictEntry { term: "Groq".into(), misheard: vec!["grok".into()], enabled: true }];
        let mut log = ChangeLog::default();
        command_add(&mut dict, &mut log, "Simplicity", 1).unwrap();
        assert!(command_add(&mut dict, &mut log, "Simplicity", 2).is_err());
        command_remove(&mut dict, &mut log, "groq", 3).unwrap();
        assert_eq!(dict.len(), 1);
        undo_last(&mut log, &mut store, &mut dict, 4).unwrap();
        assert_eq!(dict[0].term, "Groq", "removed entry restored at its index");
        assert_eq!(dict[0].misheard, vec!["grok"]);
    }

    #[test]
    fn store_and_log_persist() {
        let dir = std::env::temp_dir().join(format!("typr_learn_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut store = LearnStore::default();
        store.observe(&[cand("tipr", "Typr", CandidateKind::Substitution)], &[], 1);
        save_store(&dir, &store).unwrap();
        assert_eq!(load_store(&dir), store);

        let mut log = ChangeLog::default();
        let mut dict = vec![];
        let e1 = store.approve(1, &mut dict, &mut log, 2).unwrap();
        append_log(&dir, &e1).unwrap();
        let e2 = undo_last(&mut log, &mut store, &mut dict, 3).unwrap();
        append_log(&dir, &e2).unwrap();
        // A torn last line must not hide the good ones.
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(dir.join(LOG_FILE)).unwrap();
        write!(f, "{{\"seq\":9,\"at").unwrap();
        let loaded = load_log(&dir);
        assert_eq!(loaded.entries(), log.entries());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
