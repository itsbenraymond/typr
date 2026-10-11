//! Keeps Typr alive across the crashes it cannot prevent.
//!
//! Typr is meant to idle in the tray indefinitely so the hotkey is always
//! there. In practice the process died 18 times in `typr.log` with:
//!
//! ```text
//! PANIC: panicked at tao-0.34.8/src/platform_impl/windows/event_loop/runner.rs:371:25:
//! cannot move state from Destroyed
//! ```
//!
//! That is a teardown race inside tao's Windows event loop — an OS message
//! arrives for a window whose runner state is already `Destroyed`. It fires
//! minutes after the last heartbeat with nobody touching the app, and it is
//! still present upstream, so there is no version to upgrade to that removes
//! it. Panicking on the event-loop thread unwinds the loop and the process
//! exits, which from Ben's seat is "the app closed itself again".
//!
//! We cannot fix tao from here. We can make the crash cost ~1 second instead
//! of a dead hotkey for the rest of the day: recognise the fatal class of
//! panic, relaunch the same executable, and exit. A rate limiter stops a
//! genuinely broken build from fork-bombing the machine.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::applog;

/// Panics whose text means "the event loop is gone, this process is finished".
/// Anything else (e.g. a one-off panic on the cpal audio thread) is survivable
/// — `LockExt::lock_safe` already recovers from the poisoned mutex it leaves —
/// so respawning on it would turn a recoverable blip into a restart loop.
const FATAL_PANIC_MARKERS: &[&str] = &[
    "cannot move state from",
    "event_loop/runner.rs",
    "event_loop\\runner.rs",
];

/// Env var carrying how many times we have already respawned in this chain.
const RESPAWN_DEPTH_VAR: &str = "TYPR_RESPAWN_DEPTH";
/// Beyond this many respawns inside `RESPAWN_WINDOW_SECS`, stop and stay dead
/// so the log is readable instead of a thousand identical restarts.
const MAX_RESPAWNS_IN_WINDOW: u32 = 5;
const RESPAWN_WINDOW_SECS: u64 = 120;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn respawn_ledger(app_dir: &Path) -> PathBuf {
    app_dir.join("respawns.txt")
}

/// Append `now` to the ledger, drop entries older than the window, and return
/// how many respawns (including this one) happened inside it.
fn record_respawn(app_dir: &Path) -> u32 {
    let path = respawn_ledger(app_dir);
    let now = now_secs();
    let mut stamps: Vec<u64> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse::<u64>().ok())
        .filter(|t| now.saturating_sub(*t) <= RESPAWN_WINDOW_SECS)
        .collect();
    stamps.push(now);
    let body = stamps
        .iter()
        .map(|t| t.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let _ = std::fs::write(&path, body);
    stamps.len() as u32
}

pub fn is_fatal_panic(text: &str) -> bool {
    FATAL_PANIC_MARKERS.iter().any(|m| text.contains(m))
}

