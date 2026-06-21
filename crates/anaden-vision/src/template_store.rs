//! テンプレート画像の管理。
//!
//! テンプレート画像の読み込みと名前解決を担当する。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use image::DynamicImage;
use thiserror::Error;
use tracing::{debug, info};

use anaden_core::GameState;

#[derive(Debug, Error)]
pub enum TemplateStoreError {
    #[error("Template image not found: {path}")]
    ImageNotFound { path: PathBuf },

    #[error("Failed to load template image: {path}: {reason}")]
    LoadFailed { path: PathBuf, reason: String },

    #[error("No templates loaded for state: {state:?}")]
    NoTemplatesForState { state: GameState },

    // T5/#34: Why not panic: read_dir failure (permission / IO error) must
    // propagate so the orchestrator/template_tool caller can surface it via `?`
    // instead of crashing the whole binary.
    #[error("Failed to read directory {path}: {reason}")]
    ReadDirFailed { path: PathBuf, reason: String },

    // T5/#34: A directory entry lacking a usable file name or stem is a data
    // error, not a programmer error. Surface it instead of panicking.
    #[error("Directory entry has no usable file name: {path}")]
    InvalidEntryName { path: PathBuf },

    #[error("Template image has no usable file stem: {path}")]
    MissingFileStem { path: PathBuf },
}

/// テンプレートのメタデータ。
#[derive(Debug, Clone)]
pub struct TemplateEntry {
    /// テンプレート画像
    pub image: DynamicImage,
    /// このテンプレートが対応するゲーム状態
    pub state: GameState,
    /// テンプレートの識別名（ファイル名等）
    pub name: String,
}

/// テンプレート画像を管理する。
///
/// 指定ディレクトリから画像を読み込み、GameState ごとに分類して保持する。
pub struct TemplateStore {
    /// 読み込み済みのテンプレート。キーはテンプレート名。
    templates: HashMap<String, TemplateEntry>,
}

impl Default for TemplateStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TemplateStore {
    /// 空のストアを作成する。
    pub fn new() -> Self {
        Self {
            templates: HashMap::new(),
        }
    }

    /// テンプレート画像を手動で登録する。
    pub fn register(&mut self, name: impl Into<String>, image: DynamicImage, state: GameState) {
        let name = name.into();
        debug!("Registered template '{}' for state {:?}", name, state);
        self.templates
            .insert(name.clone(), TemplateEntry { image, state, name });
    }

    /// 指定ディレクトリからテンプレート画像を一括読み込みする。
    ///
    /// ディレクトリ構造:
    /// ```text
    /// templates/
    /// └── scenes/       ← GameState 名のサブディレクトリ
    ///     ├── title_screen/
    ///     │   └── tap_to_start.png
    ///     ├── battle/
    ///     │   └── player_turn.png
    ///     └── ...
    /// ```
    ///
    /// 各サブディレクトリ名を GameState の識別子として使用する。
    pub fn load_from_directory(&mut self, base_dir: &Path) -> Result<usize, TemplateStoreError> {
        let mut loaded_count = 0;

        if !base_dir.exists() {
            info!("Template directory does not exist: {:?}", base_dir);
            return Ok(0);
        }

        // T5/#34: collect into a Vec so a read_dir IO failure becomes a single
        // propagated Err instead of a panic. Why not iterate + unwrap: the old
        // `unwrap_or_else(|e| panic!(...))` crashed the whole binary on any IO
        // error (permission, locked handle) on the template directory.
        let base_entries =
            std::fs::read_dir(base_dir).map_err(|e| TemplateStoreError::ReadDirFailed {
                path: base_dir.to_path_buf(),
                reason: e.to_string(),
            })?;

        for entry in base_entries {
            let entry = entry.map_err(|e| TemplateStoreError::ReadDirFailed {
                path: base_dir.to_path_buf(),
                reason: e.to_string(),
            })?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            // file_name はパス終端のコンポーネント; 通常は存在するが、稀に
            // (ルートや `..` 終端) 取れないため safe accessor で検証する。
            let state_name = path
                .file_name()
                .ok_or_else(|| TemplateStoreError::InvalidEntryName { path: path.clone() })?
                .to_string_lossy()
                .to_string();
            let state = parse_state_from_dir_name(&state_name);

            // ディレクトリ内の画像ファイルを読み込む。
            let inner_entries =
                std::fs::read_dir(&path).map_err(|e| TemplateStoreError::ReadDirFailed {
                    path: path.clone(),
                    reason: e.to_string(),
                })?;

            for img_entry in inner_entries {
                let img_entry = img_entry.map_err(|e| TemplateStoreError::ReadDirFailed {
                    path: path.clone(),
                    reason: e.to_string(),
                })?;
                let img_path = img_entry.path();

                if !is_image_file(&img_path) {
                    continue;
                }

                let img = image::open(&img_path).map_err(|e| TemplateStoreError::LoadFailed {
                    path: img_path.clone(),
                    reason: e.to_string(),
                })?;

                // file_stem は拡張子なしの名前; `..png` のような稀なケースでは
                // None になるため、欠落時は明示的なエラーにする。
                let template_name = img_path
                    .file_stem()
                    .ok_or_else(|| TemplateStoreError::MissingFileStem {
                        path: img_path.clone(),
                    })?
                    .to_string_lossy()
                    .to_string();

                info!("Loaded template '{}' -> {:?}", template_name, state);

                self.register(template_name, img, state.clone());
                loaded_count += 1;
            }
        }

        info!("Loaded {} template images total", loaded_count);
        Ok(loaded_count)
    }

