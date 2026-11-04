use crate::history::{RunOutcome, RunRecord};
use crate::settings::StudioSettings;
use std::path::PathBuf;

pub const LOG_TAIL_LINES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonState {
    pub running: bool,
}

impl ButtonState {
    #[must_use]
    pub fn rerun_enabled(self) -> bool {
        !self.running
    }

    #[must_use]
    pub fn stop_enabled(self) -> bool {
        self.running
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryAction {
    Rerun,
    Stop,
    SaveSettings,
    LoadSettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsStatus {
    Ok(PathBuf),
    Err(String),
    None,
}

pub struct HistoryPanel {
    entries: Vec<RunRecord>,
    selected: Option<usize>,
    buttons: ButtonState,
    pending_action: Option<HistoryAction>,
    settings_status: SettingsStatus,
}

impl Default for HistoryPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl HistoryPanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            selected: None,
            buttons: ButtonState { running: false },
            pending_action: None,
            settings_status: SettingsStatus::None,
        }
    }

    pub fn refresh_from(&mut self, records: &[RunRecord]) {
        self.entries = records.to_vec();
        match self.selected {
            Some(i) if i < self.entries.len() => {}
            _ => self.selected = (!self.entries.is_empty()).then_some(0),
        }
    }

    #[must_use]
    pub fn entries(&self) -> &[RunRecord] {
        &self.entries
    }

    #[must_use]
    pub fn selected(&self) -> Option<&RunRecord> {
        self.selected.and_then(|i| self.entries.get(i))
    }

    pub fn select(&mut self, index: usize) {
        if index < self.entries.len() {
            self.selected = Some(index);
        }
    }

    pub fn set_running(&mut self, running: bool) {
        self.buttons.running = running;
    }

    #[must_use]
    pub fn buttons(&self) -> ButtonState {
        self.buttons
    }

    pub fn take_action(&mut self) -> Option<HistoryAction> {
        self.pending_action.take()
    }

    #[must_use]
    pub fn settings_status(&self) -> &SettingsStatus {
        &self.settings_status
    }

    pub fn request(&mut self, action: HistoryAction) {
        self.pending_action = Some(action);
    }

    pub fn save_settings(&mut self, settings: &StudioSettings, path: &std::path::Path) {
        self.settings_status = match settings.save(path) {
            Ok(()) => SettingsStatus::Ok(path.to_path_buf()),
            Err(e) => SettingsStatus::Err(format!("設定保存失敗: {e}")),
        };
    }

    pub fn load_settings(&mut self, path: &std::path::Path) -> Option<StudioSettings> {
        use crate::settings::LoadOutcome;
        match StudioSettings::load(path) {
            LoadOutcome::Loaded(s) => {
                self.settings_status = SettingsStatus::Ok(path.to_path_buf());
                Some(s)
            }
            LoadOutcome::Fallback => {
                self.settings_status = SettingsStatus::Ok(path.to_path_buf());
                None
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("実行履歴");
        ui.add_space(4.0);

        if self.entries.is_empty() {
            ui.weak("（実行履歴なし）");
        } else {
            let labels: Vec<String> = self.entries.iter().map(record_label).collect();
            let mut clicked: Option<usize> = None;
            for (i, label) in labels.iter().enumerate() {
                let is_sel = self.selected == Some(i);
                if ui.selectable_label(is_sel, label).clicked() {
                    clicked = Some(i);
                }
            }
            if let Some(i) = clicked {
                self.select(i);
            }
        }

        ui.add_space(6.0);
        ui.separator();

        if let Some(entry) = self.selected() {
            if let Some(run_id) = &entry.evidence_run_id {
                ui.label(format!("evidence: .omc/logs/{run_id}/"));
            }
            ui.label(format!("「{}」のログ末尾:", entry.strategy));
            egui::ScrollArea::vertical()
                .max_height(160.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let start = entry.log_tail.len().saturating_sub(LOG_TAIL_LINES);
                    ui.monospace(entry.log_tail[start..].join("\n"));
                });
        }

        ui.separator();

        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.buttons.rerun_enabled(), egui::Button::new("🔁 再実行"))
                .clicked()
            {
                self.pending_action = Some(HistoryAction::Rerun);
            }
            if ui
                .add_enabled(self.buttons.stop_enabled(), egui::Button::new("⏹ 中止"))
                .clicked()
            {
                self.pending_action = Some(HistoryAction::Stop);
            }
            if ui.button("💾 設定を保存").clicked() {
                self.pending_action = Some(HistoryAction::SaveSettings);
            }
            if ui.button("📂 設定を読込").clicked() {
                self.pending_action = Some(HistoryAction::LoadSettings);
            }
        });

        match &self.settings_status {
            SettingsStatus::Ok(p) => {
                ui.label(format!("設定: {}", p.display()));
            }
            SettingsStatus::Err(e) => {
                ui.colored_label(egui::Color32::RED, e.clone());
            }
            SettingsStatus::None => {}
        }
    }
}

