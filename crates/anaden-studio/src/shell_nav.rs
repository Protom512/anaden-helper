//! 統合シェルのナビゲーション型群 (Issue #162 Shard 3)。
//!
//! [`UnifiedMode`] (ホーム/ツールの 2 タブ)・[`ToolsSection`] (ツール内
//! サブセクション)・[`UnifiedPane`] (描画ペイン) と遷移純関数 [`active_pane`]
//! を shell.rs から分離したモジュール。egui 非依存の純関数群であり、
//! ヘッドレスでユニットテスト可能。
//!
//! 呼び出し元 (main.rs / tests/) は従来どおり `crate::shell::{...}`
//! (`anaden_studio::shell::{...}`) で参照できる（shell.rs が facade re-export）。

use crate::app::AppMode;
use crate::runner::RunnerPane;

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
}
