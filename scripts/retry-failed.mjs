#!/usr/bin/env node
// Re-transcribe every recording Typr could not (2026-11-28).
//
// When the whole speech-to-text chain fails, Typr copies the audio to
// %APPDATA%\com.typr.app\failed\<unix-ts>.wav instead of losing it. This sends
// each one again: ffmpeg squeezes it to a small mp3 (so size is never the
// problem), Groq turbo -> Groq large-v3 -> Cloudflare turbo, writes the text
// next to the wav as <ts>.txt and copies the newest one to the clipboard.
//
//   node projects/typr/scripts/retry-failed.mjs          # every wav with no .txt yet
//   node projects/typr/scripts/retry-failed.mjs <file>   # one specific wav
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, writeFileSync, mkdirSync } from "node:fs";
import { join, basename } from "node:path";
import { tmpdir } from "node:os";

const appDir = join(process.env.APPDATA, "com.typr.app");
const cfg = JSON.parse(readFileSync(join(appDir, "config.json"), "utf8"));
const failedDir = join(appDir, "failed");
const work = join(tmpdir(), "typr-retry");
mkdirSync(work, { recursive: true });

const files = process.argv[2]
  ? [process.argv[2]]
  : existsSync(failedDir)
    ? readdirSync(failedDir)
        .filter((f) => f.endsWith(".wav") && !existsSync(join(failedDir, f.replace(/\.wav$/, ".txt"))))
        .sort()
        .map((f) => join(failedDir, f))
    : [];
if (!files.length) {
  console.log("Nothing to retry: no failed recordings without a transcript.");
  process.exit(0);
}

// 3-minute mp3 pieces: under every provider's size cap, cut on the timeline.
function pieces(wav) {
  const stem = join(work, basename(wav, ".wav"));
  execFileSync("ffmpeg", ["-y", "-v", "error", "-i", wav, "-ac", "1", "-ar", "16000", "-b:a", "48k",
    "-f", "segment", "-segment_time", "180", `${stem}-%03d.mp3`]);
  return readdirSync(work).filter((f) => f.startsWith(basename(stem) + "-")).sort().map((f) => join(work, f));
}

async function groq(file, model) {
  const fd = new FormData();
  fd.append("file", new Blob([readFileSync(file)], { type: "audio/mpeg" }), "audio.mp3");
  fd.append("model", model);
  fd.append("language", "en");
  fd.append("response_format", "json");
  const r = await fetch("https://api.groq.com/openai/v1/audio/transcriptions", {
    method: "POST", headers: { Authorization: `Bearer ${cfg.groqApiKey}` }, body: fd,
  });
  if (!r.ok) throw new Error(`${model} HTTP ${r.status}: ${(await r.text()).slice(0, 200)}`);
  return (await r.json()).text.trim();
}

async function cloudflare(file) {
  if (!cfg.cloudflareAccountId || !cfg.cloudflareApiToken) throw new Error("no Cloudflare keys in config.json");
  const r = await fetch(
    `https://api.cloudflare.com/client/v4/accounts/${cfg.cloudflareAccountId}/ai/run/@cf/openai/whisper-large-v3-turbo`,
    { method: "POST", headers: { Authorization: `Bearer ${cfg.cloudflareApiToken}`, "Content-Type": "application/json" },
      body: JSON.stringify({ audio: readFileSync(file).toString("base64"), language: "en" }) });
  if (!r.ok) throw new Error(`cloudflare HTTP ${r.status}: ${(await r.text()).slice(0, 200)}`);
  return (await r.json()).result.text.trim();
}

async function one(file) {
  const errs = [];
  for (const f of [() => groq(file, "whisper-large-v3-turbo"), () => groq(file, "whisper-large-v3"), () => cloudflare(file)]) {
    try { return await f(); } catch (e) { errs.push(e.message); }
  }
  throw new Error(errs.join(" | "));
}

let last = "";
for (const wav of files) {
  const parts = pieces(wav);
  const texts = [];
  for (const p of parts) texts.push(await one(p));
  const text = texts.join(" ");
  const out = wav.replace(/\.wav$/, ".txt");
  writeFileSync(out, text + "\n", "utf8");
  console.log(`\n=== ${basename(wav)} (${parts.length} piece${parts.length > 1 ? "s" : ""}, ${text.split(/\s+/).length} words) -> ${out}\n${text}`);
  last = text;
}
if (last) {
  execFileSync("powershell", ["-NoProfile", "-Command", "$input | Set-Clipboard"], { input: last });
  console.log("\nNewest transcript copied to the clipboard.");
}
