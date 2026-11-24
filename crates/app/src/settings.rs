//! `%APPDATA%\QuotaGlass\settings.json`: v1–v4 backwards-compatible window, glass, and source settings.
//! Keeps `SETTINGS_VERSION = 4` so rollback to v0.4.0 Tauri never quarantines the file.
//! No tokens, credentials, or emails are ever stored here.

use crate::state::JsonStore;
use quotaglass_core::Service;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

/// Kept at 4 for clean v0.4.0 Tauri rollback compatibility.
pub const SETTINGS_VERSION: u32 = 4;
/// The pill's smallest and largest scale.
pub const PILL_SCALE: (f64, f64) = (0.65, 1.6);
pub const MAX_LABEL_CHARS: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Clamps a scale into `PILL_SCALE`; a non-finite value (untrusted input) becomes 1.0.
pub fn clamp_scale(scale: f64) -> f64 {
    if scale.is_finite() {
        scale.clamp(PILL_SCALE.0, PILL_SCALE.1)
    } else {
        1.0
    }
}

/// Trims whitespace, allows apostrophes (e.g. `Adyr's Claude`), enforces <= 40 Unicode scalar chars,
/// and rejects control characters. Empty/whitespace-only clears the custom label (`Ok(None)`).
pub fn sanitize_label(raw: &str) -> Result<Option<String>, &'static str> {
    if raw.chars().any(char::is_control) {
        return Err("Label cannot contain control characters.");
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > MAX_LABEL_CHARS {
        return Err("Label must be 40 characters or fewer.");
    }
    Ok(Some(trimmed.to_string()))
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

/// v0.5.0 native glass modes, mapped cleanly to and from legacy `Material` for rollback.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GlassMode {
    #[default]
    LiquidGlass,
    Clear,
    Frosted,
}

impl From<Material> for GlassMode {
    fn from(m: Material) -> Self {
        match m {
            Material::Acrylic => Self::LiquidGlass,
            Material::Mica => Self::Frosted,
            Material::Clear => Self::Clear,
        }
    }
}

impl From<GlassMode> for Material {
    fn from(g: GlassMode) -> Self {
        match g {
            GlassMode::LiquidGlass => Self::Acrylic,
            GlassMode::Frosted => Self::Mica,
            GlassMode::Clear => Self::Clear,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderTab {
    #[default]
    All,
    Claude,
    Codex,
    Antigravity,
}

/// Taskbar-relative pill placement saved per monitor topology signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskbarAnchor {
    pub offset_from_right_px: i32,
    pub offset_from_taskbar_top_px: i32,
    pub on_taskbar_strip: bool,
    pub monitor_x: i32,
    pub monitor_y: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToastSound {
    /// The Windows default notification sound.
    #[default]
    Default,
    Silent,
}

/// A reminder toast when a limit window resets. The master switch is off until the user turns it on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResetToasts {
    pub enabled: bool,
    pub claude: bool,
    pub codex: bool,
    pub antigravity: bool,
    pub sound: ToastSound,
}

impl Default for ResetToasts {
    fn default() -> Self {
        Self {
            enabled: false,
            claude: true,
            codex: true,
            antigravity: true,
            sound: ToastSound::Default,
        }
    }
}

/// A one-click source the user switched on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Connection {
    ClaudeCurrent,
    CodexCurrent,
    AntigravityApp,
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
    pub hidden_accounts: Vec<String>,
    /// Account ID to label, at most 40 characters each.
    pub labels: BTreeMap<String, String>,
    /// The pill's size factor, within `PILL_SCALE`.
    pub pill_scale: f64,
    /// The window lock ("Pin pill"): no drag, no resize, and the rect is the home restored at launch.
    pub pill_locked: bool,
    pub reset_toasts: ResetToasts,
    /// Ask GitHub once a day for a newer release. With this off, QuotaGlass makes no update request.
    pub check_updates: bool,
    /// v0.5.0 native glass selector. When `None`, derived from legacy `material`.
    pub glass_mode: Option<GlassMode>,
    /// Taskbar-relative pill anchors keyed by monitor topology signature.
    pub taskbar_anchors: BTreeMap<String, TaskbarAnchor>,
    /// Last selected provider filter tab in the popover.
    pub active_tab: ProviderTab,
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
            hidden_accounts: Vec::new(),
            labels: BTreeMap::new(),
            pill_scale: 1.0,
            pill_locked: false,
            reset_toasts: ResetToasts::default(),
            check_updates: true,
            glass_mode: None,
            taskbar_anchors: BTreeMap::new(),
            active_tab: ProviderTab::default(),
        }
    }
}

