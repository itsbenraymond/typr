//! Every dictation, kept forever in a local SQLite file.
//!
//! Ben asked for this on 2026-11-22: see everything Typr has ever recorded,
//! and see what the AI cleanup changed, so the weekly audit can catch the
//! cleanup doing things he did not ask for.
//!
//! The file is `history.db` next to `config.json` and `typr.log`.
//!
//! Rules this module keeps:
//!
//! * **A history write never touches the paste.** The recorder pastes first
//!   and then hands the row to [`record_in_background`], which writes on its
//!   own thread. A locked, corrupt or missing database costs one line in
//!   `typr.log`, never a delayed or lost dictation.
//! * **Raw is the Whisper output, untouched.** `corrected_text` is after the
//!   personal dictionary, `final_text` is what was pasted. The diff Ben sees
//!   is raw against final, so it shows everything between his voice and the
//!   screen.
//! * **The log backfill runs once.** Old `Toggle result:` lines only carry the
//!   final text, so those rows have no raw transcript and are marked
//!   `source = 'log_backfill'`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::applog;

pub const DB_FILE: &str = "history.db";
pub const SOURCE_LIVE: &str = "live";
pub const SOURCE_BACKFILL: &str = "log_backfill";

const BACKFILL_DONE_KEY: &str = "log_backfill_done";

pub fn db_path(app_dir: &Path) -> PathBuf {
    app_dir.join(DB_FILE)
}

/// One dictation, as captured by the recorder.
#[derive(Debug, Clone, Default)]
pub struct NewDictation {
    /// Unix seconds.
    pub ts: i64,
    pub source: String,
    pub mode: Option<String>,
    pub app_exe: Option<String>,
    pub window_title: Option<String>,
    /// Whisper's output before anything else touched it. None for backfill.
    pub raw_text: Option<String>,
    /// After the personal dictionary, before the model.
    pub corrected_text: Option<String>,
    pub final_text: String,
    /// `openai/gpt-oss-20b`, `claude-haiku-5-5`, `fallback`, `snippet`, ...
    pub model: Option<String>,
    pub warning: Option<String>,
    pub audio_ms: Option<i64>,
    pub stt_ms: Option<i64>,
    pub cleanup_ms: Option<i64>,
    pub total_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Dictation {
    pub id: i64,
    pub ts: i64,
    pub source: String,
    pub mode: Option<String>,
    pub app_exe: Option<String>,
    pub window_title: Option<String>,
    pub raw_text: Option<String>,
    pub corrected_text: Option<String>,
    pub final_text: String,
    pub model: Option<String>,
    pub warning: Option<String>,
    pub audio_ms: Option<i64>,
    pub stt_ms: Option<i64>,
    pub cleanup_ms: Option<i64>,
    pub total_ms: Option<i64>,
    pub word_count: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub total: i64,
    pub total_words: i64,
    pub since_count: i64,
    pub since_words: i64,
}

pub fn word_count(text: &str) -> i64 {
    text.split_whitespace().count() as i64
}

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let conn = Connection::open(path)?;
    // The UI reads while the recorder writes. WAL lets both happen, and the
    // busy timeout turns a brief lock into a short wait instead of an error.
    conn.busy_timeout(Duration::from_millis(2000))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS dictations (
            id             INTEGER PRIMARY KEY,
            ts             INTEGER NOT NULL,
            source         TEXT    NOT NULL,
            mode           TEXT,
            app_exe        TEXT,
            window_title   TEXT,
            raw_text       TEXT,
            corrected_text TEXT,
            final_text     TEXT    NOT NULL,
            model          TEXT,
            warning        TEXT,
            audio_ms       INTEGER,
            stt_ms         INTEGER,
            cleanup_ms     INTEGER,
            total_ms       INTEGER,
            word_count     INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS dictations_ts ON dictations(ts);
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )
}

