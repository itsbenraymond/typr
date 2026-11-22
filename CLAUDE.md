# Typr — Ben's dictation app

Tauri + vanilla TS. Ctrl+Shift+Space, Whisper via Groq, auto-pastes into
whatever had focus. **Ben talks to Claude through this app all day**, so a
broken build is not a background annoyance — it takes out his main input
method until it is fixed.

## Ship it with `npm run deploy`. Never `cargo build`.

`cargo build --release` produces a binary that compiles, runs, registers the
hotkey, transcribes, pastes — and points every webview at the Vite dev server.
With no dev server running, each window renders WebView2's **"Can't reach this
page"**. Only `tauri build` embeds the frontend.

Nothing in the binary tells you which kind you have: the dev URL string is
present in a correctly built binary too, because it comes from
`tauri.conf.json` either way. A grep-based check fires on every build. (Tested
before relying on it — see `scripts/deploy.mjs`.)

So the check happens at the delivery layer. `npm run deploy`:

1. builds through the Tauri CLI,
2. stops Typr, installs over `%LOCALAPPDATA%\Typr\typr.exe`, restarts it,
3. reads the live log and **refuses to report success** unless the app says
   `Overlay navigating to: http://tauri.localhost/...`.

It also prints whether the hotkey registered and what the OS thinks of
autostart. If it exits non-zero, Typr may be mid-swap — check it is running
before walking away.

## Where things live

| What | Path |
|---|---|
| Installed binary | `C:\Users\Ben\AppData\Local\Typr\typr.exe` |
| Config | `C:\Users\Ben\AppData\Roaming\com.typr.app\config.json` |
| **Log** | `C:\Users\Ben\AppData\Roaming\com.typr.app\typr.log` |

**The log is the primary diagnostic.** Every hotkey event, panic, mode
decision, context read and transcription result lands there, with a heartbeat
every 10 minutes so a dead process can be told from an idle one. Read it before
theorising.

This is its **own git repo** (`github.com/itsbenraymond/typr`) and is
gitignored from AI-OS, so the root `git status` shows nothing. Commit and push
from inside this folder. The AI-OS deploy-drift guard compares against this
repo's origin, so **commit and push before deploying** or it blocks.

## Locked decisions (Ben, 2026-10-11)

- **Groq cloud, not local inference.** He was asked directly, with a
  fully-local option on the table, and chose cloud for sub-second latency.
  Dictation text leaves the machine. Do not drift this toward local.
- **Mode auto-detects from the focused app**, chosen over a mode hotkey because
  it has to cost zero keystrokes. Terminals and most editors get Developer,
  Gmail gets Email, Slack gets Chat, a Claude tab gets Prompt, unknown apps fall
  back to Clean.
- **VS Code and Cursor get Prompt, not Developer (Ben, 2026-11-22).** That is
  where he talks to Claude Code all day. Developer mode pasted his briefs as one
  wall of text with every "okay" and "right?" kept. Prompt mode still keeps
  paths and identifiers exact.
- **Paragraphs and no opening "Okay," are guaranteed in code, not just asked
  for** (`paragraphs.rs`, after the model). The prompt alone got paragraphs on
  6 of 12 real dictations (A/B 2026-11-22); the backstop splits any output over
  60 words still in one block. Whitespace and openers only, never words.
  Skipped for Developer, Chat and Verbatim.

## Failover when Groq is rate-limited or down (2026-11-16)

Groq is still the primary for both steps. When it fails:

| Step | Chain | Key in `config.json` |
|---|---|---|
| Speech-to-text | Groq `whisper-large-v3-turbo` → Groq `whisper-large-v3` → Mistral `voxtral-mini-latest` | `groqApiKey`, `mistralApiKey` |
| Cleanup | Groq `gpt-oss-20b` → `gpt-oss-120b` → Claude `claude-haiku-5-5` | `groqApiKey`, `anthropicApiKey` (+ `anthropicWorkspaceId`) |

