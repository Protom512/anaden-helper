//! 戦略カタログ: 選択可能な戦略とオプションの宣言的定義（UI・設定向け）。
//!
//! 戦略の実体 (`MiniGameStrategy` trait 実装) とは分離されたメタ情報層。
//! anaden-studio 等 GUI はこのカタログからドロップダウン/チェックボックスを生成し、
//! 選択結果を [`StrategySelection`] (serde/TOML互換) に射影する。

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

/// 選択可能な戦略の定義。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyDef {
    /// 戦略の識別子（TOML キー・レジストリ名と一致）。
    pub id: String,
    /// UI 表示名。
    pub label: String,
    /// ON/OFF 可能なオプション群。
    pub options: Vec<StrategyOptionDef>,
}

/// 戦略カタログ。組み込み戦略のメタ情報一覧。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyCatalog {
    strategies: Vec<StrategyDef>,
}

impl StrategyCatalog {
    /// 組み込み戦略を列挙したカタログを構築する。
    ///
    /// 段階拡張（Issue #83 シャード3）: 当面は主要戦略 Fishing のみ。
    /// 全戦略網羅は非スコープ。
    #[must_use]
    pub fn builtin() -> Self {
        Self {
            strategies: vec![StrategyDef {
                id: "fishing".to_string(),
                label: "釣りミニゲーム".to_string(),
                options: vec![
                    StrategyOptionDef {
                        id: "auto_release".to_string(),
                        label: "釣り上げ後に自動リリース".to_string(),
                        default: true,
                    },
                    StrategyOptionDef {
                        id: "skip_animation".to_string(),
                        label: "リールアニメーション省略".to_string(),
                        default: false,
                    },
                ],
            }],
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

    #[test]
    fn builtin_catalog_contains_fishing_with_two_options() {
        let catalog = StrategyCatalog::builtin();
        let fishing = catalog.find("fishing").expect("fishing must exist");
        assert_eq!(fishing.label, "釣りミニゲーム");
        assert_eq!(fishing.options.len(), 2);
        assert!(fishing.options.iter().any(|o| o.id == "auto_release"));
        assert!(fishing.options.iter().any(|o| o.id == "skip_animation"));
    }

    #[test]
    fn find_unknown_strategy_is_none() {
        let catalog = StrategyCatalog::builtin();
        assert!(catalog.find("nonexistent").is_none());
    }

    #[test]
    fn default_selection_has_defaults_and_no_strategy() {
        let catalog = StrategyCatalog::builtin();
        let sel = catalog.default_selection();
        assert_eq!(sel.strategy, None);
        assert_eq!(sel.option("fishing", "auto_release"), Some(true));
        assert_eq!(sel.option("fishing", "skip_animation"), Some(false));
    }

    #[test]
    fn set_and_get_option_roundtrip() {
        let mut sel = StrategySelection::default();
        sel.set_option("fishing", "skip_animation", true);
        assert_eq!(sel.option("fishing", "skip_animation"), Some(true));
        assert_eq!(sel.option("fishing", "auto_release"), None);
    }

    #[test]
    fn selection_toml_roundtrip_keeps_compatibility() {
        // 既存 TOML 設定形式との互換: toml 往復で値が保存される。
        let mut sel = StrategySelection::from_defaults(&StrategyCatalog::builtin());
        sel.strategy = Some("fishing".to_string());
        sel.set_option("fishing", "auto_release", false);

        let toml_str = toml::to_string(&sel).expect("serialize");
        assert!(toml_str.contains(r#"strategy = "fishing""#));
        // toml クレートはドット含有キーを引用符付き (`"fishing.auto_release" = false`) で
        // 出力するため、キー名のみを部分一致で検証する。
        assert!(toml_str.contains("fishing.auto_release"));

        let parsed: StrategySelection = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(parsed, sel);
    }

    #[test]
    fn selection_parses_unknown_strategy_without_error() {
        // フォワード互換: 未知の戦略でもデシリアライズ自体は成功する。
        let toml_str = r#"strategy = "future-strategy""#;
        let parsed: StrategySelection = toml::from_str(toml_str).expect("deserialize");
        assert_eq!(parsed.strategy.as_deref(), Some("future-strategy"));
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

        sel = StrategySelection {
            strategy: Some("fishing".to_string()),
            ..Default::default()
        };
        assert!(sel.validate(&catalog).is_ok());
        sel = StrategySelection::default();
        assert!(sel.validate(&catalog).is_ok());
    }
}
