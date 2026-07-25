pub mod settings;
pub mod audio;
pub mod transcriber;
pub mod groq_adapter;
pub mod transcribe_groq;
pub mod cleanup;
pub mod format_groq;
pub mod paste;
pub mod recorder;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
