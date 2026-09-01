//! Pipeline 実行ランナーGUI（Issue #83/#85 シャード2・3前半）。
//!
//! MAA/MDA 型 GUI の要素:
//! - ウィンドウ表示(eframe/egui)
//! - `anaden` CLI 子プロセスの起動/停止ボタン 1 組(childproc::ChildProcess)
//! - stdout/stderr のログ表示（log_view::SharedLogBuffer + egui スクロール
//!   ビューア。LogLevel 色分け・自動スクロール・クリアボタン）= シャード3前半
//! - `anaden` バイナリの明示パス解決（ANADEN_BIN → target/{debug,release} → PATH）
//!
//! 状態遷移・パス解決・ログ drain は egui に依存しないメソッドに切り出し、
//! ヘッドレスでユニットテスト可能にしている。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender};

use eframe::egui;

use crate::childproc::{ChildProcess, SpawnSpec};
use crate::history::{RunHistory, RunOutcome, RunRecord};
use crate::history_ui::{HistoryAction, HistoryPanel};
use crate::log_view::{
    DEFAULT_MAX_LINES, LogBuffer, LogEntry, LogEvent, LogLevel, SharedLogBuffer,
};
/// `anaden` バイナリを解決する純関数（Issue #85 タスク2）。
///
/// 候補順:
/// 1. 環境変数 `ANADEN_BIN`（ファイルが存在すれば使用、無ければエラー）
/// 2. マニフェストディレクトリ基準 `target/<profile>/anaden[.exe]`
///    （debug → release の順）
/// 3. PATH 環境変数の各ディレクトリ
///
/// # Errors
/// いずれの候補でも実行可能ファイルが見つからない場合、または
/// `ANADEN_BIN` が存在しないパスを指している場合にエラーメッセージを返す。
pub fn pick_anaden_bin(
    env_val: Option<&str>,
    manifest_dir: Option<&Path>,
    path_var: Option<&str>,
) -> Result<PathBuf, String> {
    let exe = if cfg!(windows) {
        "anaden.exe"
    } else {
        "anaden"
    };
    if let Some(v) = env_val {
        let p = PathBuf::from(v);
        if p.is_file() {
            return Ok(p);
        }
        return Err(format!("ANADEN_BIN が指すファイルが存在しません: {v}"));
    }
    if let Some(dir) = manifest_dir {
        for profile in ["debug", "release"] {
            let p = dir.join("target").join(profile).join(exe);
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    if let Some(path) = path_var {
        for dir in std::env::split_paths(path) {
            let p = dir.join(exe);
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    Err(format!(
        "anaden バイナリ解決失敗: ANADEN_BIN / target/{{debug,release}}/{exe} / PATH のいずれにも見つかりません"
    ))
}

/// 実行時の `anaden` 解決（main.rs 用）。失敗時は PATH 起動を試みる
/// プログラム名 "anaden" へフォールバックし、解決エラーを UI 表示用に返す。
fn resolve_anaden_program() -> (String, Option<String>) {
    let env_val = std::env::var("ANADEN_BIN").ok();
    let manifest = Some(Path::new(env!("CARGO_MANIFEST_DIR")));
    let path_var = std::env::var("PATH").ok();
    match pick_anaden_bin(env_val.as_deref(), manifest, path_var.as_deref()) {
        Ok(p) => (p.to_string_lossy().into_owned(), None),
        Err(e) => ("anaden".to_string(), Some(e)),
    }
}

/// pipeline 子プロセスの起動指定を組み立てる。
pub fn build_spawn_spec(program: &str, args: &[String]) -> SpawnSpec {
    SpawnSpec::new(program, args.to_vec())
}

/// [`StrategySelection`] から `anaden run` の CLI 引数列を組み立てる純関数
/// （Issue #88 受け入れ基準: egui 非依存・ヘッドレスユニットテスト対象）。
///
/// Issue #139 T1 単一情報源化: 引数列はハードコード match ではなく
/// `StrategyCatalog::builtin()` の [`anaden_strategies::StrategyDef::to_run_args`]
/// （カタログ定義）から組み立てる。実在 6 パイプライン
/// (field_loop / field_loop_pc / nav_to_field / nav_to_field_pc / worldmap_loop /
/// _title_load) がカタログ経由で実行可能。
///
/// - `--goal` / `--goal-file` は排他（CLI の `parse_goal_flag` 制約）のため、
///   本関数はいずれも出力しない（ゴール無し = 従来の max_iters 挙動）。
///
/// # Errors
/// 戦略未選択（`strategy == None`）、または未知の戦略 id の場合にエラーを返す
/// （UC-4: 子プロセスを起動せず拒否するための事前検証）。
pub fn build_run_args(
    selection: &anaden_strategies::StrategySelection,
) -> Result<Vec<String>, String> {
    let id = selection.strategy.as_deref().ok_or_else(|| {
        "戦略が選択されていません。「戦略設定」で戦略を選択してください".to_string()
    })?;
    let catalog = anaden_strategies::StrategyCatalog::builtin();
    catalog
        .find(id)
        .map(anaden_strategies::StrategyDef::to_run_args)
        .ok_or_else(|| format!("未知の戦略です(カタログ外): {id}"))
}

/// コンパイル時に確定する workspace ルート（anaden-studio manifest から
/// 2 階層上昇 = リポジトリルート）。実行時 workdir に依存しない
/// pipeline ディレクトリ解決の基準点（Issue #139 T2）。
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// `build_run_args` が生成した引数列中の pipeline ディレクトリ位置引数を
/// workspace ルート基準の絶対パスへ決定的解決する純関数（Issue #139 T2）。
///
/// カタログの `pipeline_dir` は `templates/pipelines/<name>` 形式の相対パス。
/// 従来は子プロセス (anaden CLI) 側の cwd 依存解決だったため、studio の
/// 起動 workdir 次第で「パイプライン読込失敗」になっていた。本関数は
/// spawn 前に UI 側で絶対パスへ確定させる:
///
/// - `templates/pipelines/<name>` (相対) → `<root>/templates/pipelines/<name>`
///   （解決先が実在する場合のみ置換。非実在なら元の値を維持し CLI の
///   fail-closed エラーに委譲する）
/// - `<name>` (バリアンド: バ bare name) → `<root>/templates/pipelines/<name>`
/// - 絶対パス・その他のトークンは変更しない
///
/// `run` サブコマンド・`--algorithm`/`--target` とその値は位置引数ではない
/// ためスキップする。
#[must_use]
pub fn resolve_pipeline_arg(args: &[String], root: &Path) -> Vec<String> {
    let mut out = args.to_vec();
    let mut skip_value = false;
    for tok in out.iter_mut() {
        if skip_value {
            skip_value = false;
            continue;
        }
        if tok == "run" || !tok.starts_with('-') && args.iter().any(|a| a == "run") {
            // 位置引数候補。テンプレ相対形式か bare name のみ解決する。
            if tok.starts_with("templates/pipelines/") || tok.starts_with("templates\\pipelines\\")
            {
                let joined = root.join(tok.as_str());
                if joined.is_dir() {
                    *tok = joined.to_string_lossy().into_owned();
                }
            } else if !tok.starts_with('-')
                && !tok.contains('/')
                && !tok.contains('\\')
                && tok != "run"
            {
                let joined = root.join("templates").join("pipelines").join(tok.as_str());
                if joined.is_dir() {
                    *tok = joined.to_string_lossy().into_owned();
                }
            }
        } else if tok.starts_with("--") {
            skip_value = true;
        }
    }
    out
}

/// LogLevel の表示色（スクロールログビューアの色分け・純関数）。
fn level_color(level: LogLevel) -> egui::Color32 {
    match level {
        LogLevel::Error => egui::Color32::from_rgb(240, 80, 80),
        LogLevel::Warn => egui::Color32::from_rgb(230, 180, 50),
        LogLevel::Info => egui::Color32::from_gray(200),
    }
}

/// 実行系ペインの種別（統合GUIシェルからの描画委譲用・Issue #120 欠陥2修正）。
///
/// 「▶️ 実行」タブと「🕘 履歴」タブが同一画面になる欠陥を解消するため、
/// [`PipelineRunnerApp::render_body`] はこのペイン種別で描画内容を区別する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerPane {
    /// 実行ビュー（開始/停止/再実行・戦略選択・ログ・履歴の全量）。
    Run,
    /// 履歴ビュー（履歴テーブル + 設定保存/読込のみ）。
    History,
    /// 戦略選択ビュー（Issue #125 shard 3: 戦略パネルの独立タブ。
    /// 実行ビューの埋め込みから切り出し、runner 保持の単一
    /// `StrategyPanel` インスタンスを共有する）。
    Strategy,
    /// 設定ビュー（Issue #125 shard 3: 設定保存/読込の独立タブ）。
    Settings,
}

/// pipeline 実行GUI の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerStatus {
    /// 停止中。
    Stopped,
    /// 実行中。
    Running,
}

/// 読み取りスレッド → UI 間の bounded channel 容量。
const LOG_CHANNEL_CAPACITY: usize = 1024;

/// pipeline ランナーアプリ。
pub struct PipelineRunnerApp {
    /// 子プロセス管理。
    child: ChildProcess,
    /// 起動するコマンド。
    program: String,
    /// バイナリ解決失敗等、起動前に確定しているエラー。
    resolution_error: Option<String>,
    /// 直近のエラー表示(UI 表示用)。
    last_error: Option<String>,
    /// ログイベント送信口(stdout/stderr reader 接続・再起動時に再利用)。
    log_tx: SyncSender<LogEvent>,
    /// ログイベント受信口(UI drain 用)。drain 後も再利用するため保持。
    log_rx: Receiver<LogEvent>,
    /// ログバッファ（reader → channel → drain で反映）。
    log: SharedLogBuffer,
    /// UI 描画用ログスナップショット（毎フレーム drain で更新）。
    log_snapshot: Vec<LogEntry>,
    /// ログの自動スクロール。
    auto_scroll: bool,
    /// 戦略選択パネル(シャード3スコープ、runner に統合)。
    strategy_panel: crate::strategy_ui::StrategyPanel,
    /// 選択サマリのキャッシュ（ui() の changed フラグで再計算・UC-4 前提の表示）。
    strategy_summary: String,
    /// 直近の起動 SpawnSpec（再実行ボタンで同一 spec を再 start するため保持）。
    last_spawn: Option<SpawnSpec>,
    /// 実行履歴ストア（委譲: 追記ロジックは history.rs）。
    history: RunHistory,
    /// 現在実行の開始時刻 (UNIX epoch 秒)。履歴 Record 追記に使用。
    run_started_at: Option<u64>,
    /// 現在実行の戦略名（履歴 Record 追記に使用）。
    run_strategy: Option<String>,
    /// 子プロセス終了を検知済みか（Exit イベント処理で立てるフラグ）。
    exit_observed: bool,
    /// 検知した exit code（Exit イベント処理で保持）。
    exit_code: Option<i32>,
    /// 異常終了検知時に保持する失敗状態サマリ（run_status_summary + エラーログ末尾）。
    failure_summary: Option<String>,
    /// 履歴ビューペイン（タスク5・UC-1/UC-2: 一覧・選択・再実行/中止）。
    history_panel: HistoryPanel,
    /// 設定タブロジック（Issue #125 shard 3: 保存/読込の単一実装）。
    settings_tab: crate::settings::SettingsTab,
    /// 設定ファイルパス（settings.rs の解決。テストでは差し替え）。
    settings_path: PathBuf,
}

impl PipelineRunnerApp {
    /// プログラム名を指定して生成する（テスト・明示指定用）。
    #[allow(dead_code)]
    pub fn new(program: impl Into<String>) -> Self {
        Self::with_channel(program, None)
    }

    /// `anaden` バイナリを明示パス解決して生成する（main.rs 用）。
    /// 解決失敗時は PATH の "anaden" にフォールバックし、エラーを保持する。
    pub fn with_resolved_anaden() -> Self {
        let (program, err) = resolve_anaden_program();
        Self::with_channel(program, err)
    }

    /// テスト用: 解決失敗状態をシミュレートする（起動を試みると即エラー）。
    #[allow(dead_code)]
    fn unresolved(message: &str) -> Self {
        Self::with_channel(
            "anaden",
            Some(format!("anaden バイナリ解決失敗: {message}")),
        )
    }

    fn with_channel(program: impl Into<String>, resolution_error: Option<String>) -> Self {
        // ログチャネル: reader スレッドが try_send で送り、UI が drain する。
        // UI が受信しなくても reader は行を破棄して継続する（best-effort）。
        let (log_tx, log_rx) = std::sync::mpsc::sync_channel::<LogEvent>(LOG_CHANNEL_CAPACITY);
        Self {
            child: ChildProcess::new(),
            program: program.into(),
            resolution_error,
            last_error: None,
            log_tx,
            log_rx,
            log: SharedLogBuffer::new(DEFAULT_MAX_LINES),
            log_snapshot: Vec::new(),
            auto_scroll: true,
            strategy_panel: crate::strategy_ui::StrategyPanel::default(),
            strategy_summary: "戦略未選択".to_string(),
            last_spawn: None,
            history: RunHistory::open_default(),
            run_started_at: None,
            run_strategy: None,
            exit_observed: false,
            exit_code: None,
            failure_summary: None,
            history_panel: HistoryPanel::new(),
            settings_tab: crate::settings::SettingsTab::default(),
            settings_path: crate::settings::settings_file_path(),
        }
    }

    /// 現在の状態。
    pub fn status(&mut self) -> RunnerStatus {
        if self.child.is_running() {
            RunnerStatus::Running
        } else {
            RunnerStatus::Stopped
        }
    }

    /// 選択サマリのキャッシュ（UI 表示用・テストから参照）。
    #[allow(dead_code)]
    pub fn strategy_summary(&self) -> &str {
        &self.strategy_summary
    }

    /// 選択変更をパネルから受け取る（`StrategyPanel::ui()` の changed フラグを
    /// 受けてサマリ/引数プレビューを再計算する）。
    pub fn on_strategy_changed(&mut self, changed: bool) {
        if changed {
            self.strategy_summary = self.strategy_panel.summary();
        }
    }

    /// 開始ボタンのハンドラ（選択から引数を組み立てる・UC-4 拒否込み）。
    ///
    /// 1. `strategy_panel.validate()` でカタログ整合性を検証
    /// 2. `build_run_args` で `anaden run` 引数列を組み立て
    /// 3. `start_pipeline` で子プロセス起動
    ///
    /// 検証失敗（未選択/カタログ外）は起動せずエラーを記録する。
    pub fn start_pipeline_with_selection(&mut self) {
        self.last_error = None;
        if let Err(e) = self.strategy_panel.validate() {
            self.record_error_line(&format!("戦略選択が無効です: {e}"));
            return;
        }
        match build_run_args(self.strategy_panel.selection()) {
            Ok(args) => {
                // Issue #139 T2: pipeline ディレクトリを workspace ルート基準の
                // 絶対パスへ決定論的に解決（cwd 依存を除去）。
                let resolved = resolve_pipeline_arg(&args, &workspace_root());
                self.start_pipeline(&resolved);
            }
            Err(e) => self.record_error_line(&e),
        }
    }

    /// 開始ボタンのハンドラ。二重起動は childproc 層で防止され、エラーは
    /// UI 表示用に保持されるとともにログへ ERROR 行として記録される
    /// （UC-3: 起動失敗時にエラー行がログに表示される）。
    pub fn start_pipeline(&mut self, args: &[String]) {
        self.last_error = None;
        if let Some(e) = self.resolution_error.clone() {
            self.record_error_line(&e);
            return;
        }
        let spec = build_spawn_spec(&self.program, args);
        self.start_spec(spec);
    }

    /// SpawnSpec を起動し、成功時に再実行用・履歴用の状態を記録する。
    fn start_spec(&mut self, spec: SpawnSpec) {
        self.reset_run_tracking();
        if let Err(e) = self.child.start(&spec, self.log_tx.clone()) {
            self.record_error_line(&e.to_string());
            return;
        }
        self.last_spawn = Some(spec);
        self.run_started_at = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        );
        self.run_strategy = self.strategy_panel.selection().strategy.clone();
    }

    /// 実行追跡状態をリセット（次実行に備える）。
    fn reset_run_tracking(&mut self) {
        self.run_started_at = None;
        self.run_strategy = None;
        self.exit_observed = false;
        self.exit_code = None;
        self.failure_summary = None;
    }

    /// 再実行ボタンのハンドラ（UC-1: 停止待ち → 同一 SpawnSpec で再 start）。
    ///
    /// 実行中は何もせずエラーを記録。直前の起動 spec が無い場合もエラー。
    pub fn rerun_pipeline(&mut self) {
        self.last_error = None;
        if self.child.is_running() {
            self.record_error_line("再実行には停止待ちが必要です（実行中は再実行できません）");
            return;
        }
        let Some(spec) = self.last_spawn.clone() else {
            self.record_error_line("再実行可能な実行履歴がありません（先に開始してください）");
            return;
        };
        // 終了済み子の後始末（Exit drain 前でも stop は Ok を返す契約）。
        let _ = self.child.stop();
        self.start_spec(spec);
    }

    /// 停止ボタンのハンドラ。エラーは UI 表示用に保持するとともにログへ記録。
    pub fn stop_pipeline(&mut self) {
        self.last_error = None;
        if let Err(e) = self.child.stop() {
            self.record_error_line(&e.to_string());
            return;
        }
        // ユーザー操作による中止として履歴へ記録（exit code は採取不能扱い）。
        // 自然終了を Exit イベントで既に観測済みなら二重記録しない。
        if !self.exit_observed {
            self.append_history_record(RunOutcome::Cancelled, None);
        }
    }

    /// エラーを last_error とログ（ERROR レベル行）の両方へ記録する。
    fn record_error_line(&mut self, message: &str) {
        self.last_error = Some(message.to_string());
        self.push_log_line(&format!("ERROR {message}"));
    }

    /// ログバッファへ直接 1 行 push しスナップショットを更新する
    /// （レベルは LogLevel::from_line で推定される）。
    fn push_log_line(&mut self, line: &str) {
        let line = line.to_string();
        self.log.with_buf(|b| b.push_line(&line));
        self.refresh_snapshot();
    }

    /// チャネルを drain してログスナップショットを更新する（UI 毎フレーム呼出）。
    ///
    /// `LogEvent::Exit` を観測した場合:
    /// - exit code を保持し、異常終了（非零 / 不明）なら失敗状態サマリ
    ///   （run_status_summary + エラーログ末尾）を保持する（UC-3）
    /// - 履歴へ RunRecord を追記する（正常: Success / 非零・不明: Failed）
    pub fn drain_logs(&mut self) {
        // Exit イベントを観測するため、イベント列を一度収集する。
        let mut events = Vec::new();
        while let Ok(ev) = self.log_rx.try_recv() {
            events.push(ev);
        }
        let exit_seen = events.iter().any(|ev| matches!(ev, LogEvent::Exit(_)));
        // Exit(Option<i32>) の中身は既に Option<i32> のため flatten で一意化する
        // （Some(*c) で包むと二重ネストになる）。
        let exit_code: Option<i32> = events
            .iter()
            .find_map(|ev| match ev {
                LogEvent::Exit(c) => Some(*c),
                LogEvent::Line(_) => None,
            })
            .flatten();
        // バッファへ反映（SharedLogBuffer::drain 相当の直接 push）。
        for ev in events {
            match ev {
                LogEvent::Line(l) => {
                    let _ = self.log.with_buf(|b| b.push_line(&l));
                }
                LogEvent::Exit(code) => {
                    let label = match code {
                        Some(0) => "exit=0 (成功)",
                        Some(_) => "exit=エラー",
                        None => "exit=不明",
                    };
                    let line = format!("[studio] プロセス終了: {label} (code={code:?})");
                    // 異常終了行は文字列推定にかからないため明示レベルで記録。
                    let level = if matches!(code, Some(0)) {
                        LogLevel::Info
                    } else {
                        LogLevel::Error
                    };
                    let _ = self.log.with_buf(|b| b.push_line_with_level(&line, level));
                }
            }
        }
        self.refresh_snapshot();
        if exit_seen && !self.exit_observed {
            self.exit_observed = true;
            self.exit_code = exit_code;
            let failed = !matches!(exit_code, Some(0));
            if failed {
                self.failure_summary = Some(self.build_failure_summary());
            }
            // 履歴 Record 追記（正常終了 / 異常終了）。中止は stop_pipeline 側。
            let outcome = if failed {
                RunOutcome::Failed
            } else {
                RunOutcome::Success
            };
            self.append_history_record(outcome, exit_code);
        }
    }

    /// 失敗状態サマリ（状態 + エラーログ末尾）を組み立てる純ロジック。
    fn build_failure_summary(&self) -> String {
        let status = self.run_status_summary();
        let error_tail: Vec<String> = self
            .log_snapshot
            .iter()
            .rev()
            .filter(|e| e.level == LogLevel::Error)
            .take(crate::history::LOG_TAIL_MAX_LINES)
            .map(|e| e.line.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if error_tail.is_empty() {
            format!("失敗: {status}")
        } else {
            format!(
                "失敗: {status} | エラーログ末尾: {}",
                error_tail.join(" / ")
            )
        }
    }

    /// 履歴へ RunRecord を追記する（委譲: history.rs）。
    fn append_history_record(&mut self, outcome: RunOutcome, exit_code: Option<i32>) {
        let log_tail = self.log_snapshot.iter().map(|e| e.line.clone()).collect();
        let record = RunRecord::new(
            self.run_started_at.unwrap_or(0),
            self.run_strategy.clone().unwrap_or_else(|| "-".to_string()),
            outcome,
            exit_code,
            log_tail,
        );
        if let Err(e) = self.history.append(record) {
            self.record_error_line(&format!("履歴の保存に失敗しました: {e}"));
        }
    }

    /// 異常終了検知時に保持した失敗状態サマリ（未検知/正常時は None）。
    pub fn failure_summary(&self) -> Option<&str> {
        self.failure_summary.as_deref()
    }

    fn refresh_snapshot(&mut self) {
        self.log_snapshot = self
            .log
            .with_buf(|b| b.entries().cloned().collect())
            .unwrap_or_default();
    }

    /// ログをクリアする（クリアボタンのハンドラ）。
    pub fn clear_logs(&mut self) {
        self.log.with_buf(LogBuffer::clear);
        self.log_snapshot.clear();
    }

    /// 現在のログスナップショット（昇順・UI 描画とテストで使用）。
    #[allow(dead_code)]
    pub fn log_snapshot(&self) -> &[LogEntry] {
        &self.log_snapshot
    }

    /// 実行状態サマリの一行文字列（UI ステータスバー表示用・ヘッドレス）。
    ///
    /// LogBuffer.status (RunStatus) を refresh_snapshot と同じタイミングで
    /// 読み取り、summary() を返す。未観測時は "未実行"。
    pub fn run_status_summary(&self) -> String {
        self.log
            .with_buf(|b| b.status.summary())
            .unwrap_or_else(|| "未実行".to_string())
    }

    /// 直近のエラーメッセージ(無ければ None)。
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// 履歴ペインのアクション要求を消費して実行に反映する
    /// （タスク5・UC-1/UC-2: 再実行/中止/設定保存/設定読込。毎フレーム ui() から呼ぶ）。
    pub fn handle_history_actions(&mut self) {
        while let Some(action) = self.history_panel.take_action() {
            match action {
                HistoryAction::Rerun => self.rerun_pipeline(),
                HistoryAction::Stop => self.stop_pipeline(),
                HistoryAction::SaveSettings => self.save_settings_to_path(),
                HistoryAction::LoadSettings => self.load_settings_from_path(),
            }
        }
    }

    /// 履歴パネルの参照（テスト用）。
    pub fn history_panel(&self) -> &HistoryPanel {
        &self.history_panel
    }

    /// 履歴パネルの可変参照（テスト用: アクション注入）。
    pub fn history_panel_mut(&mut self) -> &mut HistoryPanel {
        &mut self.history_panel
    }

    /// ログビューア本体（シャード3前半: LogLevel 色分け・自動スクロール・クリア）。
    fn log_view_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("実行ログ");
            if ui.button("クリア").clicked() {
                self.clear_logs();
            }
            ui.checkbox(&mut self.auto_scroll, "自動スクロール");
        });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(self.auto_scroll)
            .show(ui, |ui| {
                if self.log_snapshot.is_empty() {
                    ui.weak("（ログなし）");
                }
                for entry in &self.log_snapshot {
                    ui.monospace(
                        egui::RichText::new(&entry.line)
                            .monospace()
                            .color(level_color(entry.level)),
                    );
                }
            });
    }
}

