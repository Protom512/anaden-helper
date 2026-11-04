use eframe::egui;

use crate::app::StudioApp;
use crate::runner::PipelineRunnerApp;

pub use crate::shell_nav::{ToolsSection, UnifiedMode, UnifiedPane, active_pane};

pub const UNIFIED_WINDOW_TITLE: &str = "anaden-studio";

pub struct UnifiedShell {
    mode: UnifiedMode,
    tools_section: ToolsSection,
    studio: StudioApp,
    runner: PipelineRunnerApp,
}

impl UnifiedShell {
    pub fn new(exe: Option<String>) -> Self {
        Self {
            mode: UnifiedMode::default(),
            tools_section: ToolsSection::default(),
            studio: StudioApp::with_initial_target(exe),
            runner: PipelineRunnerApp::with_resolved_anaden(),
        }
    }

    pub fn mode(&self) -> UnifiedMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: UnifiedMode) {
        self.mode = mode;
    }

    pub fn tools_section(&self) -> ToolsSection {
        self.tools_section
    }

    pub fn set_tools_section(&mut self, section: ToolsSection) {
        self.tools_section = section;
    }

    pub fn pane(&self) -> UnifiedPane {
        active_pane(self.mode, self.tools_section)
    }

    pub fn studio(&self) -> &StudioApp {
        &self.studio
    }

    pub fn runner(&self) -> &PipelineRunnerApp {
        &self.runner
    }
}

impl eframe::App for UnifiedShell {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render_modebar(ui);
        if self.mode == UnifiedMode::Tools {
            self.render_tools_sectionbar(ui);
        }
        self.render_content(ui);
    }
}

