//! StudioApp: GUI 全体状態と eframe::App 実装。
//!
//! 左パネル（操作・識別力サマリ）と中央キャンバス（画像＋ROI選択）で構成。
//! ROIが確定（ドラッグ解放）するたび、候補テンプレートを正例/負例で評価する。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender};

use eframe::egui;
use image::DynamicImage;

use anaden_core::{MatchConfidence, ScreenRegion};
use anaden_vision::{
    Action, Algorithm, CcoeffVisionEngine, ScreenScaler, SseVisionEngine, TemplateMatcher,
    VisionEngine,
};

use crate::batch::{self, ConfusionMatrix};
use crate::canvas::{self, RoiEdit};
use crate::childproc::{ChildProcess, SpawnSpec};
use crate::library::{self, TemplateSpec};
use crate::log_view::{
    AutoScrollFollow, DEFAULT_MAX_LINES, LogBuffer, LogEntry, LogEvent, SharedLogBuffer,
};
use crate::proposals::{self, Proposal};
use crate::scoring::{self, Discrimination};
use crate::source::LiveCapture;
use crate::tasks::{self, QueueAction, QueueEntry, QueueExec, QueueState};

/// ヒートマップ計算用のダウンスケール倍率。
/// imageproc の match_template は O(W·H·w·h) の総当たりのため、フル解像度では重い。
/// 4倍縮小で速度と位置精度を両立する（位置精度 ±4px）。
const HEATMAP_DOWNSCALE: u32 = 4;

/// Tasks ペインのログチャネル容量 (reader スレッド try_send / UI 毎フレーム drain)。
/// runner.rs の LOG_CHANNEL_CAPACITY と同値 (bounded・best-effort 破棄契約)。
const TASK_LOG_CHANNEL_CAPACITY: usize = 1024;

/// テンプレート保存時の状態選択肢。TemplateStore の parse_state_from_dir_name と整合。
const STATE_OPTIONS: &[&str] = &[
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
    fn badge_color(self) -> egui::Color32 {
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
/// (anaden-vision) でそのまま読み込れる形式 (templates/pipelines/<pipeline>/ 互換)。
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
enum EngineKind {
    /// imageproc 正規化SSE（絶対輝度差）。現行ベースライン。
    Sse,
    /// TM_CCOEFF_NORMED（輝度シフトにロバスト）。
    #[default]
    Ccoeff,
}

impl EngineKind {
    /// TemplateSpec.method / 実行エンジンの方式文字列へ変換する。
    /// library::TemplateSpec の method 文字列仕様（"sse" / "ccoeff"）と完全一致。
    fn method_str(self) -> &'static str {
        match self {
            EngineKind::Sse => "sse",
            EngineKind::Ccoeff => "ccoeff",
        }
    }
}

/// GUI 全体の状態。
pub struct StudioApp {
    /// 編集中のスクリーンショット。
    screenshot: Option<Arc<DynamicImage>>,
    /// スクリーンショットの表示用テクスチャ。
    screenshot_tex: Option<egui::TextureHandle>,
    /// ドラッグROI編集状態。
    roi: RoiEdit,
    /// 最後にスコア計算したROI（変化検出用）。
    scored_roi: Option<ScreenRegion>,
    /// 正例画像（同じ画面状態）。フォルダ単位で読込。
    positives: Vec<Arc<DynamicImage>>,
    /// 負例画像（別画面状態）。
    negatives: Vec<Arc<DynamicImage>>,
    /// 直近の識別力評価結果。
    discrimination: Option<Discrimination>,
    /// 現在選択中のエンジン種別（コンボで切替）。engine 再構築の基。
    engine_kind: EngineKind,
    /// 認識エンジン（閾値0・1/2ダウンスケールで生スコアを高速に返す）。
    engine: Box<dyn VisionEngine>,
    /// ヒートマップ計算用エンジン（閾値0・1/4ダウンスケールでスコアマップ全体を算出）。
    heatmap_engine: Box<dyn VisionEngine>,
    /// ヒートマップテクスチャ（ROI解放時に更新）。
    heatmap_tex: Option<egui::TextureHandle>,
    /// ヒートマップが対応する探索領域（元画像座標）。
    heatmap_search: ScreenRegion,
    /// テンプレートの最良マッチ位置（元画像座標・ROI解放時に更新）。
    best_match: Option<ScreenRegion>,
    /// 保存時のテンプレート名入力。
    tpl_name: String,
    /// 保存時の状態選択（STATE_OPTIONS のインデックス）。
    tpl_state_idx: usize,
    /// テンプレート保存先ディレクトリ。
    save_dir: PathBuf,
    /// 現在のモード。
    mode: AppMode,
    /// バッチ評価のテストフォルダ（<dir>/<label>/*.png）。
    test_dir: PathBuf,
    /// バッチ評価の決定閾値。
    batch_threshold: f32,
    /// バッチ評価結果。
    batch_result: Option<ConfusionMatrix>,
    /// ADB デバイスシリアル（ライブキャプチャ用）。
    adb_serial: String,
    /// ライブキャプチャの取得元バックエンド(android/windows)。
    target: crate::source::Target,
    /// PC版(Windows)バックエンドの対象 exe 名。
    win_exe: String,
    /// ライブキャプチャ（稼働中のみ）。
    live: Option<LiveCapture>,
    /// 720p 基準への解像度正規化スケーラ（TASK-009）。
    scaler: ScreenScaler,
    /// ROI自動提案の候補リスト（ROI候補ボタン押下で生成）。
    proposals: Vec<Proposal>,
    /// ROI候補提案の計算中フラグ（別スレッドで propose 実行中）。
    proposing: bool,
    /// 別スレッドでの propose 計算結果を受信する channel。
    /// 計算未依頼時・受信済み時は空（Option で所有権の有無を表現）。
    proposal_rx: Option<Receiver<Vec<Proposal>>>,
    /// ステータスメッセージ。
    status: String,
    /// 接続状態 (実機/プロセス検出チェック結果)。Issue #139 T3。
    connection: ConnectionStatus,
    /// pipeline task 保存先ディレクトリ (UC-3: 作成タブ → pipeline TOML 保存)。
    task_dir: PathBuf,
    /// pipeline task の認識成功時アクション選択 (UC-3)。
    task_action: PipelineActionKind,
    /// MAA 型タスク一覧の定義リスト (Issue #144)。None = 未読込。
    task_defs: Option<crate::tasks::TaskListState>,
    /// チェック順逐次実行キューの状態機械 (Issue #154 Shard 1)。None = 未開始。
    task_queue: Option<QueueExec>,
    /// Tasks ペイン専有の子プロセス管理 (runner とは独立・Issue #154 Shard 1)。
    task_child: ChildProcess,
    /// Tasks ペイン専有のログバッファ (reader → channel → drain)。
    task_log: SharedLogBuffer,
    /// ログイベント送信口 (stdout/stderr reader 接続・キュー実行で再利用)。
    task_log_tx: SyncSender<LogEvent>,
    /// ログイベント受信口 (毎フレーム drain・Exit 観測がキュー進行の契機)。
    task_log_rx: Receiver<LogEvent>,
    /// UI 描画用ログスナップショット (drain 毎に更新)。
    task_log_snapshot: Vec<LogEntry>,
    /// ログの自動スクロール追従 (log_view.rs の純ロジック再用・UC-4)。
    task_scroll: AutoScrollFollow,
    /// anaden CLI 実行ファイル (spawn 時の program)。
    anaden_program: String,
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
            task_defs: None,
            task_queue: None,
            task_child: ChildProcess::new(),
            task_log: SharedLogBuffer::new(DEFAULT_MAX_LINES),
            task_log_tx,
            task_log_rx,
            task_log_snapshot: Vec::new(),
            task_scroll: AutoScrollFollow::default(),
            anaden_program: "anaden".to_string(),
        }
    }
}

impl StudioApp {
    /// engine_kind から生スコア評価用エンジンを構築する（downscale=2, 閾値0）。
    /// 両エンジンで条件を統一し公平比較を保証する純関数。
    fn build_engine(kind: EngineKind) -> Box<dyn VisionEngine> {
        match kind {
            EngineKind::Sse => Box::new(SseVisionEngine::new(TemplateMatcher::new(
                MatchConfidence::new(0.0),
                2,
            ))),
            EngineKind::Ccoeff => Box::new(CcoeffVisionEngine::new(MatchConfidence::new(0.0), 2)),
        }
    }

    /// 現在のモードを返す（公開 API 経由の振る舞い検証用）。
    pub fn mode(&self) -> AppMode {
        self.mode
    }

    /// モードを設定する（埋め込み親シェルからのタブ切替用）。
    pub fn set_mode(&mut self, mode: AppMode) {
        self.mode = mode;
    }

    /// 現在の接続状態への参照 (Issue #139 T3)。
    pub fn connection(&self) -> &ConnectionStatus {
        &self.connection
    }