pub fn insert(conn: &Connection, d: &NewDictation) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO dictations (ts, source, mode, app_exe, window_title, raw_text,
            corrected_text, final_text, model, warning, audio_ms, stt_ms, cleanup_ms,
            total_ms, word_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            d.ts,
            d.source,
            d.mode,
            d.app_exe,
            d.window_title,
            d.raw_text,
            d.corrected_text,
            d.final_text,
            d.model,
            d.warning,
            d.audio_ms,
            d.stt_ms,
            d.cleanup_ms,
            d.total_ms,
            word_count(&d.final_text),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Newest first. `query` matches the final or the raw text, case-insensitive.
pub fn list(
    conn: &Connection,
    query: Option<&str>,
    limit: i64,
    offset: i64,
) -> rusqlite::Result<Vec<Dictation>> {
    let q = query.map(str::trim).filter(|q| !q.is_empty());
    let pattern = q.map(|q| {
        let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        format!("%{}%", escaped)
    });
    let mut stmt = conn.prepare(
        "SELECT id, ts, source, mode, app_exe, window_title, raw_text, corrected_text,
                final_text, model, warning, audio_ms, stt_ms, cleanup_ms, total_ms, word_count
         FROM dictations
         WHERE ?1 IS NULL
            OR final_text LIKE ?1 ESCAPE '\\'
            OR raw_text LIKE ?1 ESCAPE '\\'
         ORDER BY ts DESC, id DESC
         LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt.query_map(params![pattern, limit, offset], |r| {
        Ok(Dictation {
            id: r.get(0)?,
            ts: r.get(1)?,
            source: r.get(2)?,
            mode: r.get(3)?,
            app_exe: r.get(4)?,
            window_title: r.get(5)?,
            raw_text: r.get(6)?,
            corrected_text: r.get(7)?,
            final_text: r.get(8)?,
            model: r.get(9)?,
            warning: r.get(10)?,
            audio_ms: r.get(11)?,
            stt_ms: r.get(12)?,
            cleanup_ms: r.get(13)?,
            total_ms: r.get(14)?,
            word_count: r.get(15)?,
        })
    })?;
    rows.collect()
}

/// Totals, plus the count since `since_ts` (the UI passes the start of the
/// local week, which only the frontend knows cheaply).
pub fn stats(conn: &Connection, since_ts: i64) -> rusqlite::Result<Stats> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(word_count), 0),
                COALESCE(SUM(ts >= ?1), 0),
                COALESCE(SUM(CASE WHEN ts >= ?1 THEN word_count ELSE 0 END), 0)
         FROM dictations",
        params![since_ts],
        |r| {
            Ok(Stats {
                total: r.get(0)?,
                total_words: r.get(1)?,
                since_count: r.get(2)?,
                since_words: r.get(3)?,
            })
        },
    )
}

pub fn count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM dictations", [], |r| r.get(0))
}

fn meta_get(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0))
        .optional()
}

