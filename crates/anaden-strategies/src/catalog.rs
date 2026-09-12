//! 戦略カタログ: 選択可能な戦略とオプションの宣言的定義（UI・設定向け）。
//!
//! 戦略の実体 (`MiniGameStrategy` trait 実装) とは分離されたメタ情報層。
//! anaden-studio 等 GUI はこのカタログからドロップダウン/チェックボックスを生成し、
//! 選択結果を [`StrategySelection`] (serde/TOML互換) に射影する。
//!
//! Issue #139 / #188: カタログは `templates/pipelines/` に実在する PC 版
//! パイプラインのみを登録する（Android 版 4 パイプラインは Issue #188 で削除。
//! fishing / login は templates/tasks のタスクバンク経由で扱うため未登録）。
//! `pipeline_dir` / `start_task` / `algorithm` / `target` を [`StrategyDef`] が持つ
//! ことで、`anaden run` 引数列の組み立てはカタログ定義単一情報源から行われる。

use serde::{Deserialize, Serialize};

/// 戦略オプションの定義（ON/OFF トグル）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyOptionDef {
    /// オプションの識別子（TOML キー）。
    pub id: String,
    /// UI 表示名。
    pub label: String,
    /// 既定値。
    pub default: bool,
}

/// 選択可能な戦略（実在パイプライン）の定義。
///
/// `pipeline_dir` / `start_task` / `algorithm` は各パイプラインの
/// TaskDef TOML (`templates/pipelines/<id>/*.toml`) の `name` / `algorithm` と
/// 一致させること（カタログ単一情報源化・Issue #139 T1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyDef {
    /// 戦略の識別子（= `templates/pipelines/` のディレクトリ名）。
    pub id: String,
    /// UI 表示名。
    pub label: String,
    /// ON/OFF 可能なオプション群。
    pub options: Vec<StrategyOptionDef>,
    /// `anaden run` へ渡す pipeline ディレクトリ（リポジトリルート相対）。
    pub pipeline_dir: String,
    /// 開始タスク名（TaskDef TOML の `name`・pipeline.toml の `start_task`）。
    pub start_task: String,
    /// テンプレートマッチアルゴリズム（`sse`|`ccoeff`・TaskDef の algorithm 準拠）。
    pub algorithm: String,
    /// `--target` 上書き（Issue #188 で CLI 側フラグを削除。settings.toml 互換で残置）。
    #[serde(default)]
    pub target: Option<String>,
}

impl StrategyDef {
    /// この定義から `anaden run` の引数列（サブコマンド以降）を組み立てる。
    ///
    /// 単一情報源化（Issue #139 T1）: 引数列はハードコード match ではなく
    /// このメソッド（カタログ定義）からのみ構成される。
    #[must_use]
    pub fn to_run_args(&self) -> Vec<String> {
        // --target フラグは Issue #188 で CLI から削除済み (Win32 固定) のため
        // 出力しない。`target` フィールドは settings.toml 互換で残置。
        vec![
            "run".to_string(),
            "--algorithm".to_string(),
            self.algorithm.clone(),
            self.pipeline_dir.clone(),
            self.start_task.clone(),
        ]
    }
}

/// 実在パイプラインのカタログ定義を構築するヘルパ（builtin の宣言性確保用）。
fn pipeline_def(
    id: &str,
    label: &str,
    pipeline_dir: &str,
    start_task: &str,
    algorithm: &str,
    target: Option<&str>,
) -> StrategyDef {
    StrategyDef {
        id: id.to_string(),
        label: label.to_string(),
        options: Vec::new(),
        pipeline_dir: pipeline_dir.to_string(),
        start_task: start_task.to_string(),
        algorithm: algorithm.to_string(),
        target: target.map(std::string::ToString::to_string),
    }
}

/// 戦略カタログ。実在パイプラインのメタ情報一覧。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyCatalog {
    strategies: Vec<StrategyDef>,
}

