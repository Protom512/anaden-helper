//! StudioApp の状態定義群 (Issue #162 Shard 1: app.rs 分割)。
//!
//! GUI 全体状態 [`StudioApp`] とその構成要素 (接続状態・pipeline task 保存・
//! エンジン種別・モード) を定義する。UI 描画 impl は app_ui.rs、
//! 状態操作 impl (キュー実行・ファイル入出力) は app.rs に置く。
//! 呼び出し元互換のため app.rs が本モジュールの公開アイテムを re-export する。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender};

use eframe::egui;
use image::DynamicImage;

use anaden_core::{MatchConfidence, ScreenRegion};
use anaden_vision::{
    Action, Algorithm, ScreenScaler, SseVisionEngine, TemplateMatcher, VisionEngine,
};

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

/// 接続状態 (MAA/MDA 参考の状態サマリバッジ・Issue #139 T3)。
///
///豆腐 (グリフ欠落) 排除のため、バッジ表示は Unicode 絵文字ではなく
/// ASCII 括弧ラベル (`[OK]` 等) + 日本語テキストで構成する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// 未チェック (起動直後)。
    Unknown,
    /// 確認中 (プローブ実行中)。
    Checking,
    /// 接続済み (実機検出 / プロセス検出成功)。
    Connected,
    /// 未接続 (検出失敗・理由あり)。
    Disconnected,
}

impl ConnectionState {
    /// 状態サマリバッジの表示文字列 (グリフ確認済み・豆腐なし)。
    #[must_use]
    pub fn badge(self) -> &'static str {
        match self {
            Self::Unknown => "[?] 接続未確認",
            Self::Checking => "[..] 接続確認中",
            Self::Connected => "[OK] 接続済み",
            Self::Disconnected => "[NG] 未接続",
        }
    }

    /// 接続済みかどうか。
    #[must_use]
    pub fn is_connected(self) -> bool {
        matches!(self, Self::Connected)
    }

    /// バッジの表示色 (egui 色)。
    pub(crate) fn badge_color(self) -> egui::Color32 {
        match self {
            Self::Unknown => egui::Color32::from_rgb(150, 150, 150),
            Self::Checking => egui::Color32::from_rgb(230, 160, 30),
            Self::Connected => egui::Color32::from_rgb(60, 180, 75),
            Self::Disconnected => egui::Color32::from_rgb(220, 60, 60),
        }
    }
}

/// 接続チェックの結果 (状態 + エラー理由)。
#[derive(Debug, Clone)]
pub struct ConnectionStatus {
    /// 接続状態。
    pub state: ConnectionState,
    /// チェックの詳細・エラー理由 (エラー理由パネルに表示)。
    pub detail: String,
}

impl Default for ConnectionStatus {
    fn default() -> Self {
        Self {
            state: ConnectionState::Unknown,
            detail: "接続チェック未実行".to_string(),
        }
    }
}

impl ConnectionStatus {
    /// エラー理由パネルの表示行。未接続時は理由を添える。
    #[must_use]
    pub fn reason_line(&self) -> String {
        match self.state {
            ConnectionState::Disconnected => format!("理由: {}", self.detail),
            _ => self.detail.clone(),
        }
    }
}

/// Android 実機 (adb) の接続チェック。
/// `adb -s <serial> get-state` の終了コードと stdout で判定する。
pub fn check_android_device(serial: &str) -> ConnectionStatus {
    if serial.trim().is_empty() {
        return ConnectionStatus {
            state: ConnectionState::Disconnected,
            detail: "adb serial が未入力".to_string(),
        };
    }
    match std::process::Command::new("adb")
        .args(["-s", serial.trim(), "get-state"])
        .output()
    {
        Ok(out) if out.status.success() => {
            let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if state == "device" {
                ConnectionStatus {
                    state: ConnectionState::Connected,
                    detail: format!("adb {serial}: device"),
                }
            } else {
                ConnectionStatus {
                    state: ConnectionState::Disconnected,
                    detail: format!("adb {serial}: 状態が device でない ({state})"),
                }
            }
        }
        Ok(out) => ConnectionStatus {
            state: ConnectionState::Disconnected,
            detail: format!(
                "adb {serial}: get-state 失敗 ({})",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        },
        Err(e) => ConnectionStatus {
            state: ConnectionState::Disconnected,
            detail: format!("adb 起動失敗 (adb への PATH を確認): {e}"),
        },
    }
}

/// PC版 (Windows) プロセス検出チェック。
/// `Win32Capture` の 1 枚キャプチャ成功をプロセス検出成功とみなす。
#[cfg(windows)]
pub fn check_windows_process(exe: &str) -> ConnectionStatus {
    if exe.trim().is_empty() {
        return ConnectionStatus {
            state: ConnectionState::Disconnected,
            detail: "exe 名が未入力".to_string(),
        };
    }
    let probe = anaden_device::Win32Capture::new(exe.trim());
    match probe.capture_blocking() {
        Ok(img) => ConnectionStatus {
            state: ConnectionState::Connected,
            detail: format!("{exe}: プロセス検出済み ({}x{})", img.width(), img.height()),
        },
        Err(e) => ConnectionStatus {
            state: ConnectionState::Disconnected,
            detail: format!("{exe}: プロセス未検出 or キャプチャ失敗 ({e})"),
        },
    }
}

/// PC版チェックの非 Windows フォールバック (GUI 表示整合用)。
#[cfg(not(windows))]
pub fn check_windows_process(_exe: &str) -> ConnectionStatus {
    ConnectionStatus {
        state: ConnectionState::Disconnected,
        detail: "Windows バックエンドはこの OS では利用不可".to_string(),
    }
}

/// pipeline task の認識成功時アクション種別 (UI コンボ選択用)。
/// anaden_vision::Action の作成タブで扱う部分集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineActionKind {
    /// マッチ位置をクリック (`click_self`)。
    ClickSelf,
    /// 何もしない (`do_nothing`)。
    DoNothing,
    /// 停止 (`stop`)。
    Stop,
}

