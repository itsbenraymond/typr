# Typr Dictation App Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a minimal cross-platform dictation app that captures audio via global hotkey, transcribes via local whisper.cpp or Groq cloud API, and auto-pastes the result.

**Architecture:** Tauri v2 app with Rust backend handling audio capture, transcription routing, text cleanup, and auto-paste. Frontend is Vite + vanilla TypeScript showing a single settings page with recording status. whisper.cpp runs as a bundled sidecar binary.

**Tech Stack:** Tauri v2, Rust, Vite, vanilla TypeScript, whisper.cpp (sidecar), cpal, enigo, reqwest, hound

---

## File Structure

```
typr/
├── src-tauri/
│   ├── src/
│   │   ├── main.rs                  # Tauri app entry, plugin registration, command handlers
│   │   ├── lib.rs                   # Module declarations
│   │   ├── settings.rs              # Settings struct, load/save from JSON
│   │   ├── audio.rs                 # Mic enumeration and audio capture via cpal
│   │   ├── transcribe_local.rs      # whisper.cpp sidecar spawning
│   │   ├── transcribe_groq.rs       # Groq REST API client
│   │   ├── cleanup.rs               # Text post-processing
│   │   ├── paste.rs                 # Clipboard + simulated Cmd/Ctrl+V
│   │   ├── recorder.rs              # Recording state machine (orchestrates audio → transcribe → paste)
│   │   └── downloader.rs            # Model download with progress events
│   ├── binaries/                    # whisper.cpp sidecar binaries (gitignored, built locally)
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/
│   │   └── default.json             # Permissions for shell, global-shortcut, etc.
│   └── build.rs                     # Build script to compile whisper.cpp sidecar
├── src/
│   ├── index.html                   # Single page
│   ├── main.ts                      # Frontend entry: IPC calls, UI event wiring
│   └── style.css                    # Minimal styling
├── package.json
├── tsconfig.json
├── vite.config.ts
└── docs/
    └── superpowers/
        └── specs/
            └── 2026-04-08-typr-dictation-app-design.md
```

---

### Task 1: Project Scaffolding

**Files:**
- Create: entire project structure via `npm create tauri-app`
- Modify: `src-tauri/Cargo.toml` (add dependencies)
- Modify: `src-tauri/tauri.conf.json` (app config)
- Modify: `package.json` (add dev dependencies)

- [ ] **Step 1: Scaffold the Tauri v2 project**

```bash
cd /Users/albertbakhoj/Desktop
rm -rf typr/implementationplan.md
cd typr
npm create tauri-app@latest . -- --template vanilla-ts --manager npm
```

If prompted, accept defaults. This creates the Vite + vanilla TS + Tauri v2 project.

- [ ] **Step 2: Add Rust dependencies**

Edit `src-tauri/Cargo.toml` and add to `[dependencies]`:

```toml
[dependencies]
tauri = { version = "2", features = [] }
tauri-plugin-global-shortcut = "2"
tauri-plugin-shell = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
cpal = "0.15"
enigo = "0.2"
reqwest = { version = "0.12", features = ["multipart", "json"] }
hound = "3.5"
tokio = { version = "1", features = ["full"] }
dirs = "6"
```

- [ ] **Step 3: Add frontend dependencies**

```bash
cd /Users/albertbakhoj/Desktop/typr
npm install @tauri-apps/api @tauri-apps/plugin-global-shortcut @tauri-apps/plugin-shell
```

- [ ] **Step 4: Configure tauri.conf.json**

Edit `src-tauri/tauri.conf.json`:
- Set `productName` to `"Typr"`
- Set `identifier` to `"com.typr.app"`
- Set `windows[0].title` to `"Typr"`
- Set `windows[0].width` to `400`
- Set `windows[0].height` to `600`
- Set `windows[0].resizable` to `false`
- Add `"externalBin"` under `"bundle"`:

```json
{
  "bundle": {
    "externalBin": [
      "binaries/whisper-cpp"
    ]
  }
}
```

- [ ] **Step 5: Configure capabilities**

Edit `src-tauri/capabilities/default.json` to include permissions:

```json
{
  "identifier": "default",
  "description": "Default capabilities",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "global-shortcut:default",
    {
      "identifier": "shell:allow-execute",
      "allow": [
        {
          "name": "binaries/whisper-cpp",
          "sidecar": true,
          "args": true
        }
      ]
    }
  ]
}
```

- [ ] **Step 6: Create module files**

Create empty module files in `src-tauri/src/`:
- `settings.rs`
- `audio.rs`
- `transcribe_local.rs`
- `transcribe_groq.rs`
- `cleanup.rs`
- `paste.rs`
- `recorder.rs`
- `downloader.rs`

Create `src-tauri/src/lib.rs`:

```rust
pub mod settings;
pub mod audio;
pub mod transcribe_local;
pub mod transcribe_groq;
pub mod cleanup;
pub mod paste;
pub mod recorder;
pub mod downloader;
```

- [ ] **Step 7: Verify it builds**

```bash
cd /Users/albertbakhoj/Desktop/typr
npm run tauri dev
```

Verify the default Tauri window opens. Close it. Then:

```bash
cd src-tauri && cargo test
```

Verify no errors.

- [ ] **Step 8: Commit**

```bash
cd /Users/albertbakhoj/Desktop/typr
git init
echo "node_modules/\ntarget/\ndist/\nsrc-tauri/binaries/" > .gitignore
git add .
git commit -m "feat: scaffold Tauri v2 project with Vite + vanilla TS"
```