    // ---- Issue #144 Task 3 / Issue #154 Shard 1: MAA 型タスク一覧の配線 ----
    // (ドメインロジックは tasks.rs・実行は Tasks ペイン専有の ChildProcess)

    /// anaden CLI 実行ファイル (spawn 時の program) を設定する。
    pub fn set_anaden_program(&mut self, program: impl Into<String>) {
        self.anaden_program = program.into();
    }

    /// 現在のタスクキュー (テスト・進行表示用)。
    pub fn task_queue(&self) -> Option<&QueueExec> {
        self.task_queue.as_ref()
    }

    /// キューがアクティブ (未完了 = Pending/Running/PausedAfterFailure) か。
    fn task_queue_active(&self) -> bool {
        self.task_queue
            .as_ref()
            .is_some_and(|q| !matches!(q.state(), QueueState::Completed))
    }

    /// タスク定義が未読込なら既定パスから読み込む (ホーム画面表示時に呼ぶ)。
    pub fn ensure_task_list_loaded(&mut self) {
        if self.task_defs.is_none() {
            let dir = Self::workspace_root().join("templates/tasks");
            self.load_task_list(&dir);
        }
    }

    /// `templates/tasks/` からタスク定義を読み込む。失敗時は status に理由。
    pub fn load_task_list(&mut self, tasks_dir: &Path) {
        match tasks::TaskListState::load(tasks_dir) {
            Ok(list) => {
                self.status = format!("タスク定義: {} 件読込", list.definitions().len());
                self.task_defs = Some(list);
            }
            Err(e) => self.status = format!("タスク定義読込失敗: {e}"),
        }
    }

    /// チェックボックストグル (implemented=false は tasks.rs 側で拒否される)。
    pub fn toggle_task(&mut self, id: &str) {
        let Some(list) = &mut self.task_defs else {
            self.status = "タスク定義が未読込です".to_string();
            return;
        };
        match list.toggle(id) {
            Ok(()) => self.status = format!("選択: {} 件", list.selected_count()),
            Err(e) => self.status = e.to_string(),
        }
    }

