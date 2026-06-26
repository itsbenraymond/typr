#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use typr_lib::audio;
use typr_lib::downloader;
use typr_lib::recorder::{Recorder, RecordingState};
use typr_lib::settings::Settings;
use typr_lib::transcribe_local;

struct AppState {
    recorder: Recorder,
    settings: Mutex<Settings>,
    app_dir: PathBuf,
}

fn get_app_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.typr.app")
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn save_settings(state: State<AppState>, settings: Settings) -> Result<(), String> {
    settings.save(&state.app_dir)?;
    *state.settings.lock().unwrap() = settings;
    Ok(())
}

#[tauri::command]
fn list_microphones() -> Vec<audio::MicDevice> {
    audio::list_microphones()
}

#[tauri::command]
fn get_recording_state(state: State<AppState>) -> RecordingState {
    state.recorder.get_state()
}

#[tauri::command]
fn check_model_downloaded(state: State<AppState>, model_size: String) -> bool {
    let model_file = transcribe_local::model_filename(&model_size);
    state.app_dir.join(&model_file).exists()
}

#[tauri::command]
async fn download_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    model_size: String,
) -> Result<(), String> {
    let url = transcribe_local::model_download_url(&model_size);
    let model_file = transcribe_local::model_filename(&model_size);
    let dest = state.app_dir.join(&model_file);
    downloader::download_model(app, &url, &dest).await
}

#[tauri::command]
async fn toggle_recording(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    do_toggle_recording(&app, &state).await
}

#[tauri::command]
fn cancel_recording(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.recorder.cancel_recording(&app)
}

/// Shared logic for the hotkey — always uses AI formatting.
async fn do_toggle_recording(
    app: &tauri::AppHandle,
    state: &AppState,
) -> Result<String, String> {
    let current_state = state.recorder.get_state();
    match current_state {
        RecordingState::Ready => {
            let mic = state.settings.lock().unwrap().microphone.clone();
            state.recorder.start_recording(app, &mic)?;
            Ok("recording".to_string())
        }
        RecordingState::Recording => {
            let settings = state.settings.lock().unwrap().clone();
            let result = state
                .recorder
                .stop_and_transcribe(app, &settings, &state.app_dir, true)
                .await?;
            Ok(result)
        }
        RecordingState::Transcribing => {
            Err("Currently transcribing, please wait".to_string())
        }
    }
}

fn main() {
    let app_dir = get_app_dir();
    let settings = Settings::load(&app_dir);
    let initial_hotkey = settings.hotkey.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .manage(AppState {
            recorder: Recorder::new(),
            settings: Mutex::new(settings),
            app_dir,
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            list_microphones,
            get_recording_state,
            check_model_downloaded,
            download_model,
            toggle_recording,
            cancel_recording,
        ])
        // ── Hide main window to tray on close; overlay just hides itself ──
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(move |app| {
            // Create the overlay window (recording bar, bottom-center, always on top, hidden until recording)
            let monitor = app.primary_monitor().ok().flatten();
            let overlay_w = 280.0_f64;
            let overlay_h = 64.0_f64;
            let bottom_margin = 32.0_f64;
            let (x, y) = if let Some(m) = monitor {
                let size = m.size();
                let scale = m.scale_factor();
                let logical_w = size.width as f64 / scale;
                let logical_h = size.height as f64 / scale;
                (
                    ((logical_w - overlay_w) / 2.0) as i32,
                    (logical_h - overlay_h - bottom_margin) as i32,
                )
            } else {
                (640, 700)
            };

            let overlay = WebviewWindowBuilder::new(
                app,
                "overlay",
                WebviewUrl::App("src/overlay.html".into()),
            )
            .title("")
            .inner_size(overlay_w, overlay_h)
            .position(x as f64, y as f64)
            .resizable(false)
            .decorations(false)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .focused(false)
            .shadow(false)
            .visible(false)
            .build();

            match overlay {
                Ok(_) => println!("[Typr] Overlay window created"),
                Err(e) => eprintln!("[Typr] Failed to create overlay: {}", e),
            }

            // ── System tray ──────────────────────────────────────────────────
            let show_item = MenuItem::with_id(app, "show", "Show Settings", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit Typr", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Typr — dictation active")
                .menu(&tray_menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => std::process::exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.hide();
                            } else {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            let handle = app.handle().clone();

            println!("[Typr] Registering global shortcut: {}", initial_hotkey);

            match app.global_shortcut().on_shortcut(
                initial_hotkey.as_str(),
                move |_app, shortcut, event| {
                    println!("[Typr] Hotkey event: {:?} state={:?}", shortcut, event.state);
                    let handle = handle.clone();
                    let state = handle.state::<AppState>();
                    let mode = state.settings.lock().unwrap().recording_mode.clone();
                    println!("[Typr] Recording mode: {}", mode);

                    match event.state {
                        ShortcutState::Pressed => {
                            tauri::async_runtime::spawn(async move {
                                let state = handle.state::<AppState>();
                                match mode.as_str() {
                                    "toggle" => {
                                        println!("[Typr] Toggle mode: calling do_toggle_recording");
                                        match do_toggle_recording(&handle, state.inner()).await {
                                            Ok(result) => println!("[Typr] Toggle result: {}", result),
                                            Err(e) => eprintln!("[Typr] Toggle error: {}", e),
                                        }
                                    }
                                    "push-to-talk" => {
                                        let current = state.recorder.get_state();
                                        println!("[Typr] PTT mode, current state: {:?}", current);
                                        if current == RecordingState::Ready {
                                            let mic = state
                                                .settings
                                                .lock()
                                                .unwrap()
                                                .microphone
                                                .clone();
                                            match state.recorder.start_recording(&handle, &mic) {
                                                Ok(_) => println!("[Typr] Recording started"),
                                                Err(e) => eprintln!("[Typr] Start recording error: {}", e),
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            });
                        }
                        ShortcutState::Released => {
                            if mode == "push-to-talk" {
                                tauri::async_runtime::spawn(async move {
                                    let state = handle.state::<AppState>();
                                    let current = state.recorder.get_state();
                                    if current == RecordingState::Recording {
                                        let settings =
                                            state.settings.lock().unwrap().clone();
                                        match state.recorder.stop_and_transcribe(
                                            &handle,
                                            &settings,
                                            &state.app_dir,
                                            true,
                                        ).await {
                                            Ok(result) => println!("[Typr] Transcription: {}", result),
                                            Err(e) => eprintln!("[Typr] Transcription error: {}", e),
                                        }
                                    }
                                });
                            }
                        }
                    }
                },
            ) {
                Ok(_) => println!("[Typr] Global shortcut registered successfully"),
                Err(e) => eprintln!("[Typr] ERROR: Failed to register global shortcut: {}", e),
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}