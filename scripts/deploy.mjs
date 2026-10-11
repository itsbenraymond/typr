#!/usr/bin/env node
/**
 * Build Typr and install it over the copy Ben actually runs.
 *
 * Why this exists
 * ---------------
 * On 2026-10-11 a build was deployed with plain `cargo build --release`. It
 * compiled, the hotkey worked, transcription worked, autostart worked — and
 * every window rendered WebView2's "Can't reach this page" error, because a
 * raw cargo build leaves the app pointing at the Vite dev server:
 *
 *     Overlay navigating to: http://localhost:1420/src/overlay.html
 *
 * Only `tauri build` embeds the frontend. Nothing about the binary tells you
 * which kind you have — the dev URL string is present in both, because it
 * comes from tauri.conf.json either way — so the check has to happen at the
 * delivery layer: start the app and read which URL it actually navigated to.
 *
 * This script therefore (a) builds only through the Tauri CLI, so the wrong
 * build cannot be produced here, and (b) refuses to call the deploy a success
 * until the running app reports loading from tauri.localhost.
 *
 *   npm run deploy
 */
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, readFileSync, existsSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(fileURLToPath(new URL(".", import.meta.url)), "..");
const builtExe = join(root, "src-tauri", "target", "release", "typr.exe");
const installedExe = join(homedir(), "AppData", "Local", "Typr", "typr.exe");
const logPath = join(homedir(), "AppData", "Roaming", "com.typr.app", "typr.log");

const GOOD_HOST = "http://tauri.localhost";
const DEV_HOST = "localhost:1420";

function run(cmd, args, opts = {}) {
  const res = spawnSync(cmd, args, { stdio: "inherit", shell: true, cwd: root, ...opts });
  if (res.status !== 0) {
    throw new Error(`${cmd} ${args.join(" ")} exited ${res.status}`);
  }
}

function ps(script) {
  return execFileSync("powershell", ["-NoProfile", "-Command", script], {
    encoding: "utf8",
  }).trim();
}

function logSize() {
  return existsSync(logPath) ? statSync(logPath).size : 0;
}

function logSince(offset) {
  if (!existsSync(logPath)) return "";
  const buf = readFileSync(logPath);
  return buf.subarray(offset).toString("utf8");
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ── 1. Build, the only supported way ──────────────────────────────────────
console.log("→ building via the Tauri CLI (never plain cargo: it bakes in the dev URL)");
run("npx", ["tauri", "build", "--no-bundle"]);

if (!existsSync(builtExe)) {
  throw new Error(`build reported success but ${builtExe} is missing`);
}

// ── 2. Swap the running app ───────────────────────────────────────────────
const offset = logSize();
console.log("→ stopping the running Typr");
ps(`Get-Process -Name typr -ErrorAction SilentlyContinue | ForEach-Object { Stop-Process -Id $_.Id -Force }`);
await sleep(1000);

console.log(`→ installing to ${installedExe}`);
copyFileSync(builtExe, installedExe);

console.log("→ starting Typr");
ps(`Start-Process '${installedExe}'`);

// ── 3. Verify at the delivery layer, not at the build ─────────────────────
// The pill is created during setup, so its navigation line appears within a
// couple of seconds of launch.
let tail = "";
for (let i = 0; i < 20; i++) {
  await sleep(500);
  tail = logSince(offset);
  if (tail.includes("Overlay navigating to:")) break;
}

const navLine = tail
  .split(/\r?\n/)
  .filter((l) => l.includes("Overlay navigating to:"))
  .pop();

if (!navLine) {
  console.error("\n✗ DEPLOY UNVERIFIED — the app never logged an overlay navigation.");
  console.error("  Typr may have failed to start. Recent log:\n");
  console.error(tail.split(/\r?\n/).slice(-15).join("\n"));
  process.exit(1);
}

if (navLine.includes(DEV_HOST)) {
  console.error(`\n✗ DEPLOY REJECTED — the installed app is pointing at the dev server:`);
  console.error(`  ${navLine.trim()}`);
  console.error("  Every window will show \"Can't reach this page\".");
  console.error("  This means the binary was not produced by `tauri build`.");
  process.exit(1);
}

if (!navLine.includes(GOOD_HOST)) {
  console.error(`\n✗ DEPLOY REJECTED — unexpected overlay URL:\n  ${navLine.trim()}`);
  process.exit(1);
}

const hotkeyOk = tail.includes("Global shortcut registered successfully");
const autostart = (tail.match(/Autostart: setting=(\w+) os=(\w+)/) || []).slice(1);

console.log("\n✓ deployed and verified live:");
console.log(`  overlay   ${navLine.trim().replace(/^\[\d+\]\s*/, "")}`);
console.log(`  hotkey    ${hotkeyOk ? "registered" : "NOT REGISTERED — check the log"}`);
if (autostart.length) {
  console.log(`  autostart setting=${autostart[0]} os=${autostart[1]}`);
}
if (!hotkeyOk) process.exit(1);