/// Partial update from settings UI.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub always_on_top: Option<bool>,
    pub autostart: Option<bool>,
    pub refresh_minutes: Option<u8>,
    pub theme: Option<Theme>,
    pub material: Option<Material>,
    pub glass_mode: Option<GlassMode>,
    pub active_tab: Option<ProviderTab>,
    pub connected: Option<Vec<Connection>>,
    pub hidden_accounts: Option<Vec<String>>,
    pub labels: Option<BTreeMap<String, String>>,
    pub reset_toasts: Option<ResetToasts>,
    pub check_updates: Option<bool>,
}

impl Settings {
    /// Returns explicit `glass_mode` if set, otherwise maps legacy `material`.
    pub fn effective_glass_mode(&self) -> GlassMode {
        self.glass_mode
            .unwrap_or_else(|| GlassMode::from(self.material))
    }

    /// Sets `glass_mode` and mirrors legacy `material` for safe v0.4.0 rollback.
    pub fn set_glass_mode(&mut self, mode: GlassMode) {
        self.glass_mode = Some(mode);
        self.material = Material::from(mode);
    }

    pub fn taskbar_anchor(&self, signature: &str) -> Option<&TaskbarAnchor> {
        self.taskbar_anchors.get(signature)
    }

    pub fn set_taskbar_anchor(&mut self, signature: impl Into<String>, anchor: TaskbarAnchor) {
        self.taskbar_anchors.insert(signature.into(), anchor);
    }

    pub fn apply(&mut self, patch: SettingsPatch) {
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
        if let Some(mode) = patch.glass_mode {
            self.set_glass_mode(mode);
        } else if let Some(mat) = patch.material {
            self.material = mat;
            if self.glass_mode.is_some() {
                self.glass_mode = Some(GlassMode::from(mat));
            }
        }
        if let Some(tab) = patch.active_tab {
            self.active_tab = tab;
        }
        if let Some(v) = patch.connected {
            self.connected = v;
        }
        if let Some(v) = patch.hidden_accounts {
            self.hidden_accounts = v;
        }
        if let Some(v) = patch.labels {
            self.labels = v;
        }
        if let Some(v) = patch.check_updates {
            self.check_updates = v;
        }
        if let Some(v) = patch.reset_toasts {
            self.reset_toasts = v;
        }
        self.clamp();
    }

    pub fn set_hidden(&mut self, account_id: &str, hidden: bool) {
        self.hidden_accounts.retain(|h| h != account_id);
        if hidden {
            self.hidden_accounts.push(account_id.to_string());
        }
    }

    /// `None` or a blank label clears it; `clamp` sanitizes and caps to 40 scalar characters.
    pub fn set_label(&mut self, account_id: &str, label: Option<String>) {
        match label {
            Some(l) if !l.trim().is_empty() => {
                self.labels.insert(account_id.to_string(), l);
                self.clamp();
            }
            _ => {
                self.labels.remove(account_id);
            }
        }
    }