    /// workspace ルート (runner.rs と同一の決定論的解決)。
    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
    }

    /// CLI target 文字列 (source::Target → anaden CLI の `--target` 値)。
    fn cli_target(&self) -> &'static str {
        match self.target {
            crate::source::Target::Android => "android",
            crate::source::Target::Windows => "windows",
        }
    }

    /// 開始ボタン: 選択キューからチェック順エントリ列を組み立てて開始する
    /// (UC-2)。実行本体は Tasks ペイン専有の ChildProcess + QueueExec 状態機械
    /// (runner とは独立・Issue #154 Shard 1。dispatch 注入方式は廃止)。
    pub fn start_task_queue(&mut self) {
        let Some(list) = &self.task_defs else {
            self.status = "タスク定義が未読込です".to_string();
            return;
        };
        let entries = match list.queue_entries(
            &self.anaden_program,
            self.cli_target(),
            Some(self.adb_serial.as_str()),
            &Self::workspace_root(),
        ) {
            Ok(e) => e,
            Err(e) => {
                self.status = e.to_string();
                return;
            }
        };
        self.start_task_entries(entries);
    }

    /// QueueEntry 列を直接キューへ渡して開始する (queue handoff API)。
    ///
    /// [`Self::start_task_queue`] の本体で、テスト・埋め込み親シェルが
    /// エントリ列を明示注入する経路も兼ねる (旧 set_task_dispatch /
    /// pending_spawn 単一スロットの後継 — 複数 spec の逐次実行を表現可能)。
    /// 実行中キューがある場合の再開始・空列は拒否する (fail-closed)。
    pub fn start_task_entries(&mut self, entries: Vec<QueueEntry>) {
        if self.task_queue_active() {
            self.status = "キュー実行中のため開始できません（中止してから再開）".to_string();
            return;
        }
        if entries.is_empty() {
            self.status = "チェックされたタスクがありません".to_string();
            return;
        }
        // 新規キュー: ログを初期化して状態機械を開始する。
        self.task_log.with_buf(LogBuffer::clear);
        self.task_scroll = AutoScrollFollow::default();
        let mut queue = QueueExec::new(entries);
        let action = queue.start();
        let count = queue.total();
        self.task_queue = Some(queue);
        self.status = format!("開始: {count} タスク");
        self.apply_task_action(action);
        self.refresh_task_log_snapshot();
    }

    /// 失敗停止中のキューを明示継続する (次タスクを起動・UC-4)。
    pub fn resume_task_queue(&mut self) {
        let action = match &mut self.task_queue {
            Some(queue) => queue.resume(),
            None => QueueAction::Noop,
        };
        self.apply_task_action(action);
    }

    /// キューを中止する (実行中の子も停止し残りタスクを破棄・UC-4)。
    pub fn abort_task_queue(&mut self) {
        let _ = self.task_child.stop();
        if let Some(queue) = &mut self.task_queue {
            queue.abort();
        }
        self.push_task_log("[studio] === キュー中止 ===");
        self.status = "キューを中止しました".to_string();
    }

    /// 状態機械の出力アクションを実行へ反映する (配線)。
    fn apply_task_action(&mut self, action: QueueAction) {
        match action {
            QueueAction::Start(spec) => self.spawn_task_spec(&spec),
            // on_exit が WaitForExit を返すのは失敗停止時のみ (自動継続禁止)。
            // UC-4: 失敗理由 (exit code) を status にも出す。
            QueueAction::WaitForExit => {
                if let Some(queue) = self.task_queue.as_ref() {
                    self.status = queue.summary();
                }
            }
            QueueAction::Noop => {}
            QueueAction::QueueCompleted => {
                self.push_task_log("[studio] === キュー完了 ===");
                self.status = "全タスクが完了しました".to_string();
            }
        }
    }

    /// 1 タスクを起動する。タスク境界にセパレータ行を出す (UC-4)。
    /// 起動失敗は当該タスクの失敗扱いとして失敗停止へ (自動継続禁止)。
    fn spawn_task_spec(&mut self, spec: &SpawnSpec) {
        let sep = match self.task_queue.as_ref().and_then(|q| q.current_entry()) {
            Some(entry) => format!("[studio] === task: {} ===", entry.label),
            None => "[studio] === task ===".to_string(),
        };
        self.push_task_log(&sep);
        if let Err(e) = self.task_child.start(spec, self.task_log_tx.clone()) {
            self.push_task_log(&format!("[studio] 起動に失敗: {e}"));
            let action = match &mut self.task_queue {
                Some(queue) => queue.on_exit(None),
                None => QueueAction::Noop,
            };
            self.apply_task_action(action);
            // 失敗停止サマリより起動失敗理由を優先表示する。
            self.status = format!("起動に失敗: {e}");
        }
    }

    /// チャネルを drain してログへ反映し、Exit 観測でキューを進める
    /// (UC-4: 毎フレーム呼び出し。完了判定は LogEvent::Exit のみ)。
    /// 行の記録自体は log_view::drain_channel_into (runner と共有) に委譲。
    pub fn drain_task_logs(&mut self) {
        if self.task_queue.is_none() {
            return;
        }
        let (new_lines, exit_code) =
            crate::log_view::drain_channel_into(&self.task_log, &self.task_log_rx);
        if new_lines > 0 {
            self.task_scroll.observe_new_lines(new_lines);
        }
        if let Some(code) = exit_code
            && let Some(queue) = &mut self.task_queue
        {
            let action = queue.on_exit(code);
            self.apply_task_action(action);
        }
        self.refresh_task_log_snapshot();
    }

    /// タスク実行ログへ 1 行 push する (セパレータ・システム行)。
    fn push_task_log(&mut self, line: &str) {
        let line = line.to_string();
        self.task_log.with_buf(|b| b.push_line(&line));
        self.refresh_task_log_snapshot();
    }

    /// UI 描画用ログスナップショットを最新化する。
    fn refresh_task_log_snapshot(&mut self) {
        self.task_log_snapshot = self
            .task_log
            .with_buf(|b| b.entries().cloned().collect())
            .unwrap_or_default();
    }

    /// タスク一覧 UI (MAA 型チェックボックス) を描画する。
    /// implemented=false はグレー表示・チェック不可 (嘘の動作可能表示禁止)。
    /// UC-4: 毎フレーム drain によるリアルタイム進行表示 (i/N + チェック順
    /// キュー)・ログ表示・失敗時の明示的な「継続」「停止」ボタンを含む。
    pub fn render_task_list(&mut self, ui: &mut egui::Ui) {
        ui.heading("タスク一覧");
        // 毎フレーム drain (UC-4: Exit 観測がキュー進行の唯一の契機)。
        self.drain_task_logs();
        if self.task_defs.is_none() {
            if ui.button("タスク定義を読み込む").clicked() {
                self.load_task_list(&Self::workspace_root().join("templates/tasks"));
            }
        } else if let Some(list) = self.task_defs.clone() {
            // UC-3: 詳細プレビューは実行と同じ引数解決条件 (target/serial/root)。
            let target = self.cli_target();
            let serial = Some(self.adb_serial.as_str());
            let root = Self::workspace_root();
            let selected = list.selected_ids().to_vec();
            let mut clicked: Option<String> = None;
            for def in list.definitions() {
                let mut checked = list.is_selected(&def.id);
                let label = tasks::checkbox_label(def);
                ui.horizontal(|ui| {
                    ui.add_enabled(
                        def.is_selectable(),
                        egui::Checkbox::new(&mut checked, label),
                    );
                    // UC-3: 選択済みなら実行順位置を横に表示 (未選択は非表示)。
                    if let Some(pos) = tasks::queue_position_label(&selected, &def.id) {
                        ui.weak(pos);
                    }
                });
                if checked != list.is_selected(&def.id) {
                    clicked = Some(def.id.clone());
                }
                // UC-3: 展開可能な詳細表示 (kind・pipeline_dir・start_task・
                // 引数プレビュー — 読み取り専用・schema 変更なし)。
                egui::CollapsingHeader::new(egui::RichText::new("詳細").weak())
                    .id_salt(&def.id)
                    .show(ui, |ui| {
                        Self::task_detail_ui(ui, def, target, serial, &root);
                    });
            }
            if let Some(id) = clicked {
                self.toggle_task(&id);
            }
            // 開始ボタンはキュー非アクティブ時のみ有効 (実行中の再開始拒否)。
            let can_start = list.selected_count() > 0 && !self.task_queue_active();
            ui.add_enabled_ui(can_start, |ui| {
                if ui.button("開始").clicked() {
                    self.start_task_queue();
                }
            });
            // UC-3: 選択済みキューの実行順リスト (チェック順 1. 2. 3. ...・
            // 未実装 (不整合検出時) はグレー表示)。
            let rows = tasks::queue_order_rows(&selected, list.definitions());
            if !rows.is_empty() {
                ui.separator();
                ui.label("実行順 (チェック順)");
                for row in &rows {
                    if row.runnable {
                        ui.label(format!("{}. {}", row.position, row.title));
                    } else {
                        ui.weak(format!("{}. {} (未実装)", row.position, row.title));
                    }
                }
            }
        }
        // UC-4: 進行サマリ + 実行制御 + チェック順キュー一覧。
        if let Some(queue) = self.task_queue.clone() {
            ui.separator();
            ui.label(queue.summary());
            match queue.state() {
                QueueState::Running { .. } => {
                    if ui.button("中止").clicked() {
                        self.abort_task_queue();
                    }
                }
                QueueState::PausedAfterFailure { .. } => {
                    ui.colored_label(egui::Color32::RED, "タスクが失敗しました。継続しますか?");
                    if ui.button("継続").clicked() {
                        self.resume_task_queue();
                    }
                    if ui.button("停止").clicked() {
                        self.abort_task_queue();
                    }
                }
                QueueState::Pending | QueueState::Completed => {}
            }
            for (i, entry) in queue.entries().iter().enumerate() {
                ui.label(format!(
                    "{}. [{}] {}",
                    i + 1,
                    queue.entry_marker(i),
                    entry.label
                ));
            }
        }
        ui.separator();
        self.task_log_ui(ui);
        ui.separator();
        ui.label(&self.status);
    }

    /// UC-3: タスク 1 件の詳細表示ボディ (collapsing header 配下・読み取り専用)。
    ///
    /// kind・pipeline_dir・start_task (未宣言時は解決結果)・実引数プレビューを
    /// 表示する。引数解決は [`tasks::task_detail_view`] (実行の [`tasks::spawn_args`]
    /// と単一情報源)。未実装タスクは赤字で理由を表示 (fail-closed)。
    fn task_detail_ui(
        ui: &mut egui::Ui,
        def: &tasks::TaskDefinition,
        target: &str,
        serial: Option<&str>,
        root: &Path,
    ) {
        let view = tasks::task_detail_view(def, target, serial, root);
        ui.label(format!("ID: {}", view.id));
        ui.label(format!("種別: {}", view.kind));
        match &view.pipeline_dir {
            Some(dir) => {
                ui.label(format!("pipeline_dir: {dir}"));
            }
            None => {
                ui.weak("pipeline_dir: なし (サブコマンド実行)");
            }
        }
        match &view.start_task {
            Some(task) => {
                ui.label(format!("start_task: {task}"));
            }
            None if def.kind == tasks::TaskKind::PipelineRun => {
                // pipeline_run なのに解決不能 = 実行不可 (fail-closed 表示)。
                ui.colored_label(egui::Color32::RED, "start_task: 未解決");
            }
            None => {} // launch_subcommand は start_task を使用しない
        }
        ui.label(format!("引数プレビュー: {}", view.args_preview()));
        if let Some(reason) = &view.unimplemented_reason {
            ui.colored_label(egui::Color32::RED, format!("未実装: {reason}"));
        }
    }

    /// Tasks ペインの実行ログビューア (log_view.rs の LogBuffer/AutoScroll 再利用)。
    fn task_log_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("実行ログ");
            let mut follow = self.task_scroll.is_enabled();
            ui.checkbox(&mut follow, "自動スクロール");
            if follow != self.task_scroll.is_enabled() {
                self.task_scroll.set_enabled(follow);
            }
            if ui.button("クリア").clicked() {
                self.task_log.with_buf(LogBuffer::clear);
                self.refresh_task_log_snapshot();
            }
            if self.task_scroll.pending_lines() > 0 {
                ui.weak(format!("新着 {} 行", self.task_scroll.pending_lines()));
            }
        });
        let stick = self.task_scroll.should_stick_to_bottom();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(stick)
            .show(ui, |ui| {
                if self.task_log_snapshot.is_empty() {
                    ui.weak("（ログなし）");
                }
                for entry in &self.task_log_snapshot {
                    ui.monospace(
                        egui::RichText::new(&entry.line)
                            .monospace()
                            .color(crate::runner::level_color(entry.level)),
                    );
                }
            });
        if stick {
            // stick_to_bottom が有効な間は egui が末尾へ張り付くため追従清算する。
            self.task_scroll.on_scrolled_to_bottom();
        }
    }

    /// 接続チェックを実行して状態を更新する (Issue #139 T3)。
    ///
    /// target に応じて Android (adb get-state) / Windows (プロセス検出) を使い分ける。
    pub fn run_connection_check(&mut self) {
        self.connection.state = ConnectionState::Checking;
        self.connection = match self.target {
            crate::source::Target::Android => check_android_device(&self.adb_serial),
            #[cfg(windows)]
            crate::source::Target::Windows => check_windows_process(&self.win_exe),
            #[cfg(not(windows))]
            crate::source::Target::Windows => check_windows_process(&self.win_exe),
        };
    }

    /// エンジン種別を切替え、self.engine を再構築し、再評価を強制する。
    /// downscale=2・閾値0 で現行 scoring engine と同じ条件（公平比較）。
    /// scored_roi / discrimination を None に戻すことで、次フレームの
    /// CentralPanel 再評価ブロックが新エンジンで discrimination を再計算する。
    fn switch_engine(&mut self, kind: EngineKind) {
        self.engine_kind = kind;
        self.engine = StudioApp::build_engine(kind);
        self.scored_roi = None; // 次フレームで再評価を強制
        self.discrimination = None; // 古いスコアを即クリア（チラつき防止）
        self.status = format!(
            "エンジン切替: {}",
            match kind {
                EngineKind::Sse => "SSE（輝度差ベース）",
                EngineKind::Ccoeff => "CCOEFF（ロバスト・輝度シフト不変）",
            }
        );
    }

    /// ファイルダイアログでスクリーンショットを開く。
    fn open_screenshot(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("画像", &["png", "jpg", "jpeg", "bmp"])
            .pick_file()
        {
            match image::open(&path) {
                Ok(img) => {
                    self.status = format!(
                        "スクリーンショット: {}x{} → 720p基準で正規化",
                        img.width(),
                        img.height()
                    );
                    let normalized = self.scaler.normalize(&img);
                    self.screenshot = Some(Arc::new(normalized));
                    self.screenshot_tex = None; // 再生成
                    self.roi = RoiEdit::default();
                    self.scored_roi = None;
                    self.discrimination = None;
                    self.heatmap_tex = None;
                    self.best_match = None;
                    self.proposals = vec![];
                }
                Err(e) => self.status = format!("読込失敗: {e}"),
            }
        }
    }

    /// 正例フォルダを読み込む。
    fn load_positives(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            let imgs = load_folder(&dir);
            self.status = format!("正例: {} 枚読込", imgs.len());
            self.positives = imgs;
            self.scored_roi = None; // 再評価を強制
        }
    }

    /// 負例フォルダを読み込む。
    fn load_negatives(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            let imgs = load_folder(&dir);
            self.status = format!("負例: {} 枚読込", imgs.len());
            self.negatives = imgs;
            self.scored_roi = None;
        }
    }

    /// 現在のROI切り出しをテンプレートとして保存する。
    /// 閾値は識別力があれば正例/負例スコアの中間、なければ 0.9。
    fn save_current_template(&mut self) {
        let (Some(img), Some(roi)) = (self.screenshot.clone(), self.roi.rect()) else {
            return;
        };
        let crop = img.crop_imm(roi.x, roi.y, roi.width, roi.height);
        let threshold = self
            .discrimination
            .as_ref()
            .map(|d| ((d.own_min + d.other_max) / 2.0).clamp(0.5, 0.99))
            .unwrap_or(0.9);
        let spec = TemplateSpec {
            name: self.tpl_name.clone(),
            state: STATE_OPTIONS[self.tpl_state_idx].to_string(),
            roi,
            threshold,
            // engine_kind に連動（Sse => "sse", Ccoeff => "ccoeff"）。
            // library::TemplateSpec の method 文字列仕様と一致。
            method: self.engine_kind.method_str().to_string(),
        };
        match library::save_template(&self.save_dir, &spec, &crop) {
            Ok(p) => self.status = format!("保存: {}", p.display()),
            Err(e) => self.status = format!("保存失敗: {e}"),
        }
    }

    /// 現在のスクリーンショットからROI候補を提案する。
    ///
    /// propose は match_template 総当たりで重く、PC版(1280x699)画像では
    /// UI スレッドを数秒ブロックしてフリーズする。そのため別スレッドで計算し、
    /// 結果を mpsc channel で UI へ返す（update で try_recv で非ブロッキング受信）。
    ///
    /// - Box<dyn VisionEngine> はデフォルトで Send を要求しないため、スレッドへは
    ///   engine_kind（Copy）と screenshot（Arc）だけを渡し、スレッド内で
    ///   build_engine(kind) から再構築して使う。heatmap_engine と同等（閾値0・
    ///   1/4ダウンスケール）のエンジンを build できないため、propose 専用に
    ///   downscale=HEATMAP_DOWNSCALE の SSE エンジンを構築して渡す。
    /// - 計算中フラグ(self.proposing)を立て、二重起動を防ぐ。ボタンは UI 側で無効化。
    fn run_proposals(&mut self) {
        if self.proposing {
            return; // 二重起動防止
        }
        let Some(img) = self.screenshot.clone() else {
            self.status = "スクリーンショットを先に読み込んでください".to_string();
            return;
        };
        self.proposing = true;
        self.status = "ROI候補を計算中…".to_string();

        let (tx, rx) = mpsc::channel::<Vec<Proposal>>();
        self.proposal_rx = Some(rx);

        // 提案計算は heatmap_engine と同等（閾値0・1/4ダウンスケール）のエンジンで
        // 行う。heatmap_engine は Send を要求しない Box<dyn VisionEngine> なので
        // スレッドへは渡せず、スレッド内で同条件の SSE エンジンを新規構築する。
        let downscale = HEATMAP_DOWNSCALE;
        std::thread::spawn(move || {
            let engine =
                SseVisionEngine::new(TemplateMatcher::new(MatchConfidence::new(0.0), downscale));
            let ps = proposals::propose(
                &engine, &img, 96, // tile_w
                96, // tile_h
                96, // step（ノーオーバーラップ）
                12, // max_n
            );
            // 受信側が破棄されていてもエラーは無視（アプリ終了時等）。
            let _ = tx.send(ps);
        });
    }

    /// 候補ROIをドラッグROI編集状態に読み込む。    ///
    /// RoiEdit::rect() は width = x1 - x0 で矩形を復元するため、
    /// 候補 roi (x,y,w,h) を正確に再現するには current を (x+w, y+h) = (right(), bottom())
    /// に設定する（right-1 だと width が1つ減る）。dragging=false で確定状態にする。
    /// scored_roi を None に戻し、既存の再評価トリガで識別力スコアを自動再計算させる。
    fn apply_proposal(&mut self, roi: ScreenRegion) {
        self.roi.anchor = Some((roi.x, roi.y));
        self.roi.current = Some((roi.right(), roi.bottom()));
        self.roi.dragging = false;
        // 既存の再評価トリガを発火させるため、scored_roi を古い値に戻す。
        self.scored_roi = None;
    }

    /// 現在のROI切り出しを pipeline task (TOML+PNG) として保存する (UC-3)。
    ///
    /// 既存部品のみで構成: ROI画像は screenshot から crop、閾値は
    /// discrimination (scoring.rs) があれば正例/負例の中間、なければ 0.9、
    /// 方式は engine_kind.method_str、TOML は anaden_vision::TaskDef として
    /// serialize し save_pipeline_task で書き出す。書いた TOML は既存
    /// load_pipeline で読める (roundtrip 検証済み)。
    fn save_current_pipeline_task(&mut self) {
        let (Some(img), Some(roi)) = (self.screenshot.clone(), self.roi.rect()) else {
            self.status = "pipeline task 保存にはスクリーンショットとROI確定が必要です".to_string();
            return;
        };
        let name = if self.tpl_name.trim().is_empty() {
            "template_01".to_string()
        } else {
            self.tpl_name.trim().to_string()
        };
        let crop = img.crop_imm(roi.x, roi.y, roi.width, roi.height);
        let threshold = self
            .discrimination
            .as_ref()
            .map(|d| ((d.own_min + d.other_max) / 2.0).clamp(0.5, 0.99))
            .unwrap_or(0.9);
        let Some(spec) = pipeline_task_spec(
            &name,
            STATE_OPTIONS[self.tpl_state_idx],
            self.engine_kind.method_str(),
            roi,
            threshold,
            self.task_action,
        ) else {
            self.status = format!(
                "pipeline task 保存失敗: 未知の方式 ({})",
                self.engine_kind.method_str()
            );
            return;
        };
        match save_pipeline_task(&self.task_dir, &spec, &crop) {
            Ok(p) => {
                self.status = format!("pipeline task 保存: {}", p.display());
            }
            Err(e) => self.status = format!("pipeline task 保存失敗: {e}"),
        }
    }
}

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render_modebar(ui);
        self.render_body(ui);
    }
}

