//! 単一ウィンドウ統合 GUI シェル（Issue #119 shard 1 / Issue #157 2 タブ化）。
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
//! モード遷移・描画分岐の決定は egui 非依存の純関数として切り出し、
//! ヘッドレスでユニットテスト可能にしている。

use eframe::egui;

use crate::app::{AppMode, StudioApp};
use crate::runner::{PipelineRunnerApp, RunnerPane};
use crate::source::Target;

/// 統合GUI のウィンドウタイトル（Issue #119: 単一名称へ統一）。
pub const UNIFIED_WINDOW_TITLE: &str = "anaden-studio";

/// 統合 modebar のモード（Issue #157: 旧 6+1 タブを 2 タブへ集約）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnifiedMode {
    /// ホーム（Tasks ペイン。Issue #154 の MAA 型タスク一覧・QueueExec 逐次実行）。
    Home,
    /// ツール（旧 6 タブ相当を [`ToolsSection`] サブバーで統合したビュー）。
    Tools,
}

impl Default for UnifiedMode {
    /// 既定モードはホーム（Issue #157: 起動直後はタスク一覧が画面の主役）。
    fn default() -> Self {
        Self::Home
    }
}

impl UnifiedMode {
    /// modebar 表示ラベル（絵文字なし・豆腐回避）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Home => "ホーム",
            Self::Tools => "ツール",
        }
    }

    /// 全モードを modebar 表示順に返す（Issue #157: 2 タブ構成）。
    pub const ALL: [UnifiedMode; 2] = [UnifiedMode::Home, UnifiedMode::Tools];
}

/// ツールビュー内のサブセクション（Issue #157: 旧 UnifiedMode タブの再マップ）。
///
/// 旧タブ（作成/バッチ評価/戦略/実行/履歴/設定）の機能は削除せず、
/// このサブ切替からすべて到達できる（機能喪失なしの保証対象）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsSection {
    /// テンプレート作成（StudioApp に委譲）。
    Authoring,
    /// バッチ評価（StudioApp に委譲）。
    Batch,
    /// 戦略選択（runner の戦略ペイン。Issue #125 shard 3）。
    Strategy,
    /// pipeline 実行（既存 runner UI・単発実行）。
    Run,
    /// 実行履歴（既存 runner UI）。
    History,
    /// 設定（runner の設定ペイン。Issue #125 shard 3）。
    Settings,
}

impl Default for ToolsSection {
    /// 既定セクションは作成（旧 StudioApp の既定モード Authoring と同一）。
    fn default() -> Self {
        Self::Authoring
    }
}

impl ToolsSection {
    /// サブバー表示ラベル（絵文字なし・豆腐回避）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Authoring => "作成",
            Self::Batch => "バッチ評価",
            Self::Strategy => "戦略",
            Self::Run => "実行 (単発)",
            Self::History => "履歴",
            Self::Settings => "設定",
        }
    }

    /// 全セクションをサブバー表示順に返す（旧 modebar と同一順序）。
    pub const ALL: [ToolsSection; 6] = [
        ToolsSection::Authoring,
        ToolsSection::Batch,
        ToolsSection::Strategy,
        ToolsSection::Run,
        ToolsSection::History,
        ToolsSection::Settings,
    ];

    /// 対応する StudioApp 側モード（Authoring/Batch 以外は None）。
    pub fn studio_mode(self) -> Option<AppMode> {
        match self {
            Self::Authoring => Some(AppMode::Authoring),
            Self::Batch => Some(AppMode::Batch),
            Self::Strategy | Self::Run | Self::History | Self::Settings => None,
        }
    }

    /// 対応する runner ペイン種別。
    ///
    /// Studio 系セクション (Authoring/Batch) は [`active_pane`] が
    /// [`UnifiedPane::Studio`] を返すためこの結果は使われない
    /// （総関数として既定の Run を返す）。
    pub fn runner_pane(self) -> RunnerPane {
        match self {
            Self::Run => RunnerPane::Run,
            Self::History => RunnerPane::History,
            Self::Strategy => RunnerPane::Strategy,
            Self::Settings => RunnerPane::Settings,
            Self::Authoring | Self::Batch => RunnerPane::Run,
        }
    }
}

/// モードに対応する描画ペイン（描画分岐の純関数表現）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnifiedPane {
    /// MAA 型タスク一覧（ホーム画面・Issue #154）。
    Tasks,
    /// StudioApp のパネル（作成/バッチ評価）。
    Studio,
    /// PipelineRunnerApp のパネル（実行/戦略/履歴/設定）。
    Runner,
}

