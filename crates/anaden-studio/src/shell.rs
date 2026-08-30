//! 単一ウィンドウ統合 GUI シェル（Issue #119 shard 1）。
//!
//! 作成GUI (`StudioApp`: Authoring/Batch) と実行GUI (`PipelineRunnerApp`:
//! Run/History) をフラグ分離ではなく 1 つのウィンドウの modebar で切替える
//! 統合シェル。runner.rs が既に 500 行ルール上限を超過しているため、
//! `PipelineRunnerApp` 拡張ではなく新規モジュールとして切り出す
//! （estimate 承認条件: 統合シェルは新規モジュールに切り出す方を採用）。
//!
//! 描画構造:
//! - modebar（上部固定）: 作成 / バッチ評価 / 実行 / 履歴 の 4 タブ
//! - Authoring / Batch → `StudioApp` のパネル描画に委譲
//! - Run / History     → `PipelineRunnerApp` のパネル描画を表示
//!
//! モード遷移・描画分岐の決定は egui 非依存の純関数として切り出し、
//! ヘッドレスでユニットテスト可能にしている。

use eframe::egui;

use crate::app::{AppMode, StudioApp};
use crate::runner::PipelineRunnerApp;
use crate::source::Target;

/// 統合GUI のウィンドウタイトル（Issue #119: 単一名称へ統一）。
pub const UNIFIED_WINDOW_TITLE: &str = "anaden-studio";

/// 統合 modebar のモード（Issue #119 UC-1: フラグなし単一起動で全タブ利用可）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnifiedMode {
    /// テンプレート作成（StudioApp に委譲）。
    Authoring,
    /// バッチ評価（StudioApp に委譲）。
    Batch,
    /// pipeline 実行（既存 runner UI）。
    Run,
    /// 実行履歴（既存 runner UI）。
    History,
}

impl Default for UnifiedMode {
    /// 既定モードは作成 (Authoring)。UC-2 の「作成→実行→履歴」連続フローの
    /// 入口であり、実行 (Run) であってはならない。
    fn default() -> Self {
        Self::Authoring
    }
}

impl UnifiedMode {
    /// modebar 表示ラベル。
    pub fn label(self) -> &'static str {
        match self {
            Self::Authoring => "✏️ 作成",
            Self::Batch => "📊 バッチ評価",
            Self::Run => "▶️ 実行",
            Self::History => "🕘 履歴",
        }
    }

    /// 全モードを modebar 表示順に返す。
    pub const ALL: [UnifiedMode; 4] = [
        UnifiedMode::Authoring,
        UnifiedMode::Batch,
        UnifiedMode::Run,
        UnifiedMode::History,
    ];

    /// 対応する StudioApp 側モード（Authoring/Batch 以外は None）。
    pub fn studio_mode(self) -> Option<AppMode> {
        match self {
            Self::Authoring => Some(AppMode::Authoring),
            Self::Batch => Some(AppMode::Batch),
            Self::Run | Self::History => None,
        }
    }
}

/// モードに対応する描画ペイン（描画分岐の純関数表現）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnifiedPane {
    /// StudioApp のパネル（作成/バッチ評価）。
    Studio,
    /// PipelineRunnerApp のパネル（実行/履歴）。
    Runner,
}

/// モード → 描画ペインの純関数（単体テスト対象）。
pub fn active_pane(mode: UnifiedMode) -> UnifiedPane {
    match mode {
        UnifiedMode::Authoring | UnifiedMode::Batch => UnifiedPane::Studio,
        UnifiedMode::Run | UnifiedMode::History => UnifiedPane::Runner,
    }
}

/// 単一ウィンドウ統合 GUI シェル。
pub struct UnifiedShell {
    /// 現在選択中の統合モード。
    mode: UnifiedMode,
    /// 作成/バッチ評価ペインの実体。
    studio: StudioApp,
    /// 実行/履歴ペインの実体。
    runner: PipelineRunnerApp,
    /// `--pipeline` 経由起動時に deprecated 警告を表示する
    /// （Issue #119 UC-3: 後方互換フラグ。同一アプリを起動する）。
    deprecated_pipeline: bool,
}

impl UnifiedShell {
    /// CLI 指定の target/exe を初期値として統合シェルを構築する。
    pub fn new(target: Target, exe: Option<String>) -> Self {
        Self::new_with_flags(target, exe, false)
    }

    /// `--pipeline` フラグの有無も指定して統合シェルを構築する。
    ///
    /// `deprecated_pipeline == true`（`--pipeline` 経由起動）の場合、
    /// Run ペイン上部に deprecated 警告バナーを表示する。
    pub fn new_with_flags(target: Target, exe: Option<String>, deprecated_pipeline: bool) -> Self {
        Self {
            mode: UnifiedMode::default(),
            studio: StudioApp::with_initial_target(target, exe),
            runner: PipelineRunnerApp::with_resolved_anaden(),
            deprecated_pipeline,
        }
    }

    /// 現在の統合モード。
    pub fn mode(&self) -> UnifiedMode {
        self.mode
    }

    /// 統合モードを設定する（modebar 選択と同一の遷移）。
    pub fn set_mode(&mut self, mode: UnifiedMode) {
        self.mode = mode;
    }

    /// 現在の描画ペイン（`active_pane(self.mode)` のショートカット）。
    pub fn pane(&self) -> UnifiedPane {
        active_pane(self.mode)
    }

    /// 作成/バッチ評価ペイン（テスト・埋め込み用）。
    pub fn studio(&self) -> &StudioApp {
        &self.studio
    }

    /// 実行/履歴ペイン（テスト・埋め込み用）。
    pub fn runner(&self) -> &PipelineRunnerApp {
        &self.runner
    }

