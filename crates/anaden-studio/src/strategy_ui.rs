use anaden_strategies::{StrategyCatalog, StrategyDef, StrategySelection};
use egui::Ui;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct StrategyPanel {
    catalog: StrategyCatalog,
    selection: StrategySelection,
}

impl Default for StrategyPanel {
    fn default() -> Self {
        Self::new(StrategyCatalog::builtin())
    }
}

impl StrategyPanel {
    #![allow(dead_code)]

    #[must_use]
    pub fn new(catalog: StrategyCatalog) -> Self {
        let selection = catalog.default_selection();
        Self { catalog, selection }
    }

    #[must_use]
    pub fn selection(&self) -> &StrategySelection {
        &self.selection
    }

    pub fn validate(&self) -> Result<(), anaden_strategies::SelectionError> {
        self.selection.validate(&self.catalog)
    }

    pub fn select_strategy(&mut self, id: &str) {
        if self.catalog.find(id).is_some() {
            self.selection.strategy = Some(id.to_string());
        }
    }

    pub fn clear_strategy(&mut self) {
        self.selection.strategy = None;
    }

    #[must_use]
    pub fn selected_def(&self) -> Option<&StrategyDef> {
        self.selection
            .strategy
            .as_deref()
            .and_then(|id| self.catalog.find(id))
    }

    pub fn toggle_option(&mut self, option_id: &str, value: bool) {
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

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(&self.selection)
    }

    pub fn load_toml(&mut self, s: &str) -> Result<(), toml::de::Error> {
        self.selection = toml::from_str(s)?;
        Ok(())
    }

    #[must_use]
    pub fn summary(&self) -> String {
        let Some(id) = self.selection.strategy.as_deref() else {
            return "戦略未選択".to_string();
        };
        let enabled: Vec<String> = self
            .selection
            .options
            .iter()
            .filter(|(k, v)| **v && k.starts_with(&format!("{id}.")))
            .map(|(k, _)| k.clone())
            .collect();
        if enabled.is_empty() {
            format!("strategy={id} (オプションなし)")
        } else {
            format!("strategy={id} on=[{}]", enabled.join(","))
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) -> bool {
        let mut changed = false;
        ui.heading("戦略設定");
        ui.add_space(4.0);

        let current = self.selection.strategy.clone().unwrap_or_default();
        let selected_label = self
            .selected_def()
            .map_or_else(|| "（なし）".to_string(), |d| d.label.clone());
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

    pub fn render_tab_body(&mut self, ui: &mut Ui, running: bool) -> bool {
        ui.heading(tab_heading());
        ui.label(tab_status_line(running));
        ui.separator();
        let changed = self.ui(ui);
        ui.separator();
        ui.weak(tab_summary_line(&self.summary()));
        changed
    }
}

#[must_use]
pub fn tab_heading() -> &'static str {
    "戦略選択"
}

#[must_use]
pub fn tab_status_line(running: bool) -> &'static str {
    if running {
        "状態: 実行中"
    } else {
        "状態: 停止"
    }
}

#[must_use]
pub fn tab_summary_line(summary: &str) -> String {
    format!("選択: {summary}")
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
        // 実在 2 パイプラインは ON/OFF オプションを持たないため空。
        assert!(panel.selection().options.is_empty());
    }

    #[test]
    fn select_strategy_updates_selection_and_selected_def() {
        let mut panel = StrategyPanel::default();
        panel.select_strategy("field_loop_pc");
        assert_eq!(panel.selection().strategy.as_deref(), Some("field_loop_pc"));
        let def = panel.selected_def().expect("def");
        assert_eq!(def.id, "field_loop_pc");
    }

    #[test]
    fn select_unknown_strategy_is_ignored() {
        let mut panel = StrategyPanel::default();
        panel.select_strategy("bogus");
        assert_eq!(panel.selection().strategy, None);
    }

    #[test]
    fn toggle_option_without_strategy_is_ignored() {
        let mut panel = StrategyPanel::default();
        panel.toggle_option("skip_animation", true);
        // 戦略未選択では何も変わらない。
        assert!(!panel.selection().options.contains_key("skip_animation"));
    }

    #[test]
    fn toggle_unknown_option_is_ignored() {
        let mut panel = StrategyPanel::default();
        panel.select_strategy("field_loop_pc");
        panel.toggle_option("nonexistent", true);
        assert!(
            !panel
                .selection()
                .options
                .contains_key("field_loop_pc.nonexistent")
        );
    }

    /// Issue #139 T1: 実在しない fishing はカタログ外のため選択できない。
    #[test]
    fn select_nonexistent_fishing_is_ignored() {
        let mut panel = StrategyPanel::default();
        panel.select_strategy("fishing");
        assert_eq!(panel.selection().strategy, None);
    }

    #[test]
    fn validate_ok_after_select_and_clear() {
        let mut panel = StrategyPanel::default();
        assert!(panel.validate().is_ok());
        panel.select_strategy("field_loop_pc");
        assert!(panel.validate().is_ok());
    }

    #[test]
    fn toml_roundtrip_preserves_selection() {
        let mut panel = StrategyPanel::default();
        panel.select_strategy("nav_to_field_pc");

        let toml_str = panel.to_toml().expect("serialize");
        let mut restored = StrategyPanel::default();
        restored.load_toml(&toml_str).expect("deserialize");

        assert_eq!(restored.selection(), panel.selection());
        assert!(restored.validate().is_ok());
    }

    #[test]
    fn summary_unselected_is_placeholder() {
        let panel = StrategyPanel::default();
        assert_eq!(panel.summary(), "戦略未選択");
    }

    #[test]
    fn summary_selected_lists_enabled_options() {
        let mut panel = StrategyPanel::default();
        panel.select_strategy("nav_to_field_pc");
        // 実在 2 パイプラインは ON/OFF オプションを持たない。
        assert_eq!(panel.summary(), "strategy=nav_to_field_pc (オプションなし)");
    }

    #[test]
    fn summary_selected_all_off_shows_no_options() {
        let mut panel = StrategyPanel::default();
        panel.select_strategy("field_loop_pc");
        assert_eq!(panel.summary(), "strategy=field_loop_pc (オプションなし)");
    }

    #[test]
    fn load_toml_invalid_string_is_err() {
        let mut panel = StrategyPanel::default();
        assert!(panel.load_toml("not = valid = toml").is_err());
    }

    // ---- Issue #125 shard 3 タスク3: 独立タブ向け純関数 ----

    #[test]
    fn tab_heading_is_strategy_selection() {
        assert_eq!(tab_heading(), "戦略選択");
    }

    #[test]
    fn tab_status_line_reflects_running_state() {
        assert_eq!(tab_status_line(true), "状態: 実行中");
        assert_eq!(tab_status_line(false), "状態: 停止");
    }

    #[test]
    fn tab_summary_line_prefixes_selection() {
        assert_eq!(tab_summary_line("戦略未選択"), "選択: 戦略未選択");
        assert_eq!(
            tab_summary_line("strategy=field_loop_pc (オプションなし)"),
            "選択: strategy=field_loop_pc (オプションなし)"
        );
    }
}
