import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

interface ModeOverride {
  app: string;
  mode: string;
}

interface DictEntry {
  term: string;
  misheard: string[];
  enabled: boolean;
}

interface Snippet {
  trigger: string;
  text: string;
  enabled: boolean;
}

interface Settings {
  microphone: string;
  groqApiKey: string;
  recordingMode: string;
  hotkey: string;
  autostart: boolean;
  autoMode: boolean;
  defaultMode: string;
  modeOverrides: ModeOverride[];
  dictionary: DictEntry[];
  snippets: Snippet[];
  customInstructions: string;
  listFormatting: boolean;
  commandPrefix: boolean;
  correctionWatch: boolean;
  correctionWatchSecs: number;
}

interface Proposal {
  id: number;
  original: string;
  corrected: string;
  context: string;
  kind: "substitution" | "case-only" | "punctuation-only";
  firstSeen: number;
  lastSeen: number;
  count: number;
  status: "pending" | "approved" | "rejected";
}

interface LogEntry {
  seq: number;
  at: number;
  action: string;
  proposalId: number | null;
  effect: { type: string; term?: string; misheard?: string | string[]; entry?: DictEntry };
  undoes: number | null;
}

interface MicDevice {
  name: string;
  is_default: boolean;
}

// DOM elements
const statusDot = document.getElementById("status-dot")!;
const statusText = document.getElementById("status-text")!;
const micSelect = document.getElementById("mic-select") as HTMLSelectElement;
const groqKey = document.getElementById("groq-key") as HTMLInputElement;
const modeToggle = document.getElementById("mode-toggle")!;
const modePtt = document.getElementById("mode-ptt")!;
const hotkeyText = document.getElementById("hotkey-text")!;
const autostartToggle = document.getElementById("autostart-toggle") as HTMLInputElement;
const autostartHint = document.getElementById("autostart-hint")!;

// Section navigation
const navItems = document.querySelectorAll(".nav-item");
const sections = document.querySelectorAll(".content-section");

navItems.forEach((item) => {
  item.addEventListener("click", () => {
    const target = item.getAttribute("data-section");
    navItems.forEach((n) => n.classList.remove("active"));
    sections.forEach((s) => s.classList.remove("active"));
    item.classList.add("active");
    document.getElementById(`section-${target}`)?.classList.add("active");
  });
});

// Window drag — titlebar and sidebar empty space
const titlebar = document.getElementById("titlebar")!;
const sidebar = document.getElementById("sidebar")!;
const appWindow = getCurrentWindow();

titlebar.addEventListener("mousedown", (e) => {
  if ((e.target as HTMLElement).closest("button, select, input, a, .nav-item")) return;
  appWindow.startDragging();
});

sidebar.addEventListener("mousedown", (e) => {
  if ((e.target as HTMLElement).closest("button, select, input, a, .nav-item")) return;
  appWindow.startDragging();
});

let currentSettings: Settings;