    /// deprecated 警告バナーの表示要否（`--pipeline` 経由起動時のみ true）。
    pub fn shows_deprecated_pipeline_warning(&self) -> bool {
        self.deprecated_pipeline
    }
}

impl eframe::App for UnifiedShell {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render_modebar(ui);
        egui::CentralPanel::default().show_inside(ui, |ui| match self.pane() {
            UnifiedPane::Studio => {
                // StudioApp 側の対応モードへ同期してからパネル描画に委譲。
                if let Some(studio_mode) = self.mode.studio_mode() {
                    self.studio.set_mode(studio_mode);
                }
                self.studio.render_body(ui);
            }
            UnifiedPane::Runner => {
                // `--pipeline` 経由起動時の deprecated 警告バナー (UC-3)。
                if self.deprecated_pipeline {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 180, 50),
                        "[deprecated] --pipeline は後方互換フラグです。フラグなし起動で同一の統合GUIが開きます (Issue #119)",
                    );
                    ui.separator();
                }
                // Run / History は既存 runner UI（実行制御 + 履歴パネル）。
                self.runner.render_body(ui);
            }
        });
    }
}

impl UnifiedShell {
    /// 統合 modebar（ウィンドウ上部のタブバー）。
    fn render_modebar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("unified_modebar").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                for mode in UnifiedMode::ALL {
                    ui.selectable_value(&mut self.mode, mode, mode.label());
                }
            });
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
    fn test_default_mode_is_authoring_not_run() {
        assert_eq!(UnifiedMode::default(), UnifiedMode::Authoring);
        // 既定モードが Run でないことの検証（タスク要件）。
        assert_ne!(UnifiedMode::default(), UnifiedMode::Run);
    }

    #[test]
    fn test_active_pane_authoring_and_batch_delegate_to_studio() {
        assert_eq!(active_pane(UnifiedMode::Authoring), UnifiedPane::Studio);
        assert_eq!(active_pane(UnifiedMode::Batch), UnifiedPane::Studio);
    }

    #[test]
    fn test_active_pane_run_and_history_use_runner() {
        assert_eq!(active_pane(UnifiedMode::Run), UnifiedPane::Runner);
        assert_eq!(active_pane(UnifiedMode::History), UnifiedPane::Runner);
    }

    #[test]
    fn test_mode_transitions_switch_active_pane() {
        let mut shell = UnifiedShell::new(Target::default(), None);
        // 既定は Studio ペイン。
        assert_eq!(shell.pane(), UnifiedPane::Studio);

        // Run へ切替 → Runner ペインへ分岐が切り替わる。
        shell.set_mode(UnifiedMode::Run);
        assert_eq!(shell.mode(), UnifiedMode::Run);
        assert_eq!(shell.pane(), UnifiedPane::Runner);

        // History も Runner ペイン。
        shell.set_mode(UnifiedMode::History);
        assert_eq!(shell.pane(), UnifiedPane::Runner);

        // Batch へ戻す → Studio ペインへ復帰。
        shell.set_mode(UnifiedMode::Batch);
        assert_eq!(shell.pane(), UnifiedPane::Studio);
        assert_eq!(shell.mode(), UnifiedMode::Batch);
    }

    #[test]
    fn test_studio_mode_mapping() {
        assert_eq!(
            UnifiedMode::Authoring.studio_mode(),
            Some(AppMode::Authoring)
        );
        assert_eq!(UnifiedMode::Batch.studio_mode(), Some(AppMode::Batch));
        assert_eq!(UnifiedMode::Run.studio_mode(), None);
        assert_eq!(UnifiedMode::History.studio_mode(), None);
    }

    #[test]
    fn test_all_modes_cover_exactly_four_tabs() {
        assert_eq!(UnifiedMode::ALL.len(), 4);
        // modebar 表示順: 作成 → バッチ評価 → 実行 → 履歴。
        assert_eq!(
            UnifiedMode::ALL,
            [
                UnifiedMode::Authoring,
                UnifiedMode::Batch,
                UnifiedMode::Run,
                UnifiedMode::History,
            ]
        );
    }

    #[test]
    fn test_labels_are_distinct_and_nonempty() {
        let labels: Vec<&str> = UnifiedMode::ALL.iter().map(|m| m.label()).collect();
        for label in &labels {
            assert!(!label.is_empty());
        }
        for (i, a) in labels.iter().enumerate() {
            for b in labels.iter().skip(i + 1) {
                assert_ne!(a, b, "modebar labels must be distinct");
            }
        }
    }

    #[test]
    fn test_shell_default_state_uses_authoring_pane() {
        let shell = UnifiedShell::new(Target::default(), None);
        assert_eq!(shell.mode(), UnifiedMode::Authoring);
        assert_eq!(shell.pane(), UnifiedPane::Studio);
        // 委譲先 StudioApp も Authoring で開始する。
        assert_eq!(shell.studio().mode(), AppMode::Authoring);
    }

    /// UC-3: `--pipeline` 経由起動時のみ deprecated 警告を表示する。
    #[test]
    fn test_deprecated_pipeline_warning_only_when_flag_given() {
        let plain = UnifiedShell::new_with_flags(Target::default(), None, false);
        assert!(!plain.shows_deprecated_pipeline_warning());
        let via_flag = UnifiedShell::new_with_flags(Target::default(), None, true);
        assert!(via_flag.shows_deprecated_pipeline_warning());
    }

    /// ウィンドウタイトルは単一名称「anaden-studio」。
    #[test]
    fn test_window_title_is_unified_single_name() {
        assert_eq!(UNIFIED_WINDOW_TITLE, "anaden-studio");
    }
}
