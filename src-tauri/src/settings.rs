use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::vocab::{default_dictionary, default_snippets, DictEntry, Snippet};

/// One row of "when I am in this app, use this mode".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModeOverride {
    /// An executable name ("code.exe") or a window-title fragment ("jira").
    pub app: String,
    pub mode: String,
}

fn default_microphone() -> String {
    "default".to_string()
}
fn default_recording_mode() -> String {
    "toggle".to_string()
}
fn default_hotkey() -> String {
    "CmdOrCtrl+Shift+Space".to_string()
}
fn default_autostart() -> bool {
    true
}
fn default_auto_mode() -> bool {
    true
}
fn default_mode() -> String {
    "clean".to_string()
}
fn default_correction_watch_secs() -> u64 {
    30
}

/// Every field carries `#[serde(default)]` on purpose.
///
/// `load` falls back to `Settings::default()` when parsing fails, so a struct
/// that rejects an older `config.json` (one missing a field added later) would
/// silently wipe Ben's saved Groq key and mic choice on the first run of a new
/// build. Per-field defaults mean an old config is upgraded in place instead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    #[serde(default = "default_microphone")]
    pub microphone: String,
    #[serde(rename = "groqApiKey", default)]
    pub groq_api_key: String,
    #[serde(rename = "recordingMode", default = "default_recording_mode")]
    pub recording_mode: String,
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    /// Start Typr when Windows starts. On by default — Ben uses it constantly
    /// and an idle tray app he has to remember to launch is an app that is off
    /// exactly when he reaches for the hotkey.
    #[serde(default = "default_autostart")]
    pub autostart: bool,

    /// Pick the transformation mode from the app being dictated into.
    #[serde(rename = "autoMode", default = "default_auto_mode")]
    pub auto_mode: bool,

    /// Used when auto-detection is off, or when the app is unrecognised.
    #[serde(rename = "defaultMode", default = "default_mode")]
    pub default_mode: String,

    /// Per-app mode overrides; these beat every built-in rule.
    #[serde(rename = "modeOverrides", default)]
    pub mode_overrides: Vec<ModeOverride>,

    #[serde(default = "default_dictionary")]
    pub dictionary: Vec<DictEntry>,

    #[serde(default = "default_snippets")]
    pub snippets: Vec<Snippet>,

    /// Free-text standing style preferences, applied to every dictation.
    #[serde(rename = "customInstructions", default)]
    pub custom_instructions: String,

    /// Turn a clearly spoken enumeration ("first ... second ... third", or
    /// "the list is a, b and c") into dot points. See `listfmt`.
    #[serde(rename = "listFormatting", default = "default_true")]
    pub list_formatting: bool,

    /// Treat "Typr, <verb> ..." as a command to Typr instead of dictation.
    /// See `command`. Anything Typr cannot carry out is still pasted.
    #[serde(rename = "commandPrefix", default = "default_true")]
    pub command_prefix: bool,

    /// Watch the field after a paste and propose dictionary entries from
    /// Ben's corrections. Proposals only; nothing is added without approval.
    /// Inert until the platform can read the field (`correction_watch`).
    #[serde(rename = "correctionWatch", default = "default_true")]
    pub correction_watch: bool,

    #[serde(rename = "correctionWatchSecs", default = "default_correction_watch_secs")]
    pub correction_watch_secs: u64,
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            microphone: default_microphone(),
            groq_api_key: String::new(),
            recording_mode: default_recording_mode(),
            hotkey: default_hotkey(),
            autostart: default_autostart(),
            auto_mode: default_auto_mode(),
            default_mode: default_mode(),
            mode_overrides: Vec::new(),
            dictionary: default_dictionary(),
            snippets: default_snippets(),
            custom_instructions: String::new(),
            list_formatting: true,
            command_prefix: true,
            correction_watch: true,
            correction_watch_secs: default_correction_watch_secs(),
        }
    }
}

impl Settings {
    pub fn config_path(app_dir: &PathBuf) -> PathBuf {
        app_dir.join("config.json")
    }

    pub fn load(app_dir: &PathBuf) -> Self {
        let path = Self::config_path(app_dir);
        match fs::read_to_string(&path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, app_dir: &PathBuf) -> Result<(), String> {
        let path = Self::config_path(app_dir);
        fs::create_dir_all(app_dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&path, json).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert_eq!(settings.microphone, "default");
        assert_eq!(settings.groq_api_key, "");
        assert_eq!(settings.recording_mode, "toggle");
        assert_eq!(settings.hotkey, "CmdOrCtrl+Shift+Space");
        assert!(settings.autostart);
    }

    #[test]
    fn test_save_and_load() {
        let dir = temp_dir().join("typr_test_settings");
        let _ = fs::remove_dir_all(&dir);

        let mut settings = Settings::default();
        settings.groq_api_key = "test-key-123".to_string();

        settings.save(&dir).unwrap();
        let loaded = Settings::load(&dir);

        assert_eq!(loaded.groq_api_key, "test-key-123");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_preferences_round_trip_under_their_json_names() {
        let mut s = Settings::default();
        s.list_formatting = false;
        s.command_prefix = false;
        s.correction_watch_secs = 45;
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"listFormatting\":false"));
        assert!(json.contains("\"commandPrefix\":false"));
        assert!(json.contains("\"correctionWatchSecs\":45"));
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn test_load_missing_file_returns_default() {
        let dir = temp_dir().join("typr_test_missing");
        let _ = fs::remove_dir_all(&dir);
        let settings = Settings::load(&dir);
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn test_load_corrupt_json_returns_default() {
        let dir = temp_dir().join("typr_test_corrupt");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.json"), "not json").unwrap();

        let settings = Settings::load(&dir);
        assert_eq!(settings, Settings::default());

        let _ = fs::remove_dir_all(&dir);
    }

    /// The real config.json on Ben's machine, written by an older build: it has
    /// dead fields (`engine`, `whisperModel`, `formatter`) and none of the ones
    /// added since. Loading it must keep the API key, not reset to defaults.
    #[test]
    fn test_load_legacy_config_preserves_api_key() {
        let dir = temp_dir().join("typr_test_legacy");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("config.json"),
            r#"{
              "microphone": "",
              "engine": "cloud",
              "whisperModel": "small",
              "groqApiKey": "gsk_legacy_key",
              "recordingMode": "toggle",
              "hotkey": "CmdOrCtrl+Shift+Space",
              "formatter": "ai"
            }"#,
        )
        .unwrap();

        let settings = Settings::load(&dir);
        assert_eq!(settings.groq_api_key, "gsk_legacy_key");
        assert_eq!(settings.recording_mode, "toggle");
        // Fields the old file never had still arrive at their defaults.
        assert!(settings.autostart);
        assert!(settings.auto_mode);
        assert_eq!(settings.default_mode, "clean");
        assert!(
            !settings.dictionary.is_empty(),
            "an upgraded config should still get the shipped dictionary"
        );
        assert!(settings.list_formatting);
        assert!(settings.command_prefix);
        assert!(settings.correction_watch);
        assert_eq!(settings.correction_watch_secs, 30);

        let _ = fs::remove_dir_all(&dir);
    }
}
