// History view: every dictation Typr has pasted, newest first, with a word
// diff of what Whisper heard against what got pasted.
//
// The data lives in history.db (Rust side, src-tauri/src/history.rs). This
// file only reads it through the history_* commands.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface Dictation {
  id: number;
  ts: number;
  source: string;
  mode: string | null;
  appExe: string | null;
  windowTitle: string | null;
  rawText: string | null;
  correctedText: string | null;
  finalText: string;
  model: string | null;
  warning: string | null;
  audioMs: number | null;
  sttMs: number | null;
  cleanupMs: number | null;
  totalMs: number | null;
  wordCount: number;
}

interface Stats {
  total: number;
  totalWords: number;
  sinceCount: number;
  sinceWords: number;
}

interface DiffOp {
  kind: "same" | "removed" | "added";
  text: string;
}

const PAGE = 100;

const listEl = document.getElementById("history-list")!;
const searchEl = document.getElementById("history-search") as HTMLInputElement;
const moreBtn = document.getElementById("history-more")!;
const statTotal = document.getElementById("stat-total")!;
const statWords = document.getElementById("stat-words")!;
const statWeek = document.getElementById("stat-week")!;
const statWeekLabel = document.getElementById("stat-week-label")!;

let offset = 0;
let query = "";
// Bumped on every fresh load so a slow response for an old search can never
// overwrite the results of a newer one.
let generation = 0;

function startOfWeek(): number {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  d.setDate(d.getDate() - ((d.getDay() + 6) % 7)); // back to Monday
  return Math.floor(d.getTime() / 1000);
}

function fmtNum(n: number): string {
  return n.toLocaleString("en-AU");
}

function fmtTime(ts: number): string {
  const d = new Date(ts * 1000);
  const now = new Date();
  const time = d.toLocaleTimeString("en-AU", { hour: "numeric", minute: "2-digit" });
  const day = new Date(d);
  day.setHours(0, 0, 0, 0);
  const today = new Date(now);
  today.setHours(0, 0, 0, 0);
  const diffDays = Math.round((today.getTime() - day.getTime()) / 86400000);
  if (diffDays === 0) return `Today ${time}`;
  if (diffDays === 1) return `Yesterday ${time}`;
  const opts: Intl.DateTimeFormatOptions =
    d.getFullYear() === now.getFullYear()
      ? { weekday: "short", day: "numeric", month: "short" }
      : { day: "numeric", month: "short", year: "numeric" };
  return `${d.toLocaleDateString("en-AU", opts)} ${time}`;
}

function fmtMs(ms: number | null): string | null {
  if (ms == null) return null;
  return ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(1)}s`;
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

async function loadStats() {
  try {
    const s = await invoke<Stats>("history_stats", { sinceTs: startOfWeek() });
    statTotal.textContent = fmtNum(s.total);
    statWords.textContent = fmtNum(s.totalWords);
    statWeek.textContent = fmtNum(s.sinceCount);
    statWeekLabel.textContent = `this week · ${fmtNum(s.sinceWords)} words`;
  } catch (e) {
    console.error("[Typr] history stats failed:", e);
  }
}

async function loadPage(reset: boolean) {
  const gen = reset ? ++generation : generation;
  if (reset) offset = 0;
  let rows: Dictation[];
  try {
    rows = await invoke<Dictation[]>("history_list", {
      query: query || null,
      limit: PAGE,
      offset,
    });
  } catch (e) {
    if (gen !== generation) return;
    listEl.innerHTML = "";
    listEl.append(el("div", "editor-empty", `Could not read the history: ${e}`));
    moreBtn.classList.add("hidden");
    return;
  }
  if (gen !== generation) return;

  if (reset) listEl.innerHTML = "";
  if (reset && rows.length === 0) {
    listEl.append(
      el(
        "div",
        "editor-empty",
        query ? "Nothing matches that." : "Nothing yet. Your next dictation lands here.",
      ),
    );
  }
  rows.forEach((d) => listEl.append(renderItem(d)));
  offset += rows.length;
  moreBtn.classList.toggle("hidden", rows.length < PAGE);
}

function renderItem(d: Dictation): HTMLElement {
  const item = el("div", "history-item");

  const head = el("div", "history-head");
  const meta = el("div", "history-meta");
  meta.append(el("span", "history-time", fmtTime(d.ts)));
  if (d.mode) meta.append(el("span", "history-tag", d.mode));
  if (d.appExe) meta.append(el("span", "history-app", d.appExe));
  if (d.warning) meta.append(el("span", "history-tag warn", "fallback"));
  head.append(meta);

  const text = el("div", "history-text", d.finalText);
  head.append(text);
  item.append(head);

  let detail: HTMLElement | null = null;
  head.addEventListener("click", () => {
    // Selecting text to copy it should not collapse the item.
    if (window.getSelection()?.toString()) return;
    const open = item.classList.toggle("open");
    if (open && !detail) {
      detail = renderDetail(d);
      item.append(detail);
    }
  });

  return item;
}

function renderDetail(d: Dictation): HTMLElement {
  const detail = el("div", "history-detail");

  if (d.rawText != null) {
    const label = el("div", "history-detail-label", "What you said vs what got pasted");
    const diff = el("div", "history-diff");
    detail.append(label, diff);
    invoke<DiffOp[]>("history_diff", { before: d.rawText, after: d.finalText })
      .then((ops) => {
        if (!ops.length) {
          diff.textContent = "(empty)";
          return;
        }
        ops.forEach((op, i) => {
          if (i > 0) diff.append(" ");
          const tag = op.kind === "removed" ? "del" : op.kind === "added" ? "ins" : "span";
          diff.append(el(tag, `diff-${op.kind}`, op.text));
        });
      })
      .catch((e) => {
        diff.textContent = `Could not build the diff: ${e}`;
      });
  } else {
    detail.append(
      el(
        "div",
        "history-detail-note",
        "Recorded before history existed, so only the pasted text was kept.",
      ),
    );
  }

  const facts = el("div", "history-facts");
  const bits = [
    d.model,
    d.warning,
    fmtMs(d.audioMs) && `${fmtMs(d.audioMs)} audio`,
    fmtMs(d.totalMs) && `${fmtMs(d.totalMs)} to text`,
    d.windowTitle,
  ].filter(Boolean) as string[];
  bits.forEach((b) => facts.append(el("span", undefined, b)));

  const copy = el("button", "history-copy", "Copy");
  copy.type = "button";
  copy.addEventListener("click", async (e) => {
    e.stopPropagation();
    try {
      await navigator.clipboard.writeText(d.finalText);
      copy.textContent = "Copied";
    } catch {
      copy.textContent = "Copy failed";
    }
    setTimeout(() => (copy.textContent = "Copy"), 1500);
  });

  const footer = el("div", "history-footer");
  footer.append(facts, copy);
  detail.append(footer);
  return detail;
}

export function refreshHistory() {
  loadStats();
  loadPage(true);
}

let searchTimer: number | undefined;
searchEl.addEventListener("input", () => {
  clearTimeout(searchTimer);
  searchTimer = window.setTimeout(() => {
    query = searchEl.value.trim();
    loadPage(true);
  }, 200);
});

moreBtn.addEventListener("click", () => loadPage(false));

// A new dictation was written. Reload the top, unless Ben is mid-search or
// has paged down, where a jump would lose his place; the stats still update.
listen("history-updated", () => {
  loadStats();
  if (!query && offset <= PAGE) loadPage(true);
});

refreshHistory();