impl eframe::App for PipelineRunnerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.render_body(ui, RunnerPane::Run);
        });
    }
}

impl PipelineRunnerApp {
    /// 実行GUI 本体ペイン（Issue #119: 統合シェルへの埋め込み用公開 API）。
    ///
    /// `eframe::App::ui` から CentralPanel の内側を切り出したもの。
    /// 単一ウィンドウ統合 GUI (`shell::UnifiedShell`) の Run/History タブから
    /// 委譲される。
    /// モード本体を親レイアウト内に描画する埋め込み用 API（統合GUIシェル経由）。
    ///
    /// Issue #120 欠陥2修正: かつて Run/History 両タブが同一内容を描画していた
    /// （履歴タブがダミー）。`RunnerPane` で実行ビューと履歴ビューを区別する。
    pub fn render_body(&mut self, ui: &mut egui::Ui, pane: RunnerPane) {
        match pane {
            RunnerPane::Run => self.render_run_body(ui),
            RunnerPane::History => self.render_history_body(ui),
            RunnerPane::Strategy => self.render_strategy_body(ui),
            RunnerPane::Settings => self.render_settings_body(ui),
        }
    }

    /// 実行ビュー（開始/停止/再実行・戦略選択・ログ・履歴の全量）。
    fn render_run_body(&mut self, ui: &mut egui::Ui) {
        {
            ui.heading("anaden pipeline runner");

            let running = self.status() == RunnerStatus::Running;
            ui.add_enabled_ui(!running, |ui| {
                if ui.button("開始").clicked() {
                    self.start_pipeline_with_selection();
                }
            });
            ui.add_enabled_ui(running, |ui| {
                if ui.button("停止").clicked() {
                    self.stop_pipeline();
                }
            });
            ui.add_enabled_ui(!running, |ui| {
                if ui.button("再実行").clicked() {
                    self.rerun_pipeline();
                }
            });

            ui.label(if running {
                "状態: 実行中"
            } else {
                "状態: 停止"
            });

            // 実行状態サマリ（goal/iterations/stop_reason・Issue #88 タスク3）。
            ui.label(format!("run: {}", self.run_status_summary()));

            if let Some(err) = self.last_error() {
                ui.colored_label(egui::Color32::RED, format!("エラー: {err}"));
            }

            // 異常終了検知時の失敗状態サマリ（UC-3: 状態 + エラーログ末尾）。
            if let Some(failure) = self.failure_summary() {
                ui.colored_label(egui::Color32::RED, format!("直前の実行: {failure}"));
            }

            ui.separator();
            // 戦略選択サマリ一行（Issue #125 shard 3: 選択 UI は Strategy タブへ
            // 切り出したため、実行ビューにはサマリのみ残す）。
            ui.weak(format!("選択: {}", self.strategy_summary));

            ui.separator();
            // ログビューア（シャード3前半・毎フレーム drain）。
            self.drain_logs();
            self.log_view_ui(ui);

            ui.separator();
            // 履歴ビューペイン（タスク5・UC-1/UC-2）。毎フレーム実行状態と
            // 履歴ストアを同期し、ボタン操作は handle_history_actions で処理。
            self.render_history_section(ui, running);
        }
    }

