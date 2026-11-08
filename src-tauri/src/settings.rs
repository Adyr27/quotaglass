//! `%APPDATA%\QuotaGlass\settings.json`: window, look and connected sources. No tokens or emails, ever.

use quotaglass_core::Service;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

pub const SETTINGS_VERSION: u32 = 2;
const MAX_PINNED: usize = 4;
const MAX_LABEL_CHARS: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Material {
    #[default]
    Acrylic,
    Mica,
    Clear,
}

/// A one-click source the user switched on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Connection {
    ClaudeCurrent,
    CodexCurrent,
    AntigravityApp,
    Orca,
}

/// An account signed in through "+ Add account". Its login lives in `profile_dir`, never in this file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddedAccount {
    pub id: String,
    pub service: Service,
    pub profile_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub version: u32,
    pub pill_position: Option<WindowRect>,
    pub always_on_top: bool,
    pub autostart: bool,
    pub refresh_minutes: u8,
    pub theme: Theme,
    pub material: Material,
    pub connected: Vec<Connection>,
    pub added_accounts: Vec<AddedAccount>,
    /// Account IDs, in display order, at most 4.
    pub pinned_accounts: Vec<String>,
    pub hidden_accounts: Vec<String>,
    /// Account ID to label, at most 40 characters each.
    pub labels: BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            pill_position: None,
            always_on_top: true,
            autostart: false,
            refresh_minutes: 2,
            theme: Theme::default(),
            material: Material::default(),
            connected: Vec::new(),
            added_accounts: Vec::new(),
            pinned_accounts: Vec::new(),
            hidden_accounts: Vec::new(),
            labels: BTreeMap::new(),
        }
    }
}

/// Everything the UI may change. Added accounts belong to the account commands, and the version is fixed.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub pill_position: Option<WindowRect>,
    pub always_on_top: Option<bool>,
    pub autostart: Option<bool>,
    pub refresh_minutes: Option<u8>,
    pub theme: Option<Theme>,
    pub material: Option<Material>,
    pub connected: Option<Vec<Connection>>,
    pub pinned_accounts: Option<Vec<String>>,
    pub hidden_accounts: Option<Vec<String>>,
    pub labels: Option<BTreeMap<String, String>>,
}

impl Settings {
    pub fn apply(&mut self, patch: SettingsPatch) {
        if let Some(v) = patch.pill_position {
            self.pill_position = Some(v);
        }
        if let Some(v) = patch.always_on_top {
            self.always_on_top = v;
        }
        if let Some(v) = patch.autostart {
            self.autostart = v;
        }
        if let Some(v) = patch.refresh_minutes {
            self.refresh_minutes = v;
        }
        if let Some(v) = patch.theme {
            self.theme = v;
        }
        if let Some(v) = patch.material {
            self.material = v;
        }
        if let Some(v) = patch.connected {
            self.connected = v;
        }
        if let Some(v) = patch.pinned_accounts {
            self.pinned_accounts = v;
        }
        if let Some(v) = patch.hidden_accounts {
            self.hidden_accounts = v;
        }
        if let Some(v) = patch.labels {
            self.labels = v;
        }
        self.clamp();
    }

    fn clamp(&mut self) {
        self.version = SETTINGS_VERSION;
        self.refresh_minutes = self.refresh_minutes.clamp(1, 5);

        let mut seen = Vec::with_capacity(MAX_PINNED);
        self.pinned_accounts.retain(|id| {
            if seen.len() == MAX_PINNED || seen.contains(id) {
                return false;
            }
            seen.push(id.clone());
            true
        });

        let mut connected = Vec::with_capacity(self.connected.len());
        for c in self.connected.drain(..) {
            if !connected.contains(&c) {
                connected.push(c);
            }
        }
        self.connected = connected;

        self.labels = std::mem::take(&mut self.labels)
            .into_iter()
            .filter_map(|(id, label)| {
                let label: String = label.trim().chars().take(MAX_LABEL_CHARS).collect();
                let label = label.trim_end().to_string();
                (!label.is_empty()).then_some((id, label))
            })
            .collect();
    }
}