async function loadSettings() {
  currentSettings = await invoke<Settings>("get_settings");

  // Populate mic dropdown
  const mics = await invoke<MicDevice[]>("list_microphones");
  micSelect.innerHTML = "";
  mics.forEach((mic) => {
    const option = document.createElement("option");
    option.value = mic.name;
    option.textContent = mic.name + (mic.is_default ? " (default)" : "");
    micSelect.appendChild(option);
  });
  // An empty or unplugged mic name matches no option, which renders as a
  // blank dropdown — it reads as "no microphone", when in fact Typr is
  // happily recording from the system default. Show the device actually in
  // use instead.
  micSelect.value = currentSettings.microphone;
  if (!micSelect.value) {
    const fallback = mics.find((m) => m.is_default) ?? mics[0];
    if (fallback) micSelect.value = fallback.name;
  }

  // Groq key
  groqKey.value = currentSettings.groqApiKey;

  // Recording mode
  setRecordingMode(currentSettings.recordingMode);

  // Hotkey
  hotkeyText.textContent = currentSettings.hotkey.replace("CmdOrCtrl", "Ctrl");

  // Autostart — show what the OS actually has registered, not just what the
  // config file claims. An installer upgrade or a Windows "Startup apps"
  // toggle can drop the entry, and a switch that reads its own saved value
  // would keep showing On while Typr silently stopped launching.
  // Modes, vocabulary and style.
  //
  // Rust guarantees these fields exist via serde defaults, but an older
  // config.json read by a brand-new build briefly has them undefined in
  // the JS layer. Coalescing here means the editors never throw on a
  // first run.
  currentSettings.modeOverrides ??= [];
  currentSettings.dictionary ??= [];
  currentSettings.snippets ??= [];
  currentSettings.customInstructions ??= "";
  autoModeToggle.checked = currentSettings.autoMode !== false;
  defaultModeSelect.value = currentSettings.defaultMode || "clean";
  customInstructions.value = currentSettings.customInstructions;
  renderAll();
  syncLearningToggles();
  refreshLearning();

  const osEnabled = await invoke<boolean>("autostart_is_enabled");
  autostartToggle.checked = osEnabled;
  if (osEnabled !== currentSettings.autostart) {
    currentSettings.autostart = osEnabled;
    autostartHint.textContent = osEnabled
      ? "Typr launches on login and waits in the tray"
      : "Off - Typr will not be running when you reach for the hotkey";
  }
}

function setRecordingMode(mode: string) {
  currentSettings.recordingMode = mode;
  modeToggle.classList.toggle("active", mode === "toggle");
  modePtt.classList.toggle("active", mode === "push-to-talk");
}

// Set the first time the user picks a microphone themselves.
//
// An empty stored name means "follow the system default", which is what
// Typr ships with and what keeps working when a USB mic is unplugged. The
// dropdown still has to *show* a device, so writing whatever it displays
// back on every save would quietly pin Ben to one device he never chose.
let micChosenByUser = false;

async function saveSettings() {
  if (micChosenByUser) currentSettings.microphone = micSelect.value;
  currentSettings.groqApiKey = groqKey.value;
  await invoke("save_settings", { settings: currentSettings });
}

// Event listeners
micSelect.addEventListener("change", () => {
  micChosenByUser = true;
  saveSettings();
});

groqKey.addEventListener("change", () => saveSettings());

autostartToggle.addEventListener("change", async () => {
  currentSettings.autostart = autostartToggle.checked;
  await saveSettings();
  // Re-read the OS, because enable()/disable() can fail (locked-down policy,
  // missing registry permission) and a toggle that moved without the OS
  // agreeing is a lie.
  const osEnabled = await invoke<boolean>("autostart_is_enabled");
  autostartToggle.checked = osEnabled;
  autostartHint.textContent = osEnabled
    ? "Typr launches on login and waits in the tray"
    : "Off - Typr will not be running when you reach for the hotkey";
});

modeToggle.addEventListener("click", () => {
  setRecordingMode("toggle");
  saveSettings();
});

modePtt.addEventListener("click", () => {
  setRecordingMode("push-to-talk");
  saveSettings();
});

// Listen for recording state changes
listen<string>("recording-state", (event) => {
  const state = event.payload;
  statusDot.className = "";
  if (state === "Recording") {
    statusDot.classList.add("recording");
    statusText.textContent = "Recording...";
  } else if (state === "Transcribing") {
    statusDot.classList.add("transcribing");
    statusText.textContent = "Transcribing...";
  } else {
    statusDot.classList.add("ready");
    statusText.textContent = "Ready";
  }
});

// Log Groq format errors for debugging
listen<string>("format-error", (event) => {
  console.warn("[Typr] Groq format error:", event.payload);
});

// Surface transcription failures — Groq is the only backend now, so a failure
// here is terminal (no local fallback). Flash the pill via overlay + log here.
listen<string>("transcription-error", (event) => {
  console.error("[Typr] Transcription failed:", event.payload);
});

// Initialize
loadSettings();

