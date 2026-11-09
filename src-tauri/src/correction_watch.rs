//! Glue between the paste path, the learning engine (`learn`) and storage.
//!
//! No Tauri types in here on purpose, so all of it tests on Linux. The Tauri
//! commands in `main.rs` and the recorder are thin callers.
//!
//! ## The one missing piece: reading the target field
//!
//! [`read_field_text`] is where the platform reports what the field Typr
//! pasted into holds at the end of the watch window. It returns `None` on
//! every platform today, and while it does, [`spawn_watch`] does nothing at
//! all (no thread, no timer). Nothing in the current code can observe another
//! app's text field: `paste.rs` only has the clipboard and the foreground
//! HWND.
//!
//! What BensPC needs to fill it in (Windows, UI Automation):
//! 1. `CoCreateInstance(CUIAutomation)`, then `ElementFromHandle(hwnd)` or
//!    better `GetFocusedElement()` captured right after the paste, since the
//!    HWND of a browser or Electron app covers the whole window.
//! 2. Read `UIA_ValuePatternId` (`CurrentValue`) for single-line and most
//!    multiline edits, else `UIA_TextPatternId` -> `DocumentRange.GetText(-1)`.
//! 3. Return `None` for password fields (`CurrentIsPassword`) and when the
//!    element is gone. Never fall back to Ctrl+A / Ctrl+C: that would select
//!    and overwrite Ben's clipboard inside his live document.
//!
//! That is a `windows` crate dependency (features `Win32_UI_Accessibility`,
//! `Win32_System_Com`) and has to be built and tried on BensPC.

use std::path::Path;
use std::sync::Mutex;

use crate::learn::{self, ChangeLog, LearnStore, LogEntry, Proposal};
use crate::settings::Settings;

/// Serialises every read-modify-write of the queue and the change log.
static FILES: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    FILES.lock().unwrap_or_else(|p| p.into_inner())
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What the focused field holds now. See the module docs: not implemented
/// on any platform yet.
pub fn read_field_text(_hwnd: usize) -> Option<String> {
    None
}

/// Whether the platform can read a field at all. While false, the watch is
/// never scheduled, so the feature costs nothing until the hook exists.
pub fn reader_available() -> bool {
    false
}

/// Diff a paste against the field and file any proposals. Returns how many
/// proposals were created or bumped.
pub fn record(app_dir: &Path, pasted: &str, field_after: &str, settings: &Settings) -> Result<usize, String> {
    let candidates = learn::extract(pasted, field_after);
    if candidates.is_empty() {
        return Ok(0);
    }
    let _g = lock();
    let mut store = learn::load_store(app_dir);
    let touched = store.observe(&candidates, &settings.dictionary, now());
    learn::save_store(app_dir, &store)?;
    Ok(touched.len())
}

/// Wait out the window, read the field, record. Fire and forget.
pub fn spawn_watch(app_dir: std::path::PathBuf, pasted: String, hwnd: usize, secs: u64) {
    if !reader_available() || pasted.trim().is_empty() {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(secs));
        let Some(after) = read_field_text(hwnd) else { return };
        // Fresh from disk: the dictionary may have changed in the window.
        let settings = Settings::load(&app_dir.to_path_buf());
        match record(&app_dir, &pasted, &after, &settings) {
            Ok(0) => {}
            Ok(n) => crate::applog::log(&format!("Correction watch: {} proposal(s) filed", n)),
            Err(e) => crate::applog::log(&format!("Correction watch failed to save: {}", e)),
        }
    });
}

pub fn queue(app_dir: &Path) -> Vec<Proposal> {
    let _g = lock();
    learn::load_store(app_dir).queue()
}

pub fn change_log(app_dir: &Path) -> Vec<LogEntry> {
    let _g = lock();
    learn::load_log(app_dir).entries().to_vec()
}

/// Apply a dictionary change to `settings`, persist config, queue and log.
///
/// Config is saved first because it is the change that matters; the log line
/// is written only once the dictionary on disk really has the change.
fn commit(
    app_dir: &Path,
    settings: &mut Settings,
    f: impl FnOnce(&mut LearnStore, &mut Vec<crate::vocab::DictEntry>, &mut ChangeLog, u64) -> Result<LogEntry, String>,
) -> Result<LogEntry, String> {
    let _g = lock();
    let mut store = learn::load_store(app_dir);
    let mut log = learn::load_log(app_dir);
    let mut dict = settings.dictionary.clone();
    let entry = f(&mut store, &mut dict, &mut log, now())?;

    let before = std::mem::replace(&mut settings.dictionary, dict);
    if let Err(e) = settings.save(&app_dir.to_path_buf()) {
        settings.dictionary = before;
        return Err(e);
    }
    learn::save_store(app_dir, &store)?;
    learn::append_log(app_dir, &entry)?;
    Ok(entry)
}

