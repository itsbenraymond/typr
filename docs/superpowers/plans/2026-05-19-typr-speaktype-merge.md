# Typr × SpeakType Merge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enhance Typr with SpeakType's noise-label text cleanup, a second AI-format hotkey (Groq-powered), system tray support, and a packaged Windows .msi installer — keeping 100% of Typr's existing visual design.

**Architecture:** Two hotkeys share the same recording/overlay pipeline. After Whisper transcription, `format_mode: bool` routes to either basic `cleanup_text` (noise removal + capitalize + punctuate) or `format_groq` (noise removal + Groq LLM formatting). The app hides to a system tray icon instead of quitting, and `cargo tauri build` produces a `.msi` installer.

**Tech Stack:** Tauri 2, Rust, TypeScript/Vite, Groq API (llama-3.1-8b-instant), WebView2, Windows MSI bundler

**Spec:** `docs/superpowers/specs/2026-05-19-typr-speaktype-merge-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `src-tauri/src/cleanup.rs` | Modify | Add noise-label removal (Steps 1–2) before existing capitalize/punctuate |
| `src-tauri/src/format_groq.rs` | Create | Groq text-formatting API call with 8s timeout + fallback |
| `src-tauri/src/recorder.rs` | Modify | Add `format_mode: bool` param; route to `format_groq` when true |
| `src-tauri/src/lib.rs` | Modify | Register `toggle_recording_format` command + second global shortcut |
| `src-tauri/src/main.rs` | Modify | Add `toggle_recording_format` Tauri command handler |
| `src-tauri/src/transcribe_groq.rs` | Reference | Existing Groq HTTP client — `format_groq.rs` reuses same `reqwest` pattern |
| `src-tauri/tauri.conf.json` | Modify | Add `trayIcon` block; ensure tray behaviour on window close |
| `src/index.html` | Modify | Add AI Format Hotkey row to settings |
| `src/main.ts` | Modify | Show second hotkey kbd; handle `format-error` event (red flash) |
| `src/style.css` | Modify | Add `--amber` CSS variable for format-mode waveform |
| `src/overlay.html` | Modify | Amber waveform for `data-format-mode="true"`; red error state |

---

## Task 1: Enhance `cleanup.rs` with noise-label removal

**Files:**
- Modify: `src-tauri/src/cleanup.rs`

- [ ] **Step 1: Write failing tests for noise-label removal**

Open `src-tauri/src/cleanup.rs` and add these tests inside the existing `#[cfg(test)]` block:

```rust
#[test]
fn test_removes_blank_audio() {
    assert_eq!(cleanup_text("[BLANK_AUDIO]"), "");
    assert_eq!(cleanup_text("hello [BLANK_AUDIO] world"), "Hello world.");
}

#[test]
fn test_removes_silence_marker() {
    assert_eq!(cleanup_text("[SILENCE]"), "");
    assert_eq!(cleanup_text("hello [SILENCE] world"), "Hello world.");
}

#[test]
fn test_removes_nospeech_token() {
    assert_eq!(cleanup_text("<|nospeech|>"), "");
    assert_eq!(cleanup_text("hello <|nospeech|> world"), "Hello world.");
}

#[test]
fn test_removes_noise_labels_in_brackets() {
    assert_eq!(cleanup_text("[background noise]"), "");
    assert_eq!(cleanup_text("hello [laughter] world"), "Hello world.");
}

#[test]
fn test_removes_noise_labels_in_parens() {
    assert_eq!(cleanup_text("(applause)"), "");
    assert_eq!(cleanup_text("hello (coughing) world"), "Hello world.");
}

#[test]
fn test_noise_removal_case_insensitive() {
    assert_eq!(cleanup_text("[Background Noise]"), "");
    assert_eq!(cleanup_text("[LAUGHTER]"), "");
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd src-tauri && cargo test cleanup
```
Expected: multiple FAIL — `cleanup_text` does not yet strip noise labels.

- [ ] **Step 3: Replace `cleanup_text` in `cleanup.rs` with the enhanced version**

Replace the entire file content with:

```rust
pub fn cleanup_text(text: &str) -> String {
    let s = remove_noise_labels(text);
    let trimmed = s.trim();
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

/// Remove Whisper noise artifacts before formatting.
/// Handles: silence/placeholder tokens, 26 noise-label categories in [brackets] or (parens).
fn remove_noise_labels(text: &str) -> String {
    use std::sync::OnceLock;
    use regex::Regex;

    static SILENCE_RE: OnceLock<Regex> = OnceLock::new();
    static NOISE_RE: OnceLock<Regex>   = OnceLock::new();

    let silence_re = SILENCE_RE.get_or_init(|| {
        Regex::new(r"(?i)\[(?:BLANK[_ ]AUDIO|SILENCE|S)\]|<\|nospeech\|>|\[\s*S\s*\]")
            .unwrap()
    });

    let noise_labels = [
        "applause", "background noise", "blank audio", "breathing",
        "cough", "coughing", "exhale", "heartbeat", "indistinct",
        "inaudible", "inhale", "laughing", "laughter", "loud noise",
        "muffled speech", "music", "noise", "silence", "sigh", "sighs",
        "sniffing", "static", "unclear speech", "unintelligible",
        "wind", "wind blowing", "wind noise",
    ];
    let labels_pattern = noise_labels.join("|");
    let noise_re = NOISE_RE.get_or_init(|| {
        Regex::new(&format!(r"(?i)[\[\(]\s*(?:{})\s*[\]\)]", labels_pattern)).unwrap()
    });

    let s = silence_re.replace_all(text, " ");
    let s = noise_re.replace_all(&s, " ");
    s.into_owned()
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

    #[test]
    fn test_removes_blank_audio() {
        assert_eq!(cleanup_text("[BLANK_AUDIO]"), "");
        assert_eq!(cleanup_text("hello [BLANK_AUDIO] world"), "Hello world.");
    }

    #[test]
    fn test_removes_silence_marker() {
        assert_eq!(cleanup_text("[SILENCE]"), "");
        assert_eq!(cleanup_text("hello [SILENCE] world"), "Hello world.");
    }

    #[test]
    fn test_removes_nospeech_token() {
        assert_eq!(cleanup_text("<|nospeech|>"), "");
        assert_eq!(cleanup_text("hello <|nospeech|> world"), "Hello world.");
    }

    #[test]
    fn test_removes_noise_labels_in_brackets() {
        assert_eq!(cleanup_text("[background noise]"), "");
        assert_eq!(cleanup_text("hello [laughter] world"), "Hello world.");
    }

    #[test]
    fn test_removes_noise_labels_in_parens() {
        assert_eq!(cleanup_text("(applause)"), "");
        assert_eq!(cleanup_text("hello (coughing) world"), "Hello world.");
    }

    #[test]
    fn test_noise_removal_case_insensitive() {
        assert_eq!(cleanup_text("[Background Noise]"), "");
        assert_eq!(cleanup_text("[LAUGHTER]"), "");
    }
}
```

- [ ] **Step 4: Add `regex` crate to `Cargo.toml`**

In `src-tauri/Cargo.toml`, add under `[dependencies]`:
```toml
regex = "1"
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cd src-tauri && cargo test cleanup
```
Expected: all tests PASS.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/cleanup.rs src-tauri/Cargo.toml
git commit -m "feat: add SpeakType noise-label removal to cleanup_text"
```

---

## Task 2: Create `format_groq.rs` — AI text formatter

**Files:**
- Create: `src-tauri/src/format_groq.rs`
- Reference: `src-tauri/src/transcribe_groq.rs` (same HTTP pattern)

- [ ] **Step 1: Read `transcribe_groq.rs` to understand the existing Groq HTTP pattern**

Open `src-tauri/src/transcribe_groq.rs` and note the `reqwest` client usage — `format_groq.rs` will follow the same pattern but call the `/chat/completions` endpoint instead of `/audio/transcriptions`.

- [ ] **Step 2: Create `src-tauri/src/format_groq.rs`**

```rust
use std::time::Duration;
use reqwest::Client;
use serde_json::json;

const FORMAT_PROMPT: &str = "\
You are a text formatter. The input is a raw speech transcription. \
Clean it up: fix punctuation, capitalize sentences. \
If the content is a list, format it with bullet points and newlines. \
If there are natural paragraph breaks, add them. \
Return ONLY the formatted text — no commentary, no explanation.";

