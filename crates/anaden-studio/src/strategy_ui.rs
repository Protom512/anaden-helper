//! 戦略選択・オプション UI（Issue #83 シャード3）。
//!
//! `anaden-strategies` のカタログからドロップダウン（戦略選択）と
//! チェックボックス群（ON/OFF オプション）を描画し、選択状態を
//! [`StrategySelection`]（serde/TOML 互換）へ反映する。
//!
//! 描画ロジックと状態操作を分離し、状態操作（純粋なモデル層）は
//! egui なしでユニットテスト可能にしている。

use egui::Ui;
use anaden_strategies::{StrategyCatalog, StrategyDef, StrategySelection};

/// 戦略 UI パネルの状態。
///
/// カタログは不変（builtin 固定）で、選択状態のみ可変。
#[derive(Debug, Clone)]
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
        if let Some(def) = self.selected_def() {
            if def.options.iter().any(|o| o.id == option_id) {
                self.selection.set_option(&def.id, option_id, value);
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
    pub fn from_toml(&mut self, s: &str) -> Result<(), toml::de::Error> {
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
        egui::ComboBox::from_id_salt("strategy_select")
            .selected_text(selected_label)
            .show_ui(ui, 6.0, |ui| {
                if ui
                    .selectable_label(current.is_empty(), "（なし）")
                    .clicked()
                {
                    self.clear_strategy();
                    changed = true;
                }
                for def in self.catalog.strategies() {
                    let is_sel = current == def.id;
                    if ui.selectable_label(is_sel, &def.label).clicked() {
                        self.select_strategy(&def.id);
                        changed = true;
                    }
                }
            });

        ui.add_space(6.0);

        // 選択中戦略のオプションをチェックボックスで表示。
        if let Some(def) = self.selected_def() {
            ui.label(format!("「{}」のオプション:", def.label));
            for opt in &def.options {
                let key = format!("{}.{}", def.id, opt.id);
                let mut value = self
                    .selection
                    .options
                    .get(&key)
                    .copied()
                    .unwrap_or(opt.default);
                if ui.checkbox(&mut value, &opt.label).changed() {
                    self.toggle_option(&opt.id, value);
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
        assert!(panel
            .selection()
            .options
            .get("fishing.nonexistent")
            .is_none());
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
        restored.from_toml(&toml_str).expect("deserialize");

        assert_eq!(restored.selection(), panel.selection());
        assert!(restored.validate().is_ok());
    }

    #[test]
    fn from_toml_invalid_string_is_err() {
        let mut panel = StrategyPanel::default();
        assert!(panel.from_toml("not = valid = toml").is_err());
    }
}
