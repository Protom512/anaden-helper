//! 戦略選択・オプション UI（Issue #83 シャード3）。
//!
//! `anaden-strategies` のカタログからドロップダウン（戦略選択）と
//! チェックボックス群（ON/OFF オプション）を描画し、選択状態を
//! [`StrategySelection`]（serde/TOML 互換）へ反映する。
//!
//! 描画ロジックと状態操作を分離し、状態操作（純粋なモデル層）は
//! egui なしでユニットテスト可能にしている。

use anaden_strategies::{StrategyCatalog, StrategyDef, StrategySelection};
use egui::Ui;

/// 戦略 UI パネルの状態。
///
/// カタログは不変（builtin 固定）で、選択状態のみ可変。
///
/// `selection` / `validate` / `to_toml` / `load_toml` はシャード2の IPC 整備で
/// 実行開始時のオプション渡しに使う公開 API（現時点では runner UI からの直接呼び出し
/// なし）。ライブラリ的公開面として dead_code 警告を意図的に抑止する。
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct StrategyPanel {
    /// 選択可能な戦略カタログ。
    catalog: StrategyCatalog,
    /// 現在の選択状態。
    selection: StrategySelection,
}

impl Default for StrategyPanel {
    fn default() -> Self {
        Self::new(StrategyCatalog::builtin())
    }
}

impl StrategyPanel {
    // selection/validate/to_toml/load_toml はシャード2 IPC 整備で実行開始時に
    // 使用する公開 API。現時点で runner UI からの呼び出しがないため
    // dead_code を impl 単位で抑止（構造体側の属性はメソッドに波及しない）。
    #![allow(dead_code)]

    /// カタログを指定して構築する。選択状態はカタログ既定値。
    #[must_use]
    pub fn new(catalog: StrategyCatalog) -> Self {
        let selection = catalog.default_selection();
        Self { catalog, selection }
    }

    /// 現在の選択状態への参照。
    #[must_use]
    pub fn selection(&self) -> &StrategySelection {
        &self.selection
    }

    /// 選択状態を検証する。
    ///
    /// # Errors
    /// 選択中の戦略がカタログに存在しない場合 [`anaden_strategies::SelectionError`]。
    pub fn validate(&self) -> Result<(), anaden_strategies::SelectionError> {
        self.selection.validate(&self.catalog)
    }

    /// 戦略を選択する。カタログ外の id は無視する（UI はカタログ由来のみ提示）。
    pub fn select_strategy(&mut self, id: &str) {
        if self.catalog.find(id).is_some() {
            self.selection.strategy = Some(id.to_string());
        }
    }

    /// 選択を解除する（戦略なし）。
    pub fn clear_strategy(&mut self) {
        self.selection.strategy = None;
    }

    /// 選択中の戦略定義。
    #[must_use]
    pub fn selected_def(&self) -> Option<&StrategyDef> {
        self.selection
            .strategy
            .as_deref()
            .and_then(|id| self.catalog.find(id))
    }

    /// オプションをトグルする。選択中戦略のオプション以外は無視。
    pub fn toggle_option(&mut self, option_id: &str, value: bool) {
        // borrow checker: `def` を借用したまま `self.selection` を可変借用できないため、
        // 判定と適用を2段階に分ける（`selected_def` の借用を先に切り上げる）。
        let def_id = self.selected_def().map(|d| d.id.clone());
        if let Some(def_id) = def_id {
            let is_valid = self
                .catalog
                .find(&def_id)
                .is_some_and(|d| d.options.iter().any(|o| o.id == option_id));
            if is_valid {
                self.selection.set_option(&def_id, option_id, value);
            }
        }
    }