impl StrategyCatalog {
    /// `templates/pipelines/` に実在するパイプラインのカタログを構築する
    /// （Issue #139: fishing は pipeline が実在しないため登録しない）。
    ///
    /// 各定義の `start_task` / `algorithm` は TaskDef TOML の実測値:
    /// - `field_loop_pc`   : pipeline.toml    start_task="TapBottomStablePc" / ccoeff
    /// - `nav_to_field_pc` : tap_to_start.toml name="TapToStartPc" / ccoeff
    ///
    /// (Android 版 field_loop / nav_to_field / worldmap_loop / _title_load は
    /// Issue #188 でテンプレート資産ごと削除したため登録しない)
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            strategies: vec![
                pipeline_def(
                    "field_loop_pc",
                    "フィールド周回（PC 16:9）",
                    "templates/pipelines/field_loop_pc",
                    "TapBottomStablePc",
                    "ccoeff",
                    Some("windows"),
                ),
                pipeline_def(
                    "nav_to_field_pc",
                    "フィールドへ遷移（PC 16:9 コールドスタート）",
                    "templates/pipelines/nav_to_field_pc",
                    "TapToStartPc",
                    "ccoeff",
                    Some("windows"),
                ),
            ],
        }
    }

    /// カタログ内の戦略定義一覧。
    #[must_use]
    pub fn strategies(&self) -> &[StrategyDef] {
        &self.strategies
    }

    /// 識別子から戦略定義を引く。
    #[must_use]
    pub fn find(&self, id: &str) -> Option<&StrategyDef> {
        self.strategies.iter().find(|s| s.id == id)
    }

    /// カタログ既定値での選択状態を生成する（全戦略オフ・オプションは既定値）。
    #[must_use]
    pub fn default_selection(&self) -> StrategySelection {
        StrategySelection::from_defaults(self)
    }
}

/// ユーザーの選択状態（serde 化され toml 設定と往復する）。
///
/// 既存 TOML 設定形式との互換: 未知の戦略/オプションが含まれていても
/// 読み込み時にエラーにせず保持する（フォワード互換）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StrategySelection {
    /// 選択された戦略の識別子。
    pub strategy: Option<String>,
    /// オプションの ON/OFF 状態（キー: "<strategy_id>.<option_id>"）。
    ///
    /// `#[serde(default)]`: 旧形式（オプション記述なし）の TOML とも互換する。
    #[serde(default)]
    pub options: std::collections::BTreeMap<String, bool>,
}

impl StrategySelection {
    /// カタログの既定値から選択状態を構築する（戦略未選択・オプションは default）。
    #[must_use]
    pub fn from_defaults(catalog: &StrategyCatalog) -> Self {
        let mut options = std::collections::BTreeMap::new();
        for s in &catalog.strategies {
            for o in &s.options {
                options.insert(format!("{}.{}", s.id, o.id), o.default);
            }
        }
        Self {
            strategy: None,
            options,
        }
    }

    /// 特定戦略のオプション値を取得する。未登録なら `None`。
    #[must_use]
    pub fn option(&self, strategy_id: &str, option_id: &str) -> Option<bool> {
        self.options
            .get(&format!("{strategy_id}.{option_id}"))
            .copied()
    }

    /// 特定戦略のオプション値を設定する。
    pub fn set_option(&mut self, strategy_id: &str, option_id: &str, value: bool) {
        self.options
            .insert(format!("{strategy_id}.{option_id}"), value);
    }

    /// 選択がカタログに対して有効か検証する。
    ///
    /// # Errors
    /// 選択された戦略がカタログに存在しない場合、`UnknownStrategy` を返す。
    pub fn validate(&self, catalog: &StrategyCatalog) -> Result<(), SelectionError> {
        if let Some(id) = &self.strategy
            && catalog.find(id).is_none()
        {
            return Err(SelectionError::UnknownStrategy { id: id.clone() });
        }
        Ok(())
    }
}

