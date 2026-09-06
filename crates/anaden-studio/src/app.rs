//! StudioApp: 状態操作 impl と app_state / app_ui への facade (Issue #162 Shard 1)。
//!
//! - app_state.rs: 状態型群 (StudioApp 構造体・接続状態・pipeline task 保存・定数)
//! - app_ui.rs: UI 描画 impl (render_* / task_log_ui / eframe::App)
//! - app.rs (本ファイル): 非UI 状態操作 impl (キュー実行・ファイル入出力・
//!   エンジン切替) と、呼び出し元互換の re-export。
//!
//! 呼び出し元 (shell.rs / tests/) は従来どおり `crate::app::{...}`
//! (`anaden_studio::app::{...}`) で参照できる。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;

use image::DynamicImage;

use anaden_core::{MatchConfidence, ScreenRegion};
use anaden_vision::{CcoeffVisionEngine, SseVisionEngine, TemplateMatcher, VisionEngine};

use crate::canvas::RoiEdit;
use crate::childproc::SpawnSpec;
use crate::library::{self, TemplateSpec};
use crate::log_view::{AutoScrollFollow, LogBuffer, LogEntry};
use crate::proposals::{self, Proposal};
use crate::tasks::{self, QueueAction, QueueEntry, QueueExec, QueueState};

pub use crate::app_state::{
    AppMode, ConnectionState, ConnectionStatus, PipelineActionKind, StudioApp,
    check_android_device, check_windows_process, pipeline_task_spec, save_pipeline_task,
};
use crate::app_state::{EngineKind, HEATMAP_DOWNSCALE, STATE_OPTIONS};