impl PipelineActionKind {
    /// UI コンボ表示ラベル (グリフ確認済み・豆腐なし)。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::ClickSelf => "click_self (マッチ位置をタップ)",
            Self::DoNothing => "do_nothing (何もしない)",
            Self::Stop => "stop (停止)",
        }
    }

    /// ラベル → 種別。UI の選択状態復元用。未知ラベルは None (fail-closed)。
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            l if l == Self::ClickSelf.label() => Some(Self::ClickSelf),
            l if l == Self::DoNothing.label() => Some(Self::DoNothing),
            l if l == Self::Stop.label() => Some(Self::Stop),
            _ => None,
        }
    }

    /// anaden_vision::Action へ変換。
    fn to_action(self) -> Action {
        match self {
            Self::ClickSelf => Action::ClickSelf,
            Self::DoNothing => Action::DoNothing,
            Self::Stop => Action::Stop,
        }
    }
}

/// 作成タブの入力 (ROI/スコア) から pipeline task (anaden_vision::TaskDef) を構築する。
///
/// `method` は engine_kind.method_str ("sse"/"ccoeff") を想定。未知文字列は
/// None (fail-closed。黙って既定方式へフォールバックしない)。
#[must_use]
pub fn pipeline_task_spec(
    name: &str,
    state: &str,
    method: &str,
    roi: ScreenRegion,
    threshold: f32,
    action: PipelineActionKind,
) -> Option<anaden_vision::TaskDef> {
    let algorithm = match method {
        "sse" => Algorithm::Sse,
        "ccoeff" => Algorithm::Ccoeff,
        _ => return None,
    };
    Some(anaden_vision::TaskDef {
        name: name.to_string(),
        state: state.to_string(),
        algorithm,
        template: PathBuf::from(format!("{name}.png")),
        roi: Some([roi.x, roi.y, roi.width, roi.height]),
        threshold,
        base: None,
        action: Some(action.to_action()),
        next: Some(vec![]),
    })
}