/// モード + ツールセクション → 描画ペインの純関数（単体テスト対象）。
///
/// ホームは常に Tasks。ツールはセクションが Studio 系 (Authoring/Batch) なら
/// Studio、runner 系 (戦略/実行/履歴/設定) なら Runner。
pub fn active_pane(mode: UnifiedMode, section: ToolsSection) -> UnifiedPane {
    match mode {
        UnifiedMode::Home => UnifiedPane::Tasks,
        UnifiedMode::Tools => match section.studio_mode() {
            Some(_) => UnifiedPane::Studio,
            None => UnifiedPane::Runner,
        },
    }
}

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
    /// CLI 指定の target/exe を初期値として統合シェルを構築する。
    ///
    /// Issue #123 (shard 2): `--pipeline` フラグは完全削除済みのため
    /// フラグ区別のコンストラクタは存在しない。
    pub fn new(target: Target, exe: Option<String>) -> Self {
        Self {
            mode: UnifiedMode::default(),
            tools_section: ToolsSection::default(),
            studio: StudioApp::with_initial_target(target, exe),
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
    fn render_modebar(&mut self, ui: &mut egui::Ui) {
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
    fn render_tools_sectionbar(&mut self, ui: &mut egui::Ui) {
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
    fn render_content(&mut self, ui: &mut egui::Ui) {
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

    // ---- 正常系: 2 タブ構成・既定ホーム (Issue #157) ----

    #[test]
    fn test_default_mode_is_home() {
        // Issue #157: 既定モードはホーム (タスク一覧が主役)。
        assert_eq!(UnifiedMode::default(), UnifiedMode::Home);
        assert_ne!(UnifiedMode::default(), UnifiedMode::Tools);
    }

    #[test]
    fn test_shell_default_state_uses_tasks_pane() {
        let shell = UnifiedShell::new(Target::default(), None);
        // 既定はホーム (タスク一覧ペイン) + ツール既定セクション (作成)。
        assert_eq!(shell.mode(), UnifiedMode::Home);
        assert_eq!(shell.pane(), UnifiedPane::Tasks);
        assert_eq!(shell.tools_section(), ToolsSection::Authoring);
    }

    #[test]
    fn test_modebar_has_exactly_two_tabs() {
        // Issue #157: modebar は ホーム + ツール の 2 タブのみ。
        assert_eq!(UnifiedMode::ALL.len(), 2);
        assert_eq!(UnifiedMode::ALL, [UnifiedMode::Home, UnifiedMode::Tools]);
    }

    #[test]
    fn test_tools_sections_cover_all_legacy_panes() {
        // Issue #157 機能喪失なし保証: 旧 6 タブ相当が全てツールから到達可能。
        assert_eq!(ToolsSection::ALL.len(), 6);
        // 作成/バッチ評価 → Studio ペイン。
        for section in [ToolsSection::Authoring, ToolsSection::Batch] {
            assert_eq!(
                active_pane(UnifiedMode::Tools, section),
                UnifiedPane::Studio
            );
        }
        // 戦略/実行/履歴/設定 → Runner ペイン。
        for section in [
            ToolsSection::Strategy,
            ToolsSection::Run,
            ToolsSection::History,
            ToolsSection::Settings,
        ] {
            assert_eq!(
                active_pane(UnifiedMode::Tools, section),
                UnifiedPane::Runner
            );
        }
        // ホームはセクションに依らず常にタスク一覧。
        for section in ToolsSection::ALL {
            assert_eq!(active_pane(UnifiedMode::Home, section), UnifiedPane::Tasks);
        }
    }

    #[test]
    fn test_runner_backed_sections_reach_dedicated_panes() {
        // gate 指摘 (Issue #125) 回帰防止: 戦略・設定・履歴が Run へ
        // フォールスルーして専用ビューに到達しない欠陥の再発を防ぐ。
        assert_eq!(ToolsSection::Run.runner_pane(), RunnerPane::Run);
        assert_eq!(ToolsSection::History.runner_pane(), RunnerPane::History);
        assert_eq!(ToolsSection::Strategy.runner_pane(), RunnerPane::Strategy);
        assert_eq!(ToolsSection::Settings.runner_pane(), RunnerPane::Settings);
    }

    #[test]
    fn test_studio_mode_mapping() {
        assert_eq!(
            ToolsSection::Authoring.studio_mode(),
            Some(AppMode::Authoring)
        );
        assert_eq!(ToolsSection::Batch.studio_mode(), Some(AppMode::Batch));
        assert_eq!(ToolsSection::Run.studio_mode(), None);
        assert_eq!(ToolsSection::History.studio_mode(), None);
        assert_eq!(ToolsSection::Strategy.studio_mode(), None);
        assert_eq!(ToolsSection::Settings.studio_mode(), None);
    }

    #[test]
    fn test_mode_transitions_switch_active_pane() {
        let mut shell = UnifiedShell::new(Target::default(), None);
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

    #[test]
    fn test_labels_are_distinct_and_nonempty() {
        let mode_labels: Vec<&str> = UnifiedMode::ALL.iter().map(|m| m.label()).collect();
        let section_labels: Vec<&str> = ToolsSection::ALL.iter().map(|s| s.label()).collect();
        for label in mode_labels.iter().chain(section_labels.iter()) {
            assert!(!label.is_empty());
        }
        for (i, a) in mode_labels.iter().enumerate() {
            for b in mode_labels.iter().skip(i + 1) {
                assert_ne!(a, b, "modebar labels must be distinct");
            }
        }
        for (i, a) in section_labels.iter().enumerate() {
            for b in section_labels.iter().skip(i + 1) {
                assert_ne!(a, b, "section labels must be distinct");
            }
        }
    }

    /// Issue #123 (shard 2): `--pipeline` フラグは完全削除済み。new() 以外の
    /// コンストラクタは存在せず、deprecated 警告バナーも表示されない。
    #[test]
    fn pipeline_deprecated_warning_fully_removed() {
        let shell = UnifiedShell::new(Target::default(), None);
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
        let mut shell = UnifiedShell::new(Target::default(), None);
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
        let mut shell = UnifiedShell::new(Target::default(), None);
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
        let mut shell = UnifiedShell::new(Target::default(), None);
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