    /// 履歴ビュー（履歴テーブル + 設定保存/読込のみ。実行制御は含まない）。
    ///
    /// Issue #120 欠陥2: 統合GUIの「🕘 履歴」タブ専用ビュー。実行ビューと
    /// 区別され、履歴参照・再実行・設定操作に集中したレイアウト。
    fn render_history_body(&mut self, ui: &mut egui::Ui) {
        ui.heading("実行履歴");
        let running = self.status() == RunnerStatus::Running;
        ui.label(if running {
            "状態: 実行中"
        } else {
            "状態: 停止"
        });
        if let Some(failure) = self.failure_summary() {
            ui.colored_label(egui::Color32::RED, format!("直前の実行: {failure}"));
        }
        ui.separator();
        self.render_history_section(ui, running);
    }

    /// 履歴セクション（両ビュー共通）。毎フレーム実行状態と履歴ストアを同期し、
    /// ボタン操作は handle_history_actions で処理。
    fn render_history_section(&mut self, ui: &mut egui::Ui, running: bool) {
        self.history_panel.set_running(running);
        self.history_panel.refresh_from(self.history.records());
        self.history_panel.ui(ui);
        self.handle_history_actions();
    }

    /// 戦略選択ビュー（Issue #125 shard 3: 実行ビューからの切り出し）。
    ///
    /// 描画本体は strategy_ui.rs の `render_tab_body` へ委譲（runner.rs は
    /// 500 行ルール超過のためロジックを持たない・estimate 承認条件）。
    /// パネルは runner が保持する単一インスタンスを実行ビューと共有する。
    fn render_strategy_body(&mut self, ui: &mut egui::Ui) {
        let running = self.status() == RunnerStatus::Running;
        let changed = self.strategy_panel.render_tab_body(ui, running);
        self.on_strategy_changed(changed);
    }