- **Claude cannot transcribe audio.** It only backs up the cleanup pass.
- **Groq's limits are per model.** The outage that started this was turbo
  hitting its 7,200 audio-seconds/hour cap ("try again in 2m") while
  large-v3 answered in 0.6s on the same key. So a 429 moves to the next
  model at once. A 5xx or dropped connection gets one retry after 300ms, and
  a timeout skips the rest of that provider (`failover.rs`).
- Ben's Claude key is an org key not scoped to a workspace. Without
  `anthropicWorkspaceId` it answers 400. haiku-5-5 also rejects
  `temperature` with a 400, so the request doesn't send one.
- The Mistral key had no budget on 2026-11-16 (402 "API budget exhausted"),
  so it only adds about 1s, and only once both Groq models have failed. Top
  it up and the Voxtral step starts working.
- These keys have no field in the Settings UI. They are set in `config.json`
  and survive a save, because the UI sends back the whole object it loaded.
- Live fallback tests: `claude_answers_when_groq_fails` (needs
  `TYPR_LIVE_CLAUDE_KEY`, plus `TYPR_LIVE_CLAUDE_WORKSPACE`) and
  `falls_over_to_the_next_whisper_model` (needs `TYPR_LIVE_WAV`, any speech
  wav).

## Things that bite

- **Groq free tier: 30 requests and 8,000 tokens per minute.** `max_tokens`
  counts against the token budget whether the model uses it or not, so a flat
  4096 allowed about two dictations a minute before 429s. `budget_tokens()`
  sizes the reservation to the utterance. Watch this if the prompt grows.
- **Groq is not deterministic at temperature 0.** Measured variance is roughly
  5 in 6 on self-correction handling. A single call proves nothing about a
  prompt change — run a case several times before concluding anything. One
  "improvement" looked like a regression purely on a single sample.
- **A mode that contradicts the base prompt must replace it, not append to
  it.** Verbatim mode's "keep the filler words" lost to the base rule that said
  to remove them, silently, for as long as it existed. `Mode::base_prompt_override`
  is how a mode opts out of the shared rules.
- **The dictation is never addressed to the cleanup model.** Ben dictates
  requests to Claude, ChatGPT and people all day ("write an email to X",
  "can you check Y"). With the transcript alone in the user turn, the model
  read those as requests to itself and carried them out: typr.log has whole
  emails, `Subject:` and `[Your Name]` included, pasted into ChatGPT tabs.
  Fixed 2026-11-19 in three layers: `user_message()` states the job and fences
  the text in `<transcript>`, the base prompt says who the speaker is talking
  to with instruction-shaped examples, and `validate()` rejects an output that
  opens like an assistant ("Sure", "I'll", "Here's", "Subject:") when the
  speaker did not open that way. Prompt mode no longer restructures into
  headings: Ben wants his words cleaned, not rewritten as a brief.
- **The dictionary runs before AND after the model.** The model re-breaks names
  it has never seen. Matching is a single longest-first scan over chars, so
  entries like `C++` are literal and a longer phrase beats a shorter one that
  overlaps it.
- **The app dies on a tao event-loop teardown panic** ("cannot move state from
  Destroyed"), still unfixed upstream. `supervisor.rs` relaunches on that
  specific panic class with a rate limiter. It is a workaround, not a fix — if
  tao ever fixes it, this can go.

## The live test suite is the one that matters

```bash
cd src-tauri
cargo test --release                       # 85 offline, fast, no network
TYPR_LIVE_GROQ_KEY=gsk_... cargo test --release -- --ignored --test-threads=1
```

The live suite hits the real API and pins the model ids, filler stripping,
retraction handling (including the bare-"no" case that used to **invert the
meaning** of a sentence), developer-mode command preservation, verbatim keeping
its fillers, and the "never answer the dictation" rule.

A decommissioned model 404'd on **every single dictation for weeks** while the
offline suite stayed green and the app looked healthy, because the failure path
silently pasted the raw transcript. Offline tests cannot catch that. **Run the
live suite after any change to the model chain, the prompts or the validation**,
and pace it — the rate limits above will otherwise fail the tests instead of
the code.