/// pipeline task を TOML + テンプレート PNG としてディレクトリへ保存する。
///
/// 出力: `<dir>/<name>.toml` + `<dir>/<name>.png`。既存 `load_pipeline`
/// (anaden-vision) でそのまま読み込める形式 (templates/pipelines/<pipeline>/ 互換)。
pub fn save_pipeline_task(
    dir: &Path,
    spec: &anaden_vision::TaskDef,
    template: &DynamicImage,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let png_path = dir.join(format!("{}.png", spec.name));
    template.save(&png_path).map_err(std::io::Error::other)?;
    let toml_path = dir.join(format!("{}.toml", spec.name));
    let toml_str = toml::to_string(spec).map_err(std::io::Error::other)?;
    std::fs::write(&toml_path, toml_str)?;
    Ok(toml_path)
}

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
    /// バッチ評価のテストフォルダ（<dir>/<label>/*.png）。
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
    use image::Luma;

    #[test]
    fn engine_kind_default_is_ccoeff() {
        assert_eq!(EngineKind::default(), EngineKind::Ccoeff);
    }

    // ---- Issue #139 T3: 接続状態可視化 ----

    #[test]
    fn connection_state_badges_are_ascii_no_tofu() {
        // バッジ文字列は Unicode 絵文字を含まない (豆腐排除)。
        for (state, expected) in [
            (ConnectionState::Unknown, "[?] 接続未確認"),
            (ConnectionState::Checking, "[..] 接続確認中"),
            (ConnectionState::Connected, "[OK] 接続済み"),
            (ConnectionState::Disconnected, "[NG] 未接続"),
        ] {
            assert_eq!(state.badge(), expected);
            // 絵文字ブロック (U+1F300 以上) を含まないことを機械検証。
            assert!(
                state.badge().chars().all(|c| c < '\u{1F300}'),
                "badge must not contain emoji: {}",
                state.badge()
            );
        }
    }

    #[test]
    fn connection_state_is_connected_only_for_connected() {
        assert!(ConnectionState::Connected.is_connected());
        assert!(!ConnectionState::Unknown.is_connected());
        assert!(!ConnectionState::Checking.is_connected());
        assert!(!ConnectionState::Disconnected.is_connected());
    }

    #[test]
    fn connection_status_default_is_unknown_with_reason() {
        let s = ConnectionStatus::default();
        assert_eq!(s.state, ConnectionState::Unknown);
        assert_eq!(s.reason_line(), "接続チェック未実行");
    }

    #[test]
    fn connection_status_reason_line_prefixes_detail_when_disconnected() {
        let s = ConnectionStatus {
            state: ConnectionState::Disconnected,
            detail: "adb が見つからない".to_string(),
        };
        assert_eq!(s.reason_line(), "理由: adb が見つからない");
        let ok = ConnectionStatus {
            state: ConnectionState::Connected,
            detail: "adb emulator-5554: device".to_string(),
        };
        assert_eq!(ok.reason_line(), "adb emulator-5554: device");
    }

    #[test]
    fn check_android_empty_serial_is_disconnected() {
        let s = check_android_device("");
        assert_eq!(s.state, ConnectionState::Disconnected);
        assert!(s.detail.contains("serial"));
    }

    #[test]
    fn check_windows_empty_exe_is_disconnected() {
        let s = check_windows_process("  ");
        assert_eq!(s.state, ConnectionState::Disconnected);
        assert!(s.detail.contains("exe"));
    }

    #[test]
    fn app_default_connection_is_unknown() {
        let app = StudioApp::default();
        assert_eq!(app.connection().state, ConnectionState::Unknown);
    }

    // ---- Issue #139 T5: UC-3 作成タブ → pipeline task TOML 保存 ----

    /// pipeline_task_spec は有効な方式文字列から TaskDef を構築する。
    /// engine_kind.method_str ("sse"/"ccoeff") がそのまま使える。
    #[test]
    fn pipeline_task_spec_builds_from_method_str() {
        let roi = ScreenRegion::new(10, 20, 30, 40);
        let spec = pipeline_task_spec(
            "my_task",
            "field",
            "ccoeff",
            roi,
            0.85,
            PipelineActionKind::ClickSelf,
        )
        .unwrap();
        assert_eq!(spec.name, "my_task");
        assert_eq!(spec.state, "field");
        assert_eq!(spec.algorithm, anaden_vision::Algorithm::Ccoeff);
        assert_eq!(spec.roi, Some([10, 20, 30, 40]));
        assert_eq!(spec.threshold, 0.85);
        assert_eq!(spec.action, Some(anaden_vision::Action::ClickSelf));
        assert_eq!(spec.next, Some(vec![]));

        let sse = pipeline_task_spec(
            "t2",
            "title",
            "sse",
            roi,
            0.9,
            PipelineActionKind::DoNothing,
        )
        .unwrap();
        assert_eq!(sse.algorithm, anaden_vision::Algorithm::Sse);
        assert_eq!(sse.action, Some(anaden_vision::Action::DoNothing));
    }

    /// 未知の方式文字列は None (fail-closed。黙って sse にフォールバックしない)。
    #[test]
    fn pipeline_task_spec_rejects_unknown_method() {
        assert!(
            pipeline_task_spec(
                "x",
                "field",
                "orb",
                ScreenRegion::new(0, 0, 1, 1),
                0.9,
                PipelineActionKind::ClickSelf
            )
            .is_none()
        );
    }

    /// save_pipeline_task が書いた TOML は既存 load_pipeline で読み込める (roundtrip)。
    /// 作成タブで保存した task が実行パイプライン (anaden-cli) からそのまま
    /// 使えることの結合保証。
    #[test]
    fn save_pipeline_task_roundtrips_through_load_pipeline() {
        let dir = tempfile::tempdir().unwrap();
        let spec = pipeline_task_spec(
            "tap_logo",
            "title",
            "ccoeff",
            ScreenRegion::new(10, 20, 100, 50),
            0.82,
            PipelineActionKind::ClickSelf,
        )
        .unwrap();
        let img = DynamicImage::ImageLuma8(image::GrayImage::from_pixel(100, 50, Luma([128])));
        let toml_path = save_pipeline_task(dir.path(), &spec, &img).unwrap();
        assert!(toml_path.exists());
        assert!(dir.path().join("tap_logo.png").exists());

        let tasks = anaden_vision::load_pipeline(dir.path()).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "tap_logo");
        assert_eq!(tasks[0].state, "title");
        assert_eq!(tasks[0].algorithm, anaden_vision::Algorithm::Ccoeff);
        assert_eq!(tasks[0].roi, Some([10, 20, 100, 50]));
        assert_eq!(tasks[0].action, Some(anaden_vision::Action::ClickSelf));
    }

    /// action 種別の label ラウンドトリップ (UI コンボ用)。
    #[test]
    fn pipeline_action_kind_labels_roundtrip() {
        for k in [
            PipelineActionKind::ClickSelf,
            PipelineActionKind::DoNothing,
            PipelineActionKind::Stop,
        ] {
            assert_eq!(PipelineActionKind::from_label(k.label()), Some(k));
        }
        assert_eq!(PipelineActionKind::from_label("bogus"), None);
    }
}