    /// Validated label update using [`sanitize_label`], returning a user-facing error when invalid.
    pub fn try_set_label(&mut self, account_id: &str, raw: &str) -> Result<(), &'static str> {
        match sanitize_label(raw)? {
            Some(clean) => {
                self.labels.insert(account_id.to_string(), clean);
            }
            None => {
                self.labels.remove(account_id);
            }
        }
        Ok(())
    }

    pub fn set_connected(&mut self, conn: Connection, on: bool) {
        self.connected.retain(|c| *c != conn);
        if on {
            self.connected.push(conn);
        }
    }

    /// Drops an added account. `forget_card` also drops the card's hidden flag and label: false when the card
    /// stays on screen through the current login.
    pub fn forget_added(&mut self, profile_id: &str, account_id: &str, forget_card: bool) {
        self.added_accounts.retain(|a| a.id != profile_id);
        let mut ids = vec![profile_id];
        if forget_card {
            ids.push(account_id);
        }
        self.hidden_accounts.retain(|h| !ids.contains(&h.as_str()));
        self.labels.retain(|k, _| !ids.contains(&k.as_str()));
    }

    pub fn clamp(&mut self) {
        self.version = SETTINGS_VERSION;
        self.refresh_minutes = self.refresh_minutes.clamp(1, 5);
        self.pill_scale = clamp_scale(self.pill_scale);

        let mut hidden = Vec::with_capacity(self.hidden_accounts.len());
        for id in self.hidden_accounts.drain(..) {
            if !hidden.contains(&id) {
                hidden.push(id);
            }
        }
        self.hidden_accounts = hidden;

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
                let cleaned: String = label
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect::<String>()
                    .trim()
                    .chars()
                    .take(MAX_LABEL_CHARS)
                    .collect();
                let cleaned = cleaned.trim_end().to_string();
                (!cleaned.is_empty()).then_some((id, cleaned))
            })
            .collect();
    }
}

/// A v0.1.0 file keeps only its window rect, refresh interval and autostart. Every email-keyed field is dropped.
fn migrate_v1(raw: &serde_json::Value) -> Settings {
    let pill_position = raw
        .get("window")
        .and_then(|w| serde_json::from_value::<WindowRect>(w.clone()).ok());
    let refresh_minutes = raw
        .get("refreshMinutes")
        .and_then(serde_json::Value::as_u64)
        .and_then(|m| u8::try_from(m).ok())
        .unwrap_or_else(|| Settings::default().refresh_minutes);
    let autostart = raw
        .get("autostart")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Settings {
        pill_position,
        refresh_minutes,
        autostart,
        ..Settings::default()
    }
}

/// Drops `connected` entries this build does not know, such as a source a later build removed.
fn drop_unknown_sources(mut value: serde_json::Value) -> serde_json::Value {
    if let Some(list) = value
        .get_mut("connected")
        .and_then(serde_json::Value::as_array_mut)
    {
        list.retain(|c| serde_json::from_value::<Connection>(c.clone()).is_ok());
    }
    value
}

#[derive(Debug, Clone)]
pub struct SettingsStore {
    store: JsonStore,
}