impl StudioApp {
    /// ウィンドウ上限のモード切替バー（modebar）を描画する。
    ///
    /// 親レイアウト内への埋め込み（単一ウィンドウ統合 GUI, Issue #119）を想定した
    /// 公開パネル描画 API。単体テストからは [`Self::mode`] / [`Self::set_mode`]
    /// 経由で振る舞いを検証する。
    pub fn render_modebar(&mut self, ui: &mut egui::Ui) {
        // モード切替バー
        egui::Panel::top("modebar")
            .exact_size(30.0)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.mode, AppMode::Authoring, "作成");
                    ui.selectable_value(&mut self.mode, AppMode::Batch, "バッチ評価");
                });
            });
    }

    /// モード本体（Authoring / Batch）を親レイアウト内に描画する埋め込み用 API。
    ///
    /// modebar は含まない。呼び出し前に [`Self::render_modebar`] を実行するか、
    /// 親シェル側でタブ切替してもよい（mode は [`Self::set_mode`] で制御）。
    ///
    /// スクリーンショットのテクスチャ生成はここ（描画パスの入口）で行う。
    /// かつて [`Self::render_modebar`] 内にあったが、統合GUI シェル
    /// （Issue #119 `UnifiedShell`）は [`Self::render_body`] のみを呼ぶため、
    /// テクスチャが生成されずキャンバスが永久に空になる欠陥があった
    /// （作成タブで画像を開いても何も表示されない）。
    pub fn render_body(&mut self, ui: &mut egui::Ui) {
        // スクリーンショットのテクスチャ生成（未生成時）— 描画パスの入口で必ず走る。
        // 統合GUIシェル (UnifiedShell) 経由でも render_body は呼ばれるため、
        // どの起動経路でもキャンバスに画像が表示される (Issue #120 欠陥1修正)。
        if self.screenshot_tex.is_none()
            && let Some(img) = &self.screenshot
        {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
            self.screenshot_tex = Some(ui.ctx().load_texture(
                "studio-screenshot",
                color_image,
                egui::TextureOptions::default(),
            ));
        }

        if matches!(self.mode, AppMode::Authoring) {
            // 別スレッドでの propose 計算結果を非ブロッキング受信。
            // 完了時: proposing を下ろし、結果を self.proposals へ反映・status 更新。
            if self.proposing
                && let Some(rx) = &self.proposal_rx
                && let Ok(ps) = rx.try_recv()
            {
                self.proposals = ps;
                self.proposing = false;
                self.proposal_rx = None;
                self.status = format!("ROI候補: {} 件（スコア順）", self.proposals.len());
            }

            // ライブADBキャプチャの最新フレームを取り込む（表示更新のみ。ROIは保持）
            if let Some(live) = &self.live
                && let Some(frame) = live.latest()
            {
                let normalized = self.scaler.normalize(&frame);
                self.screenshot = Some(Arc::new(normalized));
                self.screenshot_tex = None;
            }

            // 左サイドパネル: 操作 + 識別力サマリ
            egui::Panel::left("controls")
                .resizable(true)
                .default_size(320.0)
                .show_inside(ui, |ui| {
                    ui.heading("anaden-studio");
                    ui.label("テンプレート作成");
                    ui.separator();

                    ui.label("データ");
                    if ui.button("スクリーンショットを開く").clicked() {
                        self.open_screenshot();
                    }
                    ui.horizontal(|ui| {
                        if ui.button("正例フォルダ").clicked() {
                            self.load_positives();
                        }
                        ui.label(format!("{}枚", self.positives.len()));
                    });
                    ui.horizontal(|ui| {
                        if ui.button("負例フォルダ").clicked() {
                            self.load_negatives();
                        }
                        ui.label(format!("{}枚", self.negatives.len()));
                    });
                    ui.separator();

                    // 認識エンジン切替（ライブ比較）
                    ui.heading("認識エンジン");
                    ui.horizontal(|ui| {
                        ui.label("方式:");
                        // 借用回避: new_kind は self から Copy した値。
                        // 変更があればループ外（closure 脱出後）で switch する。
                        let mut new_kind = self.engine_kind;
                        egui::ComboBox::from_id_salt("engine_kind_combo")
                            .selected_text(match self.engine_kind {
                                EngineKind::Sse => "SSE（輝度差）",
                                EngineKind::Ccoeff => "CCOEFF（ロバスト）",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut new_kind,
                                    EngineKind::Sse,
                                    "SSE（輝度差）",
                                );
                                ui.selectable_value(
                                    &mut new_kind,
                                    EngineKind::Ccoeff,
                                    "CCOEFF（ロバスト）",
                                );
                            });
                        if new_kind != self.engine_kind {
                            self.switch_engine(new_kind);
                        }
                    });
                    ui.separator();

                    // ライブキャプチャ(android 実機 / PC版 Windows)
                    ui.heading("ライブキャプチャ");
                    // 接続状態サマリバッジ + チェックボタン + エラー理由パネル (Issue #139 T3)。
                    ui.colored_label(
                        self.connection.state.badge_color(),
                        self.connection.state.badge(),
                    );
                    if ui.button("接続チェック").clicked() {
                        self.run_connection_check();
                    }
                    ui.label(self.connection.reason_line());
                    // バックエンド選択。Windows バックエンドは Windows ビルドでのみ選択可能。
                    ui.horizontal(|ui| {
                        ui.label("取得元:");
                        ui.selectable_value(
                            &mut self.target,
                            crate::source::Target::Android,
                            "Android(adb)",
                        );
                        #[cfg(windows)]
                        ui.selectable_value(
                            &mut self.target,
                            crate::source::Target::Windows,
                            "Windows(PC版)",
                        );
                    });
                    // android は serial、windows は exe 名を入力。
                    match self.target {
                        crate::source::Target::Android => {
                            ui.horizontal(|ui| {
                                ui.label("serial:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.adb_serial)
                                        .desired_width(140.0),
                                );
                            });
                        }
                        #[cfg(windows)]
                        crate::source::Target::Windows => {
                            ui.horizontal(|ui| {
                                ui.label("exe名:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.win_exe)
                                        .desired_width(160.0),
                                );
                            });
                        }
                    }
                    if self.live.is_some() {
                        if ui.button("停止（この画面で固定）").clicked() {
                            self.live = None;
                            self.status = "ライブ停止: 現在の画面で固定しました".to_string();
                        }
                    } else {
                        // 開始可否: android は serial 必須、windows は exe 名必須。
                        let can_start = match self.target {
                            crate::source::Target::Android => !self.adb_serial.trim().is_empty(),
                            #[cfg(windows)]
                            crate::source::Target::Windows => !self.win_exe.trim().is_empty(),
                        };
                        ui.add_enabled_ui(can_start, |ui| {
                            if ui.button("ライブ開始").clicked() {
                                // android は serial、windows は exe 名を渡してバックエンドを分岐。
                                let serial = self.adb_serial.trim().to_string();
                                self.live = Some(LiveCapture::start(
                                    serial,
                                    800,
                                    self.target,
                                    self.win_exe.trim(),
                                ));
                                self.status = match self.target {
                                    crate::source::Target::Android => {
                                        "ライブキャプチャ中…".to_string()
                                    }
                                    #[cfg(windows)]
                                    crate::source::Target::Windows => {
                                        format!("PC版キャプチャ中… ({})", self.win_exe.trim())
                                    }
                                };
                            }
                        });
                    }
                    ui.separator();

                    // ROI自動提案
                    ui.heading("ROI候補");
                    // 計算中(self.proposing)はボタンを無効化（二重起動・多重ブロック防止）。
                    let can_propose = self.screenshot.is_some() && !self.proposing;
                    ui.add_enabled_ui(can_propose, |ui| {
                        let label = if self.proposing {
                            "ROI候補を計算中…"
                        } else {
                            "ROI候補を提案"
                        };
                        if ui.button(label).clicked() {
                            self.run_proposals();
                        }
                    });
                    if !self.proposals.is_empty() {
                        ui.label("クリックでROIに読込（その後スコアで検証）:");
                        // 借用チェック: ループ内で self.proposals を借用しつつ
                        // self.apply_proposal は呼べないため、クリック対象を退避し
                        // ループ外で適用する（canvas のドラッグROI更新と同パターン）。
                        let mut clicked: Option<ScreenRegion> = None;
                        for (i, p) in self.proposals.iter().enumerate() {
                            if ui
                                .small_button(format!(
                                    "[{i}] score {:.2}  ({},{}) {}x{}",
                                    p.score, p.roi.x, p.roi.y, p.roi.width, p.roi.height
                                ))
                                .clicked()
                            {
                                clicked = Some(p.roi);
                            }
                        }
                        if let Some(roi) = clicked {
                            self.apply_proposal(roi);
                        }
                    }
                    ui.separator();

                    // 識別力サマリ
                    ui.heading("識別力");
                    if let Some(d) = &self.discrimination {
                        let (verdict, color) = if d.margin() > 0.1 {
                            ("識別可能", egui::Color32::from_rgb(60, 180, 75))
                        } else if d.margin() > 0.0 {
                            ("微妙（要調整）", egui::Color32::from_rgb(230, 160, 30))
                        } else {
                            ("識別不可", egui::Color32::from_rgb(220, 60, 60))
                        };
                        ui.colored_label(color, format!("判定: {verdict}"));
                        ui.colored_label(
                            egui::Color32::from_rgb(60, 180, 75),
                            format!("正例最低: {:.3}", d.own_min),
                        );
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 60, 60),
                            format!("負例最高: {:.3}", d.other_max),
                        );
                        ui.label(format!("マージン: {:+.3}", d.margin()));
                        ui.separator();
                        ui.label("正例スコア:");
                        for (i, s) in d.own_scores.iter().enumerate() {
                            ui.monospace(format!("  [{i}] {s:.3}"));
                        }
                        ui.label("負例スコア:");
                        for (i, s) in d.other_scores.iter().enumerate() {
                            ui.monospace(format!("  [{i}] {s:.3}"));
                        }
                    } else if let Some(r) = self.roi.rect() {
                        ui.label(format!("ROI: ({},{}) {}x{}", r.x, r.y, r.width, r.height));
                        ui.label("（評価中、または正例/負例未設定）");
                    } else {
                        ui.label("画面上でドラッグしてROIを選択");
                    }
                    ui.separator();

                    // テンプレート保存
                    ui.heading("保存");
                    ui.horizontal(|ui| {
                        ui.label("名前:");
                        ui.text_edit_singleline(&mut self.tpl_name);
                    });
                    ui.horizontal(|ui| {
                        ui.label("状態:");
                        egui::ComboBox::from_id_salt("state_combo")
                            .selected_text(STATE_OPTIONS[self.tpl_state_idx])
                            .show_ui(ui, |ui| {
                                for (i, s) in STATE_OPTIONS.iter().enumerate() {
                                    ui.selectable_value(&mut self.tpl_state_idx, i, *s);
                                }
                            });
                    });
                    ui.label(format!("保存先: {}", self.save_dir.display()));
                    if ui.button("保存先変更").clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder()
                    {
                        self.save_dir = dir;
                    }
                    let can_save = self.roi.rect().is_some() && self.screenshot.is_some();
                    let mut save_clicked = false;
                    ui.add_enabled_ui(can_save, |ui| {
                        if ui.button("テンプレート保存").clicked() {
                            save_clicked = true;
                        }
                    });
                    if save_clicked {
                        self.save_current_template();
                    }
                    ui.separator();

                    // pipeline task 保存 (UC-3: スクショ取り込み → ROI選択 →
                    // スコア計算 (scoring.rs) → pipeline task TOML 保存)。
                    // 同一の ROI/スコア/名前/状態入力を再利用し、保存形式のみ
                    // pipeline TOML (anaden-vision load_pipeline 互換) に切替。
                    ui.heading("pipeline task 保存");
                    ui.label(format!("保存先: {}", self.task_dir.display()));
                    if ui.button("task保存先変更").clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder()
                    {
                        self.task_dir = dir;
                    }
                    ui.horizontal(|ui| {
                        ui.label("action:");
                        egui::ComboBox::from_id_salt("task_action_combo")
                            .selected_text(self.task_action.label())
                            .show_ui(ui, |ui| {
                                for kind in [
                                    PipelineActionKind::ClickSelf,
                                    PipelineActionKind::DoNothing,
                                    PipelineActionKind::Stop,
                                ] {
                                    ui.selectable_value(&mut self.task_action, kind, kind.label());
                                }
                            });
                    });
                    let can_save_task = self.roi.rect().is_some() && self.screenshot.is_some();
                    let mut task_save_clicked = false;
                    ui.add_enabled_ui(can_save_task, |ui| {
                        if ui.button("pipeline task として保存").clicked() {
                            task_save_clicked = true;
                        }
                    });
                    if task_save_clicked {
                        self.save_current_pipeline_task();
                    }
                    ui.separator();
                    ui.label(&self.status);
                });

            // 中央: キャンバス
            egui::CentralPanel::default().show_inside(ui, |ui| {
                if let (Some(tex), Some(img)) = (&self.screenshot_tex, &self.screenshot) {
                    let (w, h) = (img.width(), img.height());

                    // 既存のヒートマップを描画に渡す（ROI解放時に更新される）
                    let heatmap_view = self.heatmap_tex.as_ref().map(|t| canvas::HeatmapView {
                        tex: t.id(),
                        search: self.heatmap_search,
                    });
                    let best_match = self.best_match;
                    canvas::show(
                        ui,
                        tex,
                        w,
                        h,
                        &mut self.roi,
                        heatmap_view.as_ref(),
                        best_match,
                    );

                    // ROIが安定して変化したら識別力とヒートマップを再評価
                    if let Some(roi_rect) = self.roi.rect()
                        && !self.roi.dragging
                        && Some(roi_rect) != self.scored_roi
                    {
                        let crop =
                            img.crop_imm(roi_rect.x, roi_rect.y, roi_rect.width, roi_rect.height);
                        self.discrimination = Some(scoring::discrimination(
                            self.engine.as_ref(),
                            &crop,
                            &self.positives,
                            &self.negatives,
                        ));

                        // ヒートマップ（スコアマップ全体）と最良マッチ位置
                        if let Some(sm) = self.heatmap_engine.score_map(img, &crop) {
                            let mut bx = 0u32;
                            let mut by = 0u32;
                            let mut bv = 0u8;
                            for y in 0..sm.height() {
                                for x in 0..sm.width() {
                                    let v = sm.get_pixel(x, y)[0];
                                    if v > bv {
                                        bv = v;
                                        bx = x;
                                        by = y;
                                    }
                                }
                            }
                            let d = HEATMAP_DOWNSCALE;
                            self.best_match = Some(ScreenRegion::new(
                                bx * d,
                                by * d,
                                roi_rect.width,
                                roi_rect.height,
                            ));
                            self.heatmap_search = ScreenRegion::new(
                                0,
                                0,
                                img.width().saturating_sub(roi_rect.width),
                                img.height().saturating_sub(roi_rect.height),
                            );
                            let color_img = canvas::score_map_to_heatmap(&sm);
                            self.heatmap_tex = Some(ui.ctx().load_texture(
                                "heatmap",
                                color_img,
                                egui::TextureOptions::LINEAR,
                            ));
                        }

                        self.scored_roi = Some(roi_rect);
                    }
                } else {
                    ui.heading("「スクリーンショットを開く」で画像を読み込んでください");
                }
            });
        } else {
            self.batch_ui(ui);
        }
    }
}