pub fn approve(app_dir: &Path, settings: &mut Settings, id: u64) -> Result<LogEntry, String> {
    commit(app_dir, settings, |store, dict, log, now| store.approve(id, dict, log, now))
}

pub fn reject(app_dir: &Path, id: u64) -> Result<LogEntry, String> {
    let _g = lock();
    let mut store = learn::load_store(app_dir);
    let mut log = learn::load_log(app_dir);
    let entry = store.reject(id, &mut log, now())?;
    learn::save_store(app_dir, &store)?;
    learn::append_log(app_dir, &entry)?;
    Ok(entry)
}

pub fn undo(app_dir: &Path, settings: &mut Settings) -> Result<LogEntry, String> {
    commit(app_dir, settings, |store, dict, log, now| learn::undo_last(log, store, dict, now))
}

pub fn add_term(app_dir: &Path, settings: &mut Settings, term: &str) -> Result<LogEntry, String> {
    commit(app_dir, settings, |_, dict, log, now| learn::command_add(dict, log, term, now))
}

pub fn remove_term(app_dir: &Path, settings: &mut Settings, term: &str) -> Result<LogEntry, String> {
    commit(app_dir, settings, |_, dict, log, now| learn::command_remove(dict, log, term, now))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("typr_cw_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn watch_is_inert_until_a_reader_exists() {
        assert!(!reader_available());
        assert_eq!(read_field_text(1234), None);
    }

    #[test]
    fn end_to_end_record_approve_undo_persists_everything() {
        let d = dir("e2e");
        let mut s = Settings::default();
        s.save(&d).unwrap();

        let n = record(&d, "set up the workflow in servis m8 today", "set up the workflow in ServiceM8 today", &s).unwrap();
        assert_eq!(n, 1);
        let q = queue(&d);
        assert_eq!(q.len(), 1);
        assert_eq!((q[0].original.as_str(), q[0].corrected.as_str()), ("servis m8", "ServiceM8"));

        approve(&d, &mut s, q[0].id).unwrap();
        assert!(queue(&d).is_empty());
        // In memory and on disk.
        assert!(s.dictionary.iter().any(|e| e.term == "ServiceM8"));
        assert!(Settings::load(&d).dictionary.iter().any(|e| e.term == "ServiceM8"));
        assert_eq!(crate::vocab::apply_dictionary(&s.dictionary, "open servis m8"), "open ServiceM8");

        undo(&d, &mut s).unwrap();
        assert!(!Settings::load(&d).dictionary.iter().any(|e| e.term == "ServiceM8"));
        assert_eq!(queue(&d).len(), 1, "undone proposal is reviewable again");

        let log = change_log(&d);
        assert_eq!(log.iter().map(|e| e.action.as_str()).collect::<Vec<_>>(), vec!["approve", "undo"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn reject_keeps_the_dictionary_and_logs_it() {
        let d = dir("reject");
        let s = Settings::default();
        record(&d, "please check mitchs reviews today", "please check Mitchs reviews today", &s).unwrap();
        // Case-only: not reviewable yet, so nothing to reject from the queue.
        assert!(queue(&d).is_empty());
        record(&d, "call the plumer about the quote", "call the plumber about the quote", &s).unwrap();
        // "plumer" -> "plumber" is a prefix-free spelling fix, so it queues.
        let q = queue(&d);
        assert_eq!(q.len(), 1);
        reject(&d, q[0].id).unwrap();
        assert!(queue(&d).is_empty());
        assert_eq!(change_log(&d)[0].action, "reject");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn voice_add_then_undo() {
        let d = dir("voice");
        let mut s = Settings::default();
        s.save(&d).unwrap();
        add_term(&d, &mut s, "Simplicity").unwrap();
        assert!(Settings::load(&d).dictionary.iter().any(|e| e.term == "Simplicity"));
        undo(&d, &mut s).unwrap();
        assert!(!s.dictionary.iter().any(|e| e.term == "Simplicity"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