impl SettingsStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            store: JsonStore::new(path),
        }
    }

    /// `%APPDATA%\QuotaGlass\settings.json`, or `None` when APPDATA is not set.
    pub fn default_path() -> Option<PathBuf> {
        JsonStore::in_appdata("settings.json")
    }

    pub fn path(&self) -> &std::path::Path {
        self.store.path()
    }

    /// Missing or unreadable file gives defaults. v1–v3 files migrate in memory; v4 loads directly.
    /// A file that does not parse or has `version > 4` is quarantined as `settings.json.bad`.
    pub fn load(&self) -> Settings {
        let Ok(raw) = std::fs::read(self.path()) else {
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
            Some(Some(2..=4)) => {
                match serde_json::from_value::<Settings>(drop_unknown_sources(value)) {
                    Ok(s) => s,
                    Err(_) => return self.quarantine(),
                }
            }
            Some(_) => return self.quarantine(),
        };
        settings.clamp();
        settings
    }

    fn quarantine(&self) -> Settings {
        tracing::warn!("settings file is not valid; using defaults");
        let _ = std::fs::rename(self.path(), self.path().with_file_name("settings.json.bad"));
        Settings::default()
    }

    /// Writes `settings.json.tmp`, then atomically renames it over `settings.json`.
    pub fn save(&self, settings: &Settings) -> io::Result<()> {
        self.store.save_pretty(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn temp_store(name: &str) -> (SettingsStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("qg-app-set-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (SettingsStore::new(dir.join("settings.json")), dir)
    }

    fn write_file(dir: &std::path::Path, bytes: &[u8]) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("settings.json"), bytes).unwrap();
    }

    #[test]
    fn unknown_source_is_dropped_not_quarantined() {
        let (store, dir) = temp_store("retired");
        let v2 = serde_json::json!({
            "version": 2,
            "refreshMinutes": 1,
            "connected": ["claudeCurrent", "retiredSource", "antigravityApp"]
        });
        write_file(&dir, &serde_json::to_vec(&v2).unwrap());
        let s = store.load();
        assert_eq!(
            s.connected,
            [Connection::ClaudeCurrent, Connection::AntigravityApp]
        );
        assert_eq!(s.refresh_minutes, 1);
        assert!(!dir.join("settings.json.bad").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn v1_file_migrates() {
        let (store, dir) = temp_store("v1");
        let v1 = serde_json::json!({
            "version": 1,
            "window": { "x": -10, "y": 20, "width": 360, "height": 520 },
            "alwaysOnTop": false,
            "refreshMinutes": 5,
            "autostart": true
        });
        write_file(&dir, &serde_json::to_vec(&v1).unwrap());

        let s = store.load();
        assert_eq!(s.version, 4);
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
        assert!(s.autostart);
        assert!(s.always_on_top);
        assert!(dir.join("settings.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn v1_file_without_version_migrates() {
        let (store, dir) = temp_store("v1-noversion");
        write_file(&dir, br#"{"refreshMinutes":3}"#);
        let s = store.load();
        assert_eq!(s.version, 4);
        assert_eq!(s.refresh_minutes, 3);
        assert!(!s.autostart);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_file_and_future_version_quarantine() {
        let (store, dir) = temp_store("broken");
        write_file(&dir, br#"{"version":"#);
        assert_eq!(store.load(), Settings::default());
        assert!(dir.join("settings.json.bad").exists());
        assert!(!dir.join("settings.json").exists());

        write_file(&dir, br#"{"version":5}"#);
        assert_eq!(store.load(), Settings::default());
        assert!(dir.join("settings.json.bad").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_v4_loads_and_maps_glass_mode_and_mirrors_on_set() {
        let (store, dir) = temp_store("v4-legacy");
        for (mat_str, expected_mat, expected_glass) in [
            ("acrylic", Material::Acrylic, GlassMode::LiquidGlass),
            ("mica", Material::Mica, GlassMode::Frosted),
            ("clear", Material::Clear, GlassMode::Clear),
        ] {
            let v4 = serde_json::json!({
                "version": 4,
                "material": mat_str,
                "theme": "dark",
                "refreshMinutes": 2
            });
            write_file(&dir, &serde_json::to_vec(&v4).unwrap());
            let s = store.load();
            assert_eq!(s.version, 4);
            assert_eq!(s.material, expected_mat);
            assert_eq!(s.glass_mode, None);
            assert_eq!(s.effective_glass_mode(), expected_glass);
            assert_eq!(s.active_tab, ProviderTab::All);
            assert!(s.taskbar_anchors.is_empty());
        }

        let mut s = Settings::default();
        s.set_glass_mode(GlassMode::Frosted);
        assert_eq!(s.glass_mode, Some(GlassMode::Frosted));
        assert_eq!(s.material, Material::Mica);
        assert_eq!(s.effective_glass_mode(), GlassMode::Frosted);

        s.set_glass_mode(GlassMode::Clear);
        assert_eq!(s.glass_mode, Some(GlassMode::Clear));
        assert_eq!(s.material, Material::Clear);

        s.set_glass_mode(GlassMode::LiquidGlass);
        assert_eq!(s.glass_mode, Some(GlassMode::LiquidGlass));
        assert_eq!(s.material, Material::Acrylic);

        store.save(&s).unwrap();
        let saved = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(saved.contains(r#""version": 4"#));
        assert!(saved.contains(r#""glassMode": "liquidGlass""#));
        assert!(saved.contains(r#""material": "acrylic""#));
        assert!(saved.contains(r#""activeTab": "all""#));
        assert!(!dir.join("settings.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trip_with_v5_fields() {
        let (store, dir) = temp_store("roundtrip-v5");
        let mut s = Settings {
            pill_position: Some(WindowRect {
                x: 1600,
                y: 1044,
                width: 104,
                height: 32,
            }),
            always_on_top: true,
            autostart: true,
            refresh_minutes: 4,
            theme: Theme::Dark,
            connected: vec![Connection::ClaudeCurrent, Connection::AntigravityApp],
            added_accounts: vec![AddedAccount {
                id: "acct-1".into(),
                service: Service::Codex,
                profile_dir: PathBuf::from(r"C:\profiles\acct-1"),
            }],
            hidden_accounts: vec!["acct-2".into()],
            labels: BTreeMap::from([("acct-1".to_string(), "Adyr's Codex".to_string())]),
            pill_scale: 1.2,
            pill_locked: true,
            reset_toasts: ResetToasts {
                enabled: true,
                codex: false,
                sound: ToastSound::Silent,
                ..ResetToasts::default()
            },
            check_updates: false,
            active_tab: ProviderTab::Claude,
            ..Default::default()
        };
        s.set_glass_mode(GlassMode::Clear);
        s.set_taskbar_anchor(
            "1:1920x1080@100",
            TaskbarAnchor {
                offset_from_right_px: 216,
                offset_from_taskbar_top_px: 4,
                on_taskbar_strip: true,
                monitor_x: 0,
                monitor_y: 0,
            },
        );
        store.save(&s).unwrap();
        assert_eq!(store.load(), s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_label_trims_allows_apostrophe_rejects_control_and_overlong() {
        assert_eq!(
            sanitize_label("  Adyr's Claude  "),
            Ok(Some("Adyr's Claude".to_string()))
        );
        assert_eq!(sanitize_label("   "), Ok(None));
        assert_eq!(sanitize_label(""), Ok(None));
        assert_eq!(
            sanitize_label(&"\u{e9}".repeat(40)),
            Ok(Some("\u{e9}".repeat(40)))
        );
        assert!(sanitize_label(&"a".repeat(41)).is_err());
        assert!(sanitize_label("bad\x00label").is_err());
        assert!(sanitize_label("bad\nlabel").is_err());
        assert!(sanitize_label("bad\tlabel").is_err());
        assert!(sanitize_label("\x1b[31mred").is_err());
    }

    #[test]
    fn settings_never_contain_secret_fields() {
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
        let mut full = Settings {
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
            active_tab: ProviderTab::Codex,
            ..Default::default()
        };
        full.set_glass_mode(GlassMode::LiquidGlass);
        full.set_taskbar_anchor(
            "1:1920x1080@100",
            TaskbarAnchor {
                offset_from_right_px: 100,
                offset_from_taskbar_top_px: 4,
                on_taskbar_strip: true,
                monitor_x: 0,
                monitor_y: 0,
            },
        );
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

    proptest! {
        #[test]
        fn legacy_material_and_glass_mode_roundtrip(idx in 0usize..3) {
            let mat = [Material::Acrylic, Material::Mica, Material::Clear][idx];
            let mut s = Settings {
                material: mat,
                ..Settings::default()
            };
            let mode = s.effective_glass_mode();
            s.set_glass_mode(mode);
            prop_assert_eq!(s.material, mat);
            prop_assert_eq!(s.effective_glass_mode(), mode);
        }

        #[test]
        fn sanitize_label_never_returns_control_chars_or_over_40_scalars(raw in "\\PC{0,80}") {
            match sanitize_label(&raw) {
                Ok(Some(clean)) => {
                    prop_assert_eq!(clean.trim(), clean.as_str());
                    prop_assert!(!clean.is_empty());
                    prop_assert!(clean.chars().count() <= MAX_LABEL_CHARS);
                    prop_assert!(!clean.chars().any(char::is_control));
                }
                Ok(None) => {
                    prop_assert!(raw.trim().is_empty());
                    prop_assert!(!raw.chars().any(char::is_control));
                }
                Err(msg) => {
                    prop_assert!(!msg.is_empty());
                    let too_long = raw.trim().chars().count() > MAX_LABEL_CHARS;
                    let has_ctrl = raw.chars().any(char::is_control);
                    prop_assert!(too_long || has_ctrl);
                }
            }
        }
    }
}
