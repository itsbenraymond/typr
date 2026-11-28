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
| **History** | `C:\Users\Ben\AppData\Roaming\com.typr.app\history.db` (SQLite) |

**The log is the primary diagnostic.** Every hotkey event, panic, mode
decision, context read and transcription result lands there, with a heartbeat
every 10 minutes so a dead process can be told from an idle one. Read it before
theorising.

This is its **own git repo** (`github.com/itsbenraymond/typr`) and is
gitignored from AI-OS, so the root `git status` shows nothing. Commit and push
from inside this folder. The AI-OS deploy-drift guard compares against this
repo's origin, so **commit and push before deploying** or it blocks.

## Dictation history and the weekly audit (2026-11-22)

Every dictation is a row in `history.db` (`src-tauri/src/history.rs`): time,
mode, app exe + window title, the raw Whisper transcript, the text after the
dictionary, the pasted text, which model produced it (`snippet` / `fallback`
when none did), any warning, and audio / speech-to-text / cleanup / total
milliseconds. The **History** tab (the default when the window opens) lists
them newest first, searchable, with totals, and each one expands to a word
diff of raw against pasted (struck red = said but not pasted, green = pasted
but never said; punctuation and capitals are ignored).

- **A history write can never block or delay a paste.** The row is staged in
  the pipeline and written on its own thread after the paste and the state
  reset. A failure is one `History write FAILED` line in typr.log.
- **The log backfill runs once**, on the first launch of the history build,
  gated by a `meta` row. It imports past `Toggle result:` lines as
  `source = 'log_backfill'` (pasted text, mode and app only; the log never
  had the raw transcript). Lines at or after the first live row are skipped.
- Startup logs `History: N dictations in history.db`.

**Weekly audit** (run by `/business-audit`):

```bash
node projects/typr/scripts/history-audit.mjs            # last 7 days
node projects/typr/scripts/history-audit.mjs --days 14 --stdout
```