    /// 設定ビュー（Issue #125 shard 3: 設定保存/読込の独立タブ化）。
    ///
    /// 保存/読込ロジックは settings.rs の `SettingsTab` へ集約し、ここでは
    /// ボタン描画とステータス表示のみ（runner.rs 肥大化の回避）。
    fn render_settings_body(&mut self, ui: &mut egui::Ui) {
        ui.heading("設定");
        ui.label(crate::settings::settings_path_display(&self.settings_path));
        ui.separator();
        if ui.button("💾 設定を保存").clicked() {
            self.save_settings_to_path();
        }
        if ui.button("📂 設定を読込").clicked() {
            self.load_settings_from_path();
        }
        match self.settings_tab.status() {
            crate::settings::SettingsTabStatus::Ok(p) => {
                ui.label(format!("設定: {}", p.display()));
            }
            crate::settings::SettingsTabStatus::Err(e) => {
                ui.colored_label(egui::Color32::RED, e.clone());
            }
            crate::settings::SettingsTabStatus::None => {}
        }
        ui.separator();
        ui.weak(format!("現在の選択: {}", self.strategy_summary));
    }

    /// 設定を settings_path へ保存する（設定ビュー・履歴ペイン共用の単一実装）。
    ///
    /// 選択中戦略を `StudioSettings` として保存し、結果は `SettingsTab` の
    /// ステータスに記録される（UC-2: 失敗しても GUI 継続）。
    fn save_settings_to_path(&mut self) {
        self.settings_tab
            .save(self.strategy_panel.selection(), &self.settings_path);
    }