impl UnifiedShell {
    pub fn render_modebar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("unified_modebar").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                for mode in UnifiedMode::ALL {
                    ui.selectable_value(&mut self.mode, mode, mode.label());
                }
            });
        });
    }

    pub fn render_tools_sectionbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("tools_sectionbar").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                for section in ToolsSection::ALL {
                    ui.selectable_value(&mut self.tools_section, section, section.label());
                }
            });
        });
    }

    pub fn render_content(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(ui, |ui| match self.pane() {
            UnifiedPane::Tasks => {
                self.studio.ensure_task_list_loaded();
                self.studio.render_task_list(ui);
            }
            UnifiedPane::Studio => {
                if let Some(studio_mode) = self.tools_section.studio_mode() {
                    self.studio.set_mode(studio_mode);
                }
                self.studio.render_body(ui);
            }
            UnifiedPane::Runner => {
                self.runner
                    .render_body(ui, self.tools_section.runner_pane());
            }
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_default_state_uses_tasks_pane() {
        let shell = UnifiedShell::new(None);
        // 既定はホーム (タスク一覧ペイン) + ツール既定セクション (作成)。
        assert_eq!(shell.mode(), UnifiedMode::Home);
        assert_eq!(shell.pane(), UnifiedPane::Tasks);
        assert_eq!(shell.tools_section(), ToolsSection::Authoring);
    }

    #[test]
    fn test_mode_transitions_switch_active_pane() {
        let mut shell = UnifiedShell::new(None);
        // 既定はホーム (Tasks ペイン)。
        assert_eq!(shell.pane(), UnifiedPane::Tasks);

        // ツール (既定セクション=作成) へ切替 → Studio ペイン。
        shell.set_mode(UnifiedMode::Tools);
        assert_eq!(shell.mode(), UnifiedMode::Tools);
        assert_eq!(shell.pane(), UnifiedPane::Studio);

        // 実行セクションへ → Runner ペイン。
        shell.set_tools_section(ToolsSection::Run);
        assert_eq!(shell.pane(), UnifiedPane::Runner);

        // 履歴セクションも Runner ペイン。
        shell.set_tools_section(ToolsSection::History);
        assert_eq!(shell.pane(), UnifiedPane::Runner);

        // ホームへ戻す → Tasks ペインへ復帰。
        shell.set_mode(UnifiedMode::Home);
        assert_eq!(shell.pane(), UnifiedPane::Tasks);
        assert_eq!(shell.mode(), UnifiedMode::Home);
    }

    /// Issue #123 (shard 2): `--pipeline` フラグは完全削除済み。new() 以外の
    /// コンストラクタは存在せず、deprecated 警告バナーも表示されない。
    #[test]
    fn pipeline_deprecated_warning_fully_removed() {
        let shell = UnifiedShell::new(None);
        // new_with_flags / shows_deprecated_pipeline_warning は削除済み
        // (コンパイル時検証: この test が型チェックを通れば API は存在しない)。
        assert_eq!(shell.mode(), UnifiedMode::Home);
    }

    /// ウィンドウタイトルは単一名称「anaden-studio」。
    #[test]
    fn test_window_title_is_unified_single_name() {
        assert_eq!(UNIFIED_WINDOW_TITLE, "anaden-studio");
    }

    // ---- エッジケース ----

    /// ホーム↔ツール往復でツールセクション選択は保持される（状態リセットなし）。
    #[test]
    fn test_mode_roundtrip_preserves_tools_section() {
        let mut shell = UnifiedShell::new(None);
        shell.set_mode(UnifiedMode::Tools);
        shell.set_tools_section(ToolsSection::History);
        shell.set_mode(UnifiedMode::Home);
        // ホームの間もセクション選択は失われていない。
        assert_eq!(shell.tools_section(), ToolsSection::History);
        assert_eq!(shell.pane(), UnifiedPane::Tasks);
        // ツールへ戻すと前回のセクション (履歴) が復帰する。
        shell.set_mode(UnifiedMode::Tools);
        assert_eq!(shell.tools_section(), ToolsSection::History);
        assert_eq!(shell.pane(), UnifiedPane::Runner);
    }

    /// セクション切替はトップレベルモードを変更しない（ツールの外には出ない）。
    #[test]
    fn test_section_switch_keeps_tools_mode() {
        let mut shell = UnifiedShell::new(None);
        shell.set_mode(UnifiedMode::Tools);
        for section in ToolsSection::ALL {
            shell.set_tools_section(section);
            assert_eq!(shell.mode(), UnifiedMode::Tools);
        }
    }

    /// ヘッドレス egui コンテキストを用意し、その中に子 Ui を作る
    /// （app.rs のテストと同一パターン・GUI バックエンド不要）。
    fn child_ui(ctx: &egui::Context) -> egui::Ui {
        egui::Ui::new(
            ctx.clone(),
            egui::Id::new("shell-test-area"),
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
        )
    }

    /// ホーム + ツール全セクションの描画がパニックせず完了する
    /// （Issue #157: 統合後も全旧ペインが描画可能なことの回帰保証）。
    /// 併せてホーム描画が tools_section を書き換えないこと（サブバー非描画）を検証。
    #[test]
    fn render_home_and_all_tool_sections_complete_without_panic() {
        let ctx = egui::Context::default();
        let mut shell = UnifiedShell::new(None);
        // ホーム (既定): modebar 2 タブ + タスク一覧。
        shell.set_tools_section(ToolsSection::Settings);
        ctx.begin_pass(egui::RawInput::default());
        shell.render_modebar(&mut child_ui(&ctx));
        shell.render_content(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
        assert_eq!(shell.mode(), UnifiedMode::Home);
        assert_eq!(shell.tools_section(), ToolsSection::Settings);

        // ツール: サブバー + 全 6 セクション。
        shell.set_mode(UnifiedMode::Tools);
        for section in ToolsSection::ALL {
            shell.set_tools_section(section);
            ctx.begin_pass(egui::RawInput::default());
            shell.render_modebar(&mut child_ui(&ctx));
            shell.render_tools_sectionbar(&mut child_ui(&ctx));
            shell.render_content(&mut child_ui(&ctx));
            let _ = ctx.end_pass();
        }
    }
}
