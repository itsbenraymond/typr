pub mod settings;
pub mod audio;
pub mod transcriber;
pub mod groq_adapter;
pub mod transcribe_groq;
pub mod cleanup;
pub mod format_groq;
pub mod paste;
pub mod recorder;
pub mod applog;
pub mod overlay;
pub mod supervisor;

/// A single panic anywhere while holding one of our Mutexes (most likely the
/// cpal audio-callback thread, which fires continuously during recording and
/// runs on an OS-managed real-time thread outside our control) poisons that
/// Mutex forever. Every later `.lock().unwrap()` on it then panics too — and
/// since release builds run under `windows_subsystem = "windows"` (no
/// console), that panic is invisible: the app looks alive in the tray but
/// the hotkey silently stops doing anything. Recovering the inner value
/// instead of propagating the poison keeps the app usable after a one-off
/// panic instead of bricking it until restart.
pub trait LockExt<T> {
    fn lock_safe(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockExt<T> for std::sync::Mutex<T> {
    fn lock_safe(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
