# Typr × SpeakType Merge — Design Spec
Date: 2026-05-19

## Overview

Merge SpeakType's text-cleanup quality and AI formatting concept into the existing Typr codebase. Keep 100% of Typr's visual design. Add a second "AI format" hotkey that routes transcription through Groq for smart formatting (lists, paragraphs, punctuation). Add system tray support and produce a packaged Windows .msi installer.

---

## Architecture & Pipeline

Typr stays Tauri (Rust + TypeScript/Vite). No framework changes.

**Hotkey 1 — Quick paste** (`Ctrl+Shift+Space`, existing)
```
record → Whisper (local or Groq) → noise cleanup + capitalize + punctuate → paste
```

**Hotkey 2 — AI format + paste** (`Ctrl+Shift+F`, new)
```
record → Whisper (local or Groq) → noise cleanup → Groq text format → paste
```

Both hotkeys share the same recording flow and overlay. The difference is a `format_mode: bool` flag passed into `stop_and_transcribe`.

---

## Text Cleanup (`cleanup.rs`)

Enhanced to combine SpeakType's noise label removal with Typr's existing sentence cleanup.

**Step 1 — Remove silence/placeholder markers** (from SpeakType)
Patterns: `[BLANK_AUDIO]`, `[SILENCE]`, `<|nospeech|>`, `[S]`, `[ S ]`

**Step 2 — Remove noise labels** (from SpeakType — 26 labels)
Labels surrounded by `[…]` or `(…)`:
`applause`, `background noise`, `blank audio`, `breathing`, `cough`, `coughing`,
`exhale`, `heartbeat`, `indistinct`, `inaudible`, `inhale`, `laughing`, `laughter`,
`loud noise`, `muffled speech`, `music`, `noise`, `silence`, `sigh`, `sighs`,
`sniffing`, `static`, `unclear speech`, `unintelligible`, `wind`, `wind blowing`, `wind noise`

**Step 3 — Collapse whitespace** (both)
Multiple spaces → single space, trim leading/trailing.

**Step 4 — Capitalize sentences + add ending punctuation** (Typr existing)
Capitalize first letter of each sentence. Add `.` if no terminal punctuation.

---

## AI Formatting (`format_groq.rs` — new file)

Called only when `format_mode = true`. Makes a Groq text completion call after basic noise cleanup (Steps 1–3 above; Step 4 skipped — Groq handles formatting).

**Model:** `llama-3.1-8b-instant` (fast, free tier)
**Timeout:** 8 seconds

**Prompt:**
```
You are a text formatter. The input is a raw speech transcription.
Clean it up: fix punctuation, capitalize sentences. If the content is a list,
format it with bullet points and newlines. If there are natural paragraph breaks,
add them. Return ONLY the formatted text — no commentary, no explanation.
```

**Error handling:**
- Groq key not set → emit `format-error` event, overlay flashes red (1.5s), fall back to full `cleanup_text` (all 4 steps), paste anyway
- API timeout / network failure → same red flash + fallback, never blocks paste
- Empty response → fall back to full `cleanup_text` result

---

## Recorder Changes (`recorder.rs`)

`stop_and_transcribe` gains a `format_mode: bool` parameter.

```rust
pub async fn stop_and_transcribe(
    &self,
    app: &AppHandle,
    settings: &Settings,
    app_dir: &PathBuf,
    format_mode: bool,   // NEW
) -> Result<String, String>
```

When `format_mode = true`: after noise cleanup (Steps 1–3), call `format_groq::format_text` instead of Step 4.
When `format_mode = false`: existing pipeline unchanged.

---

## Hotkey Registration (`lib.rs` / `main.rs`)

Register two global shortcuts:
- `CmdOrCtrl+Shift+Space` → `toggle_recording` (existing, `format_mode = false`)
- `CmdOrCtrl+Shift+F` → `toggle_recording_format` (new, `format_mode = true`)

Both commands share the same `Recorder` state — can't trigger both simultaneously.

---

## System Tray

Uses Tauri 2's built-in tray-icon feature (already enabled: `features = ["tray-icon"]` in Cargo.toml — no new dependency).

- Tray icon: `icons/32x32.png`
- Context menu: **Open Settings** | **Quit**
- Window `close` event → hide to tray (not quit)
- On first launch: settings window opens automatically
- No auto-start on Windows boot (not added — user can add manually via Task Scheduler if wanted)

---

## Overlay Visual Diff

When recording in AI format mode, waveform bars are **amber** instead of white. When transcribing in AI format mode, same amber pulse animation. This gives immediate visual feedback about which hotkey was triggered.

Implementation: `overlay.html` checks a `data-format-mode` attribute on `#bar`. `recorder.rs` sets this via `overlay.eval()` when starting AI format recording.

**Error state:** `data-state="error"` — bars turn red, pulse 1.5s, then hide.

---

## Settings Window UI

One new row added to the General section:

| Label | Control |
|-------|---------|
| Quick Paste Hotkey | `kbd: Ctrl+Shift+Space` + Change button |
| AI Format Hotkey | `kbd: Ctrl+Shift+F` + Change button |

No other UI changes. All existing visual design preserved.

---

## Packaging

`tauri.conf.json` changes:
- `productName`, `identifier`, and `version` already set — no change needed
- `bundle.targets` already `"all"` — produces `.msi` on Windows automatically
- Add `trayIcon` block: `{ "iconPath": "icons/32x32.png", "iconAsTemplate": false }`

Build command: `npm run tauri build`
Output: `src-tauri/target/release/bundle/msi/typr_x.x.x_x64_en-US.msi`

---

## File Change Summary

| File | Type | Change |
|------|------|--------|
| `src-tauri/src/cleanup.rs` | Modify | Add Steps 1–2 (noise label removal) before existing logic |
| `src-tauri/src/format_groq.rs` | Create | Groq text formatting call with fallback |
| `src-tauri/src/recorder.rs` | Modify | Add `format_mode` param, route to `format_groq` |
| `src-tauri/src/lib.rs` | Modify | Register second hotkey |
| `src-tauri/src/main.rs` | Modify | Add `toggle_recording_format` command |
| `src-tauri/tauri.conf.json` | Modify | Add tray config, bundle targets |
| `src/index.html` | Modify | Add AI format hotkey row to settings |
| `src/main.ts` | Modify | Show second hotkey, handle `format-error` event |
| `src/style.css` | Modify | Add `--amber` CSS variable for format mode waveform |
| `src/overlay.html` | Modify | Amber waveform + red error state for format mode |

---

## Out of Scope

- Transcription history
- Language selection
- Auto-start on Windows boot
- Any SpeakType Swift code (entirely different stack, not portable)