// ───────────────────────────────────────────────────────────────────────────
// Modes, dictionary, snippets and style
//
// All three lists are the same shape: a growing set of rows, each with a
// remove control on the row itself and an "add" button underneath. Rendering
// from the settings array every time (rather than appending DOM and tracking
// indices) means a removal can never leave a stale index behind.
// ───────────────────────────────────────────────────────────────────────────

const MODES = [
  "clean",
  "polish",
  "email",
  "chat",
  "developer",
  "prompt",
  "notes",
  "verbatim",
] as const;

const autoModeToggle = document.getElementById("auto-mode-toggle") as HTMLInputElement;
const defaultModeSelect = document.getElementById("default-mode") as HTMLSelectElement;
const overridesList = document.getElementById("overrides-list")!;
const dictionaryList = document.getElementById("dictionary-list")!;
const snippetsList = document.getElementById("snippets-list")!;
const customInstructions = document.getElementById("custom-instructions") as HTMLTextAreaElement;

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  return node;
}

function field(
  value: string,
  placeholder: string,
  onInput: (v: string) => void,
  opts: { mono?: boolean; textarea?: boolean } = {},
) {
  const input = opts.textarea ? el("textarea", "editor-field") : el("input", "editor-field");
  if (opts.mono) input.classList.add("mono");
  (input as HTMLInputElement).value = value;
  (input as HTMLInputElement).placeholder = placeholder;
  input.addEventListener("input", () => onInput((input as HTMLInputElement).value));
  // Save on blur rather than on every keystroke: a save per character would
  // rewrite config.json dozens of times per word.
  input.addEventListener("blur", () => saveSettings());
  return input;
}

function enabledToggle(checked: boolean, onChange: (v: boolean) => void) {
  const label = el("label", "toggle-switch");
  const input = el("input");
  input.type = "checkbox";
  input.checked = checked;
  input.addEventListener("change", () => {
    onChange(input.checked);
    saveSettings();
    renderAll();
  });
  const track = el("span", "toggle-track");
  label.append(input, track);
  return label;
}

function removeButton(onClick: () => void) {
  const btn = el("button", "btn-remove");
  btn.type = "button";
  btn.textContent = "×";
  btn.title = "Remove";
  btn.addEventListener("click", () => {
    onClick();
    saveSettings();
    renderAll();
  });
  return btn;
}

function emptyNote(text: string) {
  const note = el("div", "editor-empty");
  note.textContent = text;
  return note;
}

function renderOverrides() {
  overridesList.innerHTML = "";
  if (!currentSettings.modeOverrides.length) {
    overridesList.append(emptyNote("No overrides. Typr is guessing from the app."));
    return;
  }
  currentSettings.modeOverrides.forEach((ov, i) => {
    const row = el("div", "editor-row");
    row.append(
      field(ov.app, "code.exe, or words from the title", (v) => {
        currentSettings.modeOverrides[i].app = v;
      }, { mono: true }),
    );

    const select = el("select", "editor-field");
    MODES.forEach((m) => {
      const opt = el("option");
      opt.value = m;
      opt.textContent = m;
      select.append(opt);
    });
    select.value = ov.mode;
    select.addEventListener("change", () => {
      currentSettings.modeOverrides[i].mode = select.value;
      saveSettings();
    });
    row.append(select);

    row.append(removeButton(() => currentSettings.modeOverrides.splice(i, 1)));
    overridesList.append(row);
  });
}

function renderDictionary() {
  dictionaryList.innerHTML = "";
  if (!currentSettings.dictionary.length) {
    dictionaryList.append(emptyNote("No words yet."));
    return;
  }
  currentSettings.dictionary.forEach((entry, i) => {
    const row = el("div", "editor-row");
    if (!entry.enabled) row.classList.add("disabled");

    row.append(
      field(entry.term, "Spelled like this", (v) => {
        currentSettings.dictionary[i].term = v;
      }),
      field(
        entry.misheard.join(", "),
        "heard as, these, instead",
        (v) => {
          // Split on commas only; a term can legitimately contain spaces
          // ("go high level"), so splitting on whitespace would shred it.
          currentSettings.dictionary[i].misheard = v
            .split(",")
            .map((s) => s.trim())
            .filter(Boolean);
        },
      ),
      enabledToggle(entry.enabled, (v) => {
        currentSettings.dictionary[i].enabled = v;
      }),
      removeButton(() => currentSettings.dictionary.splice(i, 1)),
    );
    dictionaryList.append(row);
  });
}