    /// 選択状態を TOML 文字列へ保存する（既存 toml 設定形式との互換）。
    ///
    /// # Errors
    /// serde/toml シリアライズ失敗時は `toml::ser::Error` を返す。
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(&self.selection)
    }

    /// TOML 文字列から選択状態を復元する。
    ///
    /// # Errors
    /// デシリアライズ失敗時は `toml::de::Error` を返す。
    /// カタログ整合性（未知戦略）はエラーにせず保持（フォワード互換）。
    pub fn load_toml(&mut self, s: &str) -> Result<(), toml::de::Error> {
        self.selection = toml::from_str(s)?;
        Ok(())
    }

    /// パネルを描画する。戻り値は「選択状態が変化した」フラグ。
    pub fn ui(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;
        ui.heading("戦略設定");
        ui.add_space(4.0);

        // 戦略選択ドロップダウン（「なし」+ カタログの各戦略）。
        let current = self.selection.strategy.clone().unwrap_or_default();
        let selected_label = self
            .selected_def()
            .map_or_else(|| "（なし）".to_string(), |d| d.label.clone());
        // egui 0.34 の ComboBox::show_ui は 2 引数 (ui, closure)。0.31 以前にあった
        // 幅指定オーバーロードは廃止されているため、`ui.set_min_width` で代用。
        // borrow checker: closure 内で self を可変借用するため、ドロップダウン項目は
        // 先に (id, label) の owned ペアへ複製してイミュータブル借用を切り上げる。
        let entries: Vec<(String, String)> = self
            .catalog
            .strategies()
            .iter()
            .map(|d| (d.id.clone(), d.label.clone()))
            .collect();
        egui::ComboBox::from_id_salt("strategy_select")
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                ui.set_min_width(160.0);
                if ui
                    .selectable_label(current.is_empty(), "（なし）")
                    .clicked()
                {
                    self.clear_strategy();
                    changed = true;
                }
                for (id, label) in &entries {
                    let is_sel = current == *id;
                    if ui.selectable_label(is_sel, label).clicked() {
                        self.select_strategy(id);
                        changed = true;
                    }
                }
            });

        ui.add_space(6.0);

        // 選択中戦略のオプションをチェックボックスで表示。
        // 同様に def/opt の借用を表示用データへ複製してから toggle_option を呼ぶ。
        let selected = self.selected_def().map(|d| {
            (
                d.label.clone(),
                d.options
                    .iter()
                    .map(|o| (o.id.clone(), o.label.clone(), o.default))
                    .collect::<Vec<_>>(),
            )
        });
        if let Some((label, options)) = selected {
            ui.label(format!("「{label}」のオプション:"));
            for (opt_id, opt_label, default) in options {
                let key = format!(
                    "{}.{}",
                    self.selection.strategy.clone().unwrap_or_default(),
                    opt_id
                );
                let mut value = self.selection.options.get(&key).copied().unwrap_or(default);
                if ui.checkbox(&mut value, &opt_label).changed() {
                    self.toggle_option(&opt_id, value);
                    changed = true;
                }
            }
        } else {
            ui.weak("戦略を選択してください");
        }

        changed
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn new_panel_has_defaults_and_no_strategy() {
        let panel = StrategyPanel::default();
        assert_eq!(panel.selection().strategy, None);
        assert_eq!(
            panel.selection().option("fishing", "auto_release"),
            Some(true)
        );
    }

    #[test]
    fn select_strategy_updates_selection_and_selected_def() {
        let mut panel = StrategyPanel::default();
        panel.select_strategy("fishing");
        assert_eq!(panel.selection().strategy.as_deref(), Some("fishing"));
        let def = panel.selected_def().expect("def");
        assert_eq!(def.id, "fishing");
    }

    #[test]
    fn select_unknown_strategy_is_ignored() {
        let mut panel = StrategyPanel::default();
        panel.select_strategy("bogus");
        assert_eq!(panel.selection().strategy, None);
    }

    #[test]
    fn toggle_option_of_selected_strategy_applies() {
        let mut panel = StrategyPanel::default();
        panel.select_strategy("fishing");
        panel.toggle_option("skip_animation", true);
        assert_eq!(
            panel.selection().option("fishing", "skip_animation"),
            Some(true)
        );
    }

    #[test]
    fn toggle_option_without_strategy_is_ignored() {
        let mut panel = StrategyPanel::default();
        panel.toggle_option("skip_animation", true);
        // 戦略未選択では何も変わらない（既定値のまま）。
        assert_eq!(
            panel.selection().option("fishing", "skip_animation"),
            Some(false)
        );
    }

    #[test]
    fn toggle_unknown_option_is_ignored() {
        let mut panel = StrategyPanel::default();
        panel.select_strategy("fishing");
        panel.toggle_option("nonexistent", true);
        assert!(
            !panel
                .selection()
                .options
                .contains_key("fishing.nonexistent")
        );
    }

    #[test]
    fn validate_ok_after_select_and_clear() {
        let mut panel = StrategyPanel::default();
        assert!(panel.validate().is_ok());
        panel.select_strategy("fishing");
        assert!(panel.validate().is_ok());
    }

    #[test]
    fn toml_roundtrip_preserves_selection() {
        let mut panel = StrategyPanel::default();
        panel.select_strategy("fishing");
        panel.toggle_option("auto_release", false);
        panel.toggle_option("skip_animation", true);

        let toml_str = panel.to_toml().expect("serialize");
        let mut restored = StrategyPanel::default();
        restored.load_toml(&toml_str).expect("deserialize");

        assert_eq!(restored.selection(), panel.selection());
        assert!(restored.validate().is_ok());
    }

    #[test]
    fn load_toml_invalid_string_is_err() {
        let mut panel = StrategyPanel::default();
        assert!(panel.load_toml("not = valid = toml").is_err());
    }
}
