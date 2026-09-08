//! StudioApp 本体・モード・エンジン種別・定数 (Issue #175: app_state.rs 分割)。
//!
//! GUI 全体状態 [`StudioApp`] (構造体 + 構築) とモード ([`AppMode`])・
//! マッチエンジン種別 (EngineKind・pub(crate)) およびテンプレート保存・
//! ヒートマップ計算の定数を定義する。接続状態は [`crate::app_state_connection`]、
//! pipeline task 構築・保存は [`crate::app_state_pipeline_task`]。
//! UI 描画 impl は app_ui 系、状態操作 impl (キュー実行・ファイル入出力) は
//! app.rs に置く。呼び出し元互換の re-export は [`crate::app_state`] (facade)。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender};

use eframe::egui;
use image::DynamicImage;

use anaden_core::{MatchConfidence, ScreenRegion};
use anaden_vision::{ScreenScaler, SseVisionEngine, TemplateMatcher, VisionEngine};

use crate::app_state_connection::ConnectionStatus;
use crate::app_state_pipeline_task::PipelineActionKind;
use crate::batch::ConfusionMatrix;
use crate::canvas::RoiEdit;
use crate::childproc::ChildProcess;
use crate::log_view::{AutoScrollFollow, DEFAULT_MAX_LINES, LogEntry, LogEvent, SharedLogBuffer};
use crate::proposals::Proposal;
use crate::scoring::Discrimination;
use crate::source::LiveCapture;
use crate::tasks::QueueExec;

/// ヒートマップ計算用のダウンスケール倍率。
/// imageproc の match_template は O(W·H·w·h) の総当たりのため、フル解像度では重い。
/// 4倍縮小で速度と位置精度を両立する（位置精度 ±4px）。
pub(crate) const HEATMAP_DOWNSCALE: u32 = 4;

/// Tasks ペインのログチャネル容量 (reader スレッド try_send / UI 毎フレーム drain)。
/// runner.rs の LOG_CHANNEL_CAPACITY と同値 (bounded・best-effort 破棄契約)。
const TASK_LOG_CHANNEL_CAPACITY: usize = 1024;

/// テンプレート保存時の状態選択肢。TemplateStore の parse_state_from_dir_name と整合。
pub(crate) const STATE_OPTIONS: &[&str] = &[
    "title", "field", "loading", "battle", "fishing", "menu", "dialog", "unknown",
];

/// GUI のモード。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    /// テンプレート作成（ROI選択＋識別力評価）。
    Authoring,
    /// バッチ評価（混同行列）。
    Batch,
}

/// 識別力評価に使うマッチエンジン。コンボでライブ切替する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum EngineKind {
    /// imageproc 正規化SSE（絶対輝度差）。現行ベースライン。
    Sse,
    /// TM_CCOEFF_NORMED（輝度シフトにロバスト）。
    #[default]
    Ccoeff,
}

impl EngineKind {
    /// TemplateSpec.method / 実行エンジンの方式文字列へ変換する。
    /// library::TemplateSpec の method 文字列仕様（"sse" / "ccoeff"）と完全一致。
    pub(crate) fn method_str(self) -> &'static str {
        match self {
            EngineKind::Sse => "sse",
            EngineKind::Ccoeff => "ccoeff",
        }
    }
}

