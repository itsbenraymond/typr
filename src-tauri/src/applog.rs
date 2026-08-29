use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Release builds run under `windows_subsystem = "windows"`, so there is no
/// console attached and every `println!`/`eprintln!` call is silently
/// discarded. Without this, a panic or a stuck hotkey handler leaves zero
/// evidence to diagnose from. Call once from `main()` before anything else.
pub fn init(app_dir: &Path) {
    let _ = std::fs::create_dir_all(app_dir);
    let path = app_dir.join("typr.log");
    let _ = LOG_PATH.set(path);

    std::panic::set_hook(Box::new(|info| {
        log(&format!("PANIC: {}", info));
    }));

    log("--- Typr started ---");
}

fn timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Unix seconds is enough to correlate "how long was it open before this line" —
    // human-readable formatting isn't worth pulling in a chrono dependency for a debug log.
    format!("{}", secs)
}

pub fn log(msg: &str) {
    let Some(path) = LOG_PATH.get() else { return };
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "[{}] {}", timestamp(), msg);
    }
}
