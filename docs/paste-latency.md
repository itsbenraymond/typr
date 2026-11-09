# Paste lag: where the time goes

Branch `gigaprompt/typr-2026-11-09`. Written on the VPS, which cannot run Typr,
so nothing below has been timed on BensPC yet. Every dictation now writes one
line to `typr.log`:

```
Timing: wav=..ms transcribe=..ms transform=..ms paste=..ms total=..ms chars=..
Timing: groq prewarm ok in ..ms
```

`total` runs from the moment recording stops to the moment Ctrl+V has been
sent. Do 10 dictations of different lengths on this branch and on `main`
(main has no timing line; use a stopwatch or compare against the log
timestamps) and compare.

## Changed on this branch (safe, small)

| Change | Where | Expected saving |
|---|---|---|
| One shared, pooled HTTP client for both Groq calls. Before, transcription and cleanup each built a new `reqwest::Client`, so every dictation did two fresh TCP+TLS handshakes to api.groq.com back to back. | `src-tauri/src/http.rs`, `format_groq.rs:120`, `transcribe_groq.rs:25` | One handshake per dictation (cleanup reuses the transcription connection). From Melbourne to Groq's US edge that is a few round trips, likely 100-400ms. |
| Prewarm: a HEAD to `https://api.groq.com/` when recording starts, so the connection is open before the transcription request. No API key, no rate-limit cost. | `recorder.rs:81`, `http.rs` | The other handshake, same size. |
| Skip `SetForegroundWindow` and its 150ms settle when the target window already has focus (the pill is created with `.focused(false)`, so this is the normal case). | `paste.rs:43-55` | 150ms on almost every paste. |

The per-attempt cleanup timeout (8s) moved from the client to each request;
behaviour is the same.

## Suspected, not changed (measure first)

| Source | Where | What to measure on BensPC |
|---|---|---|
| Fixed 100ms sleep before the paste. Probably there so the hotkey's Ctrl/Shift are physically released before Ctrl+V. In toggle mode they were released long ago. | `paste.rs:39` | Set to 0 in toggle mode only and dictate into Notepad, Chrome, Slack, VS Code. If any paste comes through as Ctrl+Shift+V or nothing, keep it. |
| Fixed 50ms sleep after setting the clipboard. | `paste.rs:20` | Try 10ms. Watch for the previous clipboard contents being pasted instead. |
| The WAV is written to disk, read back, and uploaded uncompressed (16kHz mono 16-bit, about 32KB per second of speech, so a 30s note is about 1MB). Upload speed on home upstream may dominate `transcribe=`. | `audio.rs:129` (`stop_and_save`), `transcribe_groq.rs` | Compare `transcribe=` for 5s vs 30s dictations. If it grows a lot with length, send FLAC (Groq accepts it) instead of WAV. |
| Cleanup model chain. On a 429 or timeout the next model is tried, adding a full round trip. | `format_groq.rs:56` (`MODEL_CHAIN`), `:72` (8s timeout) | Look for "Format attempt failed" lines next to slow `transform=` values. |
| `paste_text` uses `std::thread::sleep` inside an async task, blocking a runtime worker for up to 300ms. Not lag by itself, but it can delay anything else scheduled on that worker. | `paste.rs`, called from `recorder.rs:208` | Only matters if `paste=` is much larger than 150-300ms. |
| Context read (`OpenProcess`, `QueryFullProcessImageNameW`) before the model call. | `recorder.rs:179`, `appctx.rs` | Should be under 5ms; only suspicious if `transform=` is high with the model call itself fast. |

Side note found while here, not a latency issue: the paste overwrites Ben's
clipboard and never restores it.