function renderSnippets() {
  snippetsList.innerHTML = "";
  if (!currentSettings.snippets.length) {
    snippetsList.append(emptyNote("No snippets yet."));
    return;
  }
  currentSettings.snippets.forEach((snip, i) => {
    const row = el("div", "editor-row");
    if (!snip.enabled) row.classList.add("disabled");

    row.append(
      field(snip.trigger, "say this on its own", (v) => {
        currentSettings.snippets[i].trigger = v;
      }),
      field(
        snip.text,
        "and Typr pastes this, exactly",
        (v) => {
          currentSettings.snippets[i].text = v;
        },
        { textarea: true },
      ),
      enabledToggle(snip.enabled, (v) => {
        currentSettings.snippets[i].enabled = v;
      }),
      removeButton(() => currentSettings.snippets.splice(i, 1)),
    );
    snippetsList.append(row);
  });
}

function renderAll() {
  renderOverrides();
  renderDictionary();
  renderSnippets();
}

document.getElementById("add-override")!.addEventListener("click", () => {
  currentSettings.modeOverrides.push({ app: "", mode: currentSettings.defaultMode || "clean" });
  renderAll();
  focusLastField(overridesList);
});

document.getElementById("add-dict")!.addEventListener("click", () => {
  currentSettings.dictionary.push({ term: "", misheard: [], enabled: true });
  renderAll();
  focusLastField(dictionaryList);
});

document.getElementById("add-snippet")!.addEventListener("click", () => {
  currentSettings.snippets.push({ trigger: "", text: "", enabled: true });
  renderAll();
  focusLastField(snippetsList);
});

// A new empty row is useless until it has focus — otherwise adding one looks
// like nothing happened.
function focusLastField(list: HTMLElement) {
  const rows = list.querySelectorAll(".editor-row");
  const last = rows[rows.length - 1];
  (last?.querySelector(".editor-field") as HTMLElement | null)?.focus();
}

autoModeToggle.addEventListener("change", () => {
  currentSettings.autoMode = autoModeToggle.checked;
  saveSettings();
});

defaultModeSelect.addEventListener("change", () => {
  currentSettings.defaultMode = defaultModeSelect.value;
  saveSettings();
});

customInstructions.addEventListener("blur", () => {
  currentSettings.customInstructions = customInstructions.value;
  saveSettings();
});

// ───────────────────────────────────────────────────────────────────────────
// Learning: suggested corrections, voice commands, list formatting
//
// Every call that changes the dictionary returns the new Settings from Rust,
// and the screen swaps its copy for that one. Keeping the old copy would let
// the next unrelated save write the old dictionary straight back.
// ───────────────────────────────────────────────────────────────────────────

const listFormattingToggle = document.getElementById("list-formatting-toggle") as HTMLInputElement;
const commandPrefixToggle = document.getElementById("command-prefix-toggle") as HTMLInputElement;
const correctionWatchToggle = document.getElementById("correction-watch-toggle") as HTMLInputElement;
const correctionsList = document.getElementById("corrections-list")!;
const changeLogList = document.getElementById("change-log")!;
const learnStatus = document.getElementById("learn-status")!;

function bindPreference(
  input: HTMLInputElement,
  key: "listFormatting" | "commandPrefix" | "correctionWatch",
) {
  input.addEventListener("change", () => {
    currentSettings[key] = input.checked;
    saveSettings();
  });
}
bindPreference(listFormattingToggle, "listFormatting");
bindPreference(commandPrefixToggle, "commandPrefix");
bindPreference(correctionWatchToggle, "correctionWatch");