fn record_label(r: &RunRecord) -> String {
    let outcome = match r.outcome {
        RunOutcome::Success => "完了",
        RunOutcome::Failed => "異常終了",
        RunOutcome::Cancelled => "中止",
        RunOutcome::BudgetExhausted => "予算切れ",
    };
    format!("@{} {} [{}]", r.started_at_unix, r.strategy, outcome)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::history::RunOutcome;

    fn record(started: u64, strategy: &str, outcome: RunOutcome, n_lines: usize) -> RunRecord {
        RunRecord::new(
            started,
            strategy,
            outcome,
            Some(0),
            (0..n_lines).map(|i| format!("line-{i}")).collect(),
        )
    }

    // ---- 構築/選択状態 ----

    #[test]
    fn new_panel_is_empty_and_no_selection() {
        let p = HistoryPanel::new();
        assert!(p.entries().is_empty());
        assert!(p.selected().is_none());
        assert!(p.buttons().rerun_enabled());
        assert_eq!(p.settings_status(), &SettingsStatus::None);
    }

    #[test]
    fn refresh_from_populates_entries_and_selects_first() {
        let mut p = HistoryPanel::new();
        let recs = vec![
            record(2, "fishing", RunOutcome::Success, 1),
            record(1, "fishing", RunOutcome::Failed, 1),
        ];
        p.refresh_from(&recs);
        assert_eq!(p.entries().len(), 2);
        // 新しい順 (index 0 = started 2)。
        assert_eq!(p.entries()[0].started_at_unix, 2);
        // 差し替え直後は最新が選択される。
        assert_eq!(p.selected().map(|r| r.started_at_unix), Some(2));
    }

    #[test]
    fn refresh_from_clamps_stale_selection() {
        let mut p = HistoryPanel::new();
        let recs = vec![record(1, "s", RunOutcome::Success, 1)];
        p.refresh_from(&recs);
        p.select(0);
        // 履歴が空になったら選択は解除される。
        p.refresh_from(&[]);
        assert!(p.selected().is_none());
    }

    #[test]
    fn select_updates_selected_entry() {
        let mut p = HistoryPanel::new();
        p.refresh_from(&[
            record(2, "a", RunOutcome::Success, 1),
            record(1, "b", RunOutcome::Success, 1),
        ]);
        p.select(1);
        assert_eq!(p.selected().map(|r| r.strategy.as_str()), Some("b"));
    }

    #[test]
    fn select_out_of_range_is_ignored() {
        let mut p = HistoryPanel::new();
        p.refresh_from(&[record(1, "a", RunOutcome::Success, 1)]);
        p.select(0);
        p.select(99);
        assert_eq!(p.selected().map(|r| r.strategy.as_str()), Some("a"));
    }

    // ---- ButtonState (UC-2) ----

    #[test]
    fn buttons_when_stopped_rerun_only() {
        let b = ButtonState { running: false };
        assert!(b.rerun_enabled());
        assert!(!b.stop_enabled());
    }

    #[test]
    fn buttons_when_running_stop_only() {
        let b = ButtonState { running: true };
        assert!(!b.rerun_enabled());
        assert!(b.stop_enabled());
    }

    #[test]
    fn set_running_updates_button_state() {
        let mut p = HistoryPanel::new();
        assert!(p.buttons().rerun_enabled());
        p.set_running(true);
        assert!(p.buttons().stop_enabled());
        assert!(!p.buttons().rerun_enabled());
    }

    // ---- アクション要求 (UI ボタンのヘッドレス等価) ----

    #[test]
    fn take_action_returns_none_initially_and_after_consume() {
        let mut p = HistoryPanel::new();
        assert_eq!(p.take_action(), None);
        p.request(HistoryAction::Rerun);
        assert_eq!(p.take_action(), Some(HistoryAction::Rerun));
        assert_eq!(p.take_action(), None);
    }

    #[test]
    fn request_overwrites_previous_action() {
        let mut p = HistoryPanel::new();
        p.request(HistoryAction::Stop);
        p.request(HistoryAction::Rerun);
        assert_eq!(p.take_action(), Some(HistoryAction::Rerun));
    }

    // ---- 設定保存/読込 (UC-2: 失敗しても GUI 継続) ----

    #[test]
    fn save_settings_records_ok_path() {
        let mut p = HistoryPanel::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        p.save_settings(&StudioSettings::default(), &path);
        assert_eq!(p.settings_status(), &SettingsStatus::Ok(path.clone()));
        assert!(path.is_file());
    }

    #[test]
    fn load_settings_returns_saved_selection() {
        let mut p = HistoryPanel::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        p.save_settings(&StudioSettings::default(), &path);
        let loaded = p.load_settings(&path);
        assert!(loaded.is_some());
        assert_eq!(p.settings_status(), &SettingsStatus::Ok(path));
    }

    #[test]
    fn load_settings_missing_file_is_not_an_error_for_gui() {
        // UC-2: 初回起動 (ファイル不在) はエラー表示にしない。
        let mut p = HistoryPanel::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.toml");
        assert!(p.load_settings(&path).is_none());
        assert!(matches!(p.settings_status(), SettingsStatus::Ok(_)));
    }

    // ---- 表示ラベル純関数 ----

    #[test]
    fn record_label_contains_strategy_and_outcome_ja() {
        let r = record(42, "fishing", RunOutcome::Cancelled, 0);
        let label = record_label(&r);
        assert!(label.contains("fishing"));
        assert!(label.contains("中止"));
        assert!(label.contains("42"));
    }
}