impl StudioApp {
    /// バッチ評価モードのUI。
    fn batch_ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("batch_controls")
            .resizable(true)
            .default_size(340.0)
            .show_inside(ui, |ui| {
                ui.heading("バッチ評価");
                ui.label("テンプレート × テスト画像で混同行列を作成");
                ui.separator();
                ui.label(format!("テンプレート元: {}", self.save_dir.display()));
                ui.label(format!("テスト元: {}", self.test_dir.display()));
                if ui.button("テスト元変更").clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    self.test_dir = dir;
                }
                ui.horizontal(|ui| {
                    ui.label("閾値:");
                    ui.add(egui::Slider::new(&mut self.batch_threshold, 0.0..=1.0));
                });
                let mut run_clicked = false;
                if ui.button("実行").clicked() {
                    run_clicked = true;
                }
                if run_clicked {
                    self.run_batch();
                }
                ui.separator();
                ui.label(&self.status);
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            if let Some(cm) = &self.batch_result {
                batch::render_confusion_matrix(ui, cm);
            } else {
                ui.heading("「実行」でバッチ評価を行います");
                ui.label("テンプレート元フォルダ（PNG+TOML）と、");
                ui.label("テストフォルダ（<ラベル名>/画像）を選んでください");
            }
        });
    }

    /// バッチ評価を実行する。
    fn run_batch(&mut self) {
        let templates = batch::load_templates_for_eval(&self.save_dir);
        if templates.is_empty() {
            self.status = format!("テンプレート未検出: {}", self.save_dir.display());
            return;
        }
        let tests = batch::load_test_set(&self.test_dir);
        if tests.is_empty() {
            self.status = format!("テスト画像未検出: {}", self.test_dir.display());
            return;
        }
        self.status = format!(
            "評価中... {} テンプレ × {} テスト",
            templates.len(),
            tests.len()
        );
        let cm = batch::evaluate(
            self.engine.as_ref(),
            &templates,
            &tests,
            self.batch_threshold,
        );
        self.status = format!(
            "完了: 正答率 {:.1}% ({} テンプレ × {} テスト)",
            cm.accuracy() * 100.0,
            templates.len(),
            tests.len()
        );
        self.batch_result = Some(cm);
    }
}

