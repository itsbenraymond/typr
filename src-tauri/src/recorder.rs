use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

use crate::audio::AudioRecorder;
use crate::cleanup::cleanup_text;
use crate::paste::{get_focused_hwnd, paste_text};
use crate::settings::Settings;
use crate::LockExt;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum RecordingState {
    Ready,
    Recording,
    Transcribing,
}

fn update_overlay(app: &AppHandle, state: &RecordingState) {
    match state {
        RecordingState::Ready => crate::overlay::hide(app),
        RecordingState::Recording => crate::overlay::show_state(app, "recording"),
        RecordingState::Transcribing => crate::overlay::show_state(app, "transcribing"),
    }
}

pub struct Recorder {
    state: Arc<Mutex<RecordingState>>,
    audio_recorder: Arc<Mutex<AudioRecorder>>,
    focused_hwnd: Arc<Mutex<usize>>,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RecordingState::Ready)),
            audio_recorder: Arc::new(Mutex::new(AudioRecorder::new())),
            focused_hwnd: Arc::new(Mutex::new(0)),
        }
    }

    pub fn get_state(&self) -> RecordingState {
        self.state.lock_safe().clone()
    }

    pub fn cancel_recording(&self, app: &AppHandle) -> Result<(), String> {
        let mut state = self.state.lock_safe();
        if *state != RecordingState::Recording {
            return Err("Not currently recording".to_string());
        }

        {
            let mut recorder = self.audio_recorder.lock_safe();
            recorder.cancel();
        }

        *state = RecordingState::Ready;
        let _ = app.emit("recording-state", RecordingState::Ready);
        update_overlay(app, &RecordingState::Ready);
        Ok(())
    }

    pub fn start_recording(&self, app: &AppHandle, mic_name: &str) -> Result<(), String> {
        let mut state = self.state.lock_safe();
        if *state != RecordingState::Ready {
            return Err("Already recording or transcribing".to_string());
        }

        // Snapshot cursor position now, before the overlay shows, so we know
        // which input field the user had focused when they triggered the hotkey.
        *self.focused_hwnd.lock_safe() = get_focused_hwnd();

        let level_arc = {
            let mut recorder = self.audio_recorder.lock_safe();
            recorder.start(mic_name)?;
            recorder.level.clone()
        };

        *state = RecordingState::Recording;
        // Open the connection to Groq while Ben is still talking, so the
        // transcription request does not wait on a TLS handshake.
        crate::http::prewarm();
        let _ = app.emit("recording-state", RecordingState::Recording);
        update_overlay(app, &RecordingState::Recording);
        drop(state);

        let app_clone = app.clone();
        let state_arc = self.state.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(33));
            let s = state_arc.lock_safe().clone();
            if s != RecordingState::Recording {
                break;
            }
            let level = *level_arc.lock_safe();
            if let Some(overlay) = crate::overlay::ensure(&app_clone) {
                let js = format!(
                    "window.__pushLevel && window.__pushLevel({:.5});",
                    level
                );
                let _ = overlay.eval(&js);
            }
        });

        Ok(())
    }

    /// `settings_store` is the app's live settings. It is read once up front,
    /// and locked again only if a voice command changes the dictionary.
    pub async fn stop_and_transcribe(
        &self,
        app: &AppHandle,
        settings_store: &Mutex<Settings>,
        app_dir: &PathBuf,
        format_mode: bool,
    ) -> Result<String, String> {
        let settings_snapshot = settings_store.lock_safe().clone();
        let settings = &settings_snapshot;
        let started = std::time::Instant::now();
        // Stop recording
        {
            let mut state = self.state.lock_safe();
            if *state != RecordingState::Recording {
                return Err("Not currently recording".to_string());
            }
            *state = RecordingState::Transcribing;
            let _ = app.emit("recording-state", RecordingState::Transcribing);
            update_overlay(app, &RecordingState::Transcribing);
        }

        // Anything the user needs told about that is not a hard failure —
        // e.g. the AI cleanup fell over and we pasted the plainer fallback.
        // Collected here because the pipeline closure returns only the text,
        // and the pill can only be flashed after state is back to Ready.
        let soft_warning: Mutex<Option<String>> = Mutex::new(None);

        // Run the whole pipeline in a closure so we can guarantee state is
        // reset to Ready afterwards — whether the pipeline succeeds or fails.
        // Previously any early-return Err left state stuck at Transcribing,
        // silently swallowing all subsequent hotkey presses.
        let result: Result<String, String> = async {
            let temp_path = app_dir.join("temp_recording.wav");

            // Save audio
            {
                let mut recorder = self.audio_recorder.lock_safe();
                recorder.stop_and_save(&temp_path)?;
            }

            // Transcribe via the Transcriber seam (Groq-backed by default).
            let transcriber = crate::groq_adapter::default_transcriber();
            let opts = crate::transcriber::TranscribeOpts {
                api_key: settings.groq_api_key.clone(),
                language: "en".to_string(),
            };
            let t_saved = started.elapsed().as_millis();
            let raw_text = transcriber.transcribe(&temp_path, &opts).await?;
            let t_transcribed = started.elapsed().as_millis();

            // Cleanup temp file
            let _ = std::fs::remove_file(&temp_path);

            // "Typr, add X to the dictionary" and friends. A command Typr
            // cannot carry out returns None and is dictated as normal.
            if format_mode && settings.command_prefix {
                let spoken = crate::cleanup::strip_noise_only(&raw_text);
                if let Some(message) = run_voice_command(app, settings_store, app_dir, &spoken) {
                    *soft_warning.lock_safe() = Some(message);
                    return Ok(String::new());
                }
            }

            // Clean up / format.
            //
            // The destination app is read from the window handle captured
            // before the pill appeared, so the mode is chosen for wherever
            // the text is actually going, not for whatever has focus now.
            let cleaned = if format_mode {
                let noise_cleaned = crate::cleanup::strip_noise_only(&raw_text);
                let ctx = crate::appctx::for_hwnd(*self.focused_hwnd.lock_safe());
                let result =
                    crate::transform::transform(settings, &noise_cleaned, &ctx).await;

                if let Some(warning) = result.warning {
                    // For weeks the failure branch was taken on every single
                    // dictation (the configured model had been decommissioned)
                    // and nothing on screen said so, so raw transcripts looked
                    // like the intended output.
                    let _ = app.emit("format-error", warning.clone());
                    *soft_warning.lock_safe() = Some(warning);
                }
                result.text
            } else {
                cleanup_text(&raw_text)
            };

            // Clear format-mode attribute on overlay
            if let Some(overlay) = app.get_webview_window(crate::overlay::LABEL) {
                let _ = overlay.eval(
                    "document.getElementById('bar')?.removeAttribute('data-format-mode');"
                );
            }

            // Auto-paste — re-click the original cursor position so the correct
            // input field has focus before Ctrl+V fires.
            let t_transformed = started.elapsed().as_millis();
            if !cleaned.is_empty() {
                let saved_hwnd = *self.focused_hwnd.lock_safe();
                paste_text(&cleaned, saved_hwnd)?;
                if settings.correction_watch {
                    crate::correction_watch::spawn_watch(
                        app_dir.clone(),
                        cleaned.clone(),
                        saved_hwnd,
                        settings.correction_watch_secs,
                    );
                }
            }
            // Where the time between "stop" and "pasted" goes. Read these
            // lines in typr.log on BensPC; see docs/paste-latency.md.
            crate::applog::log(&format!(
                "Timing: wav={}ms transcribe={}ms transform={}ms paste={}ms total={}ms chars={}",
                t_saved,
                t_transcribed - t_saved,
                t_transformed - t_transcribed,
                started.elapsed().as_millis() - t_transformed,
                started.elapsed().as_millis(),
                cleaned.len()
            ));

            Ok(cleaned)
        }
        .await;

        // Always reset state to Ready — even if the pipeline errored.
        // This is the fix: previously only the success path reset state,
        // leaving it permanently stuck at Transcribing on any failure.
        {
            let mut state = self.state.lock_safe();
            *state = RecordingState::Ready;
            let _ = app.emit("recording-state", RecordingState::Ready);
            update_overlay(app, &RecordingState::Ready);
        }

        match &result {
            Err(e) => {
                crate::applog::log(&format!(
                    "Transcription pipeline error (state reset to Ready): {}",
                    e
                ));
                let _ = app.emit("transcription-error", e.clone());
                crate::overlay::flash_error(app, e);
            }
            Ok(_) => {
                if let Some(warning) = soft_warning.lock_safe().take() {
                    crate::overlay::flash_error(app, &warning);
                }
            }
        }

        result
    }
}

