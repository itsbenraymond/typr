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

    pub async fn stop_and_transcribe(
        &self,
        app: &AppHandle,
        settings: &Settings,
        app_dir: &PathBuf,
        format_mode: bool,
    ) -> Result<String, String> {
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
            let raw_text = transcriber.transcribe(&temp_path, &opts).await?;

            // Cleanup temp file
            let _ = std::fs::remove_file(&temp_path);

            // Clean up / format
            let cleaned = if format_mode {
                let noise_cleaned = crate::cleanup::strip_noise_only(&raw_text);
                match crate::format_groq::format_text(&settings.groq_api_key, &noise_cleaned).await {
                    Ok(formatted) => formatted,
                    Err(e) => {
                        crate::applog::log(&format!("Groq format failed: {}", e));
                        let _ = app.emit("format-error", e);
                        // For weeks this branch was taken on every single
                        // dictation (the configured model had been
                        // decommissioned) and nothing on screen said so, so
                        // raw transcripts looked like the intended output.
                        *soft_warning.lock_safe() = Some("AI cleanup failed - pasted raw".into());
                        cleanup_text(&raw_text)
                    }
                }
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
            if !cleaned.is_empty() {
                let saved_hwnd = *self.focused_hwnd.lock_safe();
                paste_text(&cleaned, saved_hwnd)?;
            }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_is_ready() {
        let recorder = Recorder::new();
        assert_eq!(recorder.get_state(), RecordingState::Ready);
    }
}