fn meta_set(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Write one dictation. Every failure is logged and swallowed.
pub fn record(app_dir: &Path, d: &NewDictation) -> bool {
    match open(&db_path(app_dir)).and_then(|c| insert(&c, d)) {
        Ok(_) => true,
        Err(e) => {
            applog::log(&format!("History write FAILED (dictation was still pasted): {}", e));
            false
        }
    }
}

/// Write on a thread of its own, so nothing about the database can hold up
/// the recorder. `on_done` runs after a successful write (the UI refresh).
pub fn record_in_background<F>(app_dir: PathBuf, d: NewDictation, on_done: F)
where
    F: FnOnce() + Send + 'static,
{
    std::thread::spawn(move || {
        if record(&app_dir, &d) {
            on_done();
        }
    });
}

// ───────────────────────────────────────────────────────────────────────────
// Backfill from typr.log
// ───────────────────────────────────────────────────────────────────────────

/// Pull every past dictation out of a typr.log body.
///
/// A dictation in the log is a `Toggle result: <text>` line (the start of a
/// recording is `Toggle result: recording`, skipped). The text can run over
/// several lines; continuation lines have no `[ts]` prefix. The mode, app and
/// window come from the last `Mode:` line since the previous result, and the
/// model from whatever failover line sits in between.
pub fn parse_log(body: &str) -> Vec<NewDictation> {
    let mut out = Vec::new();
    let mut mode: Option<(String, String, String)> = None;
    let mut model: Option<String> = None;
    let mut warning: Option<String> = None;
    // The dictation currently being read, with any continuation lines.
    let mut pending: Option<(i64, String)> = None;

    let finish = |pending: &mut Option<(i64, String)>,
                  mode: &mut Option<(String, String, String)>,
                  model: &mut Option<String>,
                  warning: &mut Option<String>,
                  out: &mut Vec<NewDictation>| {
        if let Some((ts, text)) = pending.take() {
            let text = text.trim().to_string();
            if !text.is_empty() {
                let (m, exe, title) = match mode.take() {
                    Some((m, e, t)) => (Some(m), Some(e), Some(t)),
                    None => (None, None, None),
                };
                out.push(NewDictation {
                    ts,
                    source: SOURCE_BACKFILL.into(),
                    mode: m,
                    app_exe: exe,
                    window_title: title,
                    final_text: text,
                    model: model.take(),
                    warning: warning.take(),
                    ..Default::default()
                });
            }
            *mode = None;
            *model = None;
            *warning = None;
        }
    };

    for line in body.lines() {
        let Some((ts, msg)) = split_log_line(line) else {
            // Continuation of a multi-line message.
            if let Some((_, text)) = pending.as_mut() {
                text.push('\n');
                text.push_str(line);
            }
            continue;
        };
        finish(&mut pending, &mut mode, &mut model, &mut warning, &mut out);

        if let Some(rest) = msg.strip_prefix("Toggle result: ") {
            if rest.trim() != "recording" {
                pending = Some((ts, rest.to_string()));
            }
        } else if let Some(rest) = msg.strip_prefix("Mode: ") {
            mode = parse_mode_line(rest);
        } else if msg.starts_with("Snippet fired") {
            model = Some("snippet".into());
        } else if let Some(rest) = msg.strip_prefix("Formatted with Claude fallback ") {
            model = rest.split_whitespace().next().map(str::to_string);
        } else if let Some(rest) = msg.strip_prefix("Formatted with fallback model ") {
            model = rest.split_whitespace().next().map(str::to_string);
        } else if msg.starts_with("Transform failed") || msg.starts_with("Groq format failed") {
            model = Some("fallback".into());
            warning = Some("AI cleanup failed - pasted uncleaned".into());
        }
    }
    finish(&mut pending, &mut mode, &mut model, &mut warning, &mut out);
    out
}

fn split_log_line(line: &str) -> Option<(i64, &str)> {
    let rest = line.strip_prefix('[')?;
    let close = rest.find("] ")?;
    let ts: i64 = rest[..close].parse().ok()?;
    Some((ts, &rest[close + 2..]))
}

/// `prompt (exe=code.exe title=Some window - VS Code)`
fn parse_mode_line(rest: &str) -> Option<(String, String, String)> {
    let (mode, tail) = rest.split_once(" (exe=")?;
    let tail = tail.strip_suffix(')').unwrap_or(tail);
    let (exe, title) = tail.split_once(" title=").unwrap_or((tail, ""));
    Some((mode.to_string(), exe.to_string(), title.to_string()))
}

/// Import typr.log once. Only lines older than the first live row are taken,
/// so a dictation already recorded live is never imported twice. Returns the
/// number of rows added (0 when it has already run).
pub fn backfill_from_log(conn: &mut Connection, log_body: &str) -> rusqlite::Result<usize> {
    if meta_get(conn, BACKFILL_DONE_KEY)?.is_some() {
        return Ok(0);
    }
    let first_live: Option<i64> = conn.query_row(
        "SELECT MIN(ts) FROM dictations WHERE source = ?1",
        params![SOURCE_LIVE],
        |r| r.get(0),
    )?;
    let rows: Vec<NewDictation> = parse_log(log_body)
        .into_iter()
        .filter(|d| first_live.map_or(true, |live| d.ts < live))
        .collect();

    let tx = conn.transaction()?;
    for d in &rows {
        insert(&tx, d)?;
    }
    meta_set(&tx, BACKFILL_DONE_KEY, &rows.len().to_string())?;
    tx.commit()?;
    Ok(rows.len())
}

/// Startup hook: open the database (creating it) and run the one-off log
/// import, on a background thread. Never blocks launch.
pub fn init_in_background(app_dir: PathBuf) {
    std::thread::spawn(move || {
        let result = (|| -> Result<(usize, i64), String> {
            let mut conn = open(&db_path(&app_dir)).map_err(|e| e.to_string())?;
            let log = std::fs::read(app_dir.join("typr.log")).unwrap_or_default();
            let added = backfill_from_log(&mut conn, &String::from_utf8_lossy(&log))
                .map_err(|e| e.to_string())?;
            Ok((added, count(&conn).map_err(|e| e.to_string())?))
        })();
        match result {
            Ok((added, total)) => {
                if added > 0 {
                    applog::log(&format!("History: imported {} past dictations from typr.log", added));
                }
                applog::log(&format!("History: {} dictations in {}", total, DB_FILE));
            }
            Err(e) => applog::log(&format!("History init FAILED (dictation unaffected): {}", e)),
        }
    });
}

// ───────────────────────────────────────────────────────────────────────────
// Word diff
// ───────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffKind {
    Same,
    Removed,
    Added,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffOp {
    pub kind: DiffKind,
    pub text: String,
}

/// The comparison key for a word: lowercase with punctuation stripped, so a
/// capital letter or a full stop is never shown as a change. Ben wants to see
/// words the cleanup removed or invented, not its punctuation.
pub fn norm_word(w: &str) -> String {
    w.chars()
        .filter(|c| c.is_alphanumeric() || *c == '\'')
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .trim_matches('\'')
        .to_string()
}

/// Word-level diff of `before` (what was said) against `after` (what was
/// pasted). Unchanged words show as they were pasted. Consecutive words of the
/// same kind are merged into one op.
pub fn word_diff(before: &str, after: &str) -> Vec<DiffOp> {
    let a: Vec<&str> = before.split_whitespace().collect();
    let b: Vec<&str> = after.split_whitespace().collect();
    let ka: Vec<String> = a.iter().map(|w| norm_word(w)).collect();
    let kb: Vec<String> = b.iter().map(|w| norm_word(w)).collect();
    let (n, m) = (a.len(), b.len());

    let mut ops: Vec<(DiffKind, &str)> = Vec::with_capacity(n + m);

    // A pure-punctuation token ("-", "...") has an empty key; never let two of
    // those count as the same word, or they anchor nonsense matches.
    let eq = |i: usize, j: usize| !ka[i].is_empty() && ka[i] == kb[j];

    // LCS table. Dictations top out around a thousand words, so the full
    // table is fine; past this size, show it as replaced wholesale rather
    // than stall the UI.
    if n.saturating_mul(m) > 4_000_000 {
        ops.extend(a.iter().map(|w| (DiffKind::Removed, *w)));
        ops.extend(b.iter().map(|w| (DiffKind::Added, *w)));
    } else {
        let w = m + 1;
        let mut t = vec![0u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[i * w + j] = if eq(i, j) {
                    t[(i + 1) * w + j + 1] + 1
                } else {
                    t[(i + 1) * w + j].max(t[i * w + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if eq(i, j) {
                ops.push((DiffKind::Same, b[j]));
                i += 1;
                j += 1;
            } else if t[(i + 1) * w + j] >= t[i * w + j + 1] {
                ops.push((DiffKind::Removed, a[i]));
                i += 1;
            } else {
                ops.push((DiffKind::Added, b[j]));
                j += 1;
            }
        }
        ops.extend(a[i..].iter().map(|w| (DiffKind::Removed, *w)));
        ops.extend(b[j..].iter().map(|w| (DiffKind::Added, *w)));
    }

    let mut merged: Vec<DiffOp> = Vec::new();
    for (kind, word) in ops {
        match merged.last_mut() {
            Some(last) if last.kind == kind => {
                last.text.push(' ');
                last.text.push_str(word);
            }
            _ => merged.push(DiffOp { kind, text: word.to_string() }),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "typr-history-test-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn live(ts: i64, raw: &str, fin: &str) -> NewDictation {
        NewDictation {
            ts,
            source: SOURCE_LIVE.into(),
            mode: Some("prompt".into()),
            app_exe: Some("code.exe".into()),
            window_title: Some("AI-OS - Visual Studio Code".into()),
            raw_text: Some(raw.into()),
            corrected_text: Some(raw.into()),
            final_text: fin.into(),
            model: Some("openai/gpt-oss-20b".into()),
            total_ms: Some(900),
            ..Default::default()
        }
    }

    #[test]
    fn a_dictation_round_trips_through_the_database() {
        let dir = temp_dir("roundtrip");
        let conn = open(&db_path(&dir)).unwrap();
        let id = insert(&conn, &live(100, "um so check the logs", "Check the logs.")).unwrap();

        let rows = list(&conn, None, 50, 0).unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.id, id);
        assert_eq!(r.raw_text.as_deref(), Some("um so check the logs"));
        assert_eq!(r.final_text, "Check the logs.");
        assert_eq!(r.model.as_deref(), Some("openai/gpt-oss-20b"));
        assert_eq!(r.app_exe.as_deref(), Some("code.exe"));
        assert_eq!(r.word_count, 3);
        assert_eq!(r.total_ms, Some(900));
        assert!(db_path(&dir).exists());
    }

    #[test]
    fn the_list_is_newest_first_searchable_and_paged() {
        let dir = temp_dir("list");
        let conn = open(&db_path(&dir)).unwrap();
        insert(&conn, &live(100, "first one", "First one.")).unwrap();
        insert(&conn, &live(300, "third one about supabase", "Third one about Supabase.")).unwrap();
        insert(&conn, &live(200, "second 50% one", "Second 50% one.")).unwrap();

        let all = list(&conn, None, 50, 0).unwrap();
        assert_eq!(all.iter().map(|d| d.ts).collect::<Vec<_>>(), vec![300, 200, 100]);

        let hit = list(&conn, Some("SUPABASE"), 50, 0).unwrap();
        assert_eq!(hit.len(), 1, "search is case-insensitive");
        assert_eq!(hit[0].ts, 300);

        // A literal % must not act as a wildcard.
        assert_eq!(list(&conn, Some("50%"), 50, 0).unwrap().len(), 1);
        assert_eq!(list(&conn, Some("%"), 50, 0).unwrap().len(), 1);
        // Blank search is no search.
        assert_eq!(list(&conn, Some("   "), 50, 0).unwrap().len(), 3);

        let page2 = list(&conn, None, 2, 2).unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].ts, 100);
    }

    #[test]
    fn stats_count_words_and_the_week() {
        let dir = temp_dir("stats");
        let conn = open(&db_path(&dir)).unwrap();
        insert(&conn, &live(100, "a", "One two three.")).unwrap();
        insert(&conn, &live(500, "b", "Four five.")).unwrap();
        let s = stats(&conn, 400).unwrap();
        assert_eq!(s, Stats { total: 2, total_words: 5, since_count: 1, since_words: 2 });

        let empty = open(&db_path(&temp_dir("stats-empty"))).unwrap();
        assert_eq!(stats(&empty, 0).unwrap().total_words, 0);
    }

    #[test]
    fn a_failed_write_is_swallowed_not_raised() {
        // The "app dir" is a file, so the database cannot be created.
        let dir = temp_dir("fail");
        let blocker = dir.join("not-a-dir");
        std::fs::write(&blocker, b"x").unwrap();
        assert!(!record(&blocker, &live(1, "a", "A.")));
    }

    #[test]
    fn record_writes_through_the_app_dir() {
        let dir = temp_dir("record");
        assert!(record(&dir, &live(1, "hello", "Hello.")));
        let conn = open(&db_path(&dir)).unwrap();
        assert_eq!(count(&conn).unwrap(), 1);
    }

    const LOG: &str = "\
[1795329477] Hotkey event: HotKey { mods: Modifiers(CONTROL | SHIFT), key: Space, id: 1 } state=Pressed
[1795329477] Toggle result: recording
[1795329512] Context: exe=code.exe title=Typr cleanup - AI-OS - Visual Studio Code
[1795329512] Mode: prompt (exe=code.exe title=Typr cleanup - AI-OS - Visual Studio Code)
[1795329512] Toggle result: I attached a photo of the account.

If you could set that up, it would be amazing.
[1795329534] Toggle result: recording
[1795329535] heartbeat: process alive
[1795329579] Mode: chat (exe=slack.exe title=Slack)
[1795329579] Format attempt failed on openai/gpt-oss-20b: HTTP 429: rate limit
[1795329580] Formatted with fallback model openai/gpt-oss-120b after: x
[1795329580] Toggle result: Send me that file.
[1792136369] Transform failed (All format models failed), using corrected transcript
[1792136370] Toggle result: .
[1788001984] Groq format failed: Groq error 404 Not Found: {\"error\":1}

[1788001984] Toggle result: Test test test.
";

    #[test]
    fn the_log_parser_finds_every_dictation_and_its_context() {
        let rows = parse_log(LOG);
        assert_eq!(rows.len(), 4, "{:#?}", rows);

        assert_eq!(rows[0].ts, 1795329512);
        assert_eq!(
            rows[0].final_text,
            "I attached a photo of the account.\n\nIf you could set that up, it would be amazing."
        );
        assert_eq!(rows[0].mode.as_deref(), Some("prompt"));
        assert_eq!(rows[0].app_exe.as_deref(), Some("code.exe"));
        assert_eq!(rows[0].window_title.as_deref(), Some("Typr cleanup - AI-OS - Visual Studio Code"));
        assert!(rows[0].raw_text.is_none(), "the log never had the raw transcript");
        assert_eq!(rows[0].source, SOURCE_BACKFILL);

        assert_eq!(rows[1].mode.as_deref(), Some("chat"));
        assert_eq!(rows[1].model.as_deref(), Some("openai/gpt-oss-120b"));

        assert_eq!(rows[2].final_text, ".");
        assert_eq!(rows[2].model.as_deref(), Some("fallback"));
        assert!(rows[2].mode.is_none(), "a mode line from an earlier dictation must not leak");

        assert_eq!(rows[3].final_text, "Test test test.");
        assert_eq!(rows[3].model.as_deref(), Some("fallback"));
    }

    #[test]
    fn the_backfill_runs_once_and_never_duplicates_live_rows() {
        let dir = temp_dir("backfill");
        let mut conn = open(&db_path(&dir)).unwrap();
        // A live row recorded at 1795329580: the log line at that second and
        // after is the same dictation, already captured.
        insert(&conn, &live(1795329580, "send me that file", "Send me that file.")).unwrap();

        let added = backfill_from_log(&mut conn, LOG).unwrap();
        assert_eq!(added, 3);
        assert_eq!(count(&conn).unwrap(), 4);

        assert_eq!(backfill_from_log(&mut conn, LOG).unwrap(), 0, "must only run once");
        assert_eq!(count(&conn).unwrap(), 4);
    }

    fn render(ops: &[DiffOp]) -> String {
        ops.iter()
            .map(|o| match o.kind {
                DiffKind::Same => o.text.clone(),
                DiffKind::Removed => format!("[-{}-]", o.text),
                DiffKind::Added => format!("{{+{}+}}", o.text),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn the_diff_strikes_fillers_and_ignores_punctuation_and_case() {
        let ops = word_diff("okay so um the header is too big", "The header is too big.");
        assert_eq!(render(&ops), "[-okay so um-] The header is too big.");
    }

    #[test]
    fn the_diff_shows_invented_words_as_added() {
        let ops = word_diff("check the logs", "Sure, I'll check the logs for you.");
        assert_eq!(render(&ops), "{+Sure, I'll+} check the logs {+for you.+}");
    }

    #[test]
    fn the_diff_shows_a_substitution_as_removed_then_added() {
        let ops = word_diff("check super base now", "Check Supabase now.");
        assert_eq!(render(&ops), "Check [-super base-] {+Supabase+} now.");
    }

    #[test]
    fn the_diff_handles_empty_sides() {
        assert!(word_diff("", "").is_empty());
        assert_eq!(render(&word_diff("", "Hello there.")), "{+Hello there.+}");
        assert_eq!(render(&word_diff("um uh", "")), "[-um uh-]");
    }

    #[test]
    fn identical_text_is_all_same() {
        let ops = word_diff("Ship it Thursday.", "ship it thursday");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].kind, DiffKind::Same);
    }

    #[test]
    fn norm_word_keeps_contractions() {
        assert_eq!(norm_word("I'll,"), "i'll");
        assert_eq!(norm_word("\"Okay.\""), "okay");
        assert_eq!(norm_word("--"), "");
    }
}