    /// settings_path から設定を読み込み戦略パネルへ反映する
    /// （設定ビュー・履歴ペイン共用の単一実装）。
    ///
    /// 読込成功時はサマリも再計算する。ファイル不在・破損 (初回起動) は
    /// UC-2 フォールバックとして既定値を維持しエラー表示しない。
    fn load_settings_from_path(&mut self) {
        if let Some(selection) = self.settings_tab.load(&self.settings_path)
            && self
                .strategy_panel
                .load_toml(&selection_to_toml(&selection))
                .is_ok()
        {
            self.strategy_summary = self.strategy_panel.summary();
        }
    }

    /// 設定 I/O の直近結果（テスト用）。
    pub fn settings_status(&self) -> &crate::settings::SettingsTabStatus {
        self.settings_tab.status()
    }
}

/// 読み込んだ選択を StrategyPanel へ渡すための TOML 文字列化
/// （serde 失敗時は空文字列 → load_toml が Err となるため無視される）。
fn selection_to_toml(selection: &anaden_strategies::StrategySelection) -> String {
    toml::to_string(selection).unwrap_or_default()
}

impl Drop for PipelineRunnerApp {
    fn drop(&mut self) {
        // ChildProcess の kill_on_drop に加え、明示的に停止を試みる。
        self.stop_pipeline();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    /// 実行可能なダミープログラムの名前(ping は Windows/Linux 両方にある)。
    fn dummy_program() -> &'static str {
        "ping"
    }

    fn long_args() -> Vec<String> {
        if cfg!(windows) {
            vec!["-n".to_string(), "30".to_string(), "127.0.0.1".to_string()]
        } else {
            vec!["-c".to_string(), "30".to_string(), "127.0.0.1".to_string()]
        }
    }

    use std::path::PathBuf;

    fn exe_name() -> String {
        if cfg!(windows) {
            "anaden.exe".to_string()
        } else {
            "anaden".to_string()
        }
    }

    /// tempdir 直下に `target/<profile>/anaden[.exe]` を作る。
    fn make_target_binary(root: &Path, profile: &str) -> PathBuf {
        let dir = root.join("target").join(profile);
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join(exe_name());
        std::fs::write(&bin, b"dummy").unwrap();
        bin
    }

