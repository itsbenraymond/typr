use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

use crate::audio::AudioRecorder;
use crate::cleanup::cleanup_text;
use crate::paste::{get_focused_hwnd, paste_text};
use crate::settings::Settings;
use crate::transcribe_local;
use crate::transcribe_groq;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum RecordingState {
    Ready,
    Recording,
    Transcribing,
}

fn update_overlay(app: &AppHandle, state: &RecordingState) {
    if let Some(overlay) = app.get_webview_window("overlay") {
        match state {
            RecordingState::Ready => {
                let _ = overlay.hide();
                let _ = overlay.eval("document.getElementById('bar')?.setAttribute('data-state', 'ready');");
            }
            RecordingState::Recording => {
                let _ = overlay.show();
                let _ = overlay.eval("document.getElementById('bar')?.setAttribute('data-state', 'recording');");
            }
            RecordingState::Transcribing => {
                let _ = overlay.show();
                let _ = overlay.eval("document.getElementById('bar')?.setAttribute('data-state', 'transcribing');");
            }
        }
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
        self.state.lock().unwrap().clone()
    }

    pub fn cancel_recording(&self, app: &AppHandle) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if *state != RecordingState::Recording {
            return Err("Not currently recording".to_string());
        }

        {
            let mut recorder = self.audio_recorder.lock().unwrap();
            recorder.cancel();
        }

        *state = RecordingState::Ready;
        let _ = app.emit("recording-state", RecordingState::Ready);
        update_overlay(app, &RecordingState::Ready);
        Ok(())
    }

    pub fn start_recording(&self, app: &AppHandle, mic_name: &str) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if *state != RecordingState::Ready {
            return Err("Already recording or transcribing".to_string());
        }

        // Snapshot cursor position now, before the overlay shows, so we know
        // which input field the user had focused when they triggered the hotkey.
        *self.focused_hwnd.lock().unwrap() = get_focused_hwnd();

        let level_arc = {
            let mut recorder = self.audio_recorder.lock().unwrap();
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
            let s = state_arc.lock().unwrap().clone();
            if s != RecordingState::Recording {
                break;
            }
            let level = *level_arc.lock().unwrap();
            if let Some(overlay) = app_clone.get_webview_window("overlay") {
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
            let mut state = self.state.lock().unwrap();
            if *state != RecordingState::Recording {
                return Err("Not currently recording".to_string());
            }
            *state = RecordingState::Transcribing;
            let _ = app.emit("recording-state", RecordingState::Transcribing);
            update_overlay(app, &RecordingState::Transcribing);
        }

        // Run the whole pipeline in a closure so we can guarantee state is
        // reset to Ready afterwards — whether the pipeline succeeds or fails.
        // Previously any early-return Err left state stuck at Transcribing,
        // silently swallowing all subsequent hotkey presses.
        let result: Result<String, String> = async {
            let temp_path = app_dir.join("temp_recording.wav");

            // Save audio
            {
                let mut recorder = self.audio_recorder.lock().unwrap();
                recorder.stop_and_save(&temp_path)?;
            }

            // Transcribe
            let raw_text = match settings.engine.as_str() {
                "local" => {
                    let model_path = app_dir.join(transcribe_local::model_filename(&settings.whisper_model));
                    transcribe_local::transcribe_local(app, &model_path, &temp_path).await?
                }
                "cloud" => {
                    transcribe_groq::transcribe_groq(&settings.groq_api_key, &temp_path).await?
                }
                _ => return Err(format!("Unknown engine: {}", settings.engine)),
            };

            // Cleanup temp file
            let _ = std::fs::remove_file(&temp_path);

            // Clean up / format
            let cleaned = if format_mode {
                let noise_cleaned = crate::cleanup::strip_noise_only(&raw_text);
                match crate::format_groq::format_text(&settings.groq_api_key, &noise_cleaned).await {
                    Ok(formatted) => formatted,
                    Err(e) => {
                        eprintln!("[Typr] Groq format failed: {}", e);
                        let _ = app.emit("format-error", e);
                        cleanup_text(&raw_text)
                    }
                }
            } else {
                cleanup_text(&raw_text)
            };

            // Clear format-mode attribute on overlay
            if let Some(overlay) = app.get_webview_window("overlay") {
                let _ = overlay.eval(
                    "document.getElementById('bar')?.removeAttribute('data-format-mode');"
                );
            }

            // Auto-paste — re-click the original cursor position so the correct
            // input field has focus before Ctrl+V fires.
            if !cleaned.is_empty() {
                let saved_hwnd = *self.focused_hwnd.lock().unwrap();
                paste_text(&cleaned, saved_hwnd)?;
            }

            Ok(cleaned)
        }
        .await;

        // Always reset state to Ready — even if the pipeline errored.
        // This is the fix: previously only the success path reset state,
        // leaving it permanently stuck at Transcribing on any failure.
        {
            let mut state = self.state.lock().unwrap();
            *state = RecordingState::Ready;
            let _ = app.emit("recording-state", RecordingState::Ready);
            update_overlay(app, &RecordingState::Ready);
        }

        if let Err(ref e) = result {
            eprintln!("[Typr] Transcription pipeline error (state reset to Ready): {}", e);
            let _ = app.emit("transcription-error", e.clone());
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
