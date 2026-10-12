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
  micSelect.value = currentSettings.microphone;

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

async function saveSettings() {
  currentSettings.microphone = micSelect.value;
  currentSettings.groqApiKey = groqKey.value;
  await invoke("save_settings", { settings: currentSettings });
}

// Event listeners
micSelect.addEventListener("change", () => saveSettings());

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