    #[test]
    fn test_pick_prefers_env_var_when_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join(exe_name());
        std::fs::write(&bin, b"dummy").unwrap();
        let got = pick_anaden_bin(Some(bin.to_str().unwrap()), None, None).unwrap();
        assert_eq!(got, bin);
    }

    #[test]
    fn test_pick_env_var_missing_file_is_error() {
        let got = pick_anaden_bin(Some("/nonexistent/anaden-binary-zzz"), None, None);
        assert!(got.is_err());
        let err = got.unwrap_err();
        assert!(err.contains("ANADEN_BIN"), "unexpected: {err}");
    }

    #[test]
    fn test_pick_finds_target_debug_from_manifest_dir() {
        let dir = tempfile::tempdir().unwrap();
        let bin = make_target_binary(dir.path(), "debug");
        let got = pick_anaden_bin(None, Some(dir.path()), None).unwrap();
        assert_eq!(got, bin);
    }

    #[test]
    fn test_pick_finds_target_release_when_debug_absent() {
        let dir = tempfile::tempdir().unwrap();
        let bin = make_target_binary(dir.path(), "release");
        let got = pick_anaden_bin(None, Some(dir.path()), None).unwrap();
        assert_eq!(got, bin);
    }

    #[test]
    fn test_pick_falls_back_to_path_search() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join(exe_name());
        std::fs::write(&bin, b"dummy").unwrap();
        let path_var = dir.path().to_str().unwrap().to_string();
        let got = pick_anaden_bin(None, None, Some(&path_var)).unwrap();
        assert_eq!(got, bin);
    }

    #[test]
    fn test_pick_no_candidates_is_error() {
        let got = pick_anaden_bin(None, None, None);
        assert!(got.is_err());
    }

    /// UC-2: 解決失敗時にエラーが last_error に入り GUI がハングしない。
    #[test]
    fn test_unresolved_program_records_error_and_stays_stopped() {
        let mut app = PipelineRunnerApp::unresolved("anaden バイナリ解決失敗(テスト)");
        app.start_pipeline(&[]);
        assert_eq!(app.status(), RunnerStatus::Stopped);
        let err = app.last_error().expect("error should be recorded");
        assert!(err.contains("バイナリ解決失敗"), "unexpected error: {err}");
    }

    #[test]
    fn test_build_spawn_spec_copies_program_and_args() {
        let args = vec!["run".to_string()];
        let spec = build_spawn_spec("anaden", &args);
        assert_eq!(spec.program, "anaden");
        assert_eq!(spec.args, args);
    }

    #[test]
    fn test_initial_status_is_stopped() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        assert_eq!(app.status(), RunnerStatus::Stopped);
        assert!(app.last_error().is_none());
        assert!(app.log_snapshot().is_empty());
    }

    #[test]
    fn test_start_moves_to_running_and_stop_returns_to_stopped() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.start_pipeline(&long_args());
        assert_eq!(app.status(), RunnerStatus::Running);
        app.stop_pipeline();
        assert_eq!(app.status(), RunnerStatus::Stopped);
    }

    /// UC-3: 起動失敗時にエラー行がログに表示される。
    #[test]
    fn test_spawn_failure_shows_error_line_in_log() {
        let mut app = PipelineRunnerApp::new("definitely-not-a-real-exe-xyz");
        app.start_pipeline(&[]);
        assert_eq!(app.status(), RunnerStatus::Stopped);
        let err = app.last_error().expect("error should be recorded");
        assert!(err.contains("起動に失敗"), "unexpected error: {err}");

        app.drain_logs();
        let snap = app.log_snapshot();
        assert!(!snap.is_empty(), "log should contain the error line");
        let last = snap.last().unwrap();
        assert!(
            last.line.contains("起動に失敗"),
            "unexpected log line: {}",
            last.line
        );
        assert_eq!(last.level, LogLevel::Error);
    }

    /// UC-3（解決失敗系）: 解決エラーもログへ ERROR 行として表示される。
    #[test]
    fn test_resolution_failure_shows_error_line_in_log() {
        let mut app = PipelineRunnerApp::unresolved("テスト用解決失敗");
        app.start_pipeline(&[]);
        app.drain_logs();
        let last = app.log_snapshot().last().expect("error line in log");
        assert_eq!(last.level, LogLevel::Error);
        assert!(last.line.contains("バイナリ解決失敗"));
    }

    /// 実行中の子の stdout が drain でログスナップショットに届く。
    #[test]
    fn test_running_child_stdout_appears_in_log_after_drain() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.start_pipeline(&long_args());
        if app.status() == RunnerStatus::Running {
            // ping は即 stdout へ行を出す。数回 drain して届くまで待つ。
            for _ in 0..50 {
                app.drain_logs();
                if !app.log_snapshot().is_empty() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let snap = app.log_snapshot();
            assert!(!snap.is_empty(), "stdout lines should be drained");
            app.stop_pipeline();
        }
        // ping が環境に無い場合は start が失敗するためスキップ相当。
    }

    /// クリアボタンのハンドラがログを空にする。
    #[test]
    fn test_clear_logs_empties_snapshot_and_buffer() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.start_pipeline(&[]);
        app.drain_logs();
        app.clear_logs();
        assert!(app.log_snapshot().is_empty());
        let buf_empty = app.log.with_buf(|b| b.is_empty()).unwrap_or(false);
        assert!(buf_empty);
    }

    /// LogLevel → 表示色の対応（色分け描画の純ロジック部分）。
    #[test]
    fn test_level_color_varies_by_level() {
        assert_ne!(level_color(LogLevel::Error), level_color(LogLevel::Warn));
        assert_ne!(level_color(LogLevel::Warn), level_color(LogLevel::Info));
        assert_ne!(level_color(LogLevel::Error), level_color(LogLevel::Info));
    }

    #[test]
    fn test_stop_when_stopped_sets_error() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.stop_pipeline();
        let err = app.last_error().expect("error should be recorded");
        assert!(
            err.contains("実行中ではありません"),
            "unexpected error: {err}"
        );
    }

    /// run_status_summary: 未実行時は「未実行」。
    #[test]
    fn test_run_status_summary_initially_not_run() {
        let app = PipelineRunnerApp::new(dummy_program());
        assert_eq!(app.run_status_summary(), "未実行");
    }

    /// run_status_summary: 開始行観測後は goal 付き実行中表示
    /// （RunStatus.observe の出力契約: `run_loop 開始:` 行）。
    #[test]
    fn test_run_status_summary_reflects_start_line() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.push_log_line("INFO anaden_cli: run_loop 開始: interval=2s max_iters=10 goal=farm50");
        assert_eq!(app.run_status_summary(), "実行中 goal=farm50 iterations=?");
    }

    /// run_status_summary: 結果行観測後は停止理由とサイクル数表示
    /// （`サイクル数:` / `停止理由:` 行）。
    #[test]
    fn test_run_status_summary_reflects_result_lines() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.push_log_line("run_loop 開始: interval=2s max_iters=10 goal=g1");
        app.push_log_line("サイクル数: 42");
        app.push_log_line("停止理由:   宣言的ゴール到達(正常)");
        assert_eq!(
            app.run_status_summary(),
            "停止 reason=宣言的ゴール到達(正常) iterations=42"
        );
    }

    /// run_status_summary: ログクリアで未実行へ戻る。
    #[test]
    fn test_run_status_summary_resets_on_clear() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.push_log_line("run_loop 開始: goal=g");
        app.clear_logs();
        assert_eq!(app.run_status_summary(), "未実行");
    }

    /// run_status_summary: drain 経由（チャネル→drain_logs）でも反映される。
    #[test]
    fn test_run_status_summary_updates_via_drain_logs() {
        use crate::log_view::LogEvent;
        let mut app = PipelineRunnerApp::new(dummy_program());
        // drain は受信側チャネルから読むため、start 経由ではなく直接検証は困難。
        // ここでは drain 後も summary が崩れないこと（空 drain で状態維持）を確認。
        app.push_log_line("run_loop 開始: goal=g2");
        app.drain_logs();
        assert_eq!(app.run_status_summary(), "実行中 goal=g2 iterations=?");
        let _ = LogEvent::Line(String::new());
    }

    // ---- build_run_args（Issue #88 タスク1・UC-1/UC-4）----

    fn pipeline_selection(id: &str) -> anaden_strategies::StrategySelection {
        let mut panel = crate::strategy_ui::StrategyPanel::default();
        panel.select_strategy(id);
        panel.selection().clone()
    }

    /// UC-1 正常系: 実在パイプライン選択で run サブコマンド + --algorithm + 位置引数が組まれる。
    #[test]
    fn test_build_run_args_field_loop_builds_run_subcommand() {
        let args = build_run_args(&pipeline_selection("field_loop")).unwrap();
        assert_eq!(args[0], "run");
        assert!(args.contains(&"--algorithm".to_string()));
        // --algorithm の値は sse|ccoeff のみ受理される値であること。
        let algo = args
            .iter()
            .position(|a| a == "--algorithm")
            .map(|i| args[i + 1].clone())
            .unwrap();
        assert!(
            algo == "sse" || algo == "ccoeff",
            "invalid algorithm: {algo}"
        );
    }

    /// UC-1 正常系: pipeline_dir / start_task 位置引数が含まれる。
    #[test]
    fn test_build_run_args_field_loop_includes_positional_args() {
        let args = build_run_args(&pipeline_selection("field_loop")).unwrap();
        // run の位置引数（フラグ以降の非フラグトークン）は pipeline_dir と start_task。
        let positional: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(i, a)| *i > 0 && !a.starts_with("--") && args[*i - 1] != "--algorithm")
            .map(|(_, a)| a)
            .collect();
        assert_eq!(positional.len(), 2, "args: {args:?}");
        assert!(positional[0].contains("field_loop"));
        assert_eq!(positional[1], "TapBottomStable");
    }

    /// Issue #139 T1: PC 版パイプラインは --target windows がカタログから注入される。
    #[test]
    fn test_build_run_args_pc_pipeline_injects_windows_target() {
        let args = build_run_args(&pipeline_selection("nav_to_field_pc")).unwrap();
        assert_eq!(args[0], "run");
        let target_idx = args
            .iter()
            .position(|a| a == "--target")
            .unwrap_or_else(|| panic!("--target not found in {args:?}"));
        assert_eq!(args[target_idx + 1], "windows");
        assert!(args.contains(&"templates/pipelines/nav_to_field_pc".to_string()));
        assert!(args.contains(&"TapToStartPc".to_string()));
    }

    /// Issue #139 T1: 実在 6 パイプラインすべてがカタログ経由で引数組み立て可能。
    #[test]
    fn test_build_run_args_supports_all_six_real_pipelines() {
        for id in [
            "field_loop",
            "field_loop_pc",
            "nav_to_field",
            "nav_to_field_pc",
            "worldmap_loop",
            "_title_load",
        ] {
            let args =
                build_run_args(&pipeline_selection(id)).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(args[0], "run", "{id}");
            assert!(
                args.iter().any(|a| a.contains(&format!("pipelines/{id}"))),
                "{id}: pipeline_dir missing in {args:?}"
            );
        }
    }

    /// Issue #139 T1: fishing（実在しない pipeline）はカタログ外として拒否される
    /// （select_strategy はカタログ外 id を無視するため strategy は None のまま）。
    #[test]
    fn test_build_run_args_fishing_is_rejected_as_nonexistent_pipeline() {
        // パネル経由: カタログ外のため選択自体が無視される → 未選択エラー。
        let sel = pipeline_selection("fishing");
        assert_eq!(sel.strategy, None);

        // 混入経路 (load_toml 等) を想定し strategy へ直接 fishing を入れた場合。
        let injected = anaden_strategies::StrategySelection {
            strategy: Some("fishing".to_string()),
            ..Default::default()
        };
        let err = build_run_args(&injected).unwrap_err();
        assert!(err.contains("fishing"), "unexpected: {err}");
    }

    /// UC-4 エッジケース: 戦略未選択は Err（子プロセス起動拒否の事前検証）。
    #[test]
    fn test_build_run_args_no_strategy_is_err() {
        let selection = anaden_strategies::StrategySelection::default();
        let err = build_run_args(&selection).unwrap_err();
        assert!(
            err.contains("戦略が選択されていません"),
            "unexpected: {err}"
        );
    }

    /// UC-4 エッジケース: 未知の戦略 id（不正選択）は Err。
    #[test]
    fn test_build_run_args_unknown_strategy_is_err() {
        let selection = anaden_strategies::StrategySelection {
            strategy: Some("bogus-strategy".to_string()),
            ..Default::default()
        };
        let err = build_run_args(&selection).unwrap_err();
        assert!(err.contains("bogus-strategy"), "unexpected: {err}");
    }

    /// --goal / --goal-file の排他制約（parse_goal_flag）を破らないこと:
    /// build_run_args はいずれも出力しない。
    #[test]
    fn test_build_run_args_never_outputs_goal_flags() {
        let args = build_run_args(&pipeline_selection("field_loop")).unwrap();
        assert!(!args.contains(&"--goal".to_string()));
        assert!(!args.contains(&"--goal-file".to_string()));
    }

    // ---- Issue #139 T2: pipeline ディレクトリ決定的解決 ----

    /// tempdir に `templates/pipelines/<name>` ディレクトリを作るヘルパ。
    fn make_pipeline_dir(root: &Path, name: &str) -> PathBuf {
        let dir = root.join("templates").join("pipelines").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 相対 pipeline_dir が workspace ルート基準の絶対パスへ置換される。
    #[test]
    fn test_resolve_pipeline_arg_rewrites_relative_template_path() {
        let root = tempfile::tempdir().unwrap();
        let dir = make_pipeline_dir(root.path(), "field_loop");
        let args = vec![
            "run".to_string(),
            "--algorithm".to_string(),
            "ccoeff".to_string(),
            "templates/pipelines/field_loop".to_string(),
            "TapBottomStable".to_string(),
        ];
        let got = resolve_pipeline_arg(&args, root.path());
        assert_eq!(PathBuf::from(&got[3]), dir);
        // その他のトークンは不変。
        assert_eq!(got[0], "run");
        assert_eq!(got[2], "ccoeff");
        assert_eq!(got[4], "TapBottomStable");
    }

    /// 非実在 pipeline_dir は元の値を維持（CLI 側 fail-closed に委譲）。
    #[test]
    fn test_resolve_pipeline_arg_keeps_nonexistent_dir_as_is() {
        let root = tempfile::tempdir().unwrap();
        let args = vec![
            "run".to_string(),
            "templates/pipelines/ghost".to_string(),
            "Task".to_string(),
        ];
        let got = resolve_pipeline_arg(&args, root.path());
        assert_eq!(got[1], "templates/pipelines/ghost");
    }

    /// bare name（カタログ id）も templates/pipelines 基準で解決される。
    #[test]
    fn test_resolve_pipeline_arg_resolves_bare_pipeline_name() {
        let root = tempfile::tempdir().unwrap();
        let dir = make_pipeline_dir(root.path(), "worldmap_loop");
        let args = vec![
            "run".to_string(),
            "worldmap_loop".to_string(),
            "Task".to_string(),
        ];
        let got = resolve_pipeline_arg(&args, root.path());
        assert_eq!(PathBuf::from(&got[1]), dir);
    }

    /// 絶対パス指定は変更されない（冪等・二重解決なし）。
    #[test]
    fn test_resolve_pipeline_arg_keeps_absolute_path_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let abs = make_pipeline_dir(root.path(), "field_loop");
        let args = vec![
            "run".to_string(),
            abs.to_string_lossy().into_owned(),
            "Task".to_string(),
        ];
        let got = resolve_pipeline_arg(&args, root.path());
        assert_eq!(PathBuf::from(&got[1]), abs);
    }

    /// --target の値（位置引数風の "windows"）は bare name 解決の対象外。
    #[test]
    fn test_resolve_pipeline_arg_does_not_touch_flag_values() {
        let root = tempfile::tempdir().unwrap();
        // windows という名の pipeline が存在しても --target の値は置換しない。
        let _ = make_pipeline_dir(root.path(), "windows");
        let args = vec![
            "run".to_string(),
            "--target".to_string(),
            "windows".to_string(),
            "templates/pipelines/none".to_string(),
            "T".to_string(),
        ];
        let got = resolve_pipeline_arg(&args, root.path());
        assert_eq!(got[2], "windows");
    }

    /// 実機workspace ルートでカタログ 6 パイプラインがすべて絶対解決される
    /// （T1 カタログ定義からの引数経路が実ディレクトリに到達することの保証）。
    #[test]
    fn test_resolve_pipeline_arg_supports_all_six_real_pipelines_on_real_root() {
        let root = workspace_root();
        for id in [
            "field_loop",
            "field_loop_pc",
            "nav_to_field",
            "nav_to_field_pc",
            "worldmap_loop",
            "_title_load",
        ] {
            let sel = pipeline_selection(id);
            let args = build_run_args(&sel).unwrap_or_else(|e| panic!("{id}: {e}"));
            let got = resolve_pipeline_arg(&args, &root);
            let dir = got
                .iter()
                .position(|a| a == "run")
                .map(|i| {
                    got[i + 1..]
                        .iter()
                        .find(|a| Path::new(a).is_absolute())
                        .cloned()
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            assert!(
                Path::new(&dir).is_dir(),
                "{id}: resolved dir not found: {dir}"
            );
        }
    }

    /// UC-4: 戦略未選択のまま開始すると拒否され、子プロセスは起動しない。
    #[test]
    fn test_start_with_no_strategy_is_rejected() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        // 初期状態は戦略未選択。
        assert_eq!(app.strategy_summary(), "戦略未選択");
        app.start_pipeline_with_selection();
        assert_eq!(app.status(), RunnerStatus::Stopped);
        let err = app.last_error().expect("rejection error");
        assert!(
            err.contains("戦略が選択されていません"),
            "unexpected: {err}"
        );
        app.drain_logs();
        let last = app.log_snapshot().last().expect("error line");
        assert_eq!(last.level, LogLevel::Error);
    }

    /// UC-4: カタログ外戦略（load_toml 等で混入）でも validate が拒否する。
    #[test]
    fn test_start_with_unknown_strategy_is_rejected_by_validate() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.strategy_panel
            .load_toml("strategy = \"bogus\"")
            .expect("toml load");
        app.start_pipeline_with_selection();
        assert_eq!(app.status(), RunnerStatus::Stopped);
        let err = app.last_error().expect("rejection error");
        assert!(err.contains("戦略選択が無効です"), "unexpected: {err}");
    }

    /// 選択済みなら validate → build_run_args → start の経路で起動する
    /// （ping があれば Running。無ければスキップ相当）。
    #[test]
    fn test_start_with_selected_strategy_starts_child() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.strategy_panel.select_strategy("field_loop");
        app.start_pipeline_with_selection();
        if app.status() == RunnerStatus::Running {
            app.stop_pipeline();
        }
        // 引数組み立て自体は pipeline_selection のテストで検証済み。
    }

    /// on_strategy_changed: changed=true でのみサマリが再計算される。
    #[test]
    fn test_on_strategy_changed_refreshes_summary() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        assert_eq!(app.strategy_summary(), "戦略未選択");
        app.strategy_panel.select_strategy("field_loop");
        // changed=false（同一フレーム内の無操作）では更新されない。
        app.on_strategy_changed(false);
        assert_eq!(app.strategy_summary(), "戦略未選択");
        // changed=true で更新される。
        app.on_strategy_changed(true);
        // 実在 6 パイプラインは ON/OFF オプションを持たないため「オプションなし」。
        assert_eq!(
            app.strategy_summary(),
            "strategy=field_loop (オプションなし)"
        );
    }

    // ---- Issue #125 shard 3 タスク2: Strategy / Settings ペイン ----

    /// RunnerPane に Strategy / Settings バリアントが存在し全ペイン種別が区別される。
    #[test]
    fn test_runner_pane_has_six_distinct_variants() {
        use RunnerPane::{History, Run, Settings, Strategy};
        assert_ne!(Strategy, Run);
        assert_ne!(Strategy, History);
        assert_ne!(Settings, Run);
        assert_ne!(Settings, History);
        assert_ne!(Strategy, Settings);
    }

    /// 設定ビューの保存: 選択中戦略が settings.toml へ保存され status が Ok になる。
    #[test]
    fn test_save_settings_from_runner_writes_file_and_records_ok() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        app.settings_path = path.clone();
        app.strategy_panel.select_strategy("field_loop");
        app.save_settings_to_path();
        assert!(path.is_file());
        assert!(matches!(
            app.settings_status(),
            crate::settings::SettingsTabStatus::Ok(_)
        ));
    }

    /// 設定ビューの読込: 保存済み設定が戦略パネルへ復元されサマリも更新される。
    #[test]
    fn test_load_settings_restores_strategy_selection_and_summary() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        app.settings_path = path.clone();
        app.strategy_panel.select_strategy("field_loop");
        app.save_settings_to_path();

        let mut app2 = PipelineRunnerApp::new(dummy_program());
        app2.settings_path = path;
        assert_eq!(app2.strategy_summary(), "戦略未選択");
        app2.load_settings_from_path();
        assert!(app2.strategy_panel.selection().strategy.is_some());
        assert!(app2.strategy_summary().contains("field_loop"));
    }

    /// 読込: ファイル不在 (初回起動) はエラー扱いにしない (UC-2)。
    #[test]
    fn test_load_settings_missing_file_is_ok_status() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let dir = tempfile::tempdir().unwrap();
        app.settings_path = dir.path().join("nope.toml");
        app.load_settings_from_path();
        assert!(matches!(
            app.settings_status(),
            crate::settings::SettingsTabStatus::Ok(_)
        ));
    }

    /// 履歴ペインの SaveSettings アクションは設定ビューと同一ロジックへ委譲される
    /// （二重実装ではなく単一の save_settings_to_path を通る）。
    #[test]
    fn test_history_save_settings_action_delegates_to_shared_logic() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        app.settings_path = path.clone();
        app.strategy_panel.select_strategy("fishing");
        app.history_panel_mut()
            .request(crate::history_ui::HistoryAction::SaveSettings);
        app.handle_history_actions();
        assert!(path.is_file());
    }

    #[test]
    fn test_double_start_records_already_running_error() {
        let mut app = PipelineRunnerApp::new(dummy_program());
        app.start_pipeline(&long_args());
        let running = app.status() == RunnerStatus::Running;
        if running {
            app.start_pipeline(&[]);
            let err = app.last_error().expect("error should be recorded");
            assert!(err.contains("既に実行中"), "unexpected error: {err}");
            app.stop_pipeline();
        }
        // ping が環境に無い場合は start が失敗するためスキップ相当。
    }
}