/// 選択検証エラー。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SelectionError {
    /// カタログに存在しない戦略が選択された。
    #[error("unknown strategy: {id}")]
    UnknownStrategy { id: String },
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// 実在が期待される PC 版 2 パイプラインの id 一覧（Issue #139 / #188）。
    const EXPECTED_IDS: [&str; 2] = ["field_loop_pc", "nav_to_field_pc"];

    #[test]
    fn builtin_catalog_contains_exactly_two_real_pc_pipelines() {
        let catalog = StrategyCatalog::builtin();
        let ids: Vec<&str> = catalog.strategies().iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, EXPECTED_IDS, "catalog ids: {ids:?}");
    }

    #[test]
    fn builtin_catalog_does_not_contain_nonexistent_fishing() {
        let catalog = StrategyCatalog::builtin();
        assert!(
            catalog.find("fishing").is_none(),
            "fishing はカタログ外 (templates/tasks のタスクバンク経由) であること"
        );
    }

    #[test]
    fn every_strategy_has_pipeline_dir_start_task_and_valid_algorithm() {
        let catalog = StrategyCatalog::builtin();
        for s in catalog.strategies() {
            assert!(!s.pipeline_dir.is_empty(), "{}: pipeline_dir 未設定", s.id);
            assert!(
                s.pipeline_dir.contains(&s.id),
                "{}: pipeline_dir に id が含まれること ({}),",
                s.id,
                s.pipeline_dir
            );
            assert!(!s.start_task.is_empty(), "{}: start_task 未設定", s.id);
            assert!(
                s.algorithm == "sse" || s.algorithm == "ccoeff",
                "{}: algorithm は sse|ccoeff のみ (actual: {})",
                s.id,
                s.algorithm
            );
        }
    }

    #[test]
    fn catalog_pipeline_dirs_actually_exist_in_repo() {
        // カタログ定義がリポジトリ実体と乖離していないことの機械検証
        //（テスト実行は crate ルートから相対でない場合があるため env!("CARGO_MANIFEST_DIR")
        // からリポジトリルートを辿る）。
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir.ancestors().nth(2).unwrap();
        let catalog = StrategyCatalog::builtin();
        for s in catalog.strategies() {
            let dir = repo_root.join(&s.pipeline_dir);
            assert!(
                dir.is_dir(),
                "{}: pipeline_dir が実在しない: {}",
                s.id,
                s.pipeline_dir
            );
        }
    }

    #[test]
    fn all_strategies_carry_windows_target() {
        // Issue #188: PC 専用化により全カタログ戦略が --target windows を持つ。
        let catalog = StrategyCatalog::builtin();
        for s in catalog.strategies() {
            assert_eq!(
                s.target.as_deref(),
                Some("windows"),
                "{}: target 設定が想定と異なる (actual: {:?})",
                s.id,
                s.target
            );
        }
    }

    #[test]
    fn to_run_args_builds_catalog_driven_argument_sequence() {
        let catalog = StrategyCatalog::builtin();
        let nav_pc = catalog.find("nav_to_field_pc").unwrap();
        assert_eq!(
            nav_pc.to_run_args(),
            vec![
                "run",
                "--algorithm",
                "ccoeff",
                "templates/pipelines/nav_to_field_pc",
                "TapToStartPc",
            ]
        );
        let field = catalog.find("field_loop_pc").unwrap();
        assert_eq!(
            field.to_run_args(),
            vec![
                "run",
                "--algorithm",
                "ccoeff",
                "templates/pipelines/field_loop_pc",
                "TapBottomStablePc",
            ]
        );
    }

    #[test]
    fn find_unknown_strategy_is_none() {
        let catalog = StrategyCatalog::builtin();
        assert!(catalog.find("nonexistent").is_none());
    }

    #[test]
    fn default_selection_has_no_strategy_and_empty_options() {
        let catalog = StrategyCatalog::builtin();
        let sel = catalog.default_selection();
        assert_eq!(sel.strategy, None);
        assert!(
            sel.options.is_empty(),
            "実在 2 パイプラインには ON/OFF オプションが無い"
        );
    }

    #[test]
    fn set_and_get_option_roundtrip() {
        let mut sel = StrategySelection::default();
        sel.set_option("field_loop_pc", "some_option", true);
        assert_eq!(sel.option("field_loop_pc", "some_option"), Some(true));
        assert_eq!(sel.option("field_loop_pc", "other"), None);
    }

    #[test]
    fn selection_toml_roundtrip_keeps_compatibility() {
        // 既存 TOML 設定形式との互換: toml 往復で値が保存される。
        let mut sel = StrategySelection::from_defaults(&StrategyCatalog::builtin());
        sel.strategy = Some("field_loop_pc".to_string());
        sel.set_option("field_loop_pc", "demo", false);

        let toml_str = toml::to_string(&sel).expect("serialize");
        assert!(toml_str.contains(r#"strategy = "field_loop_pc""#));

        let parsed: StrategySelection = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(parsed, sel);
    }

    #[test]
    fn selection_parses_unknown_strategy_without_error() {
        // フォワード互換: 未知の戦略（旧 fishing 設定ファイル等）でも
        // デシリアライズ自体は成功する。
        let toml_str = r#"strategy = "fishing""#;
        let parsed: StrategySelection = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(parsed.strategy.as_deref(), Some("fishing"));
    }

    #[test]
    fn validate_rejects_unknown_strategy() {
        let catalog = StrategyCatalog::builtin();
        let mut sel = StrategySelection {
            strategy: Some("bogus".to_string()),
            ..Default::default()
        };
        assert!(matches!(
            sel.validate(&catalog),
            Err(SelectionError::UnknownStrategy { .. })
        ));

        // fishing はカタログ外（タスクバンク経由）のため検証も拒否する。
        sel = StrategySelection {
            strategy: Some("fishing".to_string()),
            ..Default::default()
        };
        assert!(matches!(
            sel.validate(&catalog),
            Err(SelectionError::UnknownStrategy { .. })
        ));

        sel = StrategySelection {
            strategy: Some("field_loop_pc".to_string()),
            ..Default::default()
        };
        assert!(sel.validate(&catalog).is_ok());
        sel = StrategySelection::default();
        assert!(sel.validate(&catalog).is_ok());
    }
}
