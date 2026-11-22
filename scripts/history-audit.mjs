#!/usr/bin/env node
/**
 * Weekly Typr cleanup audit.
 *
 * Reads history.db (written by the app on every dictation, see
 * src-tauri/src/history.rs) and lists the dictations where the AI cleanup
 * probably did something Ben did not ask for, plus the names Whisper keeps
 * spelling different ways (dictionary candidates).
 *
 *   node scripts/history-audit.mjs                 # last 7 days
 *   node scripts/history-audit.mjs --days 14
 *   node scripts/history-audit.mjs --out report.md
 *   node scripts/history-audit.mjs --stdout        # print, write nothing
 *   node scripts/history-audit.mjs --db path\to\history.db
 *
 * Default report path: %APPDATA%\com.typr.app\audits\YYYY-MM-DD.md. It holds
 * dictation text, so it stays next to the data and out of every git repo.
 *
 * Every check is a heuristic. A flag means "look at this one", not "this is
 * wrong". The diff compares the dictionary-corrected transcript against the
 * pasted text, so a dictionary fix never counts as a model edit, and
 * punctuation and capitalisation are ignored (same rule as the in-app diff).
 */
import { DatabaseSync } from "node:sqlite";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

// ── args ──────────────────────────────────────────────────────────────────
const args = process.argv.slice(2);
const arg = (name, fallback) => {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1] : fallback;
};
const appDir = join(process.env.APPDATA || join(homedir(), "AppData", "Roaming"), "com.typr.app");
const dbPath = arg("--db", join(appDir, "history.db"));
const days = Number(arg("--days", "7"));
const toStdout = args.includes("--stdout");
const today = new Date().toISOString().slice(0, 10);
const outPath = arg("--out", join(appDir, "audits", `${today}.md`));

if (!existsSync(dbPath)) {
  console.error(`No history database at ${dbPath}. Has Typr run since the history build was deployed?`);
  process.exit(1);
}

// ── data ──────────────────────────────────────────────────────────────────
const db = new DatabaseSync(dbPath, { readOnly: true });
const since = Math.floor(Date.now() / 1000) - days * 86400;
const rows = db
  .prepare(
    `SELECT id, ts, source, mode, app_exe, window_title, raw_text, corrected_text,
            final_text, model, warning, total_ms
     FROM dictations WHERE ts >= ? ORDER BY ts ASC`,
  )
  .all(since);

let dictionary = [];
try {
  const cfg = JSON.parse(readFileSync(join(appDir, "config.json"), "utf8"));
  dictionary = (cfg.dictionary || []).filter((d) => d.enabled !== false);
} catch {
  /* no config, no dictionary: the vocabulary section just lists everything */
}
const knownTerms = new Set(dictionary.map((d) => squash(d.term)));

