# Typr -- Local Dictation App

## Overview

Typr is a minimal, cross-platform (macOS + Windows) dictation app built with Tauri. It captures audio via a global hotkey, transcribes it using either a local whisper.cpp sidecar or Groq's cloud Whisper API, applies basic text cleanup, and auto-pastes the result into the currently focused application.

Personal-use tool. No accounts, no cloud sync, no history.

## Architecture

```
┌─────────────────────────────────┐
│  Tauri App                      │
│  ┌───────────┐  ┌────────────┐  │
│  │ Frontend   │  │ Rust       │  │
│  │ (Vite +   │◄►│ Backend    │  │
│  │  vanilla   │  │            │  │
│  │  TS)       │  └─────┬──────┘ │
│  └───────────┘        │        │
│              ┌────────┴───────┐ │
│              │                │ │
│        ┌─────▼─────┐  ┌──────▼──┐
│        │whisper.cpp │  │ Groq    │
│        │ sidecar    │  │ REST API│
│        └───────────┘  └─────────┘
└─────────────────────────────────┘
```

### Components

**Rust Backend** -- core logic layer:
- Global hotkey registration and listening
- Audio capture from selected microphone via `cpal` crate
- Spawning whisper.cpp sidecar process and piping audio
- Calling Groq REST API for cloud transcription
- Basic text cleanup post-processing
- Simulated keyboard input via `enigo` crate for auto-paste
- Settings persistence (JSON file)
- Model download management (HTTP download from Hugging Face)

**Frontend (Vite + vanilla TypeScript)** -- single-page settings UI:
- Recording status indicator
- Settings controls
- Model download UI with progress bar

**whisper.cpp Sidecar** -- bundled pre-built binary:
- Receives WAV audio input
- Returns transcribed text
- Supports small and medium model sizes

## Recording Flow

1. User presses global hotkey (default: `Cmd+Shift+D` on Mac, `Ctrl+Shift+D` on Windows)
2. Rust backend starts capturing audio from selected microphone via `cpal`
3. Recording status indicator updates to "Recording..."
4. Recording stops when:
   - **Toggle mode:** user presses hotkey again
   - **Push-to-talk mode:** user releases the hotkey
5. Audio is saved as a temporary WAV file
6. Status updates to "Transcribing..."
7. Backend routes audio to whisper.cpp sidecar or Groq API based on current engine setting
8. Transcription text is returned
9. Basic text cleanup runs
10. Text is typed into the focused application via simulated keyboard input (`enigo`)
11. Status returns to "Ready"
12. Temporary WAV file is deleted

## Transcription Engines

### Local: whisper.cpp Sidecar

- Pre-built whisper.cpp binary bundled as a Tauri sidecar
- Separate binaries for macOS (arm64, x86_64) and Windows (x86_64)
- Supports two model sizes:
  - **Small** (~466 MB) -- faster, slightly less accurate
  - **Medium** (~1.5 GB) -- slower, more accurate
- Models stored in Tauri app data directory
- Models downloaded in-app: user clicks "Download" button next to model selector, progress bar shows download progress
- On first launch, if no model exists, the app prompts the user to download one before local transcription can be used

### Cloud: Groq Whisper API

- REST API call to Groq's Whisper endpoint
- Requires user-provided API key (entered in settings)
- Sends WAV audio as multipart form data
- Returns transcription text

## Text Cleanup

Post-processing runs in the Rust backend after transcription, before pasting:

1. Trim leading and trailing whitespace
2. Normalize multiple consecutive spaces into a single space
3. Capitalize the first letter of each sentence
4. Ensure ending punctuation (add period if missing)

No LLM processing. No filler word removal.

## Auto-Paste

Uses the `enigo` crate on both macOS and Windows. After cleanup, the transcribed text is placed on the clipboard and a Cmd+V / Ctrl+V keystroke is simulated to paste it into the focused application. This is more reliable and faster than typing character-by-character.

## Frontend UI

Single-page layout built with Vite + vanilla TypeScript.

### Top Section
- Recording status indicator: "Ready" (green dot), "Recording..." (red pulsing dot), "Transcribing..." (yellow dot)

### Settings Section
- **Microphone** -- dropdown of available input devices (populated from backend)
- **Engine** -- toggle between "Local Whisper" and "Groq Cloud"
- **Model size** -- dropdown: Small / Medium (visible only when engine is Local)
  - "Download" button next to selector with inline progress bar
  - Checkmark shown if model is already downloaded
- **Groq API key** -- text input (visible only when engine is Cloud)
- **Recording mode** -- toggle: "Toggle" / "Push-to-talk"
- **Hotkey** -- displays current hotkey, click to rebind

### Styling
- Clean, minimal design
- Light/dark follows system preference
- Small fixed-size window

## Settings Storage

JSON file stored in the Tauri app data directory (platform-specific):
- macOS: `~/Library/Application Support/com.typr.app/config.json`
- Windows: `%APPDATA%/com.typr.app/config.json`

```json
{
  "microphone": "default",
  "engine": "local",
  "whisperModel": "small",
  "groqApiKey": "",
  "recordingMode": "toggle",
  "hotkey": "CmdOrCtrl+Shift+D"
}
```

## Dependencies

### Rust (Backend)
- `tauri` -- app framework
- `cpal` -- cross-platform audio capture
- `enigo` -- simulated keyboard input
- `reqwest` -- HTTP client (Groq API + model downloads)
- `serde` / `serde_json` -- settings serialization
- `hound` -- WAV file writing

### Frontend
- Vite -- build tooling
- TypeScript -- type-safe frontend code
- Tauri API (`@tauri-apps/api`) -- IPC with Rust backend

### External
- whisper.cpp -- pre-built sidecar binary (bundled)
- Groq Whisper API -- cloud transcription

## Platform Support

- **macOS:** arm64 (Apple Silicon) and x86_64 (Intel)
- **Windows:** x86_64

## Out of Scope

- Transcription history or storage
- User accounts or authentication
- Cloud sync
- Multiple language support (English only for v1)
- Linux support
- LLM-based text processing
- Streaming/real-time transcription (batch only)