impl StudioApp {
    /// engine_kind から生スコア評価用エンジンを構築する（downscale=2, 閾値0）。
    /// 両エンジンで条件を統一し公平比較を保証する純関数。
    pub(crate) fn build_engine(kind: EngineKind) -> Box<dyn VisionEngine> {
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

    /// タスク実行ログのスナップショット (読み取り専用)。
    ///
    /// runner.rs の `log_snapshot` と同じ公開パターンで、ヘッドレス E2E テスト
    /// (`tests/task_queue_e2e_tests.rs`) がログの内容・順序を機械検証する経路。
    pub fn task_log_lines(&self) -> &[LogEntry] {
        &self.task_log_snapshot
    }

    /// キューがアクティブ (未完了 = Pending/Running/PausedAfterFailure) か。
    pub(crate) fn task_queue_active(&self) -> bool {
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

    /// UC-3 (Shard 4): シナリオパネルのイベントを処理する。
    ///
    /// タスク登録・有効化 (`TaskEnabled`) 後はホーム一覧 (`task_defs`) を
    /// 既定パスから再読込して有効化タスクを選択可能にする (4 段階フローの
    /// (d) queue 追加 = 既存ホーム一覧への反映)。再読込でチェック済み選択が
    /// 失われるため、保持して復元する (定義が消えた/未実装化した ID は除外)。
    pub(crate) fn on_scenario_task_event(
        &mut self,
        event: crate::scenario_task_link::ScenarioPanelEvent,
    ) {
        match event {
            crate::scenario_task_link::ScenarioPanelEvent::TaskEnabled { message } => {
                let selected: Vec<String> = self
                    .task_defs
                    .as_ref()
                    .map(|list| list.selected_ids().to_vec())
                    .unwrap_or_default();
                self.load_task_list(&Self::workspace_root().join("templates/tasks"));
                if let Some(list) = &mut self.task_defs {
                    for id in selected {
                        if list.find(&id).is_some_and(|def| def.is_selectable()) {
                            // 事前条件 (定義存在 + 選択可能) を満たすため失敗しない。
                            let _ = list.toggle(&id);
                        }
                    }
                }
                // load_task_list が status を上書きするため、成功メッセージを再設定。
                self.status = message;
            }
        }
    }

    /// workspace ルート (runner.rs と同一の決定論的解決)。
    pub(crate) fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
    }

    /// CLI target 文字列 (source::Target → anaden CLI の `--target` 値)。
    pub(crate) fn cli_target(&self) -> &'static str {
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

    /// UI 描画用ログスナップショットを最新化する (Issue #160 UC-5: 差分更新)。
    ///
    /// 従来は毎フレームバッファ全行 (上限 5000 行) を clone していた。
    /// 現在は [`SharedLogBuffer::changed_entries`] で改訂番号を比較し、
    /// バッファが変化したフレームのみ複製する (新着行なしのフレームは
    /// ロック 1 回 + 整数比較で完了)。内容の契約は不変 (更新後は全行相当)。
    pub(crate) fn refresh_task_log_snapshot(&mut self) {
        if let Some((rev, entries)) = self.task_log.changed_entries(self.task_log_revision) {
            self.task_log_revision = rev;
            self.task_log_snapshot = entries;
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
    pub(crate) fn switch_engine(&mut self, kind: EngineKind) {
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
    pub(crate) fn open_screenshot(&mut self) {
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
    pub(crate) fn load_positives(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            let imgs = load_folder(&dir);
            self.status = format!("正例: {} 枚読込", imgs.len());
            self.positives = imgs;
            self.scored_roi = None; // 再評価を強制
        }
    }

    /// 負例フォルダを読み込む。
    pub(crate) fn load_negatives(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            let imgs = load_folder(&dir);
            self.status = format!("負例: {} 枚読込", imgs.len());
            self.negatives = imgs;
            self.scored_roi = None;
        }
    }

    /// 現在のROI切り出しをテンプレートとして保存する。
    /// 閾値は識別力があれば正例/負例スコアの中間、なければ 0.9。
    pub(crate) fn save_current_template(&mut self) {
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
    pub(crate) fn run_proposals(&mut self) {
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
    pub(crate) fn apply_proposal(&mut self, roi: ScreenRegion) {
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
    pub(crate) fn save_current_pipeline_task(&mut self) {
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

    /// 現在のROI・入力からシナリオ追加候補 (TaskDef + crop PNG) を構築する
    /// (Issue #160 T3 / UC-1)。名前/状態/閾値/action は
    /// [`Self::save_current_pipeline_task`] と同一の導出を使う。
    /// スクショ/ROI 未確定・未知方式は None (fail-closed)。
    pub(crate) fn scenario_candidate(&self) -> Option<(anaden_vision::TaskDef, DynamicImage)> {
        let roi = self.roi.rect()?;
        let img = self.screenshot.as_ref()?;
        let name = if self.tpl_name.trim().is_empty() {
            "template_01".to_string()
        } else {
            self.tpl_name.trim().to_string()
        };
        let threshold = self
            .discrimination
            .as_ref()
            .map(|d| ((d.own_min + d.other_max) / 2.0).clamp(0.5, 0.99))
            .unwrap_or(0.9);
        let spec = pipeline_task_spec(
            &name,
            STATE_OPTIONS[self.tpl_state_idx],
            self.engine_kind.method_str(),
            roi,
            threshold,
            self.task_action,
        )?;
        let crop = img.crop_imm(roi.x, roi.y, roi.width, roi.height);
        Some((spec, crop))
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
    use crate::scoring::Discrimination;
    use image::{GrayImage, Luma};

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
    fn build_engine_produces_ccoeff_by_default() {
        // デフォルトエンジンは CCOEFF。構築できること（panic しない）が最小保証。
        let _engine = StudioApp::build_engine(EngineKind::default());
        let _sse = StudioApp::build_engine(EngineKind::Sse);
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

    #[test]
    fn run_connection_check_updates_state() {
        let mut app = StudioApp::default();
        // Android 既定 + serial 未入力 → チェック後に Disconnected (未入力理由)。
        app.run_connection_check();
        assert_eq!(app.connection().state, ConnectionState::Disconnected);
        assert!(app.connection().reason_line().contains("理由"));
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

    /// UC-5 (Issue #160): 差分更新スナップショット (revision-gated) でも
    /// 行が欠けず、新着行のないフレームを連続してもスナップショット内容が
    /// 冪等に保たれる (60 フレーム相当 = 1 秒 @60fps のアイドル drain)。
    #[test]
    fn task_log_snapshot_idempotent_across_idle_drains_and_keeps_lines() {
        let mut app = StudioApp::default();
        // 起動即失敗 (存在しない program を持つ spec) → セパレータ + 起動失敗行
        // を記録して失敗停止。以降はキュー滞留のまま毎フレーム drain が回る状態。
        let bogus = SpawnSpec::new("anaden-nonexistent-bin-xyz", Vec::new());
        app.start_task_entries(vec![queue_entry("失敗", bogus)]);
        pump_until(
            &mut app,
            30_000,
            |s| matches!(s, QueueState::PausedAfterFailure { .. }),
            "pause after spawn failure",
        );
        app.drain_task_logs();
        let before: Vec<String> = app
            .task_log_lines()
            .iter()
            .map(|e| e.line.clone())
            .collect();
        assert!(
            before.iter().any(|l| l.contains("起動に失敗")),
            "lines: {before:?}"
        );
        for _ in 0..60 {
            app.drain_task_logs();
        }
        let after: Vec<String> = app
            .task_log_lines()
            .iter()
            .map(|e| e.line.clone())
            .collect();
        assert_eq!(before, after);
    }

    /// UC-3 (Shard 4): タスク登録・有効化イベントでホーム一覧が再読込され、
    /// 有効化タスクが選択可能になる。既存のチェック選択は保持される。
    /// (実リポジトリ templates/tasks は読み取り専用に使用 — 書き込み無し)
    #[test]
    fn scenario_task_enabled_event_reloads_home_list_preserving_selection() {
        let mut app = StudioApp::default();
        app.load_task_list(&tasks_dir());
        app.toggle_task("login");
        let selected_before = app.task_defs.as_ref().unwrap().selected_ids().to_vec();
        assert_eq!(selected_before, vec!["login".to_string()]);

        app.on_scenario_task_event(crate::scenario_task_link::ScenarioPanelEvent::TaskEnabled {
            message: "タスク登録・有効化: テスト".to_string(),
        });

        let list = app.task_defs.as_ref().unwrap();
        assert!(
            list.find("login").is_some_and(|def| def.is_selectable()),
            "再読込後も有効化タスクが選択可能"
        );
        assert_eq!(
            list.selected_ids(),
            selected_before.as_slice(),
            "選択は復元"
        );
        assert_eq!(app.status, "タスク登録・有効化: テスト");
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
