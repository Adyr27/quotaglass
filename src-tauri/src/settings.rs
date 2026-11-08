//! `%APPDATA%\QuotaGlass\settings.json`: window and switches. No tokens, ever.

use quotaglass_core::Service;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Sources {
    pub direct: bool,
    pub orca: bool,
}

impl Default for Sources {
    fn default() -> Self {
        Self {
            direct: true,
            orca: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub version: u32,
    pub window: Option<WindowRect>,
    pub pinned: bool,
    pub always_on_top: bool,
    pub autostart: bool,
    pub refresh_minutes: u8,
    pub sources: Sources,
    pub collapsed: Vec<Service>,
    pub hidden_services: Vec<Service>,
    pub first_run_done: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            window: None,
            pinned: false,
            always_on_top: false,
            autostart: false,
            refresh_minutes: 2,
            sources: Sources::default(),
            collapsed: Vec::new(),
            hidden_services: Vec::new(),
            first_run_done: false,
        }
    }
}

/// Everything the UI may change. The window rect is saved by the app itself, and the version is fixed.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub pinned: Option<bool>,
    pub always_on_top: Option<bool>,
    pub autostart: Option<bool>,
    pub refresh_minutes: Option<u8>,
    pub sources: Option<Sources>,
    pub collapsed: Option<Vec<Service>>,
    pub hidden_services: Option<Vec<Service>>,
    pub first_run_done: Option<bool>,
}

impl Settings {
    pub fn apply(&mut self, patch: SettingsPatch) {
        if let Some(v) = patch.pinned {
            self.pinned = v;
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
        if let Some(v) = patch.sources {
            self.sources = v;
        }
        if let Some(v) = patch.collapsed {
            self.collapsed = v;
        }
        if let Some(v) = patch.hidden_services {
            self.hidden_services = v;
        }
        if let Some(v) = patch.first_run_done {
            self.first_run_done = v;
        }
        self.clamp();
    }

    fn clamp(&mut self) {
        self.refresh_minutes = self.refresh_minutes.clamp(1, 5);
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

    /// Missing or unreadable file gives defaults. A file that does not parse is kept as `settings.json.bad`.
    pub fn load(&self) -> Settings {
        let Ok(raw) = std::fs::read(&self.path) else {
            return Settings::default();
        };
        match serde_json::from_slice::<Settings>(&raw) {
            Ok(mut s) => {
                s.clamp();
                s
            }
            Err(_) => {
                tracing::warn!("settings file is not valid; using defaults");
                let _ = std::fs::rename(&self.path, self.sibling("settings.json.bad"));
                Settings::default()
            }
        }
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

    #[test]
    fn settings_roundtrip_and_atomic() {
        let (store, dir) = temp_store("roundtrip");
        assert_eq!(store.load(), Settings::default());

        let mut s = Settings {
            window: Some(WindowRect {
                x: -10,
                y: 20,
                width: 360,
                height: 520,
            }),
            pinned: true,
            collapsed: vec![Service::Codex],
            ..Default::default()
        };
        s.sources.orca = false;
        store.save(&s).unwrap();

        assert_eq!(store.load(), s);
        assert!(
            !dir.join("settings.json.tmp").exists(),
            "the temp file is renamed away"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_settings_fall_back() {
        let (store, dir) = temp_store("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), b"{ not json").unwrap();

        assert_eq!(store.load(), Settings::default());
        assert!(dir.join("settings.json.bad").exists());
        assert!(!dir.join("settings.json").exists());
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
        let value = serde_json::to_value(Settings::default()).unwrap();
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
            ] {
                assert!(!k.contains(bad), "settings key {k} looks like a secret");
            }
        }
    }
}