/// GUI 全体の状態。
///
/// フィールドは sibling モジュール (app.rs / app_ui.rs) の impl から直接
/// 操作するため `pub(crate)`。クレート外へは漏らさない。
pub struct StudioApp {
    /// 編集中のスクリーンショット。
    pub(crate) screenshot: Option<Arc<DynamicImage>>,
    /// スクリーンショットの表示用テクスチャ。
    pub(crate) screenshot_tex: Option<egui::TextureHandle>,
    /// ドラッグROI編集状態。
    pub(crate) roi: RoiEdit,
    /// 最後にスコア計算したROI（変化検出用）。
    pub(crate) scored_roi: Option<ScreenRegion>,
    /// 正例画像（同じ画面状態）。フォルダ単位で読込。
    pub(crate) positives: Vec<Arc<DynamicImage>>,
    /// 負例画像（別画面状態）。
    pub(crate) negatives: Vec<Arc<DynamicImage>>,
    /// 直近の識別力評価結果。
    pub(crate) discrimination: Option<Discrimination>,
    /// 現在選択中のエンジン種別（コンボで切替）。engine 再構築の基。
    pub(crate) engine_kind: EngineKind,
    /// 認識エンジン（閾値0・1/2ダウンスケールで生スコアを高速に返す）。
    pub(crate) engine: Box<dyn VisionEngine>,
    /// ヒートマップ計算用エンジン（閾値0・1/4ダウンスケールでスコアマップ全体を算出）。
    pub(crate) heatmap_engine: Box<dyn VisionEngine>,
    /// ヒートマップテクスチャ（ROI解放時に更新）。
    pub(crate) heatmap_tex: Option<egui::TextureHandle>,
    /// ヒートマップが対応する探索領域（元画像座標）。
    pub(crate) heatmap_search: ScreenRegion,
    /// テンプレートの最良マッチ位置（元画像座標・ROI解放時に更新）。
    pub(crate) best_match: Option<ScreenRegion>,
    /// 保存時のテンプレート名入力。
    pub(crate) tpl_name: String,
    /// 保存時の状態選択（STATE_OPTIONS のインデックス）。
    pub(crate) tpl_state_idx: usize,
    /// テンプレート保存先ディレクトリ。
    pub(crate) save_dir: PathBuf,
    /// 現在のモード。
    pub(crate) mode: AppMode,
    /// バッチ評価のテストフォルダ（`<dir>/<label>/*.png`）。
    pub(crate) test_dir: PathBuf,
    /// バッチ評価の決定閾値。
    pub(crate) batch_threshold: f32,
    /// バッチ評価結果。
    pub(crate) batch_result: Option<ConfusionMatrix>,
    /// ADB デバイスシリアル（ライブキャプチャ用）。
    pub(crate) adb_serial: String,
    /// ライブキャプチャの取得元バックエンド(android/windows)。
    pub(crate) target: crate::source::Target,
    /// PC版(Windows)バックエンドの対象 exe 名。
    pub(crate) win_exe: String,
    /// ライブキャプチャ（稼働中のみ）。
    pub(crate) live: Option<LiveCapture>,
    /// 720p 基準への解像度正規化スケーラ（TASK-009）。
    pub(crate) scaler: ScreenScaler,
    /// ROI自動提案の候補リスト（ROI候補ボタン押下で生成）。
    pub(crate) proposals: Vec<Proposal>,
    /// ROI候補提案の計算中フラグ（別スレッドで propose 実行中）。
    pub(crate) proposing: bool,
    /// 別スレッドでの propose 計算結果を受信する channel。
    /// 計算未依頼時・受信済み時は空（Option で所有権の有無を表現）。
    pub(crate) proposal_rx: Option<Receiver<Vec<Proposal>>>,
    /// ステータスメッセージ。
    pub(crate) status: String,
    /// 接続状態 (実機/プロセス検出チェック結果)。Issue #139 T3。
    pub(crate) connection: ConnectionStatus,
    /// pipeline task 保存先ディレクトリ (UC-3: 作成タブ → pipeline TOML 保存)。
    pub(crate) task_dir: PathBuf,
    /// pipeline task の認識成功時アクション選択 (UC-3)。
    pub(crate) task_action: PipelineActionKind,
    /// シナリオ作成パネル (Issue #160 T3: UC-1/UC-2 Authoring 埋め込み)。
    /// ドメインは scenario_ui、ここは配線のみ。
    pub(crate) scenario: crate::scenario_ui::ScenarioPanel,
    /// MAA 型タスク一覧の定義リスト (Issue #144)。None = 未読込。
    pub(crate) task_defs: Option<crate::tasks::TaskListState>,
    /// チェック順逐次実行キューの状態機械 (Issue #154 Shard 1)。None = 未開始。
    pub(crate) task_queue: Option<QueueExec>,
    /// Tasks ペイン専有の子プロセス管理 (runner とは独立・Issue #154 Shard 1)。
    pub(crate) task_child: ChildProcess,
    /// Tasks ペイン専有のログバッファ (reader → channel → drain)。
    pub(crate) task_log: SharedLogBuffer,
    /// ログイベント送信口 (stdout/stderr reader 接続・キュー実行で再利用)。
    pub(crate) task_log_tx: SyncSender<LogEvent>,
    /// ログイベント受信口 (毎フレーム drain・Exit 観測がキュー進行の契機)。
    pub(crate) task_log_rx: Receiver<LogEvent>,
    /// UI 描画用ログスナップショット (drain 毎に更新)。
    pub(crate) task_log_snapshot: Vec<LogEntry>,
    /// スナップショット差分更新用の改訂番号キャッシュ (Issue #160 UC-5:
    /// 新着行のないフレームの全行 clone を回避。LogBuffer::revision と比較)。
    pub(crate) task_log_revision: u64,
    /// ログの自動スクロール追従 (log_view.rs の純ロジック再用・UC-4)。
    pub(crate) task_scroll: AutoScrollFollow,
    /// anaden CLI 実行ファイル (spawn 時の program)。
    pub(crate) anaden_program: String,
}