---

### Task 2: Settings Module

**Files:**
- Create: `src-tauri/src/settings.rs`

- [ ] **Step 1: Write tests for settings**

In `src-tauri/src/settings.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    pub microphone: String,
    pub engine: String,
    #[serde(rename = "whisperModel")]
    pub whisper_model: String,
    #[serde(rename = "groqApiKey")]
    pub groq_api_key: String,
    #[serde(rename = "recordingMode")]
    pub recording_mode: String,
    pub hotkey: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            microphone: "default".to_string(),
            engine: "local".to_string(),
            whisper_model: "small".to_string(),
            groq_api_key: String::new(),
            recording_mode: "toggle".to_string(),
            hotkey: "CmdOrCtrl+Shift+D".to_string(),
        }
    }
}

impl Settings {
    pub fn config_path(app_dir: &PathBuf) -> PathBuf {
        app_dir.join("config.json")
    }

    pub fn load(app_dir: &PathBuf) -> Self {
        let path = Self::config_path(app_dir);
        match fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, app_dir: &PathBuf) -> Result<(), String> {
        let path = Self::config_path(app_dir);
        fs::create_dir_all(app_dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert_eq!(settings.microphone, "default");
        assert_eq!(settings.engine, "local");
        assert_eq!(settings.whisper_model, "small");
        assert_eq!(settings.groq_api_key, "");
        assert_eq!(settings.recording_mode, "toggle");
        assert_eq!(settings.hotkey, "CmdOrCtrl+Shift+D");
    }

    #[test]
    fn test_save_and_load() {
        let dir = temp_dir().join("typr_test_settings");
        let _ = fs::remove_dir_all(&dir);

        let mut settings = Settings::default();
        settings.engine = "cloud".to_string();
        settings.groq_api_key = "test-key-123".to_string();

        settings.save(&dir).unwrap();
        let loaded = Settings::load(&dir);

        assert_eq!(loaded.engine, "cloud");
        assert_eq!(loaded.groq_api_key, "test-key-123");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_missing_file_returns_default() {
        let dir = temp_dir().join("typr_test_missing");
        let _ = fs::remove_dir_all(&dir);
        let settings = Settings::load(&dir);
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn test_load_corrupt_json_returns_default() {
        let dir = temp_dir().join("typr_test_corrupt");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.json"), "not json").unwrap();

        let settings = Settings::load(&dir);
        assert_eq!(settings, Settings::default());

        let _ = fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

```bash
cd /Users/albertbakhoj/Desktop/typr/src-tauri && cargo test settings
```

Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
cd /Users/albertbakhoj/Desktop/typr
git add src-tauri/src/settings.rs
git commit -m "feat: add settings module with load/save and tests"
```

---

### Task 3: Text Cleanup Module

**Files:**
- Create: `src-tauri/src/cleanup.rs`

- [ ] **Step 1: Write tests for text cleanup**

In `src-tauri/src/cleanup.rs`:

```rust
pub fn cleanup_text(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Normalize multiple spaces to single space
    let normalized: String = trimmed
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ");

    // Capitalize first letter of each sentence
    let mut result = String::new();
    let mut capitalize_next = true;

    for ch in normalized.chars() {
        if capitalize_next && ch.is_alphabetic() {
            result.extend(ch.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(ch);
            if ch == '.' || ch == '!' || ch == '?' {
                capitalize_next = true;
            }
        }
    }

    // Ensure ending punctuation
    if let Some(last) = result.chars().last() {
        if !matches!(last, '.' | '!' | '?') {
            result.push('.');
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim_whitespace() {
        assert_eq!(cleanup_text("  hello world  "), "Hello world.");
    }

    #[test]
    fn test_normalize_spaces() {
        assert_eq!(cleanup_text("hello    world"), "Hello world.");
    }

    #[test]
    fn test_capitalize_first_letter() {
        assert_eq!(cleanup_text("hello world"), "Hello world.");
    }

    #[test]
    fn test_capitalize_after_period() {
        assert_eq!(cleanup_text("hello. world"), "Hello. World.");
    }

    #[test]
    fn test_capitalize_after_question_mark() {
        assert_eq!(cleanup_text("hello? world"), "Hello? World.");
    }

    #[test]
    fn test_capitalize_after_exclamation() {
        assert_eq!(cleanup_text("hello! world"), "Hello! World.");
    }

    #[test]
    fn test_ensure_ending_punctuation() {
        assert_eq!(cleanup_text("hello world"), "Hello world.");
    }

    #[test]
    fn test_preserve_existing_ending_punctuation() {
        assert_eq!(cleanup_text("hello world."), "Hello world.");
        assert_eq!(cleanup_text("hello world!"), "Hello world!");
        assert_eq!(cleanup_text("hello world?"), "Hello world?");
    }

    #[test]
    fn test_empty_string() {
        assert_eq!(cleanup_text(""), "");
        assert_eq!(cleanup_text("   "), "");
    }

    #[test]
    fn test_already_clean() {
        assert_eq!(cleanup_text("Hello world."), "Hello world.");
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

```bash
cd /Users/albertbakhoj/Desktop/typr/src-tauri && cargo test cleanup
```

Expected: all 10 tests pass.

- [ ] **Step 3: Commit**

```bash
cd /Users/albertbakhoj/Desktop/typr
git add src-tauri/src/cleanup.rs
git commit -m "feat: add text cleanup module with tests"
```

---

### Task 4: Audio Capture Module

**Files:**
- Create: `src-tauri/src/audio.rs`

- [ ] **Step 1: Implement microphone enumeration and audio capture**

In `src-tauri/src/audio.rs`:

```rust
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{WavSpec, WavWriter};
use std::sync::{Arc, Mutex};
use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize)]
pub struct MicDevice {
    pub name: String,
    pub is_default: bool,
}