/// Carry out a "Typr, ..." command. Returns the message to show on the pill,
/// or None when the utterance is not a command Typr can act on (the caller
/// then dictates it, so a false positive never loses what was said).
fn run_voice_command(
    app: &AppHandle,
    settings_store: &Mutex<Settings>,
    app_dir: &PathBuf,
    spoken: &str,
) -> Option<String> {
    use crate::command::{classify, parse_action, Action, Intent};

    let Intent::Command(cmd) = classify(spoken) else {
        return None;
    };
    let action = parse_action(&cmd);
    crate::applog::log(&format!("Voice command {:?} -> {:?}", spoken, action));

    let result: Result<String, String> = match &action {
        Action::Unsupported => return None,
        Action::ShowSettings => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            Ok("Opened Typr".to_string())
        }
        Action::UndoLastChange => {
            let mut s = settings_store.lock_safe();
            crate::correction_watch::undo(app_dir, &mut s).map(|_| "Undid last dictionary change".to_string())
        }
        Action::AddWord(term) => {
            let mut s = settings_store.lock_safe();
            crate::correction_watch::add_term(app_dir, &mut s, term).map(|_| format!("Added {} to dictionary", term))
        }
        Action::RemoveWord(term) => {
            let mut s = settings_store.lock_safe();
            crate::correction_watch::remove_term(app_dir, &mut s, term)
                .map(|_| format!("Removed {} from dictionary", term))
        }
    };

    let message = match result {
        Ok(m) => {
            // The settings screen holds its own copy; tell it to reload so a
            // later save from there does not write the old dictionary back.
            let fresh = settings_store.lock_safe().clone();
            let _ = app.emit("settings-changed", fresh);
            m
        }
        Err(e) => e,
    };
    crate::applog::log(&format!("Voice command result: {}", message));
    Some(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_is_ready() {
        let recorder = Recorder::new();
        assert_eq!(recorder.get_state(), RecordingState::Ready);
    }
}