impl Default for StudioApp {
    fn default() -> Self {
        Self::with_initial_target(crate::source::Target::default(), None)
    }
}

/// PC版(Windows)バックエンドの既定 exe 名を返す。
///
/// Windows ビルドでは anaden-device の DEFAULT_PROCESS_NAME("AnotherEden.exe") を使い、
/// Linux ビルドでは同定数が存在しないため同一の固定文字列を使う(Linux では windows
/// バックエンドが選択できないので実行されることはなく、GUI 表示用の初期値のみ)。
fn default_win_exe() -> String {
    #[cfg(windows)]
    {
        crate::source::DEFAULT_PROCESS_NAME.to_string()
    }
    #[cfg(not(windows))]
    {
        "AnotherEden.exe".to_string()
    }
}

impl StudioApp {
    /// CLI 指定の target/exe を初期値として StudioApp を構築する。
    /// target 未指定時(default) は android。exe 未指定時は既定 exe 名。
    pub fn with_initial_target(target: crate::source::Target, exe: Option<String>) -> Self {
        // engine は engine_kind（デフォルト CCOEFF）から構築。閾値0・ダウンスケール2。
        let default_kind = EngineKind::default();
        // Tasks ペイン専有のログチャネル (reader try_send / UI drain)。
        let (task_log_tx, task_log_rx) = mpsc::sync_channel::<LogEvent>(TASK_LOG_CHANNEL_CAPACITY);
        Self {
            screenshot: None,
            screenshot_tex: None,
            roi: RoiEdit::default(),
            scored_roi: None,
            positives: vec![],
            negatives: vec![],
            discrimination: None,
            engine_kind: default_kind,
            engine: StudioApp::build_engine(default_kind),
            heatmap_engine: Box::new(SseVisionEngine::new(TemplateMatcher::new(
                MatchConfidence::new(0.0),
                HEATMAP_DOWNSCALE,
            ))),
            heatmap_tex: None,
            heatmap_search: ScreenRegion::new(0, 0, 0, 0),
            best_match: None,
            tpl_name: String::from("template_01"),
            tpl_state_idx: 0,
            save_dir: PathBuf::from("./templates/scenes"),
            mode: AppMode::Authoring,
            test_dir: PathBuf::from("./templates/tests"),
            batch_threshold: 0.5,
            batch_result: None,
            adb_serial: String::new(),
            target,
            win_exe: exe.unwrap_or_else(default_win_exe),
            live: None,
            scaler: ScreenScaler::new(),
            proposals: vec![],
            proposing: false,
            proposal_rx: None,
            status: String::from("スクリーンショットと正例/負例フォルダを読み込んでください"),
            connection: ConnectionStatus::default(),
            task_dir: PathBuf::from("./templates/pipelines/created"),
            task_action: PipelineActionKind::ClickSelf,
            scenario: crate::scenario_ui::ScenarioPanel::new(
                Self::workspace_root().join("templates/pipelines"),
            ),
            task_defs: None,
            task_queue: None,
            task_child: ChildProcess::new(),
            task_log: SharedLogBuffer::new(DEFAULT_MAX_LINES),
            task_log_tx,
            task_log_rx,
            task_log_snapshot: Vec::new(),
            task_log_revision: 0,
            task_scroll: AutoScrollFollow::default(),
            anaden_program: "anaden".to_string(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app_state_connection::ConnectionState;

    #[test]
    fn engine_kind_default_is_ccoeff() {
        assert_eq!(EngineKind::default(), EngineKind::Ccoeff);
    }

    #[test]
    fn app_default_connection_is_unknown() {
        let app = StudioApp::default();
        assert_eq!(app.connection().state, ConnectionState::Unknown);
    }
}