// ── text helpers (same normalisation as history.rs norm_word) ─────────────
function norm(w) {
  return w
    .toLowerCase()
    .replace(/[^\p{L}\p{N}']/gu, "")
    .replace(/^'+|'+$/g, "");
}
function words(s) {
  return (s || "").split(/\s+/).filter(Boolean);
}
function squash(s) {
  return (s || "").toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");
}

/** Letters-only edit similarity, 0..1. "super base" vs "Supabase" is 0.78. */
function similar(x, y) {
  const a = squash(x);
  const b = squash(y);
  if (!a || !b) return 0;
  const prev = Array.from({ length: b.length + 1 }, (_, j) => j);
  for (let i = 1; i <= a.length; i++) {
    let diag = prev[0];
    prev[0] = i;
    for (let j = 1; j <= b.length; j++) {
      const tmp = prev[j];
      prev[j] = Math.min(prev[j] + 1, prev[j - 1] + 1, diag + (a[i - 1] === b[j - 1] ? 0 : 1));
      diag = tmp;
    }
  }
  return 1 - prev[b.length] / Math.max(a.length, b.length);
}

/** Word diff on normalised keys. Returns [{kind, words:[original tokens]}]. */
function diff(before, after) {
  const a = words(before);
  const b = words(after);
  const ka = a.map(norm);
  const kb = b.map(norm);
  const n = a.length;
  const m = b.length;
  const eq = (i, j) => ka[i] !== "" && ka[i] === kb[j];
  const ops = [];
  if (n * m > 4_000_000) {
    a.forEach((w) => ops.push(["removed", w]));
    b.forEach((w) => ops.push(["added", w]));
  } else {
    const w = m + 1;
    const t = new Uint32Array((n + 1) * (m + 1));
    for (let i = n - 1; i >= 0; i--)
      for (let j = m - 1; j >= 0; j--)
        t[i * w + j] = eq(i, j) ? t[(i + 1) * w + j + 1] + 1 : Math.max(t[(i + 1) * w + j], t[i * w + j + 1]);
    let i = 0;
    let j = 0;
    while (i < n && j < m) {
      if (eq(i, j)) {
        ops.push(["same", b[j]]);
        i++;
        j++;
      } else if (t[(i + 1) * w + j] >= t[i * w + j + 1]) ops.push(["removed", a[i++]]);
      else ops.push(["added", b[j++]]);
    }
    while (i < n) ops.push(["removed", a[i++]]);
    while (j < m) ops.push(["added", b[j++]]);
  }
  const merged = [];
  for (const [kind, word] of ops) {
    const last = merged[merged.length - 1];
    if (last && last.kind === kind) last.words.push(word);
    else merged.push({ kind, words: [word] });
  }
  return merged;
}

// Words a faithful cleanup is allowed to add: grammar glue, and the
// expansions of contractions or slang it is told to tidy.
const GLUE = new Set(
  `a an the and or but so to of in on at for with is are was were be been it its it's
   that this i i'm you we they he she them us my your our their me do does did not no
   going gonna want wanna got have has had will would can could should just also then
   as if by from up out about there here what which who when where how all any some`
    .split(/\s+/)
    .filter(Boolean),
);
const FILLERS = new Set(
  `um uh ah er hmm like okay ok alright so yeah cool anyway basically literally right
   actually you know i mean sort kind of pretty much guess whatever honestly`
    .split(/\s+/)
    .filter(Boolean),
);

const OPENERS = [
  /^sure\b/i,
  /^certainly\b/i,
  /^of course\b/i,
  /^absolutely\b/i,
  /^here('s| is| are)\b/i,
  /^i('ll| will) /i,
  /^subject:/i,
  /^dear\b/i,
  /^(hi|hello|hey)\b/i,
  /^great question/i,
  /^got it\b/i,
];
const QUESTION_START =
  /^(can|could|would|will|should|is|are|do|does|did|what|why|how|when|where|who|which|have|has)\b/i;

// ── checks ────────────────────────────────────────────────────────────────
const flagged = []; // {row, reasons:[], diff}
const subs = new Map(); // "heard -> pasted" => {count, examples}
let liveCount = 0;

for (const r of rows) {
  const reasons = [];
  const fin = r.final_text || "";
  const said = r.corrected_text ?? r.raw_text;
  const finLow = fin.trim().toLowerCase();

  // Assistant-style opener the speaker did not say. Works for backfilled
  // rows too, since it only needs the pasted text.
  const opener = OPENERS.find((re) => re.test(fin.trim()));
  if (opener && !(said && opener.test(said.trim()))) {
    reasons.push(`opens like an assistant ("${fin.trim().split(/\s+/).slice(0, 3).join(" ")}")`);
  }
  if (/\[your name\]|\[name\]|\[recipient/i.test(fin)) reasons.push("placeholder like [Your Name]");

  let d = null;
  if (said != null && r.source === "live") {
    liveCount++;
    const sw = words(said).filter((w) => !FILLERS.has(norm(w)));
    const fw = words(fin);
    d = diff(said, fin);

    // Length: compared against what was said minus fillers, which a cleanup
    // is supposed to drop anyway.
    if (sw.length >= 8) {
      const ratio = fw.length / sw.length;
      if (ratio < 0.6) reasons.push(`much shorter than spoken (${fw.length} vs ${sw.length} words, fillers excluded)`);
      if (ratio > 1.25) reasons.push(`longer than spoken (${fw.length} vs ${sw.length} words)`);
    }

    // Words the speaker never said, beyond grammar glue. A respelling of
    // what was just removed ("super base" -> "Supabase") is a mishearing
    // fixed, not an invention; it shows up in the vocabulary table instead.
    const invented = d
      .filter((op) => op.kind === "added")
      .filter((op) => {
        const i = d.indexOf(op);
        const prev = d[i - 1];
        return !(prev && prev.kind === "removed" && similar(prev.words.join(" "), op.words.join(" ")) >= 0.6);
      })
      .flatMap((op) => op.words)
      .map(norm)
      .filter((w) => w && !GLUE.has(w) && !FILLERS.has(w));
    if (invented.length >= 3 || (invented.length >= 1 && invented.length / Math.max(sw.length, 1) > 0.15)) {
      reasons.push(`added words not spoken: ${[...new Set(invented)].slice(0, 12).join(", ")}`);
    }

    // A question that came out as a statement and grew: probably answered.
    const saidQ = QUESTION_START.test(said.trim()) || said.includes("?");
    if (saidQ && !fin.includes("?") && (fw.length > sw.length || /^(yes|no|sure)\b/.test(finLow))) {
      reasons.push("question went in, no question came out (answered?)");
    }

    // Substitutions: a removed run straight followed by an added run, both
    // short. Recurring ones are Whisper mishearing a word the same way.
    for (let i = 0; i + 1 < d.length; i++) {
      if (d[i].kind === "removed" && d[i + 1].kind === "added" && d[i].words.length <= 3 && d[i + 1].words.length <= 3) {
        const heard = d[i].words.map(norm).filter((w) => !FILLERS.has(w)).join(" ");
        const pasted = d[i + 1].words.map((w) => w.replace(/[.,!?;:"]+$/g, "")).join(" ");
        if (!heard || squash(heard) === squash(pasted)) continue;
        if (d[i + 1].words.every((w) => GLUE.has(norm(w)))) continue;
        const key = `${heard} -> ${pasted}`;
        const e = subs.get(key) || { heard, pasted, count: 0 };
        e.count++;
        subs.set(key, e);
      }
    }
  }

  if (reasons.length) flagged.push({ r, reasons, d });
}

// ── modes ─────────────────────────────────────────────────────────────────
// Which apps fell back to the default mode (unrecognised: an override
// candidate), and which app got more than one mode this week.
const byApp = new Map();
for (const r of rows) {
  if (!r.app_exe) continue;
  const key = r.app_exe;
  const e = byApp.get(key) || { modes: new Map(), titles: new Map(), n: 0 };
  e.n++;
  e.modes.set(r.mode, (e.modes.get(r.mode) || 0) + 1);
  if (r.mode === "clean" && r.window_title) e.titles.set(r.window_title, (e.titles.get(r.window_title) || 0) + 1);
  byApp.set(key, e);
}

// ── vocabulary ────────────────────────────────────────────────────────────
// Capitalised words and runs of them, grouped by their letters, so "Go High
// Level", "GoHighLevel" and "Gohighlevel" land in one group.
const names = new Map();
for (const r of rows) {
  const toks = words(r.final_text);
  for (let i = 0; i < toks.length; i++) {
    const clean = toks[i].replace(/^[("']+|[)"'.,!?;:]+$/g, "");
    const sentenceStart = i === 0 || /[.!?:]$/.test(toks[i - 1]) || /\n\s*$/.test(toks[i - 1]);
    if (!/^\p{Lu}/u.test(clean) || clean === "I" || clean.startsWith("I'")) continue;
    if (sentenceStart && !/\p{Lu}.*\p{Lu}/u.test(clean)) continue; // ordinary sentence start
    let run = [clean];
    let j = i + 1;
    while (j < toks.length && run.length < 3 && !/[.,!?;:]$/.test(toks[j - 1])) {
      const c = toks[j].replace(/^[("']+|[)"'.,!?;:]+$/g, "");
      if (!/^\p{Lu}/u.test(c)) break;
      run.push(c);
      j++;
    }
    const spelling = run.join(" ");
    const key = squash(spelling);
    if (key.length < 3 || knownTerms.has(key)) continue;
    const e = names.get(key) || { spellings: new Map(), count: 0 };
    e.count++;
    e.spellings.set(spelling, (e.spellings.get(spelling) || 0) + 1);
    names.set(key, e);
    i = j - 1;
  }
}
const nameRows = [...names.values()]
  .filter((e) => e.spellings.size > 1 || e.count >= 3)
  .sort((a, b) => b.spellings.size - a.spellings.size || b.count - a.count)
  .slice(0, 25);
const subRows = [...subs.values()].filter((e) => e.count >= 2).sort((a, b) => b.count - a.count).slice(0, 25);

// ── report ────────────────────────────────────────────────────────────────
const fmtDate = (ts) =>
  new Date(ts * 1000).toLocaleString("en-AU", { weekday: "short", day: "numeric", month: "short", hour: "numeric", minute: "2-digit" });
const quote = (s, max = 400) => {
  const t = (s || "").replace(/\s+/g, " ").trim();
  return t.length > max ? t.slice(0, max) + "..." : t;
};
const renderDiff = (d) =>
  d
    .map((op) =>
      op.kind === "same" ? op.words.join(" ") : op.kind === "removed" ? `~~${op.words.join(" ")}~~` : `**${op.words.join(" ")}**`,
    )
    .join(" ");

const totalWords = rows.reduce((n, r) => n + words(r.final_text).length, 0);
const fallbacks = rows.filter((r) => r.model === "fallback").length;
const models = new Map();
rows.forEach((r) => models.set(r.model || "unknown", (models.get(r.model || "unknown") || 0) + 1));

const L = [];
L.push(`# Typr cleanup audit, last ${days} days (to ${today})`);
L.push("");
L.push(
  `${rows.length} dictations, ${totalWords.toLocaleString("en-AU")} words. ${liveCount} have the raw transcript to diff against. ` +
    `**${flagged.length} flagged** for a look. ${fallbacks} pasted uncleaned because the AI cleanup failed.`,
);
L.push("");
L.push(`Models: ${[...models].sort((a, b) => b[1] - a[1]).map(([m, n]) => `${m} ${n}`).join(", ")}.`);
L.push("");
L.push("Diffs: ~~struck~~ = said but not pasted, **bold** = pasted but never said. Punctuation and capitals are ignored.");
L.push("");

L.push(`## Flagged dictations (${flagged.length})`);
L.push("");
if (!flagged.length) L.push("Nothing flagged. The cleanup did what it was asked.");
flagged
  .slice()
  .reverse()
  .slice(0, 60)
  .forEach(({ r, reasons, d }, i) => {
    L.push(`### ${i + 1}. ${fmtDate(r.ts)} · ${r.mode || "?"} · ${r.app_exe || "?"} · ${r.model || "?"} (#${r.id})`);
    L.push("");
    reasons.forEach((x) => L.push(`- ${x}`));
    L.push("");
    if (d) L.push(`> ${quote(renderDiff(d), 900)}`);
    else L.push(`> Pasted: ${quote(r.final_text)}`);
    L.push("");
  });
if (flagged.length > 60) L.push(`_${flagged.length - 60} more not shown._\n`);

L.push("## Modes");
L.push("");
const fellBack = [...byApp].filter(([, e]) => e.modes.has("clean"));
if (fellBack.length) {
  L.push("Apps that got **clean** (the fallback for apps Typr does not recognise). An override in Settings > Modes fixes these:");
  L.push("");
  fellBack
    .sort((a, b) => b[1].modes.get("clean") - a[1].modes.get("clean"))
    .forEach(([app, e]) => {
      const tops = [...e.titles].sort((a, b) => b[1] - a[1]).slice(0, 3).map(([t]) => `"${quote(t, 60)}"`).join(", ");
      L.push(`- \`${app}\`: ${e.modes.get("clean")} of ${e.n}${tops ? ` (e.g. ${tops})` : ""}`);
    });
} else L.push("No dictation fell back to the default mode.");
L.push("");
const multi = [...byApp].filter(([, e]) => e.modes.size > 1);
if (multi.length) {
  L.push("Apps that got more than one mode (normal for browsers, where the tab decides):");
  L.push("");
  multi.forEach(([app, e]) =>
    L.push(`- \`${app}\`: ${[...e.modes].map(([m, n]) => `${m || "?"} ${n}`).join(", ")}`),
  );
  L.push("");
}

L.push("## Vocabulary: dictionary candidates");
L.push("");
L.push("Words the model swapped for something else more than once. If the right side is the right word, add it to the dictionary with the left side as a mishearing, so it is fixed before the model ever sees it:");
L.push("");
if (subRows.length) {
  L.push("| Whisper heard | Pasted as | Times |");
  L.push("|---|---|---|");
  subRows.forEach((e) => L.push(`| ${e.heard} | ${e.pasted} | ${e.count} |`));
} else L.push("_No repeated substitutions this period._");
L.push("");
L.push("Names spelled more than one way, or said often and not yet in the dictionary:");
L.push("");
if (nameRows.length) {
  L.push("| Spellings seen | Times |");
  L.push("|---|---|");
  nameRows.forEach((e) =>
    L.push(`| ${[...e.spellings].sort((a, b) => b[1] - a[1]).map(([s, n]) => `${s} (${n})`).join(", ")} | ${e.count} |`),
  );
} else L.push("_None._");
L.push("");

const report = L.join("\n");
if (toStdout) {
  process.stdout.write(report + "\n");
} else {
  mkdirSync(dirname(outPath), { recursive: true });
  writeFileSync(outPath, report);
  console.log(`Typr audit: ${rows.length} dictations, ${flagged.length} flagged, ${subRows.length} substitution + ${nameRows.length} name candidates`);
  console.log(`Report: ${outPath}`);
}