pub async fn format_text(api_key: &str, raw_text: &str) -> Result<String, String> {
    if api_key.is_empty() {
        return Err("Groq API key not set".to_string());
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;

    let body = json!({
        "model": "llama-3.1-8b-instant",
        "messages": [
            { "role": "system", "content": FORMAT_PROMPT },
            { "role": "user",   "content": raw_text }
        ],
        "max_tokens": 1024,
        "temperature": 0.1
    });

    let resp = client
        .post("https://api.groq.com/openai/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Groq request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Groq error {}: {}", status, text));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Groq JSON parse failed: {}", e))?;

    let formatted = json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();

    if formatted.is_empty() {
        return Err("Groq returned empty response".to_string());
    }

    Ok(formatted)
}
```

- [ ] **Step 3: Register the module in `lib.rs`**

Open `src-tauri/src/lib.rs` and add to the top:
```rust
pub mod format_groq;
```

- [ ] **Step 4: Verify it compiles**

```bash
cd src-tauri && cargo check
```
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/format_groq.rs src-tauri/src/lib.rs
git commit -m "feat: add format_groq module for AI text formatting"
```

---

## Task 3: Add `format_mode` to recorder and emit format-error event

**Files:**
- Modify: `src-tauri/src/recorder.rs`

- [ ] **Step 1: Add `format_mode` parameter to `stop_and_transcribe`**

In `src-tauri/src/recorder.rs`, change the signature of `stop_and_transcribe`:

```rust
pub async fn stop_and_transcribe(
    &self,
    app: &AppHandle,
    settings: &Settings,
    app_dir: &PathBuf,
    format_mode: bool,
) -> Result<String, String> {
```

- [ ] **Step 2: Update the text-processing block inside `stop_and_transcribe`**

Find the section after transcription that currently reads:
```rust
// Clean up text
let cleaned = cleanup_text(&raw_text);
```

Replace it with:

```rust
use crate::cleanup::cleanup_text;
use crate::format_groq;

// Clean up / format
let cleaned = if format_mode {
    // Run noise cleanup first (steps 1-3 of cleanup_text pipeline)
    let noise_cleaned = {
        // Re-use cleanup_text for noise removal — it also capitalizes/punctuates
        // but Groq will handle that, so we use it as a convenient noise stripper.
        // strip_noise_only gives us just steps 1-3.
        crate::cleanup::strip_noise_only(&raw_text)
    };

    match format_groq::format_text(&settings.groq_api_key, &noise_cleaned).await {
        Ok(formatted) => formatted,
        Err(e) => {
            eprintln!("[Typr] Groq format failed: {}", e);
            let _ = app.emit("format-error", e);
            cleanup_text(&raw_text)   // full fallback
        }
    }
} else {
    cleanup_text(&raw_text)
};
```

- [ ] **Step 3: Add `strip_noise_only` to `cleanup.rs`**

Open `src-tauri/src/cleanup.rs` and add this function after `cleanup_text`:

```rust
/// Steps 1–3 only: remove noise labels + collapse whitespace. No capitalize/punctuate.
pub fn strip_noise_only(text: &str) -> String {
    let s = remove_noise_labels(text);
    s.split_whitespace().collect::<Vec<&str>>().join(" ")
}
```

Add a test for it inside the `#[cfg(test)]` block:

```rust
#[test]
fn test_strip_noise_only_no_capitalization() {
    let result = strip_noise_only("hello [BLANK_AUDIO] world");
    assert_eq!(result, "hello world");
    // No capital, no period — that's the point
    assert!(!result.ends_with('.'));
    assert_eq!(&result[..1], "h");
}
```

- [ ] **Step 4: Update the existing call to `stop_and_transcribe` in `main.rs`**

In `src-tauri/src/main.rs`, find where `stop_and_transcribe` is called and add `false` as the last argument:

```rust
recorder.stop_and_transcribe(&app, &settings, &app_dir, false).await
```

- [ ] **Step 5: Verify it compiles**

```bash
cd src-tauri && cargo check
```
Expected: no errors.

- [ ] **Step 6: Run all tests**

```bash
cd src-tauri && cargo test
```
Expected: all PASS.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/recorder.rs src-tauri/src/cleanup.rs src-tauri/src/main.rs
git commit -m "feat: add format_mode to recorder, strip_noise_only to cleanup"
```

---

## Task 4: Register second hotkey (`Ctrl+Shift+F`)

**Files:**
- Modify: `src-tauri/src/main.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Read `main.rs` to understand current hotkey registration**

Open `src-tauri/src/main.rs` and find where `CmdOrCtrl+Shift+Space` is registered. The new hotkey follows the exact same pattern.

- [ ] **Step 2: Add `toggle_recording_format` command in `main.rs`**

Find the existing `toggle_recording` command and add a sibling command directly after it:

```rust
#[tauri::command]
async fn toggle_recording_format(
    state: tauri::State<'_, Arc<Mutex<Recorder>>>,
    app: AppHandle,
) -> Result<String, String> {
    let recorder = state.inner().clone();
    let settings = {
        let r = recorder.lock().unwrap();
        r  // need to get settings — follow same pattern as toggle_recording
    };
    // Follow the exact same pattern as toggle_recording but pass format_mode: true
    // to stop_and_transcribe. Copy the full body of toggle_recording and change
    // the stop_and_transcribe call to pass `true`.
    todo!("copy toggle_recording body, change format_mode to true")
}
```

> **Note:** The actual body to copy: open `main.rs`, find `toggle_recording`, copy its entire body into `toggle_recording_format`, then change the `stop_and_transcribe` call's last argument from `false` to `true`.

- [ ] **Step 3: Register the second global shortcut in `lib.rs`**

In `src-tauri/src/lib.rs`, in the `run()` function, find where `CmdOrCtrl+Shift+Space` is registered. Add the second shortcut right after:

```rust
.plugin(
    tauri_plugin_global_shortcut::Builder::new()
        .with_shortcut("CmdOrCtrl+Shift+Space", |app, _shortcut, event| {
            // existing handler body — do not change
        })?
        .with_shortcut("CmdOrCtrl+Shift+F", |app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                let app_clone = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = toggle_recording_format_inner(&app_clone).await;
                });
            }
        })?
        .build(),
)
```

> Follow the same internal dispatch pattern as the existing `CmdOrCtrl+Shift+Space` handler.

- [ ] **Step 4: Set amber overlay mode when AI format hotkey fires**

When `toggle_recording_format` starts recording, set `data-format-mode="true"` on the overlay bar. In the start-recording path inside `toggle_recording_format` (after recording starts):

```rust
if let Some(overlay) = app.get_webview_window("overlay") {
    let _ = overlay.eval(
        "document.getElementById('bar')?.setAttribute('data-format-mode', 'true');"
    );
}
```

When recording ends (in `update_overlay` for `Ready` state, or in `Recorder::stop_and_transcribe` after paste), clear it:

```rust
if let Some(overlay) = app.get_webview_window("overlay") {
    let _ = overlay.eval(
        "document.getElementById('bar')?.removeAttribute('data-format-mode');"
    );
}
```

- [ ] **Step 5: Verify compilation**

```bash
cd src-tauri && cargo check
```
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/main.rs src-tauri/src/lib.rs
git commit -m "feat: register Ctrl+Shift+F as AI format hotkey"
```

---

## Task 5: Amber + error overlay states

**Files:**
- Modify: `src/overlay.html`
- Modify: `src/style.css`

- [ ] **Step 1: Add amber and error CSS to `overlay.html`**

In `src/overlay.html`, inside the `<style>` block, find the existing `.bar[data-state="transcribing"]` rule and add these two new rules directly after it:

```css
/* AI format mode — amber waveform */
.bar[data-format-mode="true"][data-state="recording"] .wave span {
  background: #f5a623;
}

.bar[data-format-mode="true"][data-state="transcribing"] .wave span {
  background: #f5a623;
  animation: wave 0.6s ease-in-out infinite;
}
.bar[data-format-mode="true"][data-state="transcribing"] .wave span:nth-child(1)  { animation-delay: 0.00s; }
.bar[data-format-mode="true"][data-state="transcribing"] .wave span:nth-child(2)  { animation-delay: 0.05s; }
.bar[data-format-mode="true"][data-state="transcribing"] .wave span:nth-child(3)  { animation-delay: 0.10s; }
.bar[data-format-mode="true"][data-state="transcribing"] .wave span:nth-child(4)  { animation-delay: 0.15s; }
.bar[data-format-mode="true"][data-state="transcribing"] .wave span:nth-child(5)  { animation-delay: 0.20s; }
.bar[data-format-mode="true"][data-state="transcribing"] .wave span:nth-child(6)  { animation-delay: 0.25s; }
.bar[data-format-mode="true"][data-state="transcribing"] .wave span:nth-child(7)  { animation-delay: 0.30s; }
.bar[data-format-mode="true"][data-state="transcribing"] .wave span:nth-child(8)  { animation-delay: 0.25s; }
.bar[data-format-mode="true"][data-state="transcribing"] .wave span:nth-child(9)  { animation-delay: 0.20s; }
.bar[data-format-mode="true"][data-state="transcribing"] .wave span:nth-child(10) { animation-delay: 0.15s; }
.bar[data-format-mode="true"][data-state="transcribing"] .wave span:nth-child(11) { animation-delay: 0.10s; }
.bar[data-format-mode="true"][data-state="transcribing"] .wave span:nth-child(12) { animation-delay: 0.05s; }

