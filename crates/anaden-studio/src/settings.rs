use std::io;
use std::path::Path;
use std::path::PathBuf;

use anaden_strategies::StrategySelection;

const SETTINGS_FILE: &str = "settings.toml";
const CONFIG_DIR_ENV: &str = "ANADEN_STUDIO_CONFIG_DIR";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct StudioSettings {
    pub selection: StrategySelection,
}

impl StudioSettings {
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let toml_str = to_toml(self);
        std::fs::write(path, toml_str)
    }

    pub fn load(path: &Path) -> LoadOutcome {
        match std::fs::read_to_string(path) {
            Ok(text) => match from_toml(&text) {
                Some(settings) => LoadOutcome::Loaded(settings),
                None => LoadOutcome::Fallback,
            },
            Err(_) => LoadOutcome::Fallback,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum LoadOutcome {
    Loaded(StudioSettings),
    Fallback,
}

impl LoadOutcome {
    #[must_use]
    pub fn into_settings_or_default(self) -> StudioSettings {
        match self {
            LoadOutcome::Loaded(s) => s,
            LoadOutcome::Fallback => StudioSettings::default(),
        }
    }
}

fn to_toml(settings: &StudioSettings) -> String {
    toml::to_string_pretty(settings).unwrap_or_default()
}

fn from_toml(text: &str) -> Option<StudioSettings> {
    toml::from_str(text).ok()
}

#[must_use]
pub fn resolve_config_dir() -> PathBuf {
    let env_dir = std::env::var_os(CONFIG_DIR_ENV).filter(|v| !v.is_empty());
    resolve_config_dir_with_override(env_dir.as_deref())
}

#[must_use]
pub fn resolve_config_dir_with_override(env_dir: Option<&std::ffi::OsStr>) -> PathBuf {
    if let Some(dir) = env_dir
        && !dir.is_empty()
    {
        return PathBuf::from(dir).join("anaden-studio");
    }
    platform_config_dir().unwrap_or_else(|| PathBuf::from(".anaden-studio"))
}

#[must_use]
pub fn settings_file_path() -> PathBuf {
    resolve_config_dir().join(SETTINGS_FILE)
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SettingsTabStatus {
    Ok(PathBuf),
    Err(String),
    #[default]
    None,
}

#[derive(Debug, Default, PartialEq)]
pub struct SettingsTab {
    status: SettingsTabStatus,
}

impl SettingsTab {
    pub fn save(&mut self, selection: &StrategySelection, path: &Path) {
        let settings = StudioSettings {
            selection: selection.clone(),
        };
        self.status = match settings.save(path) {
            Ok(()) => SettingsTabStatus::Ok(path.to_path_buf()),
            Err(e) => SettingsTabStatus::Err(format!("設定保存失敗: {e}")),
        };
    }

    pub fn load(&mut self, path: &Path) -> Option<StrategySelection> {
        match StudioSettings::load(path) {
            LoadOutcome::Loaded(s) => {
                self.status = SettingsTabStatus::Ok(path.to_path_buf());
                Some(s.selection)
            }
            LoadOutcome::Fallback => {
                self.status = SettingsTabStatus::Ok(path.to_path_buf());
                None
            }
        }
    }

    #[must_use]
    pub fn status(&self) -> &SettingsTabStatus {
        &self.status
    }
}

fn platform_config_dir() -> Option<PathBuf> {
    if let Some(appdata) = std::env::var_os("APPDATA")
        && !appdata.is_empty()
    {
        return Some(PathBuf::from(appdata).join("anaden-studio"));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("anaden-studio"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config").join("anaden-studio"))
}

impl serde::Serialize for StudioSettings {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("StudioSettings", 1)?;
        s.serialize_field("selection", &self.selection)?;
        s.end()
    }
}

impl<'de> serde::Deserialize<'de> for StudioSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Raw {
            #[serde(default)]
            selection: StrategySelection,
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(Self {
            selection: raw.selection,
        })
    }
}

#[must_use]
pub fn settings_path_display(path: &Path) -> String {
    format!("設定ファイル: {}", path.display())
}

#[must_use]
pub fn load_outcome_notice(outcome: &LoadOutcome) -> String {
    match outcome {
        LoadOutcome::Loaded(_) => "設定を読み込みました".to_string(),
        LoadOutcome::Fallback => {
            "設定ファイルが見つからないか破損のため既定値を使用します (UC-2)".to_string()
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn settings_path_display_formats_path() {
        let path = std::path::Path::new("/tmp/x/settings.toml");
        assert_eq!(
            settings_path_display(path),
            "設定ファイル: /tmp/x/settings.toml"
        );
    }

    #[test]
    fn load_outcome_notice_loaded_is_plain_message() {
        let outcome = LoadOutcome::Loaded(StudioSettings::default());
        assert_eq!(load_outcome_notice(&outcome), "設定を読み込みました");
    }

    #[test]
    fn load_outcome_notice_fallback_mentions_uc2_and_default() {
        assert_eq!(
            load_outcome_notice(&LoadOutcome::Fallback),
            "設定ファイルが見つからないか破損のため既定値を使用します (UC-2)"
        );
    }

    fn sample_settings() -> StudioSettings {
        let mut options = std::collections::BTreeMap::new();
        options.insert("fishing.auto_throw".to_string(), true);
        options.insert("fishing.rare_only".to_string(), false);
        StudioSettings {
            selection: StrategySelection {
                strategy: Some("fishing".to_string()),
                options,
            },
        }
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let settings = sample_settings();
        settings.save(&path).unwrap();
        let loaded = StudioSettings::load(&path);
        assert_eq!(
            loaded.into_settings_or_default().selection.strategy,
            Some("fishing".to_string())
        );
    }

    #[test]
    fn load_missing_file_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.toml");
        assert_eq!(StudioSettings::load(&path), LoadOutcome::Fallback);
        assert_eq!(
            StudioSettings::load(&path).into_settings_or_default(),
            StudioSettings::default()
        );
    }

    #[test]
    fn load_corrupted_file_falls_back_uc2() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "not [ valid toml {{{").unwrap();
        assert_eq!(StudioSettings::load(&path), LoadOutcome::Fallback);
        assert_eq!(
            StudioSettings::load(&path).into_settings_or_default(),
            StudioSettings::default()
        );
    }

    #[test]
    fn save_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("nested")
            .join("deeper")
            .join("settings.toml");
        sample_settings().save(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn saved_file_is_human_readable_toml_with_strategy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        sample_settings().save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("fishing"));
    }

    #[test]
    fn load_toml_missing_options_field_uses_default_options() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "[selection]\nstrategy = \"fishing\"\n").unwrap();
        let settings = StudioSettings::load(&path).into_settings_or_default();
        assert_eq!(settings.selection.strategy, Some("fishing".to_string()));
        assert!(settings.selection.options.is_empty());
    }

    #[test]
    fn resolve_config_dir_respects_env_override_not_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        let resolved = resolve_config_dir_with_override(Some(dir.path().as_os_str()));
        let resolved2 = resolve_config_dir_with_override(Some(dir2.path().as_os_str()));
        assert_eq!(resolved, dir.path().join("anaden-studio"));
        assert_ne!(resolved, resolved2);
        assert!(resolved.is_absolute());
    }

    #[test]
    fn env_override_empty_is_ignored() {
        let resolved = resolve_config_dir_with_override(Some(std::ffi::OsStr::new("")));
        assert!(resolved.is_absolute() || cfg!(not(any(windows, unix))));
    }

    #[test]
    fn settings_file_path_under_config_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path =
            resolve_config_dir_with_override(Some(dir.path().as_os_str())).join(SETTINGS_FILE);
        assert!(path.is_absolute());
        assert_eq!(path, dir.path().join("anaden-studio").join(SETTINGS_FILE));
    }

    #[test]
    fn roundtrip_preserves_each_of_two_real_pc_pipeline_strategies() {
        for id in ["field_loop_pc", "nav_to_field_pc"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.toml");
            let settings = StudioSettings {
                selection: StrategySelection {
                    strategy: Some(id.to_string()),
                    options: std::collections::BTreeMap::new(),
                },
            };
            settings.save(&path).unwrap();
            let loaded = StudioSettings::load(&path).into_settings_or_default();
            assert_eq!(
                loaded.selection.strategy.as_deref(),
                Some(id),
                "roundtrip lost strategy {id}"
            );
        }
    }

    #[test]
    fn saved_toml_contains_selected_pipeline_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let settings = StudioSettings {
            selection: StrategySelection {
                strategy: Some("nav_to_field_pc".to_string()),
                options: std::collections::BTreeMap::new(),
            },
        };
        settings.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#"strategy = "nav_to_field_pc""#), "{text}");
    }

    #[test]
    fn loaded_selection_validates_against_builtin_catalog() {
        let catalog = anaden_strategies::StrategyCatalog::builtin();
        for s in catalog.strategies() {
            let sel = StrategySelection {
                strategy: Some(s.id.clone()),
                options: std::collections::BTreeMap::new(),
            };
            assert!(sel.validate(&catalog).is_ok(), "{}", s.id);
        }
    }

    #[test]
    fn roundtrip_of_legacy_unknown_strategy_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "[selection]\nstrategy = \"fishing\"\n").unwrap();
        let loaded = StudioSettings::load(&path).into_settings_or_default();
        assert_eq!(loaded.selection.strategy.as_deref(), Some("fishing"));
    }
}