function syncLearningToggles() {
  listFormattingToggle.checked = currentSettings.listFormatting !== false;
  commandPrefixToggle.checked = currentSettings.commandPrefix !== false;
  correctionWatchToggle.checked = currentSettings.correctionWatch !== false;
}

function adoptSettings(fresh: Settings) {
  // The mic choice and key field are live inputs; keep what is on screen.
  currentSettings = fresh;
  syncLearningToggles();
  renderAll();
}

function when(secs: number) {
  return secs ? new Date(secs * 1000).toLocaleString() : "";
}

function describeEffect(e: LogEntry) {
  const fx = e.effect;
  switch (fx.type) {
    case "created-entry":
      return `added "${fx.term}"`;
    case "added-misheard":
      return `"${fx.misheard}" now becomes "${fx.term}"`;
    case "removed-entry":
      return `removed "${fx.entry?.term ?? ""}"`;
    default:
      return "no dictionary change";
  }
}

async function refreshLearning() {
  try {
    const [queue, log] = await Promise.all([
      invoke<Proposal[]>("list_corrections"),
      invoke<LogEntry[]>("correction_log"),
    ]);

    correctionsList.innerHTML = "";
    if (!queue.length) correctionsList.append(emptyNote("Nothing to review."));
    queue.forEach((p) => {
      const row = el("div", "editor-row");
      const text = el("div", "editor-field");
      text.textContent =
        `"${p.original}" -> "${p.corrected}"  ·  seen ${p.count}x, last ${when(p.lastSeen)}  ·  ${p.context}`;
      text.title = `${p.kind}, first seen ${when(p.firstSeen)}`;
      const approve = el("button", "btn-add");
      approve.type = "button";
      approve.textContent = "Approve";
      approve.addEventListener("click", async () => {
        adoptSettings(await invoke<Settings>("approve_correction", { id: p.id }));
        refreshLearning();
      });
      const reject = actionButton("Reject", async () => {
        await invoke("reject_correction", { id: p.id });
        refreshLearning();
      });
      row.append(text, approve, reject);
      correctionsList.append(row);
    });

    changeLogList.innerHTML = "";
    if (!log.length) changeLogList.append(emptyNote("No changes yet."));
    [...log].reverse().forEach((e) => {
      const row = el("div", "editor-row");
      const text = el("div", "editor-field");
      const what = e.action === "undo" ? `undo #${e.undoes}: ${describeEffect(e)}` : `${e.action}: ${describeEffect(e)}`;
      text.textContent = `#${e.seq}  ${when(e.at)}  ${what}`;
      row.append(text);
      changeLogList.append(row);
    });
  } catch (err) {
    learnStatus.textContent = `Could not load suggestions: ${err}`;
  }
}

function actionButton(label: string, onClick: () => void) {
  const btn = el("button", "btn-remove");
  btn.type = "button";
  btn.textContent = label;
  btn.addEventListener("click", onClick);
  return btn;
}

document.getElementById("undo-correction")!.addEventListener("click", async () => {
  try {
    adoptSettings(await invoke<Settings>("undo_correction"));
    learnStatus.textContent = "Undone.";
  } catch (err) {
    learnStatus.textContent = String(err);
  }
  refreshLearning();
});

document.getElementById("submit-sample")!.addEventListener("click", async () => {
  const pasted = (document.getElementById("sample-pasted") as HTMLTextAreaElement).value;
  const after = (document.getElementById("sample-after") as HTMLTextAreaElement).value;
  try {
    const n = await invoke<number>("submit_correction_sample", { pasted, after });
    learnStatus.textContent = n
      ? `${n} suggestion(s) filed. Case or punctuation-only fixes show up after 3 sightings.`
      : "No deliberate correction found in that pair.";
  } catch (err) {
    learnStatus.textContent = String(err);
  }
  refreshLearning();
});

// A voice command ("Typr, add X to the dictionary") changed settings from the
// Rust side. Take the new copy before anything here saves over it.
listen<Settings>("settings-changed", (event) => {
  adoptSettings(event.payload);
  refreshLearning();
});

document.querySelector('[data-section="learning"]')?.addEventListener("click", () => refreshLearning());