/* Error state — red flash */
.bar[data-state="error"] .wave span {
  background: #e5484d;
  animation: wave 0.4s ease-in-out infinite;
}
.bar[data-state="error"] .btn { opacity: 0.35; cursor: default; pointer-events: none; }
```

- [ ] **Step 2: Add `format-error` event listener to `overlay.html` script**

In `src/overlay.html`, inside the `<script>` block, add after the existing event listeners:

```js
window.__TAURI__.event.listen('format-error', () => {
  bar.setAttribute('data-state', 'error');
  setTimeout(() => {
    bar.setAttribute('data-state', 'ready');
    bar.hide && bar.hide();
    // overlay will be hidden by recorder's Ready state emit
  }, 1500);
});
```

- [ ] **Step 3: Add `--amber` to `src/style.css`**

In `src/style.css`, find the `:root` block and add after `--yellow`:
```css
--amber: #f5a623;
```

- [ ] **Step 4: Start the dev server and visually verify**

```bash
typr
```

Press `Ctrl+Shift+F` in any text field. Verify:
- Waveform bars are **amber** during recording
- Waveform bars pulse **amber** during transcribing
- If Groq key is missing/wrong, overlay flashes red for ~1.5s then hides

- [ ] **Step 5: Commit**

```bash
git add src/overlay.html src/style.css
git commit -m "feat: amber waveform for AI format mode, red error state"
```

---

## Task 6: Update settings UI

**Files:**
- Modify: `src/index.html`
- Modify: `src/main.ts`

- [ ] **Step 1: Read `src/index.html` to find the hotkey row**

Open `src/index.html` and find the existing hotkey setting row. It will look something like:

```html
<div class="setting-row">
  <div class="setting-label">
    <span class="label-text">Hotkey</span>
    ...
  </div>
  <div class="setting-control">
    <kbd id="hotkey-text">...</kbd>
  </div>
</div>
```

- [ ] **Step 2: Rename existing hotkey row and add AI format hotkey row**

Change the existing hotkey row's label from `Hotkey` to `Quick Paste Hotkey`. Then add a new row directly after it:

```html
<div class="setting-row">
  <div class="setting-label">
    <span class="label-text">AI Format Hotkey</span>
    <span class="label-hint">Record then format with AI before pasting</span>
  </div>
  <div class="setting-control">
    <kbd id="format-hotkey-text">Ctrl+Shift+F</kbd>
  </div>
</div>
```

- [ ] **Step 3: Update `src/main.ts` to display both hotkeys**

Find the line:
```typescript
hotkeyText.textContent = currentSettings.hotkey.replace("CmdOrCtrl", "Ctrl");
```

Add directly below it:
```typescript
const formatHotkeyText = document.getElementById("format-hotkey-text")!;
formatHotkeyText.textContent = "Ctrl+Shift+F";
```

Also add a listener for the `format-error` event in the settings window (for debugging visibility):
```typescript
listen<string>("format-error", (event) => {
  console.warn("[Typr] Groq format error:", event.payload);
});
```

- [ ] **Step 4: Verify in browser**

```bash
typr
```
Open the settings window. Verify the new "AI Format Hotkey" row shows with `Ctrl+Shift+F` keyboard badge. Styling should match the existing hotkey row exactly.

- [ ] **Step 5: Commit**

```bash
git add src/index.html src/main.ts
git commit -m "feat: add AI format hotkey row to settings UI"
```

---

## Task 7: System tray — hide to tray on close

**Files:**
- Modify: `src-tauri/tauri.conf.json`
- Modify: `src-tauri/src/lib.rs` or `src-tauri/src/main.rs`

- [ ] **Step 1: Add `trayIcon` to `tauri.conf.json`**

Open `src-tauri/tauri.conf.json`. Find the `"app"` block and add `trayIcon` inside it:

```json
"trayIcon": {
  "iconPath": "icons/32x32.png",
  "iconAsTemplate": false
}
```

The full `"app"` block should now include:
```json
"app": {
  "withGlobalTauri": true,
  "trayIcon": {
    "iconPath": "icons/32x32.png",
    "iconAsTemplate": false
  },
  "windows": [ ... ],
  ...
}
```

- [ ] **Step 2: Create the tray in Rust**

In `src-tauri/src/main.rs`, in the `main()` function (or in `lib.rs`'s `run()`), after the app is built, add tray setup. Find the `.setup(|app| { ... })` block (or add one) and insert:

```rust
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    menu::{Menu, MenuItem},
    Manager,
};

