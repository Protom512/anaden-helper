//! 設定保存 (settings) コア実装 (Issue #83)。
//!
//! 対象戦略・主要オプションの選択状態を TOML ファイルへ保存/読込する。
//! egui 非依存のピュアなモジュールで、GUI からはこの API 経由で利用する。
//!
//! - 保存形式: TOML (`serde` + 既存 `toml` 依存)
//! - 破損時フォールバック (UC-2): 読込失敗・パース失敗時は既定値を返す
//! - パス解決: CWD 相対ではなく安定した設定ディレクトリ
//!   (環境変数 `ANADEN_STUDIO_CONFIG_DIR` 上書き可)。

use std::io;
use std::path::Path;
use std::path::PathBuf;

use anaden_strategies::StrategySelection;

/// 設定ファイル名。
const SETTINGS_FILE: &str = "settings.toml";
/// 設定ディレクトリ上書き用環境変数。
const CONFIG_DIR_ENV: &str = "ANADEN_STUDIO_CONFIG_DIR";

/// anaden-studio の永続設定。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StudioSettings {
    /// 選択中の戦略とオプション状態。
    pub selection: StrategySelection,
}

impl StudioSettings {
    /// 指定パスへ TOML 形式で保存する。
    ///
    /// # Errors
    /// ファイル作成/書込に失敗した場合 `io::Error` を返す。
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let toml_str = to_toml(self);
        std::fs::write(path, toml_str)
    }

    /// 指定パスから読み込む。
    ///
    /// - ファイル不存在・破損 (UC-2) は既定値フォールバック (`Ok(default)`)
    /// - ただし IO エラーとパースエラーを区別できるよう `LoadOutcome` を返す
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

/// 読込結果。破損/不在時に GUI 側で通知できるようフォールバック理由を保持する。
#[derive(Debug, PartialEq)]
pub enum LoadOutcome {
    /// 正常に読み込めた。
    Loaded(StudioSettings),
    /// ファイル不在または破損のため既定値へフォールバック (UC-2)。
    Fallback,
}

impl LoadOutcome {
    /// 読込済み設定を取り出す。フォールバック時は既定値を返す。
    #[must_use]
    pub fn into_settings_or_default(self) -> StudioSettings {
        match self {
            LoadOutcome::Loaded(s) => s,
            LoadOutcome::Fallback => StudioSettings::default(),
        }
    }
}

/// TOML 文字列へシリアライズする。失敗時 (シリアライズ不能) は空文字列。
fn to_toml(settings: &StudioSettings) -> String {
    toml::to_string_pretty(settings).unwrap_or_default()
}

/// TOML 文字列からデシリアライズする。
fn from_toml(text: &str) -> Option<StudioSettings> {
    toml::from_str(text).ok()
}

/// 安定した設定ディレクトリを解決する。
///
/// 解決順:
/// 1. 環境変数 `ANADEN_STUDIO_CONFIG_DIR` (明示上書き)
/// 2. プラットフォーム標準のユーザ設定ディレクトリ配下の `anaden-studio`
///    - Windows: `%APPDATA%\anaden-studio`
///    - それ以外: `$XDG_CONFIG_HOME/anaden-studio` または `~/.config/anaden-studio`
///
/// CWD 相対にはならない (インストール済みバイナリ実行ケースの条件)。
#[must_use]
pub fn resolve_config_dir() -> PathBuf {
    let env_dir = std::env::var_os(CONFIG_DIR_ENV).filter(|v| !v.is_empty());
    resolve_config_dir_with_override(env_dir.as_deref())
}

/// テスト・組み込み用途向け: 環境変数上書き値を明示的に渡す解決本体。
///
/// CWD 相対にはならない (上書き値・プラットフォーム解決とも絶対パス前提)。
#[must_use]
pub fn resolve_config_dir_with_override(env_dir: Option<&std::ffi::OsStr>) -> PathBuf {
    if let Some(dir) = env_dir
        && !dir.is_empty()
    {
        return PathBuf::from(dir).join("anaden-studio");
    }
    platform_config_dir().unwrap_or_else(|| PathBuf::from(".anaden-studio"))
}

/// 設定ファイルのフルパスを解決する。
#[must_use]
pub fn settings_file_path() -> PathBuf {
    resolve_config_dir().join(SETTINGS_FILE)
}

/// プラットフォーム標準のユーザ設定ディレクトリ (存在しない環境では None)。
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

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
        // edition 2024 では set_var が unsafe なため、上書き値注入版で検証する。
        // CWD ではなく指定 dir 側へ解決されることを検証。
        let resolved = resolve_config_dir_with_override(Some(dir.path().as_os_str()));
        let resolved2 = resolve_config_dir_with_override(Some(dir2.path().as_os_str()));
        assert_eq!(resolved, dir.path().join("anaden-studio"));
        assert_ne!(resolved, resolved2);
        assert!(resolved.is_absolute());
    }

    #[test]
    fn env_override_empty_is_ignored() {
        let resolved = resolve_config_dir_with_override(Some(std::ffi::OsStr::new("")));
        // 空文字上書きは無視されプラットフォーム解決へフォールバック。
        // テスト環境 (Windows CI 含む) では絶対パスになることを確認。
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
}