    /// 登録されている全テンプレートを返す。
    pub fn all_templates(&self) -> impl Iterator<Item = &TemplateEntry> {
        self.templates.values()
    }

    /// 指定したゲーム状態に対応するテンプレートを返す。
    pub fn templates_for_state(&self, state: &GameState) -> Vec<&TemplateEntry> {
        self.templates
            .values()
            .filter(|t| t.state == *state)
            .collect()
    }

    /// テンプレート数を返す。
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    /// テンプレートが空かどうか。
    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}

/// ディレクトリ名から GameState を推測する。
fn parse_state_from_dir_name(name: &str) -> GameState {
    match name.to_lowercase().as_str() {
        "title" | "title_screen" => GameState::TitleScreen,
        "home" | "home_screen" | "field" => GameState::Field,
        "loading" => GameState::Loading,
        "battle" | "in_battle" => GameState::InBattle(anaden_core::BattlePhase::PlayerTurn),
        "fishing" => GameState::MiniGame(anaden_core::MiniGameType::Fishing),
        _ => GameState::Unknown,
    }
}

/// 画像ファイルの拡張子かどうか。
fn is_image_file(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .as_deref(),
        Some("png") | Some("jpg") | Some("jpeg") | Some("bmp")
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use image::RgbImage;

    #[test]
    fn register_and_retrieve_template() {
        let mut store = TemplateStore::new();
        let img = DynamicImage::ImageRgb8(RgbImage::new(100, 100));

        store.register("test_title", img, GameState::TitleScreen);

        assert_eq!(store.len(), 1);
        let results = store.templates_for_state(&GameState::TitleScreen);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "test_title");
    }

    #[test]
    fn parse_state_from_dir_name_variants() {
        assert_eq!(parse_state_from_dir_name("title"), GameState::TitleScreen);
        assert_eq!(
            parse_state_from_dir_name("Title_Screen"),
            GameState::TitleScreen
        );
        assert_eq!(
            parse_state_from_dir_name("battle"),
            GameState::InBattle(anaden_core::BattlePhase::PlayerTurn)
        );
        assert_eq!(
            parse_state_from_dir_name("unknown_thing"),
            GameState::Unknown
        );
    }

    // T5 ride-along cleanup (#34): load_from_directory must propagate errors via
    // Result instead of panicking. Why not panic: a malformed template directory
    // (unreadable entry, non-UTF8 name, missing file stem) must not crash the
    // whole CLI/studio/engine binary; the caller (orchestrator/template_tool)
    // already uses `?` and can surface the error gracefully.

    #[test]
    fn load_from_directory_returns_zero_for_missing_dir() {
        // 既存契約の回帰防止: 存在しないディレクトリは Ok(0) (panic しない)。
        let mut store = TemplateStore::new();
        let result = store.load_from_directory(Path::new("/nonexistent/definitely_missing_dir"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn load_from_directory_loads_images_from_subdirs() {
        // 正常系: サブディレクトリ名=state, 画像を読み込む。
        let tmp = tempfile::tempdir().unwrap();
        let battle_dir = tmp.path().join("battle");
        std::fs::create_dir_all(&battle_dir).unwrap();
        let img = DynamicImage::ImageRgb8(RgbImage::new(8, 8));
        img.save(battle_dir.join("player_turn.png")).unwrap();

        let mut store = TemplateStore::new();
        let count = store.load_from_directory(tmp.path()).unwrap();
        assert_eq!(count, 1);
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
    }

    #[test]
    fn load_from_directory_skips_non_image_files() {
        // 非画像ファイル (.toml/.txt) はスキップされ panic しない。
        let tmp = tempfile::tempdir().unwrap();
        let field_dir = tmp.path().join("field");
        std::fs::create_dir_all(&field_dir).unwrap();
        std::fs::write(field_dir.join("hud.toml"), b"x = 1").unwrap();
        std::fs::write(field_dir.join("notes.txt"), b"ignore me").unwrap();

        let mut store = TemplateStore::new();
        let count = store.load_from_directory(tmp.path()).unwrap();
        assert_eq!(count, 0);
        assert!(store.is_empty());
    }

    #[test]
    fn load_from_directory_handles_empty_subdir() {
        // 空サブディレクトリは Ok(0)、panic しない (read_dir on empty は有効)。
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("empty_state")).unwrap();

        let mut store = TemplateStore::new();
        let result = store.load_from_directory(tmp.path());
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }
}