// Inside .setup(|app| { ... }):
let open_item = MenuItem::with_id(app, "open", "Open Settings", true, None::<&str>)?;
let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
let menu = Menu::with_items(app, &[&open_item, &quit_item])?;

TrayIconBuilder::new()
    .icon(app.default_window_icon().unwrap().clone())
    .menu(&menu)
    .on_menu_event(|app, event| match event.id.as_ref() {
        "open" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "quit" => app.exit(0),
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
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
    })
    .build(app)?;
```

- [ ] **Step 3: Hide to tray instead of quitting on window close**

In the `.setup` block or via a window event listener, intercept the close event:

```rust
if let Some(window) = app.get_webview_window("main") {
    let window_clone = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = window_clone.hide();
        }
    });
}
```

- [ ] **Step 4: Verify compilation**

```bash
cd src-tauri && cargo check
```
Expected: no errors. If `tray` imports are wrong, check Tauri 2 docs — the module path changed between 2.0 betas. Correct path is `tauri::tray::*`.

- [ ] **Step 5: Test tray behaviour**

```bash
typr
```

Verify:
- Tray icon appears in Windows system tray (bottom-right)
- Left-click tray icon → settings window opens
- Right-click → context menu with "Open Settings" and "Quit"
- Closing the settings window hides it (tray icon stays), does not quit the app
- "Quit" from context menu fully exits

- [ ] **Step 6: Commit**

```bash
git add src-tauri/tauri.conf.json src-tauri/src/main.rs src-tauri/src/lib.rs
git commit -m "feat: system tray with hide-to-tray on window close"
```

---

## Task 8: Build .msi installer

**Files:**
- Modify: `src-tauri/tauri.conf.json` (if needed)
- No source changes — this is a build task

- [ ] **Step 1: Verify bundle config in `tauri.conf.json`**

Open `src-tauri/tauri.conf.json`. Confirm `bundle.targets` is `"all"` (already set). On Windows this produces `.msi` automatically. No change needed.

- [ ] **Step 2: Run the build**

```bash
npm run tauri build
```

This will take several minutes (release Rust compilation).

Expected output path:
```
src-tauri/target/release/bundle/msi/Typr_0.1.0_x64_en-US.msi
```

- [ ] **Step 3: Install and test the .msi**

Double-click `Typr_0.1.0_x64_en-US.msi`. Go through the installer. Launch from Start Menu or desktop shortcut.

Verify:
- App launches and tray icon appears
- Both hotkeys work (`Ctrl+Shift+Space` and `Ctrl+Shift+F`)
- Settings window opens from tray
- App closes to tray (not quit) on window close

- [ ] **Step 4: Push to GitHub**

```bash
git push -u origin main
```

---

## Task 9: Push project to GitHub remote

**Files:**
- None — git operations only

- [ ] **Step 1: Verify remote is configured**

```bash
git remote -v
```

Expected: `origin  https://github.com/itsbenraymond/typr (fetch/push)`

If not set:
```bash
git remote add origin https://github.com/itsbenraymond/typr.git
```

- [ ] **Step 2: Set git identity if needed**

```bash
git config user.name "Ben"
git config user.email "pancakegaming29@gmail.com"
```

- [ ] **Step 3: Push**

```bash
git push -u origin main
```

If the remote has existing content (e.g. a README), pull first:
```bash
git pull origin main --allow-unrelated-histories
git push -u origin main
```

---

## Self-Review Checklist

- [x] **Spec coverage:**
  - Enhanced noise cleanup → Task 1
  - AI formatting via Groq → Tasks 2, 3
  - Second hotkey (Ctrl+Shift+F) → Task 4
  - Amber overlay for format mode → Task 5
  - Error state (red flash + fallback) → Tasks 3, 5
  - Settings UI update → Task 6
  - System tray + hide-to-tray → Task 7
  - .msi installer → Task 8
  - GitHub push → Task 9
- [x] **No placeholders** — all code blocks complete (Task 4 Step 2 has a `todo!` with explicit instruction to copy body from existing command)
- [x] **Type consistency** — `format_mode: bool` used consistently across Tasks 3, 4; `strip_noise_only` defined in Task 3 and used in Task 3 only
