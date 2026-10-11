#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use typr_lib::applog;
use typr_lib::audio;
use typr_lib::overlay;
use typr_lib::recorder::{Recorder, RecordingState};
use typr_lib::settings::Settings;
use typr_lib::supervisor;
use typr_lib::LockExt;

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
    state.settings.lock_safe().clone()
}

#[tauri::command]
fn save_settings(
    app: tauri::AppHandle,
    state: State<AppState>,
    settings: Settings,
) -> Result<(), String> {
    let previous = state.settings.lock_safe().clone();

    settings.save(&state.app_dir)?;
    *state.settings.lock_safe() = settings.clone();

    // Apply the side-effecting fields immediately. A setting that only lands
    // on the next launch reads as a broken toggle.
    if settings.autostart != previous.autostart {
        apply_autostart(&app, settings.autostart);
    }
    if settings.hotkey != previous.hotkey {
        rebind_hotkey(&app, &previous.hotkey, &settings.hotkey);
    }

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

/// Whether the OS actually has Typr registered to launch at login — the real
/// state, not what `config.json` claims.
#[tauri::command]
fn autostart_is_enabled(app: tauri::AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
async fn toggle_recording(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    do_toggle_recording(&app, &state).await
}

#[tauri::command]
fn cancel_recording(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.recorder.cancel_recording(&app)
}

fn apply_autostart(app: &tauri::AppHandle, enabled: bool) {
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    match result {
        Ok(_) => applog::log(&format!(
            "Autostart {} (OS reports enabled={})",
            if enabled { "enabled" } else { "disabled" },
            manager.is_enabled().unwrap_or(false)
        )),
        Err(e) => applog::log(&format!("Autostart change FAILED: {}", e)),
    }
}

/// Shared logic for the hotkey — always uses AI formatting.
async fn do_toggle_recording(
    app: &tauri::AppHandle,
    state: &AppState,
) -> Result<String, String> {
    let current_state = state.recorder.get_state();
    match current_state {
        RecordingState::Ready => {
            let mic = state.settings.lock_safe().microphone.clone();
            state.recorder.start_recording(app, &mic)?;
            Ok("recording".to_string())
        }
        RecordingState::Recording => {
            let settings = state.settings.lock_safe().clone();
            let result = state
                .recorder
                .stop_and_transcribe(app, &settings, &state.app_dir, true)
                .await?;
            Ok(result)
        }
        RecordingState::Transcribing => Err("Currently transcribing, please wait".to_string()),
    }
}

/// The hotkey handler, split out of `setup` so the watchdog and the settings
/// screen can both re-register it without duplicating the body.
fn register_hotkey(app: &tauri::AppHandle, accelerator: &str) -> Result<(), String> {
    let handle = app.clone();
    applog::log(&format!("Registering global shortcut: {}", accelerator));

    app.global_shortcut()
        .on_shortcut(accelerator, move |_app, shortcut, event| {
            applog::log(&format!(
                "Hotkey event: {:?} state={:?}",
                shortcut, event.state
            ));
            let handle = handle.clone();
            let state = handle.state::<AppState>();
            let mode = state.settings.lock_safe().recording_mode.clone();

            match event.state {
                ShortcutState::Pressed => {
                    tauri::async_runtime::spawn(async move {
                        let state = handle.state::<AppState>();
                        match mode.as_str() {
                            "toggle" => match do_toggle_recording(&handle, state.inner()).await {
                                Ok(result) => applog::log(&format!("Toggle result: {}", result)),
                                Err(e) => applog::log(&format!("Toggle error: {}", e)),
                            },
                            "push-to-talk" => {
                                let current = state.recorder.get_state();
                                if current == RecordingState::Ready {
                                    let mic = state.settings.lock_safe().microphone.clone();
                                    match state.recorder.start_recording(&handle, &mic) {
                                        Ok(_) => applog::log("Recording started"),
                                        Err(e) => {
                                            applog::log(&format!("Start recording error: {}", e))
                                        }
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
                                let settings = state.settings.lock_safe().clone();
                                match state
                                    .recorder
                                    .stop_and_transcribe(
                                        &handle,
                                        &settings,
                                        &state.app_dir,
                                        true,
                                    )
                                    .await
                                {
                                    Ok(result) => {
                                        applog::log(&format!("Transcription: {}", result))
                                    }
                                    Err(e) => {
                                        applog::log(&format!("Transcription error: {}", e))
                                    }
                                }
                            }
                        });
                    }
                }
            }
        })
        .map_err(|e| e.to_string())
}

fn rebind_hotkey(app: &tauri::AppHandle, old: &str, new: &str) {
    let _ = app.global_shortcut().unregister(old);
    match register_hotkey(app, new) {
        Ok(_) => applog::log(&format!("Hotkey rebound {} -> {}", old, new)),
        Err(e) => {
            applog::log(&format!("Rebind to {} failed ({}) — restoring {}", new, e, old));
            let _ = register_hotkey(app, old);
        }
    }
}

/// Re-register the hotkey if it ever stops being ours.
///
/// Windows can drop a global hotkey without telling the owner — another app
/// grabs the same combination, or the registration is lost across a session
/// switch, RDP reconnect or resume from sleep. The symptom is the worst kind:
/// Typr looks perfectly alive in the tray and the hotkey just does nothing,
/// which is half of "it stopped working". `typr.log` shows 10 startups where
/// registration failed outright and the app then sat there useless for hours.
fn spawn_hotkey_watchdog(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(30));

        let accelerator = {
            let state = app.state::<AppState>();
            let accelerator = state.settings.lock_safe().hotkey.clone();
            accelerator
        };

        let registered = app
            .global_shortcut()
            .is_registered(accelerator.as_str());

        if !registered {
            applog::log(&format!(
                "WATCHDOG: hotkey {} is no longer registered — re-registering",
                accelerator
            ));
            match register_hotkey(&app, &accelerator) {
                Ok(_) => applog::log("WATCHDOG: hotkey recovered"),
                Err(e) => applog::log(&format!("WATCHDOG: re-register failed: {}", e)),
            }
        }
    });
}

fn main() {
    let app_dir = get_app_dir();
    applog::init(&app_dir);
    // Replaces applog's logging-only hook with one that also relaunches Typr
    // when the panic is the tao event-loop teardown crash.
    supervisor::install(app_dir.clone());
    // If this process was started by a crashing predecessor, let it finish
    // dying before competing for the hotkey and the single-instance lock.
    supervisor::wait_for_predecessor();

    // Heartbeat — proves the process (and its message loop) is still alive
    // and pumping while idle, so a "stopped working" report can be told
    // apart from a hung/crashed process vs. an OS-level hotkey deregistration.
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(600));
        applog::log("heartbeat: process alive");
    });

    let settings = Settings::load(&app_dir);
    let initial_hotkey = settings.hotkey.clone();
    let want_autostart = settings.autostart;

    tauri::Builder::default()
        // Must be the first plugin registered. A second Typr cannot take the
        // hotkey (the log is full of "HotKey already registered" startups that
        // left a live-looking but inert app), so a duplicate launch now
        // surfaces the window that already works instead of adding a zombie.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            applog::log("Second instance attempted — focusing the existing window");
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
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
            toggle_recording,
            cancel_recording,
            autostart_is_enabled,
        ])
        // ── Hide main window to tray on close; overlay just hides itself ──
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                } else if window.label() == overlay::LABEL {
                    // Never let the pill be destroyed. A destroyed window is
                    // both a dead overlay for the rest of the session and a
                    // candidate trigger for tao's teardown panic.
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();

            // Build the recording pill up front so the first hotkey press has
            // nothing to wait for. `overlay::ensure` rebuilds it on demand if
            // it ever goes missing, and re-places it on every show.
            if overlay::ensure(&handle).is_some() {
                applog::log("Overlay window created");
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
                    "quit" => {
                        applog::log("Quit from tray menu — exiting on purpose");
                        std::process::exit(0)
                    }
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

            // Reconcile the saved preference with what the OS actually has
            // registered, every launch. An installer upgrade or a Windows
            // "startup apps" toggle can silently drop the entry, and the only
            // honest check is to ask the OS.
            let os_enabled = handle.autolaunch().is_enabled().unwrap_or(false);
            applog::log(&format!(
                "Autostart: setting={} os={}",
                want_autostart, os_enabled
            ));
            if want_autostart != os_enabled {
                apply_autostart(&handle, want_autostart);
            }

            match register_hotkey(&handle, &initial_hotkey) {
                Ok(_) => applog::log("Global shortcut registered successfully"),
                Err(e) => applog::log(&format!(
                    "ERROR: Failed to register global shortcut: {} — watchdog will retry",
                    e
                )),
            }
            spawn_hotkey_watchdog(handle.clone());

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