Writes `%APPDATA%\com.typr.app\audits\YYYY-MM-DD.md` (it quotes dictation
text, so it stays next to the data and out of git) and prints the path and
counts. It flags dictations where the cleanup probably did something Ben did
not ask for: much shorter or longer than spoken (fillers excluded), words
added that were never said (beyond grammar glue), assistant openers
("Sure", "Here's", "Subject:", `[Your Name]`), a question that came out
answered, and apps that fell back to the default mode. It ends with
dictionary candidates: words the model keeps substituting the same way, and
names spelled more than one way. Fix a flag with a prompt or validation
change (then the live suite) or a dictionary entry, never by editing the
history.

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
- **Few paragraphs, held in code (Ben, 2026-11-24, reversing 2026-11-22).** The
  2026-11-22 rule ("paragraphs required", break at every new item, split anything
  over 60 words) overshot: real dictations came out one sentence per line (#2076,
  49 words over 10 lines) and 108 words in 5 paragraphs (#2074). Ben: "it should
  be two or three. Some things don't need to be broken into paragraphs at all."
  Now the prompt says keep paragraphs few, and `paragraphs.rs` (after the model)
  joins single line breaks back into the paragraph, makes anything under 80
  words one paragraph, caps everything at 3, and only splits a single block over
  140 words. Whitespace and openers only, never words. Lists, a spoken "new
  line"/"new paragraph", Email and Notes keep their layout; Developer and
  Verbatim are never touched. A/B on 4 real transcripts x 3 runs: old prompt 5
  paragraphs + 5 line breaks on #2074, new prompt 1 paragraph and 0 line breaks
  on all 12 runs.

- **Counted lists become lists (Ben, 2026-11-28: "when I say lists it doesn't
  actually make it into a list").** 8 of 8 dictations where he counted items
  off came out as one paragraph. Cause: the base prompt said "no markdown" and
  "never use a single line break", and nothing made a list the exception. Now
  (1) the prompt allows list lines and says counting ("number one... two...",
  "first... second...", "four things: X, Y, Z, W") is a list, while "also" and
  "the other thing" are not; (2) `list_spoken_counts` in `paragraphs.rs` is the
  backstop: 3+ sentences in a row opening with ascending counts become
  numbered lines, because on a 1,400-word ramble (#2167, Ben answering
  Claude's numbered points "two we already do that, three..., four yes")
  gpt-oss-20b kept the sentences flat on every run; (3) `cap_paragraphs` still
  caps the prose around a list at 3 instead of switching off. A/B, 9 cases x 3
  runs: counted cases 0/9 -> 6/6 lists from the prompt, #2167 handled by the
  backstop, all 5 guards (an "also" ramble, a short message, "bullet points"
  said as a word, "that's the first thing") unchanged.
- **Numbered replies said out loud (same day, #2217).** Ben answered 30
  numbered points ("number one... two... seven, do 72 hours...") and the model
  deleted every number. `spoken_count_chain` scans the RAW transcript for an
  ascending chain of count words (4+ long, starts at 1-2, steps of 1-2, under
  150 words apart, skipping "two prospects"-style quantities) and
  `transform.rs` adds a per-dictation note naming the exact numbers said.
  Fires on 4 of 227 history rows, all real numbered replies. #2217: 0/3
  numbered lines without the note, 27 numbered lines (one per number said)
  with it, full point text kept. `drop_placeholder_lines` removes any
  "14. (No content)" line the model invents for a skipped number.

## Failover when Groq is rate-limited or down (2026-11-16)

Groq is still the primary for both steps. When it fails:

| Step | Chain | Key in `config.json` |
|---|---|---|
| Speech-to-text | Groq `whisper-large-v3-turbo` → Groq `whisper-large-v3` → Cloudflare Workers AI `@cf/openai/whisper-large-v3-turbo` → Mistral `voxtral-mini-latest` | `groqApiKey`, `cloudflareAccountId` + `cloudflareApiToken`, `mistralApiKey` |
| Cleanup | Groq `gpt-oss-20b` → `gpt-oss-120b` → Claude `claude-haiku-5-5` | `groqApiKey`, `anthropicApiKey` (+ `anthropicWorkspaceId`) |

- **Claude cannot transcribe audio.** It only backs up the cleanup pass.
- **Long recordings are split, and failed audio is kept (2026-11-28).** A
  17-minute brief was a 31 MB WAV; Groq caps uploads at 25 MB, so every model
  in the chain returned 413 and the audio sat in `temp_recording.wav` until
  the next dictation overwrote it. Now `split_wav` cuts anything over 3 minutes
  on the quietest 100 ms before each mark, the pieces run through the chain 4
  at a time (one more full pass for a piece that fails), and the text is
  joined in order. If everything still fails, the audio is copied to
  `%APPDATA%\com.typr.appailed\<ts>.wav` (newest 30 kept) and the error
  names the path. Get the words back with
  `node projects/typr/scripts/retry-failed.mjs` (writes `<ts>.txt`, copies the
  newest to the clipboard).
- **Groq's audio cap is per hour, per model: 7,200 audio-seconds.** One
  17-minute dictation is 1,020 of them, so a few long rambles in an hour can
  exhaust both Groq models. That is what the Cloudflare step is for (its own
  free quota, same token as `.env` `CLOUDFLARE_API_TOKEN`). Cloudflare's older
  `@cf/openai/whisper` was tried and dropped: 39-78s per 3-minute piece.
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

- **A lowercase drive letter breaks the frontend build.** From a `c:\` working
  directory (VS Code terminals) Vite fails with "No matching HTML proxy module
  found ... overlay.html?html-proxy". Same tree from `C:\` builds fine.
  `scripts/deploy.mjs` now uppercases the drive letter before anything runs;
  a bare `npm run build` from `c:\` still hits it (2026-11-24).
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
cargo test --release                       # 127 offline, fast, no network
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