pub fn list_microphones() -> Vec<MicDevice> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();

    let mut devices = Vec::new();
    if let Ok(input_devices) = host.input_devices() {
        for device in input_devices {
            if let Ok(name) = device.name() {
                devices.push(MicDevice {
                    is_default: name == default_name,
                    name,
                });
            }
        }
    }
    devices
}

pub struct AudioRecorder {
    samples: Arc<Mutex<Vec<f32>>>,
    stream: Option<cpal::Stream>,
}

impl AudioRecorder {
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            stream: None,
        }
    }

    pub fn start(&mut self, mic_name: &str) -> Result<(), String> {
        let host = cpal::default_host();

        let device = if mic_name == "default" {
            host.default_input_device()
                .ok_or("No default input device found")?
        } else {
            host.input_devices()
                .map_err(|e| e.to_string())?
                .find(|d| d.name().map(|n| n == mic_name).unwrap_or(false))
                .ok_or(format!("Microphone '{}' not found", mic_name))?
        };

        let config = cpal::StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(16000),
            buffer_size: cpal::BufferSize::Default,
        };

        let samples = self.samples.clone();
        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mut buf = samples.lock().unwrap();
                    buf.extend_from_slice(data);
                },
                |err| {
                    eprintln!("Audio stream error: {}", err);
                },
                None,
            )
            .map_err(|e| e.to_string())?;

        stream.play().map_err(|e| e.to_string())?;
        self.stream = Some(stream);
        Ok(())
    }

    pub fn stop_and_save(&mut self, output_path: &PathBuf) -> Result<PathBuf, String> {
        self.stream = None; // Drop stops the stream

        let samples = self.samples.lock().unwrap();
        if samples.is_empty() {
            return Err("No audio captured".to_string());
        }

        let spec = WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let mut writer = WavWriter::create(output_path, spec).map_err(|e| e.to_string())?;
        for &sample in samples.iter() {
            let amplitude = (sample * i16::MAX as f32) as i16;
            writer.write_sample(amplitude).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;

        drop(samples);
        self.samples.lock().unwrap().clear();

        Ok(output_path.clone())
    }
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /Users/albertbakhoj/Desktop/typr/src-tauri && cargo check
```

Expected: compiles without errors. (Audio capture requires hardware so we skip unit tests -- tested via integration in Task 9.)

- [ ] **Step 3: Commit**

```bash
cd /Users/albertbakhoj/Desktop/typr
git add src-tauri/src/audio.rs
git commit -m "feat: add audio capture module with mic enumeration"
```

---

### Task 5: Local Transcription (whisper.cpp Sidecar)

**Files:**
- Create: `src-tauri/src/transcribe_local.rs`
- Create: `src-tauri/build.rs`

- [ ] **Step 1: Create a build script to compile whisper.cpp**

Create `src-tauri/build.rs`:

```rust
use std::process::Command;
use std::path::Path;
use std::env;

fn main() {
    tauri_build::build();

    // Only build whisper.cpp if the binary doesn't exist
    let target_triple = env::var("TARGET").unwrap();
    let binary_name = format!("whisper-cpp-{}", target_triple);
    let binary_path = Path::new("binaries").join(&binary_name);

    if binary_path.exists() {
        println!("cargo:warning=whisper-cpp sidecar already exists, skipping build");
        return;
    }

    println!("cargo:warning=Building whisper.cpp sidecar...");

    let whisper_dir = Path::new("..").join("whisper.cpp");

    if !whisper_dir.exists() {
        let status = Command::new("git")
            .args(["clone", "--depth", "1", "https://github.com/ggml-org/whisper.cpp.git"])
            .arg(&whisper_dir)
            .status()
            .expect("Failed to clone whisper.cpp");
        assert!(status.success(), "git clone failed");
    }

    let status = Command::new("cmake")
        .args(["-B", "build", "-S", "."])
        .current_dir(&whisper_dir)
        .status()
        .expect("Failed to run cmake");
    assert!(status.success(), "cmake configure failed");

    let status = Command::new("cmake")
        .args(["--build", "build", "--config", "Release", "-j"])
        .current_dir(&whisper_dir)
        .status()
        .expect("Failed to build whisper.cpp");
    assert!(status.success(), "cmake build failed");

    // Copy the built binary to the binaries directory
    std::fs::create_dir_all("binaries").ok();

    // The CLI binary is typically at build/bin/whisper-cli
    let built_binary = whisper_dir.join("build").join("bin").join("whisper-cli");
    if built_binary.exists() {
        std::fs::copy(&built_binary, &binary_path)
            .expect("Failed to copy whisper-cli binary");
    } else {
        // Fallback: try build/bin/main
        let fallback = whisper_dir.join("build").join("bin").join("main");
        std::fs::copy(&fallback, &binary_path)
            .expect("Failed to copy whisper binary (tried whisper-cli and main)");
    }

    // Make executable on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755)).ok();
    }

    println!("cargo:warning=whisper-cpp sidecar built successfully at {:?}", binary_path);
}
```

- [ ] **Step 2: Implement local transcription**

In `src-tauri/src/transcribe_local.rs`:

```rust
use std::path::PathBuf;
use std::process::Command;