/// Replaces the plain logging panic hook with one that also relaunches Typr
/// when the panic killed the event loop. Call once from `main`, after
/// `applog::init`.
pub fn install(app_dir: PathBuf) {
    let exe = std::env::current_exe().ok();

    std::panic::set_hook(Box::new(move |info| {
        let text = format!("{}", info);
        applog::log(&format!("PANIC: {}", text));

        if !is_fatal_panic(&text) {
            applog::log("PANIC is non-fatal (process stays up, mutex recovery handles it)");
            return;
        }

        let depth: u32 = std::env::var(RESPAWN_DEPTH_VAR)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let count = record_respawn(&app_dir);
        if count > MAX_RESPAWNS_IN_WINDOW {
            applog::log(&format!(
                "FATAL PANIC: {} respawns within {}s — giving up so this is diagnosable \
                 instead of looping. Start Typr by hand once the cause is fixed.",
                count, RESPAWN_WINDOW_SECS
            ));
            return;
        }

        match &exe {
            Some(path) => {
                applog::log(&format!(
                    "FATAL PANIC — respawning Typr (depth {} -> {}, {} respawns in last {}s)",
                    depth,
                    depth + 1,
                    count,
                    RESPAWN_WINDOW_SECS
                ));
                // The dying process still owns the global hotkey and the
                // single-instance lock, so the child would lose the race and
                // exit immediately. Hand it a short delay via a detached
                // waiter: spawn ourselves with a flag the new process uses to
                // wait for this PID to disappear before claiming anything.
                let mut cmd = std::process::Command::new(path);
                cmd.env(RESPAWN_DEPTH_VAR, (depth + 1).to_string());
                cmd.env("TYPR_WAIT_FOR_PID", std::process::id().to_string());
                match cmd.spawn() {
                    Ok(child) => applog::log(&format!("Respawned as PID {}", child.id())),
                    Err(e) => applog::log(&format!("Respawn FAILED: {}", e)),
                }
            }
            None => applog::log("FATAL PANIC — cannot respawn: current_exe() unavailable"),
        }

        // Do not let the unwind continue: tao's loop is already unusable and a
        // half-unwound event loop can leave the tray icon and the hotkey
        // registration behind, which is what produced the
        // "HotKey already registered" dead-app startups in the log.
        std::process::exit(42);
    }));
}

/// A respawned child is started by a parent that is still unwinding. Wait for
/// that PID to actually be gone (up to `timeout_ms`) before continuing, so the
/// child does not trip the single-instance guard or fail to take the hotkey.
pub fn wait_for_predecessor() {
    let Ok(pid_str) = std::env::var("TYPR_WAIT_FOR_PID") else {
        return;
    };
    let Ok(pid) = pid_str.parse::<u32>() else {
        return;
    };
    applog::log(&format!("Respawned: waiting for predecessor PID {} to exit", pid));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if !pid_is_alive(pid) {
            applog::log("Predecessor gone — continuing startup");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    applog::log("Predecessor still alive after 10s — continuing anyway");
}

#[cfg(target_os = "windows")]
fn pid_is_alive(pid: u32) -> bool {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn GetExitCodeProcess(handle: isize, code: *mut u32) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h == 0 {
            return false;
        }
        let mut code: u32 = 0;
        let ok = GetExitCodeProcess(h, &mut code);
        CloseHandle(h);
        ok != 0 && code == STILL_ACTIVE
    }
}

#[cfg(not(target_os = "windows"))]
fn pid_is_alive(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[test]
    fn classifies_the_tao_teardown_panic_as_fatal() {
        // The exact string from typr.log — the one that has been closing the app.
        let real = "panicked at C:\\Users\\Ben\\.cargo\\registry\\src\\index.crates.io-1949cf8c6b5b557f\\tao-0.34.8\\src\\platform_impl\\windows\\event_loop\\runner.rs:371:25:\ncannot move state from Destroyed";
        assert!(is_fatal_panic(real));
    }

    #[test]
    fn leaves_an_audio_thread_panic_survivable() {
        let audio = "panicked at src/audio.rs:88:17:\ncalled `Option::unwrap()` on a `None` value";
        assert!(!is_fatal_panic(audio));
    }

    #[test]
    fn rate_limiter_counts_only_inside_the_window() {
        let dir = temp_dir().join("typr_test_respawns");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // An old crash from last week must not count against today's budget.
        let stale = now_secs() - (RESPAWN_WINDOW_SECS + 60);
        std::fs::write(respawn_ledger(&dir), format!("{}", stale)).unwrap();

        assert_eq!(record_respawn(&dir), 1);
        assert_eq!(record_respawn(&dir), 2);
        assert_eq!(record_respawn(&dir), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rate_limiter_trips_after_the_cap() {
        let dir = temp_dir().join("typr_test_respawns_cap");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut last = 0;
        for _ in 0..(MAX_RESPAWNS_IN_WINDOW + 1) {
            last = record_respawn(&dir);
        }
        assert!(last > MAX_RESPAWNS_IN_WINDOW);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