/// A v0.1.0 file keeps only its window rect and refresh interval. Every email-keyed field is dropped.
fn migrate_v1(raw: &serde_json::Value) -> Settings {
    let pill_position = raw
        .get("window")
        .and_then(|w| serde_json::from_value::<WindowRect>(w.clone()).ok());
    let refresh_minutes = raw
        .get("refreshMinutes")
        .and_then(serde_json::Value::as_u64)
        .and_then(|m| u8::try_from(m).ok())
        .unwrap_or(Settings::default().refresh_minutes);
    Settings {
        pill_position,
        refresh_minutes,
        ..Settings::default()
    }
}

pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// `%APPDATA%\QuotaGlass\settings.json`, or `None` when APPDATA is not set.
    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("APPDATA")
            .map(|a| PathBuf::from(a).join("QuotaGlass").join("settings.json"))
    }

    /// Missing or unreadable file gives defaults. A v1 file is migrated in memory; nothing is written here.
    /// A file that does not parse, or has an unknown version, is kept as `settings.json.bad`.
    pub fn load(&self) -> Settings {
        let Ok(raw) = std::fs::read(&self.path) else {
            return Settings::default();
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw) else {
            return self.quarantine();
        };
        if !value.is_object() {
            return self.quarantine();
        }
        let mut settings = match value.get("version").map(serde_json::Value::as_u64) {
            None | Some(Some(1)) => migrate_v1(&value),
            Some(Some(2)) => match serde_json::from_value::<Settings>(value) {
                Ok(s) => s,
                Err(_) => return self.quarantine(),
            },
            Some(_) => return self.quarantine(),
        };
        settings.clamp();
        settings
    }

    fn quarantine(&self) -> Settings {
        tracing::warn!("settings file is not valid; using defaults");
        let _ = std::fs::rename(&self.path, self.sibling("settings.json.bad"));
        Settings::default()
    }

    /// Writes `settings.json.tmp`, then renames it over `settings.json`.
    pub fn save(&self, settings: &Settings) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.sibling("settings.json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(settings).map_err(io::Error::other)?,
        )?;
        std::fs::rename(&tmp, &self.path)
    }

    fn sibling(&self, name: &str) -> PathBuf {
        self.path.with_file_name(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(name: &str) -> (SettingsStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("qg-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        (SettingsStore::new(dir.join("settings.json")), dir)
    }

    fn write_file(dir: &std::path::Path, bytes: &[u8]) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("settings.json"), bytes).unwrap();
    }

    #[test]
    fn v1_file_migrates() {
        let (store, dir) = temp_store("v1");
        let v1 = serde_json::json!({
            "version": 1,
            "window": { "x": -10, "y": 20, "width": 360, "height": 520 },
            "pinned": true,
            "alwaysOnTop": false,
            "refreshMinutes": 5,
            "overrides": { "someone@example.com": { "label": "Work" } },
            "sources": { "direct": true, "orca": true, "omp": true },
            "collapsed": ["codex"],
            "hiddenServices": ["claude"],
            "firstRunDone": true
        });
        write_file(&dir, &serde_json::to_vec(&v1).unwrap());

        let s = store.load();
        assert_eq!(s.version, 2);
        assert_eq!(
            s.pill_position,
            Some(WindowRect {
                x: -10,
                y: 20,
                width: 360,
                height: 520
            })
        );
        assert_eq!(s.refresh_minutes, 5);
        assert!(s.always_on_top);
        assert!(s.connected.is_empty());
        assert!(s.labels.is_empty());
        assert!(
            dir.join("settings.json").exists(),
            "load does not move or rewrite a v1 file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn v1_file_without_version_migrates() {
        let (store, dir) = temp_store("v1-noversion");
        write_file(&dir, br#"{"refreshMinutes":3,"sources":{"direct":false}}"#);
        let s = store.load();
        assert_eq!(s.version, 2);
        assert_eq!(s.refresh_minutes, 3);
        assert_eq!(s.pill_position, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_file_kept() {
        let (store, dir) = temp_store("broken");
        write_file(&dir, br#"{"version":"#);

        assert_eq!(store.load(), Settings::default());
        assert!(dir.join("settings.json.bad").exists());
        assert!(!dir.join("settings.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn v2_type_error_kept() {
        let (store, dir) = temp_store("v2-type");
        write_file(&dir, br#"{"version":2,"theme":"purple"}"#);

        assert_eq!(store.load(), Settings::default());
        assert!(dir.join("settings.json.bad").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pins_capped_at_four() {
        let mut s = Settings::default();
        s.apply(SettingsPatch {
            pinned_accounts: Some(["a", "b", "a", "c", "d", "e"].map(String::from).to_vec()),
            ..Default::default()
        });
        assert_eq!(s.pinned_accounts, ["a", "b", "c", "d"]);
    }

    #[test]
    fn labels_trimmed_to_40_chars() {
        let mut s = Settings::default();
        s.apply(SettingsPatch {
            labels: Some(BTreeMap::from([
                ("long".to_string(), "\u{e9}".repeat(50)),
                ("empty".to_string(), String::new()),
                ("blank".to_string(), "   ".to_string()),
                ("ok".to_string(), " Work ".to_string()),
            ])),
            ..Default::default()
        });
        assert_eq!(s.labels.len(), 2);
        assert_eq!(s.labels["long"], "\u{e9}".repeat(40));
        assert_eq!(s.labels["ok"], "Work");
    }

    #[test]
    fn round_trip_v2() {
        let (store, dir) = temp_store("roundtrip");
        assert_eq!(store.load(), Settings::default());

        let s = Settings {
            pill_position: Some(WindowRect {
                x: -10,
                y: 20,
                width: 360,
                height: 520,
            }),
            always_on_top: false,
            autostart: true,
            refresh_minutes: 4,
            theme: Theme::Dark,
            material: Material::Mica,
            connected: vec![Connection::ClaudeCurrent, Connection::Orca],
            added_accounts: vec![AddedAccount {
                id: "acct-1".into(),
                service: Service::Codex,
                profile_dir: PathBuf::from(r"C:\profiles\acct-1"),
            }],
            pinned_accounts: vec!["acct-1".into()],
            hidden_accounts: vec!["acct-2".into()],
            labels: BTreeMap::from([("acct-1".to_string(), "Work".to_string())]),
            ..Default::default()
        };
        store.save(&s).unwrap();

        assert_eq!(store.load(), s);
        assert!(
            !dir.join("settings.json.tmp").exists(),
            "the temp file is renamed away"
        );
        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(text.contains(r#""claudeCurrent""#));
        assert!(text.contains(r#""pillPosition""#));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_minutes_clamped() {
        let mut s = Settings::default();
        s.apply(SettingsPatch {
            refresh_minutes: Some(9),
            ..Default::default()
        });
        assert_eq!(s.refresh_minutes, 5);
        s.apply(SettingsPatch {
            refresh_minutes: Some(0),
            ..Default::default()
        });
        assert_eq!(s.refresh_minutes, 1);
    }

    #[test]
    fn settings_json_has_no_secret_fields() {
        fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
            match v {
                serde_json::Value::Object(m) => {
                    for (k, v) in m {
                        out.push(k.to_lowercase());
                        walk(v, out);
                    }
                }
                serde_json::Value::Array(a) => a.iter().for_each(|v| walk(v, out)),
                _ => {}
            }
        }
        let full = Settings {
            pill_position: Some(WindowRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            }),
            added_accounts: vec![AddedAccount {
                id: "a".into(),
                service: Service::Claude,
                profile_dir: PathBuf::from("p"),
            }],
            ..Default::default()
        };
        let value = serde_json::to_value(full).unwrap();
        let mut keys = Vec::new();
        walk(&value, &mut keys);
        assert!(!keys.is_empty());
        for k in keys {
            for bad in [
                "token",
                "secret",
                "password",
                "cookie",
                "session",
                "credential",
                "apikey",
                "email",
            ] {
                assert!(!k.contains(bad), "settings key {k} looks like a secret");
            }
        }
    }
}