pub fn transcribe_local(
    whisper_binary: &PathBuf,
    model_path: &PathBuf,
    audio_path: &PathBuf,
) -> Result<String, String> {
    if !model_path.exists() {
        return Err("Whisper model not found. Please download a model first.".to_string());
    }

    let output = Command::new(whisper_binary)
        .args([
            "-m",
            model_path.to_str().unwrap(),
            "-f",
            audio_path.to_str().unwrap(),
            "--no-timestamps",
            "-l",
            "en",
        ])
        .output()
        .map_err(|e| format!("Failed to run whisper.cpp: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("whisper.cpp failed: {}", stderr));
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(text)
}

pub fn model_filename(model_size: &str) -> String {
    format!("ggml-{}.bin", model_size)
}

pub fn model_download_url(model_size: &str) -> String {
    format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
        model_size
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_filename() {
        assert_eq!(model_filename("small"), "ggml-small.bin");
        assert_eq!(model_filename("medium"), "ggml-medium.bin");
    }

    #[test]
    fn test_model_download_url() {
        assert_eq!(
            model_download_url("small"),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
        );
    }

    #[test]
    fn test_transcribe_missing_model() {
        let binary = PathBuf::from("/nonexistent/whisper");
        let model = PathBuf::from("/nonexistent/model.bin");
        let audio = PathBuf::from("/nonexistent/audio.wav");
        let result = transcribe_local(&binary, &model, &audio);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cd /Users/albertbakhoj/Desktop/typr/src-tauri && cargo test transcribe_local
```

Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
cd /Users/albertbakhoj/Desktop/typr
git add src-tauri/build.rs src-tauri/src/transcribe_local.rs
git commit -m "feat: add whisper.cpp sidecar build script and local transcription"
```

---

### Task 6: Cloud Transcription (Groq API)

**Files:**
- Create: `src-tauri/src/transcribe_groq.rs`

- [ ] **Step 1: Implement Groq transcription**

In `src-tauri/src/transcribe_groq.rs`:

```rust
use reqwest::multipart;
use std::path::PathBuf;

pub async fn transcribe_groq(api_key: &str, audio_path: &PathBuf) -> Result<String, String> {
    if api_key.is_empty() {
        return Err("Groq API key not set. Please enter your API key in settings.".to_string());
    }

    let audio_bytes = std::fs::read(audio_path)
        .map_err(|e| format!("Failed to read audio file: {}", e))?;

    let file_part = multipart::Part::bytes(audio_bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| e.to_string())?;

    let form = multipart::Form::new()
        .text("model", "whisper-large-v3-turbo")
        .text("language", "en")
        .text("response_format", "json")
        .part("file", file_part);

    let client = reqwest::Client::new();
    let response = client
        .post("https://api.groq.com/openai/v1/audio/transcriptions")
        .header("Authorization", format!("Bearer {}", api_key))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Groq API request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("Groq API error ({}): {}", status, body));
    }

    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Groq response: {}", e))?;

    json["text"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or("No 'text' field in Groq response".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_empty_api_key() {
        let path = PathBuf::from("/tmp/test.wav");
        let result = transcribe_groq("", &path).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key not set"));
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cd /Users/albertbakhoj/Desktop/typr/src-tauri && cargo test transcribe_groq
```

Expected: 1 test passes.

- [ ] **Step 3: Commit**

```bash
cd /Users/albertbakhoj/Desktop/typr
git add src-tauri/src/transcribe_groq.rs
git commit -m "feat: add Groq cloud transcription module"
```

---

### Task 7: Auto-Paste Module

**Files:**
- Create: `src-tauri/src/paste.rs`

- [ ] **Step 1: Implement clipboard paste**

In `src-tauri/src/paste.rs`:

```rust
use enigo::{Enigo, Keyboard, Settings, Key, Direction};

pub fn paste_text(text: &str) -> Result<(), String> {
    // Set clipboard content
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;

    // Use arboard for clipboard (more reliable cross-platform)
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard.set_text(text).map_err(|e| e.to_string())?;

    // Small delay to ensure clipboard is set
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Simulate Cmd+V (macOS) or Ctrl+V (Windows)
    #[cfg(target_os = "macos")]
    {
        enigo.key(Key::Meta, Direction::Press).map_err(|e| e.to_string())?;
        enigo.key(Key::Unicode('v'), Direction::Click).map_err(|e| e.to_string())?;
        enigo.key(Key::Meta, Direction::Release).map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "windows")]
    {
        enigo.key(Key::Control, Direction::Press).map_err(|e| e.to_string())?;
        enigo.key(Key::Unicode('v'), Direction::Click).map_err(|e| e.to_string())?;
        enigo.key(Key::Control, Direction::Release).map_err(|e| e.to_string())?;
    }

    Ok(())
}
```

- [ ] **Step 2: Add arboard dependency**

In `src-tauri/Cargo.toml`, add:

```toml
arboard = "3"
```

- [ ] **Step 3: Verify it compiles**

```bash
cd /Users/albertbakhoj/Desktop/typr/src-tauri && cargo check
```

Expected: compiles without errors.

- [ ] **Step 4: Commit**

```bash
cd /Users/albertbakhoj/Desktop/typr
git add src-tauri/src/paste.rs src-tauri/Cargo.toml
git commit -m "feat: add auto-paste module with clipboard + simulated Cmd/Ctrl+V"
```

---

### Task 8: Model Downloader

**Files:**
- Create: `src-tauri/src/downloader.rs`

- [ ] **Step 1: Implement model downloader with progress events**

In `src-tauri/src/downloader.rs`:

```rust
use reqwest;
use std::io::Write;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

#[derive(Clone, serde::Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: u64,
    pub percent: f64,
}

pub async fn download_model(
    app: AppHandle,
    url: &str,
    dest: &PathBuf,
) -> Result<(), String> {
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Download request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Download failed with status: {}", response.status()));
    }

    let total = response.content_length().unwrap_or(0);

    // Ensure parent directory exists
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut downloaded: u64 = 0;

    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download stream error: {}", e))?;
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;

        let percent = if total > 0 {
            (downloaded as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        let _ = app.emit("download-progress", DownloadProgress {
            downloaded,
            total,
            percent,
        });
    }

    Ok(())
}
```

- [ ] **Step 2: Add futures-util dependency**

In `src-tauri/Cargo.toml`, add:

```toml
futures-util = "0.3"
```

- [ ] **Step 3: Verify it compiles**

```bash
cd /Users/albertbakhoj/Desktop/typr/src-tauri && cargo check
```

Expected: compiles without errors.

- [ ] **Step 4: Commit**

```bash
cd /Users/albertbakhoj/Desktop/typr
git add src-tauri/src/downloader.rs src-tauri/Cargo.toml
git commit -m "feat: add model downloader with progress events"
```

---

### Task 9: Recording State Machine (Orchestrator)

**Files:**
- Create: `src-tauri/src/recorder.rs`

- [ ] **Step 1: Implement recording orchestrator**

In `src-tauri/src/recorder.rs`:

```rust
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

use crate::audio::AudioRecorder;
use crate::cleanup::cleanup_text;
use crate::paste::paste_text;
use crate::settings::Settings;
use crate::transcribe_local;
use crate::transcribe_groq;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum RecordingState {
    Ready,
    Recording,
    Transcribing,
}

pub struct Recorder {
    state: Arc<Mutex<RecordingState>>,
    audio_recorder: Arc<Mutex<AudioRecorder>>,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RecordingState::Ready)),
            audio_recorder: Arc::new(Mutex::new(AudioRecorder::new())),
        }
    }

    pub fn get_state(&self) -> RecordingState {
        self.state.lock().unwrap().clone()
    }

    pub fn start_recording(&self, app: &AppHandle, mic_name: &str) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if *state != RecordingState::Ready {
            return Err("Already recording or transcribing".to_string());
        }

        let mut recorder = self.audio_recorder.lock().unwrap();
        recorder.start(mic_name)?;

        *state = RecordingState::Recording;
        let _ = app.emit("recording-state", RecordingState::Recording);
        Ok(())
    }

    pub async fn stop_and_transcribe(
        &self,
        app: &AppHandle,
        settings: &Settings,
        app_dir: &PathBuf,
    ) -> Result<String, String> {
        // Stop recording
        {
            let mut state = self.state.lock().unwrap();
            if *state != RecordingState::Recording {
                return Err("Not currently recording".to_string());
            }
            *state = RecordingState::Transcribing;
            let _ = app.emit("recording-state", RecordingState::Transcribing);
        }

        let temp_path = app_dir.join("temp_recording.wav");

        // Save audio
        {
            let mut recorder = self.audio_recorder.lock().unwrap();
            recorder.stop_and_save(&temp_path)?;
        }

        // Transcribe
        let raw_text = match settings.engine.as_str() {
            "local" => {
                let whisper_binary = app_dir.join("whisper-cpp");
                let model_path = app_dir.join(transcribe_local::model_filename(&settings.whisper_model));
                transcribe_local::transcribe_local(&whisper_binary, &model_path, &temp_path)?
            }
            "cloud" => {
                transcribe_groq::transcribe_groq(&settings.groq_api_key, &temp_path).await?
            }
            _ => return Err(format!("Unknown engine: {}", settings.engine)),
        };

        // Cleanup temp file
        let _ = std::fs::remove_file(&temp_path);

        // Clean up text
        let cleaned = cleanup_text(&raw_text);

        // Auto-paste
        if !cleaned.is_empty() {
            paste_text(&cleaned)?;
        }

        // Reset state
        {
            let mut state = self.state.lock().unwrap();
            *state = RecordingState::Ready;
            let _ = app.emit("recording-state", RecordingState::Ready);
        }

        Ok(cleaned)
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
```

- [ ] **Step 2: Run tests**

```bash
cd /Users/albertbakhoj/Desktop/typr/src-tauri && cargo test recorder
```

Expected: 1 test passes.

- [ ] **Step 3: Commit**

```bash
cd /Users/albertbakhoj/Desktop/typr
git add src-tauri/src/recorder.rs
git commit -m "feat: add recording state machine orchestrator"
```

---

### Task 10: Tauri Commands and Main Entry Point

**Files:**
- Modify: `src-tauri/src/main.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Update lib.rs with module declarations**

In `src-tauri/src/lib.rs`:

```rust
pub mod settings;
pub mod audio;
pub mod transcribe_local;
pub mod transcribe_groq;
pub mod cleanup;
pub mod paste;
pub mod recorder;
pub mod downloader;
```

- [ ] **Step 2: Implement main.rs with Tauri commands**

Replace `src-tauri/src/main.rs` with:

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

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
    let current_state = state.recorder.get_state();
    match current_state {
        RecordingState::Ready => {
            let mic = state.settings.lock().unwrap().microphone.clone();
            state.recorder.start_recording(&app, &mic)?;
            Ok("recording".to_string())
        }
        RecordingState::Recording => {
            let settings = state.settings.lock().unwrap().clone();
            let result = state.recorder.stop_and_transcribe(&app, &settings, &state.app_dir).await?;
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
        ])
        .setup(move |app| {
            // Register global hotkey
            let handle = app.handle().clone();
            let shortcut: Shortcut = initial_hotkey.parse()
                .expect("Failed to parse hotkey");

            app.global_shortcut().on_shortcut(shortcut, move |_app, _shortcut, event| {
                let handle = handle.clone();
                match event.state {
                    ShortcutState::Pressed => {
                        tauri::async_runtime::spawn(async move {
                            let state: State<AppState> = handle.state();
                            let mode = state.settings.lock().unwrap().recording_mode.clone();
                            match mode.as_str() {
                                "toggle" => {
                                    let _ = toggle_recording(handle.clone(), state).await;
                                }
                                "push-to-talk" => {
                                    let current = state.recorder.get_state();
                                    if current == RecordingState::Ready {
                                        let mic = state.settings.lock().unwrap().microphone.clone();
                                        let _ = state.recorder.start_recording(&handle, &mic);
                                    }
                                }
                                _ => {}
                            }
                        });
                    }
                    ShortcutState::Released => {
                        tauri::async_runtime::spawn(async move {
                            let state: State<AppState> = handle.state();
                            let mode = state.settings.lock().unwrap().recording_mode.clone();
                            if mode == "push-to-talk" {
                                let current = state.recorder.get_state();
                                if current == RecordingState::Recording {
                                    let settings = state.settings.lock().unwrap().clone();
                                    let _ = state.recorder.stop_and_transcribe(
                                        &handle, &settings, &state.app_dir
                                    ).await;
                                }
                            }
                        });
                    }
                }
            })?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

Note: The crate name in `Cargo.toml` should be set. Check if it's `typr` or adjust imports. The default Tauri scaffold may name the lib crate differently. In `Cargo.toml`:

```toml
[lib]
name = "typr_lib"
crate-type = ["staticlib", "cdylib", "rlib"]
```

- [ ] **Step 3: Verify it compiles**

```bash
cd /Users/albertbakhoj/Desktop/typr/src-tauri && cargo check
```

Expected: compiles without errors. Fix any import issues.

- [ ] **Step 4: Commit**

```bash
cd /Users/albertbakhoj/Desktop/typr
git add src-tauri/src/main.rs src-tauri/src/lib.rs src-tauri/Cargo.toml
git commit -m "feat: wire up Tauri commands and global hotkey in main.rs"
```

---

### Task 11: Frontend UI

**Files:**
- Modify: `src/index.html`
- Modify: `src/main.ts`
- Modify: `src/style.css`

- [ ] **Step 1: Create the HTML layout**

Replace `src/index.html` with:

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Typr</title>
    <link rel="stylesheet" href="/style.css" />
  </head>
  <body>
    <div id="app">
      <header>
        <div id="status-indicator">
          <span id="status-dot"></span>
          <span id="status-text">Ready</span>
        </div>
      </header>

      <main>
        <section class="setting-group">
          <label for="mic-select">Microphone</label>
          <select id="mic-select"></select>
        </section>

        <section class="setting-group">
          <label>Engine</label>
          <div class="toggle-group">
            <button id="engine-local" class="toggle-btn active">Local Whisper</button>
            <button id="engine-cloud" class="toggle-btn">Groq Cloud</button>
          </div>
        </section>

        <section class="setting-group" id="local-settings">
          <label for="model-select">Model Size</label>
          <div class="model-row">
            <select id="model-select">
              <option value="small">Small (~466 MB)</option>
              <option value="medium">Medium (~1.5 GB)</option>
            </select>
            <button id="download-btn">Download</button>
          </div>
          <div id="download-progress" class="hidden">
            <div id="progress-bar"><div id="progress-fill"></div></div>
            <span id="progress-text">0%</span>
          </div>
        </section>

        <section class="setting-group hidden" id="cloud-settings">
          <label for="groq-key">Groq API Key</label>
          <input type="password" id="groq-key" placeholder="Enter your Groq API key" />
        </section>

        <section class="setting-group">
          <label>Recording Mode</label>
          <div class="toggle-group">
            <button id="mode-toggle" class="toggle-btn active">Toggle</button>
            <button id="mode-ptt" class="toggle-btn">Push to Talk</button>
          </div>
        </section>

        <section class="setting-group">
          <label>Hotkey</label>
          <div id="hotkey-display">
            <kbd id="hotkey-text">Cmd+Shift+D</kbd>
          </div>
        </section>
      </main>
    </div>
    <script type="module" src="/main.ts"></script>
  </body>
</html>
```

- [ ] **Step 2: Implement the frontend logic**

Replace `src/main.ts` with:

```typescript
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface Settings {
  microphone: string;
  engine: string;
  whisperModel: string;
  groqApiKey: string;
  recordingMode: string;
  hotkey: string;
}

interface MicDevice {
  name: string;
  is_default: boolean;
}

interface DownloadProgress {
  downloaded: number;
  total: number;
  percent: number;
}

// DOM elements
const statusDot = document.getElementById("status-dot")!;
const statusText = document.getElementById("status-text")!;
const micSelect = document.getElementById("mic-select") as HTMLSelectElement;
const engineLocal = document.getElementById("engine-local")!;
const engineCloud = document.getElementById("engine-cloud")!;
const localSettings = document.getElementById("local-settings")!;
const cloudSettings = document.getElementById("cloud-settings")!;
const modelSelect = document.getElementById("model-select") as HTMLSelectElement;
const downloadBtn = document.getElementById("download-btn")!;
const downloadProgress = document.getElementById("download-progress")!;
const progressFill = document.getElementById("progress-fill")!;
const progressText = document.getElementById("progress-text")!;
const groqKey = document.getElementById("groq-key") as HTMLInputElement;
const modeToggle = document.getElementById("mode-toggle")!;
const modePtt = document.getElementById("mode-ptt")!;
const hotkeyText = document.getElementById("hotkey-text")!;

let currentSettings: Settings;

async function loadSettings() {
  currentSettings = await invoke<Settings>("get_settings");

  // Populate mic dropdown
  const mics = await invoke<MicDevice[]>("list_microphones");
  micSelect.innerHTML = "";
  mics.forEach((mic) => {
    const option = document.createElement("option");
    option.value = mic.name;
    option.textContent = mic.name + (mic.is_default ? " (default)" : "");
    micSelect.appendChild(option);
  });
  micSelect.value = currentSettings.microphone;

  // Engine
  setEngine(currentSettings.engine);

  // Model
  modelSelect.value = currentSettings.whisperModel;
  await checkModelStatus();

  // Groq key
  groqKey.value = currentSettings.groqApiKey;

  // Recording mode
  setRecordingMode(currentSettings.recordingMode);

  // Hotkey
  hotkeyText.textContent = currentSettings.hotkey.replace("CmdOrCtrl", "Cmd");
}

function setEngine(engine: string) {
  currentSettings.engine = engine;
  engineLocal.classList.toggle("active", engine === "local");
  engineCloud.classList.toggle("active", engine === "cloud");
  localSettings.classList.toggle("hidden", engine !== "local");
  cloudSettings.classList.toggle("hidden", engine !== "cloud");
}

function setRecordingMode(mode: string) {
  currentSettings.recordingMode = mode;
  modeToggle.classList.toggle("active", mode === "toggle");
  modePtt.classList.toggle("active", mode === "push-to-talk");
}

async function checkModelStatus() {
  const downloaded = await invoke<boolean>("check_model_downloaded", {
    modelSize: modelSelect.value,
  });
  downloadBtn.textContent = downloaded ? "\u2713" : "Download";
  downloadBtn.disabled = downloaded;
}

async function saveSettings() {
  currentSettings.microphone = micSelect.value;
  currentSettings.whisperModel = modelSelect.value;
  currentSettings.groqApiKey = groqKey.value;
  await invoke("save_settings", { settings: currentSettings });
}

// Event listeners
engineLocal.addEventListener("click", () => {
  setEngine("local");
  saveSettings();
});

engineCloud.addEventListener("click", () => {
  setEngine("cloud");
  saveSettings();
});

micSelect.addEventListener("change", () => saveSettings());

modelSelect.addEventListener("change", async () => {
  await checkModelStatus();
  saveSettings();
});

downloadBtn.addEventListener("click", async () => {
  downloadBtn.disabled = true;
  downloadProgress.classList.remove("hidden");
  progressFill.style.width = "0%";
  progressText.textContent = "0%";

  try {
    await invoke("download_model", { modelSize: modelSelect.value });
    downloadBtn.textContent = "\u2713";
  } catch (e) {
    downloadBtn.textContent = "Retry";
    downloadBtn.disabled = false;
    console.error("Download failed:", e);
  }
  downloadProgress.classList.add("hidden");
});

groqKey.addEventListener("change", () => saveSettings());

modeToggle.addEventListener("click", () => {
  setRecordingMode("toggle");
  saveSettings();
});

modePtt.addEventListener("click", () => {
  setRecordingMode("push-to-talk");
  saveSettings();
});

// Listen for recording state changes
listen<string>("recording-state", (event) => {
  const state = event.payload;
  statusDot.className = "";
  if (state === "Recording") {
    statusDot.classList.add("recording");
    statusText.textContent = "Recording...";
  } else if (state === "Transcribing") {
    statusDot.classList.add("transcribing");
    statusText.textContent = "Transcribing...";
  } else {
    statusDot.classList.add("ready");
    statusText.textContent = "Ready";
  }
});

// Listen for download progress
listen<DownloadProgress>("download-progress", (event) => {
  const { percent } = event.payload;
  progressFill.style.width = `${percent}%`;
  progressText.textContent = `${Math.round(percent)}%`;
});

// Initialize
loadSettings();
```

- [ ] **Step 3: Create the CSS**

Replace `src/style.css` with:

```css
:root {
  --bg: #1a1a1a;
  --surface: #2a2a2a;
  --text: #e0e0e0;
  --text-secondary: #888;
  --accent: #4a9eff;
  --green: #4caf50;
  --red: #f44336;
  --yellow: #ff9800;
  --border: #333;
  --radius: 8px;
}

@media (prefers-color-scheme: light) {
  :root {
    --bg: #f5f5f5;
    --surface: #ffffff;
    --text: #1a1a1a;
    --text-secondary: #666;
    --border: #ddd;
  }
}

* {
  margin: 0;
  padding: 0;
  box-sizing: border-box;
}

body {
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  background: var(--bg);
  color: var(--text);
  user-select: none;
  -webkit-user-select: none;
}

#app {
  padding: 20px;
  max-width: 400px;
  margin: 0 auto;
}

header {
  text-align: center;
  margin-bottom: 24px;
  padding: 16px;
  background: var(--surface);
  border-radius: var(--radius);
  border: 1px solid var(--border);
}

#status-indicator {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 10px;
  font-size: 18px;
  font-weight: 500;
}

#status-dot {
  width: 12px;
  height: 12px;
  border-radius: 50%;
  background: var(--green);
}

#status-dot.ready {
  background: var(--green);
}

#status-dot.recording {
  background: var(--red);
  animation: pulse 1s infinite;
}

#status-dot.transcribing {
  background: var(--yellow);
  animation: pulse 1.5s infinite;
}

@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.4; }
}

main {
  display: flex;
  flex-direction: column;
  gap: 16px;
}

.setting-group {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 14px;
}

.setting-group label {
  display: block;
  font-size: 12px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  color: var(--text-secondary);
  margin-bottom: 8px;
}

select, input[type="password"] {
  width: 100%;
  padding: 8px 12px;
  background: var(--bg);
  color: var(--text);
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 14px;
  outline: none;
}

select:focus, input:focus {
  border-color: var(--accent);
}

.toggle-group {
  display: flex;
  gap: 4px;
  background: var(--bg);
  border-radius: 6px;
  padding: 3px;
}

.toggle-btn {
  flex: 1;
  padding: 8px 12px;
  border: none;
  border-radius: 4px;
  background: transparent;
  color: var(--text-secondary);
  font-size: 13px;
  cursor: pointer;
  transition: all 0.2s;
}

.toggle-btn.active {
  background: var(--accent);
  color: white;
}

.model-row {
  display: flex;
  gap: 8px;
}

.model-row select {
  flex: 1;
}

#download-btn {
  padding: 8px 16px;
  background: var(--accent);
  color: white;
  border: none;
  border-radius: 6px;
  font-size: 13px;
  cursor: pointer;
  white-space: nowrap;
}

#download-btn:disabled {
  opacity: 0.6;
  cursor: default;
}

#download-progress {
  margin-top: 8px;
  display: flex;
  align-items: center;
  gap: 8px;
}

#progress-bar {
  flex: 1;
  height: 6px;
  background: var(--bg);
  border-radius: 3px;
  overflow: hidden;
}

#progress-fill {
  height: 100%;
  background: var(--accent);
  border-radius: 3px;
  transition: width 0.3s;
  width: 0%;
}

#progress-text {
  font-size: 12px;
  color: var(--text-secondary);
  min-width: 35px;
}

#hotkey-display {
  text-align: center;
}

kbd {
  display: inline-block;
  padding: 6px 14px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: 6px;
  font-family: inherit;
  font-size: 14px;
  color: var(--text);
}

.hidden {
  display: none !important;
}
```

- [ ] **Step 4: Verify the frontend builds**

```bash
cd /Users/albertbakhoj/Desktop/typr && npm run build
```

Expected: Vite builds without errors.

- [ ] **Step 5: Commit**

```bash
cd /Users/albertbakhoj/Desktop/typr
git add src/index.html src/main.ts src/style.css
git commit -m "feat: implement single-page settings UI with status indicator"
```

---

### Task 12: Integration and End-to-End Test

**Files:**
- No new files

- [ ] **Step 1: Run full build**

```bash
cd /Users/albertbakhoj/Desktop/typr && npm run tauri build -- --debug
```

Expected: builds successfully, creates a debug binary.

- [ ] **Step 2: Run Rust tests**

```bash
cd /Users/albertbakhoj/Desktop/typr/src-tauri && cargo test
```

Expected: all unit tests pass (settings, cleanup, transcribe_local, recorder).

- [ ] **Step 3: Manual smoke test**

Launch the debug build:

```bash
cd /Users/albertbakhoj/Desktop/typr && npm run tauri dev
```

Verify:
1. Window opens at 400x600 with the settings UI
2. Microphone dropdown populates with available devices
3. Engine toggle switches between local/cloud sections
4. Model download button is clickable
5. Status shows "Ready" with green dot

- [ ] **Step 4: Commit any fixes**

```bash
cd /Users/albertbakhoj/Desktop/typr
git add -A
git commit -m "fix: integration fixes from smoke test"
```

(Only if there were fixes needed. Skip if everything worked.)

---

### Task 13: Clean Up and Final Polish

**Files:**
- Modify: `src-tauri/tauri.conf.json` (ensure app icon, metadata)
- Remove: any scaffold boilerplate files (default Tauri welcome page assets)

- [ ] **Step 1: Remove scaffold boilerplate**

Delete any default Tauri/Vite template files that are no longer needed (e.g., default `assets/`, example code).

```bash
cd /Users/albertbakhoj/Desktop/typr
rm -f src/assets/* public/*
```

- [ ] **Step 2: Verify final build**

```bash
cd /Users/albertbakhoj/Desktop/typr && npm run tauri dev
```

Verify everything still works after cleanup.

- [ ] **Step 3: Final commit**

```bash
cd /Users/albertbakhoj/Desktop/typr
git add -A
git commit -m "chore: remove scaffold boilerplate, final cleanup"
```