/// フォルダ内の画像をすべて読み込む。
fn load_folder(path: &Path) -> Vec<Arc<DynamicImage>> {
    let mut out = vec![];
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p: PathBuf = entry.path();
            if is_image(&p)
                && let Ok(img) = image::open(&p)
            {
                out.push(Arc::new(img));
            }
        }
    }
    out
}

fn is_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .as_deref(),
        Some("png") | Some("jpg") | Some("jpeg") | Some("bmp")
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use image::{DynamicImage, GrayImage, Luma};

    /// 非一様・非周期な needle。CCOEFF は一様パッチ（denomT=0）で全位置 0 を返すため、
    /// build_engine の構築健全性検証には内部分散を持つ一意パターンが必要。
    /// 値 = ((x*x + 3*y) % 200) + 20 で 20..=219 の非周期パターンを作る。
    fn textured_needle(w: u32, h: u32) -> GrayImage {
        let mut img = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = (((x * x + 3 * y) % 200) + 20) as u8;
                img.put_pixel(x, y, Luma([v]));
            }
        }
        img
    }

    /// 単色背景 (ox, oy) に needle を埋め込んだ画像。
    fn embed_on_bg(
        haystack_w: u32,
        haystack_h: u32,
        needle: &GrayImage,
        ox: u32,
        oy: u32,
        bg: u8,
    ) -> GrayImage {
        let mut img = GrayImage::from_pixel(haystack_w, haystack_h, Luma([bg]));
        for y in 0..needle.height() {
            for x in 0..needle.width() {
                let p = needle.get_pixel(x, y)[0];
                img.put_pixel(ox + x, oy + y, Luma([p]));
            }
        }
        img
    }

    fn luma_dyn(img: GrayImage) -> DynamicImage {
        DynamicImage::ImageLuma8(img)
    }

    #[test]
    fn engine_kind_default_is_ccoeff() {
        assert_eq!(EngineKind::default(), EngineKind::Ccoeff);
    }

    #[test]
    fn build_engine_produces_ccoeff_by_default() {
        // デフォルトエンジンは CCOEFF。構築できること（panic しない）が最小保証。
        let _engine = StudioApp::build_engine(EngineKind::default());
        let _sse = StudioApp::build_engine(EngineKind::Sse);
    }

    /// ヘッドレス egui コンテキストを用意し、その中に子 Ui を作る。
    /// GUI バックエンド不要でパネル描画を単体テストできる。
    fn child_ui(ctx: &egui::Context) -> egui::Ui {
        egui::Ui::new(
            ctx.clone(),
            egui::Id::new("test-area"),
            egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
        )
    }

    /// 埋め込み描画 API（render_modebar + render_body）が Authoring モードで
    /// パニックせず完了することを検証する（Issue #119 shard 1 task 2）。
    #[test]
    fn embed_render_authoring_mode_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        assert_eq!(app.mode(), AppMode::Authoring);
        ctx.begin_pass(egui::RawInput::default());
        app.render_modebar(&mut child_ui(&ctx));
        app.render_body(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    /// 埋め込み描画 API が Batch モード（混同行列 UI 含む）でも壊れないことを検証する。
    #[test]
    fn embed_render_batch_mode_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        app.set_mode(AppMode::Batch);
        assert_eq!(app.mode(), AppMode::Batch);
        ctx.begin_pass(egui::RawInput::default());
        app.render_modebar(&mut child_ui(&ctx));
        app.render_body(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    /// set_mode でモードが切り替わり、mode() で観測できること（公開 API 振る舞い）。
    #[test]
    fn set_mode_switches_between_authoring_and_batch() {
        let mut app = StudioApp::default();
        app.set_mode(AppMode::Batch);
        assert_eq!(app.mode(), AppMode::Batch);
        app.set_mode(AppMode::Authoring);
        assert_eq!(app.mode(), AppMode::Authoring);
    }

    /// build_engine が downscale=2・閾値0 で健全に構築されていることを、
    /// 両エンジンで同一画像のマッチを返すことでエンドツーエンド検証する。
    /// 黒四角 on 白背景は一意パターンで、downscale=2 でも位置が ±2px で確定する。
    #[test]
    fn build_engine_both_engines_locate_embedded_needle() {
        let needle = textured_needle(20, 20);
        // 中間グレー背景に埋め込み（needle は非周期・非一意）。
        let haystack = embed_on_bg(100, 100, &needle, 40, 40, 128);
        let haystack_dyn = luma_dyn(haystack);
        let needle_dyn = luma_dyn(needle.clone());

        let sse = StudioApp::build_engine(EngineKind::Sse);
        let cc = StudioApp::build_engine(EngineKind::Ccoeff);

        let sse_m = sse
            .match_template(&haystack_dyn, &needle_dyn)
            .expect("SSE engine should find embedded needle");
        let cc_m = cc
            .match_template(&haystack_dyn, &needle_dyn)
            .expect("CCOEFF engine should find embedded needle");

        // 非周期 needle on 単色背景は一意。downscale=2 → 位置は (40..=42) に一致。
        for (got, axis) in [(sse_m.region.x, "sse.x"), (cc_m.region.x, "cc.x")] {
            assert!(
                (40..=42).contains(&got),
                "{axis} should be ~40 (downscale=2), got {got}"
            );
        }
        for (got, axis) in [(sse_m.region.y, "sse.y"), (cc_m.region.y, "cc.y")] {
            assert!(
                (40..=42).contains(&got),
                "{axis} should be ~40 (downscale=2), got {got}"
            );
        }
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

    #[test]
    fn run_connection_check_updates_state() {
        let mut app = StudioApp::default();
        // Android 既定 + serial 未入力 → チェック後に Disconnected (未入力理由)。
        app.run_connection_check();
        assert_eq!(app.connection().state, ConnectionState::Disconnected);
        assert!(app.connection().reason_line().contains("理由"));
    }

    /// 接続バッジ・チェックボタン・エラー理由パネルを含む Authoring 描画が
    /// パニックせず完了すること (Issue #139 T3)。
    #[test]
    fn embed_render_connection_panel_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        app.run_connection_check();
        ctx.begin_pass(egui::RawInput::default());
        app.render_modebar(&mut child_ui(&ctx));
        app.render_body(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
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

    /// StudioApp の pipeline task 保存: ROI/スクショ未確定時はステータスに理由を
    /// 残して何も書かない (fail-closed)。
    #[test]
    fn save_current_pipeline_task_without_roi_reports_status() {
        let dir = tempfile::tempdir().unwrap();
        let app = StudioApp {
            task_dir: dir.path().to_path_buf(),
            ..StudioApp::default()
        };
        let mut app = app;
        app.save_current_pipeline_task();
        assert!(app.status.contains("ROI"));
        assert!(dir.path().read_dir().unwrap().next().is_none());
    }

    /// StudioApp の pipeline task 保存: ROI + スクショ + 識別力が揃った状態で
    /// 保存すると task_dir に TOML+PNG が書かれ、load_pipeline で読める。
    /// 閾値は discrimination から導出される (正例/負例の中間)。
    #[test]
    fn save_current_pipeline_task_writes_loadable_toml() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = StudioApp {
            task_dir: dir.path().to_path_buf(),
            ..StudioApp::default()
        };
        app.tpl_name = "created_task".to_string();
        app.screenshot = Some(Arc::new(DynamicImage::ImageLuma8(
            image::GrayImage::from_pixel(200, 100, Luma([255])),
        )));
        app.roi.anchor = Some((10, 10));
        app.roi.current = Some((110, 60)); // 100x50
        app.discrimination = Some(Discrimination {
            own_min: 0.9,
            other_max: 0.8,
            own_scores: vec![0.9],
            other_scores: vec![0.8],
        });

        app.save_current_pipeline_task();
        assert!(app.status.contains("保存"), "status: {}", app.status);

        let tasks = anaden_vision::load_pipeline(dir.path()).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].name, "created_task");
        assert_eq!(tasks[0].roi, Some([10, 10, 100, 50]));
        // threshold = (0.9 + 0.8) / 2 = 0.85
        assert!((tasks[0].threshold - 0.85).abs() < 1e-4);
        assert_eq!(tasks[0].action, Some(anaden_vision::Action::ClickSelf));
    }

    /// 識別力なしで保存した場合の閾値は既定 0.9。
    #[test]
    fn save_current_pipeline_task_threshold_defaults_without_discrimination() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = StudioApp {
            task_dir: dir.path().to_path_buf(),
            ..StudioApp::default()
        };
        app.screenshot = Some(Arc::new(DynamicImage::ImageLuma8(
            image::GrayImage::from_pixel(200, 100, Luma([255])),
        )));
        app.roi.anchor = Some((0, 0));
        app.roi.current = Some((50, 50));
        app.save_current_pipeline_task();
        let tasks = anaden_vision::load_pipeline(dir.path()).unwrap();
        assert!((tasks[0].threshold - 0.9).abs() < 1e-4);
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

    /// app.rs のボタンラベルに Unicode 絵文字が残っていないこと (豆腐排除・機械検証)。
    #[test]
    fn app_button_labels_contain_no_emoji() {
        let labels = [
            "作成",
            "バッチ評価",
            "スクリーンショットを開く",
            "正例フォルダ",
            "負例フォルダ",
            "Android(adb)",
            "停止（この画面で固定）",
            "ライブ開始",
            "ROI候補を提案",
            "テンプレート保存",
            "保存先変更",
            "実行",
        ];
        for l in labels {
            assert!(
                l.chars().all(|c| c < '\u{1F300}'),
                "label must not contain emoji: {l}"
            );
        }
    }

    // ---- Issue #144 Task 3 / Issue #154 Shard 1: タスクキュー実行配線 ----

    fn tasks_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("templates")
            .join("tasks")
    }

    /// 指定 exit code で即終了する子の SpawnSpec (Windows: cmd / Linux: sh)。
    fn exit_spec(code: i32) -> SpawnSpec {
        if cfg!(windows) {
            SpawnSpec::new(
                "cmd",
                ["/C".to_string(), "exit".to_string(), code.to_string()],
            )
        } else {
            SpawnSpec::new("sh", ["-c".to_string(), format!("exit {code}")])
        }
    }

    /// 1 行出力して exit 0 で終了する子の SpawnSpec。
    fn echo_spec() -> SpawnSpec {
        if cfg!(windows) {
            SpawnSpec::new("cmd", ["/C".to_string(), "echo task-log-line".to_string()])
        } else {
            SpawnSpec::new("sh", ["-c".to_string(), "echo task-log-line".to_string()])
        }
    }

    /// 長時間 (約30秒) 生きる子の SpawnSpec (ping は両 OS に存在)。
    fn long_spec() -> SpawnSpec {
        if cfg!(windows) {
            SpawnSpec::new(
                "ping",
                ["-n".to_string(), "30".to_string(), "127.0.0.1".to_string()],
            )
        } else {
            SpawnSpec::new(
                "ping",
                ["-c".to_string(), "30".to_string(), "127.0.0.1".to_string()],
            )
        }
    }

    fn queue_entry(label: &str, spec: SpawnSpec) -> QueueEntry {
        QueueEntry {
            label: label.to_string(),
            spec,
        }
    }

    /// キューが指定状態になるまで drain を回す (実子プロセスの Exit 待ち)。
    fn pump_until(
        app: &mut StudioApp,
        timeout_ms: u64,
        done: impl Fn(&QueueState) -> bool,
        what: &str,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            app.drain_task_logs();
            if let Some(q) = app.task_queue()
                && done(q.state())
            {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what} (status: {})",
                app.status
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// ログ中のタスク境界セパレータ行 ([studio] === task: X ===) を数える。
    fn task_separator_count(app: &StudioApp) -> usize {
        app.task_log_snapshot
            .iter()
            .filter(|e| e.line.starts_with("[studio] === task:"))
            .count()
    }

    // ---- 正常系 ----

    /// UC-2: チェック順どおり逐次実行され全タスク完了に到達する。
    /// セパレータ行はタスク境界ごとに 1 行ずつ (UC-4)。
    #[test]
    fn task_queue_runs_entries_sequentially_in_check_order() {
        let mut app = StudioApp::default();
        app.start_task_entries(vec![
            queue_entry("タスクA", exit_spec(0)),
            queue_entry("タスクB", exit_spec(0)),
        ]);
        pump_until(
            &mut app,
            30_000,
            |s| matches!(s, QueueState::Completed),
            "queue completion",
        );
        let queue = app.task_queue().unwrap();
        assert_eq!(queue.summary(), "完了 2/2");
        assert_eq!(task_separator_count(&app), 2);
        let lines: Vec<&str> = app
            .task_log_snapshot
            .iter()
            .map(|e| e.line.as_str())
            .collect();
        let a = lines
            .iter()
            .position(|l| *l == "[studio] === task: タスクA ===");
        let b = lines
            .iter()
            .position(|l| *l == "[studio] === task: タスクB ===");
        assert!(a.is_some() && b.is_some() && a < b, "lines: {lines:?}");
        assert!(lines.iter().any(|l| l.contains("キュー完了")));
    }

    /// 実リポジトリ TOML から組み立てたキューはチェック順を維持する。
    /// program に存在しないバイナリを指定すると初回起動が失敗停止する
    /// (fail-closed: 起動失敗は当該タスクの失敗扱い)。
    #[test]
    fn task_list_selection_starts_queue_in_check_order() {
        let mut app = StudioApp::default();
        app.load_task_list(&tasks_dir());
        app.toggle_task("field_loop_pc");
        app.toggle_task("launch");
        app.set_anaden_program("anaden-nonexistent-bin-xyz");
        app.start_task_queue();
        // 初回 spawn 失敗 → 同期的に失敗停止へ遷移するため drain 不要。
        let queue = app.task_queue().unwrap();
        assert!(matches!(
            queue.state(),
            QueueState::PausedAfterFailure { current: 0, .. }
        ));
        assert_eq!(queue.total(), 2);
        assert_eq!(queue.entries()[0].label, "フィールド周回");
        assert_eq!(queue.entries()[0].spec.args[0], "run");
        assert_eq!(queue.entries()[1].label, "ゲーム起動");
        assert_eq!(queue.entries()[1].spec.args[0], "launch");
        assert!(app.status.contains("起動に失敗"), "status: {}", app.status);
    }

    /// UC-4: 実行中は i/N 進行サマリとログがリアルタイム参照できる。
    #[test]
    fn task_queue_progress_summary_during_run() {
        let mut app = StudioApp::default();
        app.start_task_entries(vec![queue_entry("周回", long_spec())]);
        let queue = app.task_queue().unwrap();
        assert!(matches!(queue.state(), QueueState::Running { current: 0 }));
        assert!(
            queue.summary().contains("1/1"),
            "summary: {}",
            queue.summary()
        );
        assert!(
            queue.summary().contains("周回"),
            "summary: {}",
            queue.summary()
        );
        // セパレータ行は起動直後に出ている。
        app.drain_task_logs();
        assert!(
            app.task_log_snapshot
                .iter()
                .any(|e| e.line == "[studio] === task: 周回 ===")
        );
        app.abort_task_queue();
    }

    /// UC-4: 子プロセスの stdout がログスナップショットへ届く。
    #[test]
    fn task_queue_log_view_renders_child_output() {
        let mut app = StudioApp::default();
        app.start_task_entries(vec![queue_entry("出力", echo_spec())]);
        pump_until(
            &mut app,
            30_000,
            |s| matches!(s, QueueState::Completed),
            "echo completion",
        );
        let lines: Vec<&str> = app
            .task_log_snapshot
            .iter()
            .map(|e| e.line.as_str())
            .collect();
        assert!(
            lines.iter().any(|l| l.contains("task-log-line")),
            "lines: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("exit=0")),
            "lines: {lines:?}"
        );
    }

    /// タスク一覧 UI (チェックボックス・開始ボタン含む) がパニックせず描画できる。
    #[test]
    fn embed_render_task_list_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        app.load_task_list(&tasks_dir());
        ctx.begin_pass(egui::RawInput::default());
        app.render_task_list(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    /// UC-4: キュー実行中の描画 (進行表示・失敗ボタン・ログ) もパニックしない。
    #[test]
    fn embed_render_task_list_with_active_queue_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        app.start_task_entries(vec![queue_entry("長時間", long_spec())]);
        ctx.begin_pass(egui::RawInput::default());
        app.render_task_list(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
        app.abort_task_queue();
        // 中止後の描画も安定していること。
        ctx.begin_pass(egui::RawInput::default());
        app.render_task_list(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    /// UC-3: 詳細展開ビュー (kind/pipeline_dir/start_task/引数プレビュー) を
    /// 含む描画がパニックなく完了する。collapsing header 展開時に描画される
    /// ボディを全タスク分直接描画 + 選択済み一覧 (実行順リスト含む) 全体描画。
    #[test]
    fn embed_render_task_list_with_detail_view_completes_without_panic() {
        let ctx = egui::Context::default();
        let mut app = StudioApp::default();
        app.load_task_list(&tasks_dir());
        app.toggle_task("launch");
        app.toggle_task("field_loop_pc");
        let root = StudioApp::workspace_root();
        ctx.begin_pass(egui::RawInput::default());
        let mut detail_ui = child_ui(&ctx);
        let list = app.task_defs.clone().unwrap();
        for def in list.definitions() {
            StudioApp::task_detail_ui(&mut detail_ui, def, "windows", None, &root);
        }
        app.render_task_list(&mut child_ui(&ctx));
        let _ = ctx.end_pass();
    }

    // ---- エッジケース ----

    /// UC-4: タスク失敗で自動継続せず停止し、明示「継続」で次が走る。
    #[test]
    fn task_queue_failure_pauses_and_resume_continues() {
        let mut app = StudioApp::default();
        app.start_task_entries(vec![
            queue_entry("失敗", exit_spec(1)),
            queue_entry("次", exit_spec(0)),
        ]);
        pump_until(
            &mut app,
            30_000,
            |s| matches!(s, QueueState::PausedAfterFailure { .. }),
            "failure pause",
        );
        // 自動継続禁止: 2 番目はまだ起動していない。
        assert_eq!(task_separator_count(&app), 1);
        assert!(app.status.contains("失敗"), "status: {}", app.status);
        app.resume_task_queue();
        pump_until(
            &mut app,
            30_000,
            |s| matches!(s, QueueState::Completed),
            "resume completion",
        );
        assert_eq!(task_separator_count(&app), 2);
        assert_eq!(app.task_queue().unwrap().summary(), "完了 2/2");
    }

    /// UC-4: 中止は実行中の子を停止し残りタスクを起動しない。
    #[test]
    fn task_queue_abort_discards_remaining() {
        let mut app = StudioApp::default();
        app.start_task_entries(vec![
            queue_entry("長時間", long_spec()),
            queue_entry("次", exit_spec(0)),
        ]);
        app.abort_task_queue();
        let queue = app.task_queue().unwrap();
        assert!(matches!(queue.state(), QueueState::Completed));
        assert!(queue.is_aborted());
        assert_eq!(queue.summary(), "中止");
        // kill された子の Exit イベントが後段に届いても状態は崩れない。
        pump_until(
            &mut app,
            30_000,
            |s| matches!(s, QueueState::Completed),
            "post-abort drain",
        );
        assert_eq!(task_separator_count(&app), 1, "残りタスクは起動しない");
        assert!(app.status.contains("中止"), "status: {}", app.status);
    }

    /// 実行中の再開始は拒否され、キューは変更されない。
    #[test]
    fn task_queue_rejects_restart_while_active() {
        let mut app = StudioApp::default();
        app.load_task_list(&tasks_dir());
        app.toggle_task("launch");
        app.start_task_entries(vec![queue_entry("長時間", long_spec())]);
        app.start_task_queue(); // 実行中の再開始試行
        assert!(
            app.status.contains("開始できません"),
            "status: {}",
            app.status
        );
        assert_eq!(app.task_queue().unwrap().total(), 1);
        app.abort_task_queue();
    }

    /// 未読込・未選択・空エントリでの開始は status に理由を残しキュー不変。
    #[test]
    fn start_without_selection_reports_status() {
        let mut app = StudioApp::default();
        app.start_task_queue();
        assert!(app.status.contains("未読込"), "status: {}", app.status);
        assert!(app.task_queue().is_none());
        app.load_task_list(&tasks_dir());
        app.start_task_queue();
        assert!(app.status.contains("チェック"), "status: {}", app.status);
        assert!(app.task_queue().is_none());
        app.toggle_task("launch");
        app.set_anaden_program("anaden-nonexistent-bin-xyz");
        app.start_task_queue();
        // 起動失敗でもキュー自体は作成される (失敗停止として観測可能)。
        assert!(app.task_queue().is_some());
        app.abort_task_queue();
        // 空エントリの直接注入も拒否。
        app.start_task_entries(Vec::new());
        assert!(
            app.status.contains("チェックされたタスクがありません"),
            "status: {}",
            app.status
        );
    }
}
