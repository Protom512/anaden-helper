//! 単一ウィンドウ統合 GUI シェル（Issue #119 shard 1 / Issue #157 2 タブ化 /
//! Issue #162 Shard 3: ナビゲーション型群を shell_nav.rs へ分離）。
//!
//! 作成GUI (`StudioApp`: Authoring/Batch) と実行GUI (`PipelineRunnerApp`:
//! Run/History 等) を 1 つのウィンドウの modebar で切替える統合シェル。
//! runner.rs が既に 500 行ルール上限を超過しているため、`PipelineRunnerApp`
//! 拡張ではなく新規モジュールとして切り出す（estimate 承認条件）。
//!
//! Issue #157: 旧 7 タブ modebar (タスク一覧/作成/バッチ評価/戦略/実行/履歴/設定)
//! を「ホーム + ツール」の 2 タブへ集約し、起動直後の「タブだらけ」を解消した。
//! - ホーム: Issue #154 の MAA 型タスク一覧 (`render_task_list`) が主役
//! - ツール: 旧 6 タブ相当を [`ToolsSection`] サブバー（排他切替）で統合。
//!   機能は削除せず全旧ペインに到達可能。
//!
//! モード遷移・描画分岐の決定は egui 非依存の純関数として
//! [`crate::shell_nav`] に切り出し、ヘッドレスでユニットテスト可能にしている。
//! 本モジュールはシェル状態 (`UnifiedShell`) と描画 (modebar/サブバー/本体) のみを
//! 担う。呼び出し元互換のためナビゲーション型群を re-export する。

use eframe::egui;

use crate::app::StudioApp;
use crate::runner::PipelineRunnerApp;

pub use crate::shell_nav::{ToolsSection, UnifiedMode, UnifiedPane, active_pane};

/// 統合GUI のウィンドウタイトル（Issue #119: 単一名称へ統一）。
pub const UNIFIED_WINDOW_TITLE: &str = "anaden-studio";

/// 単一ウィンドウ統合 GUI シェル。
pub struct UnifiedShell {
    /// 現在選択中の統合モード（ホーム/ツール）。
    mode: UnifiedMode,
    /// ツールビューの選択中サブセクション（モード切替を跨いで保持される）。
    tools_section: ToolsSection,
    /// 作成/バッチ評価ペインの実体。
    studio: StudioApp,
    /// 実行/戦略/履歴/設定ペインの実体。
    runner: PipelineRunnerApp,
}

impl UnifiedShell {
    /// CLI 指定の exe を初期値として統合シェルを構築する。
    ///
    /// Issue #123 (shard 2): `--pipeline` フラグは完全削除済みのため
    /// フラグ区別のコンストラクタは存在しない。
    /// Issue #188: target 引数は Windows 固定化に伴い廃止。
    pub fn new(exe: Option<String>) -> Self {
        Self {
            mode: UnifiedMode::default(),
            tools_section: ToolsSection::default(),
            studio: StudioApp::with_initial_target(exe),
            runner: PipelineRunnerApp::with_resolved_anaden(),
        }
    }

    /// 現在の統合モード。
    pub fn mode(&self) -> UnifiedMode {
        self.mode
    }

    /// 統合モードを設定する（modebar 選択と同一の遷移）。
    /// tools_section はリセットされない（ホーム↔ツール往復で復帰可能）。
    pub fn set_mode(&mut self, mode: UnifiedMode) {
        self.mode = mode;
    }

    /// ツールビューの選択中サブセクション。
    pub fn tools_section(&self) -> ToolsSection {
        self.tools_section
    }

    /// ツールセクションを設定する（サブバー選択と同一の遷移）。
    pub fn set_tools_section(&mut self, section: ToolsSection) {
        self.tools_section = section;
    }

    /// 現在の描画ペイン（`active_pane(self.mode, self.tools_section)` のショートカット）。
    pub fn pane(&self) -> UnifiedPane {
        active_pane(self.mode, self.tools_section)
    }

    /// 作成/バッチ評価ペイン（テスト・埋め込み用）。
    pub fn studio(&self) -> &StudioApp {
        &self.studio
    }

    /// 実行/履歴ペイン（テスト・埋め込み用）。
    pub fn runner(&self) -> &PipelineRunnerApp {
        &self.runner
    }
}

impl eframe::App for UnifiedShell {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render_modebar(ui);
        // サブバーはツールモードの時のみ表示（ホームは modebar 2 タブのみ）。
        if self.mode == UnifiedMode::Tools {
            self.render_tools_sectionbar(ui);
        }
        self.render_content(ui);
    }
}

impl UnifiedShell {
    /// 統合 modebar（ウィンドウ上部のタブバー・Issue #157: 2 タブ構成）。
    ///
    /// 公開 API (app.rs の render_modebar/render_body と同一パターン)。
    /// `eframe::App::ui` は `&mut eframe::Frame` を要求するためヘッドレス
    /// テストからは呼べず、E2E 証跡テスト (`tests/e2e_evidence_tests.rs`) は
    /// modebar/サブバー/content を分割呼出して実シェル構成を描画する。
    pub fn render_modebar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("unified_modebar").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                for mode in UnifiedMode::ALL {
                    ui.selectable_value(&mut self.mode, mode, mode.label());
                }
            });
        });
    }

    /// ツールビューのサブバー（旧 6 タブ相当セクションの排他切替）。
    ///
    /// 排他切替方式の理由: 旧ペインの内部は `Panel::left` + `CentralPanel` を
    /// 使うため、同一親 Ui 内で複数セクションを同時展開すると panel id 衝突が
    /// 起きる。排他切替なら旧ペインの描画コードを無変更で再利用できる
    /// （Issue #157: 統合・再配置のみでロジック変更なし）。
    pub fn render_tools_sectionbar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("tools_sectionbar").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                for section in ToolsSection::ALL {
                    ui.selectable_value(&mut self.tools_section, section, section.label());
                }
            });
        });
    }

    /// モード本体（ホーム = タスク一覧 / ツール = 旧ペイン）を描画する。
    ///
    /// `eframe::App::ui` から切り出した内部 API。ヘッドレス描画テストから
    /// modebar/サブバーと分割して呼べるようにしている。
    pub fn render_content(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(ui, |ui| match self.pane() {
            UnifiedPane::Tasks => {
                // ホーム (Issue #154): タスク一覧 + 開始 + 状態。
                // 起動時に定義を自動読込 (未読込なら UI 内ボタンで読込)。
                self.studio.ensure_task_list_loaded();
                self.studio.render_task_list(ui);
            }
            UnifiedPane::Studio => {
                // StudioApp 側の対応モードへ同期してからパネル描画に委譲。
                if let Some(studio_mode) = self.tools_section.studio_mode() {
                    self.studio.set_mode(studio_mode);
                }
                self.studio.render_body(ui);
            }
            UnifiedPane::Runner => {
                // 実行/戦略/履歴/設定は既存 runner UI（Issue #120 欠陥2修正:
                // ペイン種別で各ビューを区別・Issue #125 で全 4 種が孤立）。
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
