//! MAA 型チェックボックスタスク一覧 UI のドメインロジック集約モジュール (Issue #144)。
//!
//! `app.rs` は配線のみに徹し、タスク定義の読み込み・パース・キュー組み立ては
//! 本モジュールに完全分離する (architecture-coupling-balance: high-cohesion)。
//!
//! - TaskDefinition: `templates/tasks/*.toml` の 1 タスク定義。
//! - TaskKind: 異種実行経路の分岐 (`LaunchSubcommand` = `anaden launch` standalone、
//!   `PipelineRun` = `anaden run <pipeline_dir>` )。
//! - TaskQueue: チェックボックス選択から実行順序を組み立てるキュー。
//! - fail-closed: TOML 欠損・不正 kind はエラー。`implemented = false` は選択不可
//!   (グレー表示) — 嘘の動作可能表示は禁止 (CEO 確定)。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// タスク定義の読み込み・パース・キュー組み立てに関するエラー。
#[derive(Debug, Error)]
pub enum TaskError {
    /// タスク定義ディレクトリが存在しない / 読めない。
    #[error("task directory not found or unreadable: {0}")]
    Directory(String),
    /// ディレクトリ内にタスク定義 TOML が 1 つも無い。
    #[error("no task definitions (*.toml) found in: {0}")]
    NoDefinitions(String),
    /// 個別 TOML ファイルの読み込み失敗 (IO)。
    #[error("failed to read task file {path}: {source}")]
    Read {
        path: PathBuf,
        source: Box<std::io::Error>,
    },
    /// 個別 TOML ファイルのパース失敗。
    #[error("failed to parse task file {path}: {source}")]
    Parse {
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
    /// 不正な kind 文字列 (fail-closed)。
    #[error("invalid kind {kind:?} in {path}: expected \"launch_subcommand\" or \"pipeline_run\"")]
    InvalidKind { kind: String, path: PathBuf },
    /// pipeline_run タスクなのに pipeline_dir が未定義。
    #[error("pipeline_run task {id:?} is missing `pipeline_dir`")]
    MissingPipelineDir { id: String },
    /// 未実装タスクを選択しようとした (チェック不可・グレー表示)。
    #[error("task {id:?} is not implemented (implemented = false) and cannot be selected")]
    NotImplemented { id: String },
    /// 選択に未知のタスク ID が含まれている。
    #[error("unknown task id: {0}")]
    UnknownTask(String),
}

/// タスクの実行経路種別。
///
/// MAA 型 UI では「ゲーム起動」(standalone サブコマンド) と
/// 「パイプライン周回」(pipeline 実行) が異種経路として混在するため、
/// kind で分岐を明示する (estimate 確定要件)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// `anaden launch [--target <t>]` standalone サブコマンド経由。
    LaunchSubcommand,
    /// `anaden run <pipeline_dir> <start_task>` パイプライン経由。
    PipelineRun,
}

impl TaskKind {
    /// kind 文字列からのパース。未知の文字列は [`TaskError::InvalidKind`] で fail-closed。
    pub fn parse(s: &str, path: &Path) -> Result<Self, TaskError> {
        match s {
            "launch_subcommand" => Ok(Self::LaunchSubcommand),
            "pipeline_run" => Ok(Self::PipelineRun),
            _ => Err(TaskError::InvalidKind {
                kind: s.to_string(),
                path: path.to_path_buf(),
            }),
        }
    }

    /// TOML シリアライズ用の kind 文字列。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LaunchSubcommand => "launch_subcommand",
            Self::PipelineRun => "pipeline_run",
        }
    }
}

/// `templates/tasks/*.toml` の (デシリアライズ直後) 生構造。
#[derive(Debug, Deserialize)]
struct RawTaskDefinition {
    id: String,
    title: String,
    kind: String,
    #[serde(default)]
    implemented: bool,
    /// `kind = "pipeline_run"` 時必須。
    #[serde(default)]
    pipeline_dir: Option<String>,
    /// `kind = "pipeline_run"` 時の開始タスク名 (任意)。
    #[serde(default)]
    start_task: Option<String>,
}

/// 検証済みタスク定義。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDefinition {
    pub id: String,
    pub title: String,
    pub kind: TaskKind,
    /// `false` のタスクは選択不可 (グレー表示)。
    pub implemented: bool,
    /// [`TaskKind::PipelineRun`] 時のパイプラインディレクトリ。
    pub pipeline_dir: Option<PathBuf>,
    /// [`TaskKind::PipelineRun`] 時の開始タスク名。
    pub start_task: Option<String>,
}

impl TaskDefinition {
    /// 選択 (チェック) 可能か。`implemented = false` は常に不可。
    pub fn is_selectable(&self) -> bool {
        self.implemented
    }

    /// TOML 文字列からパース (fail-closed: 不正 kind / pipeline_dir 欠損はエラー)。
    pub fn parse_toml(source: &str, path: &Path) -> Result<Self, TaskError> {
        let raw: RawTaskDefinition = toml::from_str(source).map_err(|source| TaskError::Parse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;
        let kind = TaskKind::parse(&raw.kind, path)?;
        let pipeline_dir = match kind {
            TaskKind::PipelineRun => Some(
                raw.pipeline_dir
                    .ok_or_else(|| TaskError::MissingPipelineDir { id: raw.id.clone() })?
                    .into(),
            ),
            TaskKind::LaunchSubcommand => raw.pipeline_dir.map(Into::into),
        };
        Ok(Self {
            id: raw.id,
            title: raw.title,
            kind,
            implemented: raw.implemented,
            pipeline_dir,
            start_task: raw.start_task,
        })
    }
}

/// ディレクトリから全タスク定義を読み込む。
///
/// TOML ファイルが 1 つも無い場合は [`TaskError::NoDefinitions`] で fail-closed
/// (空リストの黙黙継続はしない)。
pub fn load_task_definitions(dir: &Path) -> Result<Vec<TaskDefinition>, TaskError> {
    let entries =
        std::fs::read_dir(dir).map_err(|_| TaskError::Directory(dir.display().to_string()))?;
    let mut defs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let source = std::fs::read_to_string(&path).map_err(|source| TaskError::Read {
            path: path.clone(),
            source: Box::new(source),
        })?;
        defs.push(TaskDefinition::parse_toml(&source, &path)?);
    }
    if defs.is_empty() {
        return Err(TaskError::NoDefinitions(dir.display().to_string()));
    }
    // ファイル名順で決定論的な表示順を保証。
    defs.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(defs)
}

/// チェックボックス選択から組み立てられた実行キュー。
///
/// 選択順 (チェック順) を維持した `Vec<TaskId>` を持ち、開始時に検証済み
/// [`TaskDefinition`] との突合を行う。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TaskQueue {
    /// チェック順に保持する選択済みタスク ID。
    selected: Vec<String>,
}

impl TaskQueue {
    pub fn new() -> Self {
        Self {
            selected: Vec::new(),
        }
    }

    /// チェックボックストグル。`implemented = false` は [`TaskError::NotImplemented`]
    /// で拒否 (グレー表示 = 選択不可の機械的保証)。
    pub fn toggle(&mut self, def: &TaskDefinition) -> Result<(), TaskError> {
        if !def.is_selectable() {
            return Err(TaskError::NotImplemented { id: def.id.clone() });
        }
        if let Some(pos) = self.selected.iter().position(|id| id == &def.id) {
            self.selected.remove(pos);
        } else {
            self.selected.push(def.id.clone());
        }
        Ok(())
    }

    /// 現在の選択済みタスク ID 一覧 (チェック順)。
    pub fn selected_ids(&self) -> &[String] {
        &self.selected
    }

    /// 実行順序組み立て: チェック順にタスク定義を解決した実行リストを返す。
    ///
    /// 未知の ID は [`TaskError::UnknownTask`] で fail-closed。
    pub fn build(&self, defs: &[TaskDefinition]) -> Result<Vec<TaskDefinition>, TaskError> {
        let by_id: BTreeMap<&str, &TaskDefinition> =
            defs.iter().map(|d| (d.id.as_str(), d)).collect();
        self.selected
            .iter()
            .map(|id| {
                by_id
                    .get(id.as_str())
                    .copied()
                    .cloned()
                    .ok_or_else(|| TaskError::UnknownTask(id.clone()))
            })
            .collect()
    }

    /// 選択クリア。
    pub fn clear(&mut self) {
        self.selected.clear();
    }
}

/// チェックボックスの表示ラベル (豆腐なし ASCII 括弧 + 日本語)。
/// implemented=false には「未実装」接尾を付けグレー表示 (嘘の動作可能表示禁止)。
#[must_use]
pub fn checkbox_label(def: &TaskDefinition) -> String {
    format!(
        "{} ({}){}",
        def.title,
        def.kind.as_str(),
        if def.implemented {
            String::new()
        } else {
            " — 未実装".to_string()
        }
    )
}

/// タスク一覧 UI の状態機械 (app.rs 配線用・Issue #144 Task 3)。
///
/// 定義リスト + 選択キュー + SpawnSpec 組み立てを集約し、app.rs は
/// この構造体の呼び出しのみに徹する (行数上限: app.rs 増分 < 100 行)。
#[derive(Debug, Default, Clone)]
pub struct TaskListState {
    defs: Vec<TaskDefinition>,
    queue: TaskQueue,
}

impl TaskListState {
    /// ディレクトリから定義を読み込んで生成する。
    pub fn load(dir: &Path) -> Result<Self, TaskError> {
        Ok(Self {
            defs: load_task_definitions(dir)?,
            queue: TaskQueue::new(),
        })
    }

    /// 読み込み済み定義一覧 (id 辞書順)。
    pub fn definitions(&self) -> &[TaskDefinition] {
        &self.defs
    }

    /// ID で定義を引く。
    pub fn find(&self, id: &str) -> Option<&TaskDefinition> {
        self.defs.iter().find(|d| d.id == id)
    }

    /// チェック状態。未読込・未知 ID は false。
    pub fn is_selected(&self, id: &str) -> bool {
        self.queue.selected_ids().iter().any(|s| s == id)
    }

    /// 選択済みタスク数。
    pub fn selected_count(&self) -> usize {
        self.queue.selected_ids().len()
    }

    /// チェックトグル。未知 ID は [`TaskError::UnknownTask`]、未実装は
    /// [`TaskError::NotImplemented`] (グレー表示 = 選択不可の機械的保証)。
    pub fn toggle(&mut self, id: &str) -> Result<(), TaskError> {
        let def = self
            .find(id)
            .ok_or_else(|| TaskError::UnknownTask(id.to_string()))?
            .clone();
        self.queue.toggle(&def)
    }

    /// 選択キューから Kind 分岐済み SpawnSpec 列を組み立てる。
    ///
    /// 各タスクは [`spawn_args`] で引数列を組み立て、`program` と対にする。
    /// 引数解決不能 (pipeline_dir 実在せず start_task 不明) のタスクがある場合は
    /// エラー (fail-closed: 一部だけ実行しない)。
    ///
    /// Issue #154 Shard 1: 実装は [`Self::queue_entries`] (ラベル付き) に委譲する
    /// 単一情報源化。
    pub fn spawn_specs(
        &self,
        program: &str,
        target: &str,
        serial: Option<&str>,
        root: &Path,
    ) -> Result<Vec<crate::childproc::SpawnSpec>, TaskError> {
        Ok(self
            .queue_entries(program, target, serial, root)?
            .into_iter()
            .map(|e| e.spec)
            .collect())
    }

    /// 選択キューから表示ラベル付き実行エントリ列を組み立てる (Issue #154 UC-2)。
    ///
    /// [`Self::spawn_specs`] のラベル付き版。チェック順 = 実行順序を維持し、
    /// ラベルはタスク定義の title (UC-4 の進行表示・ログセパレータに使用)。
    /// 引数解決不能のタスクがある場合はエラー (fail-closed)。
    pub fn queue_entries(
        &self,
        program: &str,
        target: &str,
        serial: Option<&str>,
        root: &Path,
    ) -> Result<Vec<QueueEntry>, TaskError> {
        let built = self.queue.build(&self.defs)?;
        if built.is_empty() {
            return Ok(Vec::new());
        }
        built
            .iter()
            .map(|def| {
                let args = spawn_args(def, target, serial, root);
                if args.is_empty() {
                    return Err(TaskError::UnknownTask(format!(
                        "タスク {} の実行引数を解決できません (pipeline_dir/start_task 不明)",
                        def.id
                    )));
                }
                Ok(QueueEntry {
                    label: def.title.clone(),
                    spec: crate::childproc::SpawnSpec::new(program, args),
                })
            })
            .collect()
    }
}

// ---- Issue #154 Shard 1 (T2): チェック順逐次実行キューの純状態機械 ----

/// 実行キューの 1 エントリ (表示ラベル + 起動指定)。
///
/// エントリ列はチェック順を維持する。開始後のチェックボックス変更は
/// キュー側に反映されない (開始時スナップショット・UC-2)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEntry {
    /// 進行表示・ログセパレータ用ラベル (タスク定義の title)。
    pub label: String,
    /// 子プロセス起動指定。
    pub spec: crate::childproc::SpawnSpec,
}

/// キュー実行の状態 (イベント駆動のみで遷移)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueState {
    /// チェック順エントリ保全・開始待ち。
    Pending,
    /// `current` (0-based) 番目を実行中。
    Running { current: usize },
    /// `current` 番目が非零/不明終了し、明示的な継続判断待ち (UC-4)。
    PausedAfterFailure {
        current: usize,
        /// 失敗タスクの exit code (wait 失敗等は None)。
        exit_code: Option<i32>,
    },
    /// 全完了または abort による終端状態。
    Completed,
}

/// 状態遷移の結果として呼び出し側が実行すべきアクション。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueAction {
    /// 次タスクを起動する (チェック順・当該 spec)。
    Start(crate::childproc::SpawnSpec),
    /// 何も起動せず待つ (実行中の子の終了待ち・失敗停止)。
    WaitForExit,
    /// 全タスクが完了した。
    QueueCompleted,
    /// 遷移なし (無効状態でのイベント・拒否された操作)。
    Noop,
}

/// チェック順逐次実行キューの純状態機械 (Issue #154 Shard 1 / UC-2・UC-4)。
///
/// 1 タスクずつ子プロセスを起動し、その終了を呼び出し側が `LogEvent::Exit`
/// 観測として [`QueueExec::on_exit`] で通知するまで次へ進まない。
/// `ChildProcess::is_running` は Exit drain 前に false になる競合があるため、
/// 完了判定は Exit 観測のみを唯一のシグナルとする
/// (runner.rs `drain_logs` と同じ先行実装パターン)。
///
/// 失敗 (非零 exit / 不明) では**自動継続しない**: `PausedAfterFailure` で
/// 停止し、[`QueueExec::resume`] の明示呼び出しでのみ次タスクを開始する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueExec {
    state: QueueState,
    entries: Vec<QueueEntry>,
    /// `abort()` で終端化した際の表示区別用 (状態遷移には関与しない)。
    aborted: bool,
}

impl QueueExec {
    /// チェック順エントリ列から生成する。
    pub fn new(entries: Vec<QueueEntry>) -> Self {
        Self {
            state: QueueState::Pending,
            entries,
            aborted: false,
        }
    }

    /// 現在状態。
    pub fn state(&self) -> &QueueState {
        &self.state
    }

    /// 全エントリ数。
    pub fn total(&self) -> usize {
        self.entries.len()
    }

    /// チェック順全エントリ (キュー一覧表示用)。abort 後は空。
    pub fn entries(&self) -> &[QueueEntry] {
        &self.entries
    }

    /// 現在対象 (Running / PausedAfterFailure) のエントリ。
    pub fn current_entry(&self) -> Option<&QueueEntry> {
        match self.state {
            QueueState::Running { current } | QueueState::PausedAfterFailure { current, .. } => {
                self.entries.get(current)
            }
            QueueState::Pending | QueueState::Completed => None,
        }
    }

    /// 開始: Pending → Running (current=0)。最初のエントリの起動を指示する。
    ///
    /// 空キューの開始、および Pending 以外での呼び出し (実行中の再開始) は
    /// 拒否する ([`QueueAction::Noop`]・状態不変)。
    pub fn start(&mut self) -> QueueAction {
        if !matches!(self.state, QueueState::Pending) {
            return QueueAction::Noop;
        }
        let Some(first) = self.entries.first() else {
            return QueueAction::Noop; // 空キュー開始は拒否
        };
        self.state = QueueState::Running { current: 0 };
        QueueAction::Start(first.spec.clone())
    }

    /// 実行中タスクの終了観測 (`LogEvent::Exit` が唯一の完了シグナル)。
    ///
    /// Running 以外での呼び出しは [`QueueAction::Noop`]。
    /// - `Some(0)`: 次エントリの起動 (`Start`)、残り無しなら完了 (`QueueCompleted`)
    /// - 非零 / `None`: 失敗停止 (`PausedAfterFailure` + `WaitForExit`)。
    ///   **自動継続禁止** — 継続は [`QueueExec::resume`] の明示呼び出し専用。
    pub fn on_exit(&mut self, code: Option<i32>) -> QueueAction {
        let QueueState::Running { current } = self.state else {
            return QueueAction::Noop;
        };
        if code == Some(0) {
            self.advance_from(current)
        } else {
            self.state = QueueState::PausedAfterFailure {
                current,
                exit_code: code,
            };
            QueueAction::WaitForExit
        }
    }

    /// 失敗停止からの明示継続: 次エントリの起動 or 完了。
    /// PausedAfterFailure 以外での呼び出しは [`QueueAction::Noop`]。
    pub fn resume(&mut self) -> QueueAction {
        let QueueState::PausedAfterFailure { current, .. } = self.state else {
            return QueueAction::Noop;
        };
        self.advance_from(current)
    }

    /// 全破棄 (任意状態 → 終端)。エントリをクリアし以降の遷移を停止する。
    /// 実行中の子プロセスの停止は呼び出し側の責務 (UI が stop してから呼ぶ)。
    pub fn abort(&mut self) {
        self.aborted = true;
        self.state = QueueState::Completed;
        self.entries.clear();
    }

    /// abort により中止済みか (完了表示との区別用)。
    pub fn is_aborted(&self) -> bool {
        self.aborted
    }

    /// 進行サマリ (UC-4 の i/N 表示用・純関数)。
    #[must_use]
    pub fn summary(&self) -> String {
        if self.aborted {
            return "中止".to_string();
        }
        match &self.state {
            QueueState::Pending => format!("待機: {} タスク", self.entries.len()),
            QueueState::Running { current } => format!(
                "実行中 {}/{}: {}",
                current + 1,
                self.entries.len(),
                self.entries
                    .get(*current)
                    .map(|e| e.label.as_str())
                    .unwrap_or("?")
            ),
            QueueState::PausedAfterFailure { current, exit_code } => {
                let code_disp = match exit_code {
                    Some(c) => format!("exit={c}"),
                    None => "exit=不明".to_string(),
                };
                format!(
                    "失敗停止 {}/{} ({code_disp}): {} — 継続/停止を選択",
                    current + 1,
                    self.entries.len(),
                    self.entries
                        .get(*current)
                        .map(|e| e.label.as_str())
                        .unwrap_or("?")
                )
            }
            QueueState::Completed => {
                format!("完了 {}/{}", self.entries.len(), self.entries.len())
            }
        }
    }

    /// エントリ行の進行マーカ (キュー一覧表示用・純関数)。
    #[must_use]
    pub fn entry_marker(&self, index: usize) -> &'static str {
        match &self.state {
            QueueState::Pending => "待機",
            QueueState::Completed => "完了",
            QueueState::Running { current } => {
                if index < *current {
                    "完了"
                } else if index == *current {
                    "実行中"
                } else {
                    "待機"
                }
            }
            QueueState::PausedAfterFailure { current, .. } => {
                if index < *current {
                    "完了"
                } else if index == *current {
                    "失敗"
                } else {
                    "待機"
                }
            }
        }
    }

    /// `current` の次へ進む (Exit(0) と明示継続の共通遷移)。
    fn advance_from(&mut self, current: usize) -> QueueAction {
        match self.entries.get(current + 1) {
            Some(next) => {
                self.state = QueueState::Running {
                    current: current + 1,
                };
                QueueAction::Start(next.spec.clone())
            }
            None => {
                self.state = QueueState::Completed;
                QueueAction::QueueCompleted
            }
        }
    }
}

/// パイプラインディレクトリから開始タスク名を解決する (start_task 未宣言タスク用)。
///
/// `pipeline.toml` (manifest) はタスク定義ではないため除外し、残る TaskDef TOML
/// のファイル名 (stem) を辞書順で最初のものを開始タスクとする。TOML が 1 つも
/// 無い場合は None (呼び出し側で fail-closed 扱い)。
pub fn resolve_start_task(pipeline_dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(pipeline_dir).ok()?;
    let mut stems: Vec<String> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("toml")
                && p.file_stem().and_then(|s| s.to_str()) != Some("pipeline")
        })
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(String::from))
        .collect();
    stems.sort();
    stems.into_iter().next()
}

/// [`TaskDefinition`] から `anaden` CLI サブコマンドの引数列を組み立てる純関数
/// (app.rs 配線用・Issue #144 Task 3)。
///
/// - [`TaskKind::LaunchSubcommand`] → `launch --target <target> [serial]`
///   (android 時のみ serial を付与。Commands::Launch 実署名と突合済み)
/// - [`TaskKind::PipelineRun`] → `run --target <target> <pipeline_dir> <start_task>`
///   (start_task 未宣言時は `resolve_start_task` で解決。解決不能なら空 Vec)
pub fn spawn_args(
    def: &TaskDefinition,
    target: &str,
    serial: Option<&str>,
    root: &Path,
) -> Vec<String> {
    match def.kind {
        TaskKind::LaunchSubcommand => {
            let mut args = vec![
                "launch".to_string(),
                "--target".to_string(),
                target.to_string(),
            ];
            if target == "android"
                && let Some(s) = serial.filter(|s| !s.trim().is_empty())
            {
                args.push(s.trim().to_string());
            }
            args
        }
        TaskKind::PipelineRun => {
            let Some(dir) = &def.pipeline_dir else {
                return Vec::new();
            };
            let abs = root.join(dir);
            let Some(start) = def.start_task.clone().or_else(|| resolve_start_task(&abs)) else {
                return Vec::new();
            };
            vec![
                "run".to_string(),
                "--target".to_string(),
                target.to_string(),
                abs.to_string_lossy().into_owned(),
                start,
            ]
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    const LAUNCH_TOML: &str = r#"
id = "launch"
title = "ゲーム起動"
kind = "launch_subcommand"
implemented = true
"#;

    const FIELD_LOOP_TOML: &str = r#"
id = "field_loop_pc"
title = "フィールド周回"
kind = "pipeline_run"
implemented = true
pipeline_dir = "templates/pipelines/field_loop_pc"
start_task = "start"
"#;

    const LOGIN_TOML: &str = r#"
id = "login"
title = "ログイン"
kind = "pipeline_run"
implemented = false
pipeline_dir = "templates/pipelines/login"
"#;

    fn parse_all(sources: &[(&str, &str)]) -> Vec<TaskDefinition> {
        sources
            .iter()
            .map(|(name, src)| TaskDefinition::parse_toml(src, Path::new(name)).unwrap())
            .collect()
    }

    // ---- 正常系: implemented タスクのキュー組み立て ----

    #[test]
    fn test_parses_launch_subcommand_task() {
        let def = TaskDefinition::parse_toml(LAUNCH_TOML, Path::new("launch.toml")).unwrap();
        assert_eq!(def.id, "launch");
        assert_eq!(def.title, "ゲーム起動");
        assert_eq!(def.kind, TaskKind::LaunchSubcommand);
        assert!(def.implemented);
        assert!(def.pipeline_dir.is_none());
        assert!(def.is_selectable());
    }

    #[test]
    fn test_parses_pipeline_run_task_with_dir() {
        let def =
            TaskDefinition::parse_toml(FIELD_LOOP_TOML, Path::new("field_loop_pc.toml")).unwrap();
        assert_eq!(def.kind, TaskKind::PipelineRun);
        assert_eq!(
            def.pipeline_dir.as_ref().unwrap(),
            Path::new("templates/pipelines/field_loop_pc")
        );
        assert_eq!(def.start_task.as_deref(), Some("start"));
    }

    #[test]
    fn test_builds_queue_in_check_order() {
        let defs = parse_all(&[
            ("launch.toml", LAUNCH_TOML),
            ("field_loop_pc.toml", FIELD_LOOP_TOML),
        ]);
        let mut queue = TaskQueue::new();
        queue.toggle(&defs[0]).unwrap(); // launch (parse_all は入力順・ソート無し)
        queue.toggle(&defs[1]).unwrap(); // field_loop_pc
        let built = queue.build(&defs).unwrap();
        let ids: Vec<&str> = built.iter().map(|d| d.id.as_str()).collect();
        // チェック順 = 実行順序 (launch を先にチェック)
        assert_eq!(ids, vec!["launch", "field_loop_pc"]);
        assert_eq!(built[0].kind, TaskKind::LaunchSubcommand);
        assert_eq!(built[1].kind, TaskKind::PipelineRun);
    }

    #[test]
    fn test_toggle_off_removes_from_queue() {
        let defs = parse_all(&[
            ("launch.toml", LAUNCH_TOML),
            ("field_loop_pc.toml", FIELD_LOOP_TOML),
        ]);
        let mut queue = TaskQueue::new();
        queue.toggle(&defs[0]).unwrap();
        queue.toggle(&defs[1]).unwrap();
        queue.toggle(&defs[0]).unwrap(); // launch 側を外す
        assert_eq!(queue.selected_ids(), &["field_loop_pc".to_string()]);
        assert_eq!(queue.build(&defs).unwrap().len(), 1);
    }

    #[test]
    fn test_load_definitions_from_directory_sorted_by_id() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("zz_launch.toml"), LAUNCH_TOML).unwrap();
        std::fs::write(tmp.path().join("aa_field.toml"), FIELD_LOOP_TOML).unwrap();
        std::fs::write(tmp.path().join("readme.txt"), "not a task").unwrap();
        let defs = load_task_definitions(tmp.path()).unwrap();
        let ids: Vec<&str> = defs.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, vec!["field_loop_pc", "launch"]); // id ソート
    }

    // ---- エッジケース: fail-closed ----

    #[test]
    fn test_not_implemented_task_cannot_be_toggled() {
        let defs = parse_all(&[("login.toml", LOGIN_TOML)]);
        assert!(!defs[0].is_selectable());
        let mut queue = TaskQueue::new();
        let err = queue.toggle(&defs[0]).unwrap_err();
        assert!(
            matches!(err, TaskError::NotImplemented { ref id } if id == "login"),
            "unexpected: {err:?}"
        );
        assert!(queue.selected_ids().is_empty());
    }

    #[test]
    fn test_invalid_kind_is_fail_closed_error() {
        let bad = r#"
id = "x"
title = "X"
kind = "magic"
implemented = true
"#;
        let err = TaskDefinition::parse_toml(bad, Path::new("x.toml")).unwrap_err();
        assert!(matches!(err, TaskError::InvalidKind { .. }), "got {err:?}");
    }

    #[test]
    fn test_missing_toml_directory_or_file_fails_closed() {
        let err = load_task_definitions(Path::new("nonexistent-dir-xyz")).unwrap_err();
        assert!(matches!(err, TaskError::Directory(_)), "got {err:?}");

        // ディレクトリは存在するが TOML が無い
        let tmp = tempfile::tempdir().unwrap();
        let err = load_task_definitions(tmp.path()).unwrap_err();
        assert!(matches!(err, TaskError::NoDefinitions(_)), "got {err:?}");
    }

    #[test]
    fn test_pipeline_run_without_pipeline_dir_is_error() {
        let bad = r#"
id = "p"
title = "P"
kind = "pipeline_run"
implemented = true
"#;
        let err = TaskDefinition::parse_toml(bad, Path::new("p.toml")).unwrap_err();
        assert!(matches!(err, TaskError::MissingPipelineDir { .. }));
    }

    #[test]
    fn test_build_with_unknown_selected_id_fails_closed() {
        let defs = parse_all(&[("launch.toml", LAUNCH_TOML)]);
        let mut queue = TaskQueue::new();
        queue.toggle(&defs[0]).unwrap();
        queue.selected.push("ghost".to_string()); // 不整合注入
        let err = queue.build(&defs).unwrap_err();
        assert!(matches!(err, TaskError::UnknownTask(ref id) if id == "ghost"));
    }

    // ---- Issue #144 Task 3: spawn_args (実行経路分岐の引数組み立て) ----

    /// タスク定義一覧 TOML (リポジトリ実ファイル) から全定義をパースできる。
    #[test]
    fn test_repo_task_definitions_all_parse() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let defs = load_task_definitions(&root.join("templates/tasks")).unwrap();
        let ids: Vec<&str> = defs.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "field_loop_pc",
                "fishing",
                "launch",
                "login",
                "nav_to_field_pc",
                "worldmap_loop",
            ]
        );
        let selectable: Vec<&str> = defs
            .iter()
            .filter(|d| d.is_selectable())
            .map(|d| d.id.as_str())
            .collect();
        assert_eq!(
            selectable,
            vec![
                "field_loop_pc",
                "launch",
                "login",
                "nav_to_field_pc",
                "worldmap_loop"
            ]
        );
    }

    #[test]
    fn test_spawn_args_launch_subcommand_windows_no_serial() {
        let def = TaskDefinition::parse_toml(LAUNCH_TOML, Path::new("launch.toml")).unwrap();
        let args = spawn_args(&def, "windows", Some("ignored"), Path::new("/root"));
        assert_eq!(args, vec!["launch", "--target", "windows"]);
    }

    #[test]
    fn test_spawn_args_launch_subcommand_android_appends_serial() {
        let def = TaskDefinition::parse_toml(LAUNCH_TOML, Path::new("launch.toml")).unwrap();
        let args = spawn_args(&def, "android", Some("localhost:5555"), Path::new("/root"));
        assert_eq!(
            args,
            vec!["launch", "--target", "android", "localhost:5555"]
        );
    }

    #[test]
    fn test_spawn_args_launch_android_empty_serial_omitted() {
        let def = TaskDefinition::parse_toml(LAUNCH_TOML, Path::new("launch.toml")).unwrap();
        let args = spawn_args(&def, "android", Some("  "), Path::new("/root"));
        assert_eq!(args, vec!["launch", "--target", "android"]);
    }

    /// start_task 宣言済みタスク: 宣言値をそのまま使う。
    #[test]
    fn test_spawn_args_pipeline_run_with_declared_start_task() {
        let def =
            TaskDefinition::parse_toml(FIELD_LOOP_TOML, Path::new("field_loop_pc.toml")).unwrap();
        let args = spawn_args(&def, "windows", None, Path::new("/root"));
        // FIELD_LOOP_TOML は start_task = "start" を宣言 (実リポジトリ TOML の
        // "TapBottomStablePc" ではなくテスト定義の宣言値が使われること)。
        assert_eq!(
            args,
            vec![
                "run",
                "--target",
                "windows",
                "/root\\templates/pipelines/field_loop_pc",
                "start",
            ]
        );
    }

    /// start_task 未宣言タスク: resolve_start_task が pipeline_dir 内の
    /// TaskDef TOML の辞書順最初の stem を解決する。
    #[test]
    fn test_resolve_start_task_picks_first_taskdef_stem() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("pipeline.toml"), "manifest = true").unwrap();
        std::fs::write(tmp.path().join("zz_last.toml"), "x = 1").unwrap();
        std::fs::write(tmp.path().join("aa_first.toml"), "x = 1").unwrap();
        assert_eq!(resolve_start_task(tmp.path()).as_deref(), Some("aa_first"));
        assert_eq!(resolve_start_task(Path::new("nonexistent-xyz")), None);
    }

    /// start_task 未宣言 + TaskDef TOML が実在する pipeline (リポジトリ実ファイル)
    /// でも解決できることの結合検証 (nav_to_field_pc)。
    #[test]
    fn test_spawn_args_pipeline_run_resolves_start_task_from_dir() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let src = r#"
id = "nav_to_field_pc"
title = "マップ移動"
kind = "pipeline_run"
implemented = true
pipeline_dir = "templates/pipelines/nav_to_field_pc"
"#;
        let def = TaskDefinition::parse_toml(src, Path::new("nav_to_field_pc.toml")).unwrap();
        let args = spawn_args(&def, "windows", None, &root);
        assert_eq!(args.len(), 5);
        assert_eq!(
            &args[0..3],
            &[
                "run".to_string(),
                "--target".to_string(),
                "windows".to_string()
            ]
        );
        assert!(
            args[3].ends_with("templates\\pipelines\\nav_to_field_pc")
                || args[3].ends_with("templates/pipelines/nav_to_field_pc")
        );
        // 辞書順最初の TaskDef: field_hud_top
        assert_eq!(args[4], "field_hud_top");
    }

    /// pipeline_dir が実在せず start_task も解決不能なら空 Vec (fail-closed)。
    #[test]
    fn test_spawn_args_pipeline_run_missing_dir_and_task_returns_empty() {
        let src = r#"
id = "ghost"
title = "G"
kind = "pipeline_run"
implemented = true
pipeline_dir = "templates/pipelines/nonexistent-xyz"
"#;
        let def = TaskDefinition::parse_toml(src, Path::new("ghost.toml")).unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        assert!(spawn_args(&def, "windows", None, &root).is_empty());
    }

    // ---- Issue #154 Shard 1 (T2): QueueExec チェック順逐次実行の純状態機械 ----

    fn queue_entry(label: &str, arg0: &str) -> QueueEntry {
        QueueEntry {
            label: label.to_string(),
            spec: crate::childproc::SpawnSpec::new("anaden", vec![arg0.to_string()]),
        }
    }

    /// 正常系: start はチェック順どおり最初のエントリを起動する。
    #[test]
    fn test_queue_start_returns_first_entry_in_check_order() {
        let mut q = QueueExec::new(vec![
            queue_entry("ゲーム起動", "launch"),
            queue_entry("周回", "run"),
        ]);
        assert!(matches!(q.state(), QueueState::Pending));
        let action = q.start();
        let QueueAction::Start(spec) = action else {
            panic!("start must return Start, got {action:?}");
        };
        assert_eq!(spec.args[0], "launch");
        assert!(matches!(q.state(), QueueState::Running { current: 0 }));
        assert_eq!(q.total(), 2);
    }

    /// 正常系: Exit(0) 観測で次タスクを起動する。
    #[test]
    fn test_queue_exit_zero_starts_next_task() {
        let mut q = QueueExec::new(vec![queue_entry("A", "launch"), queue_entry("B", "run")]);
        let _ = q.start();
        let QueueAction::Start(spec) = q.on_exit(Some(0)) else {
            panic!("on_exit(0) must return Start");
        };
        assert_eq!(spec.args[0], "run");
        assert!(matches!(q.state(), QueueState::Running { current: 1 }));
    }

    /// 正常系: 最終タスクの Exit(0) でキュー完了に到達する。
    #[test]
    fn test_queue_exit_zero_on_last_completes_queue() {
        let mut q = QueueExec::new(vec![queue_entry("A", "launch"), queue_entry("B", "run")]);
        let _ = q.start();
        let _ = q.on_exit(Some(0));
        assert!(matches!(q.on_exit(Some(0)), QueueAction::QueueCompleted));
        assert!(matches!(q.state(), QueueState::Completed));
    }

    /// 正常系: 失敗停止からの明示 resume は次タスクを起動する。
    #[test]
    fn test_queue_resume_after_failure_starts_next() {
        let mut q = QueueExec::new(vec![queue_entry("A", "launch"), queue_entry("B", "run")]);
        let _ = q.start();
        let _ = q.on_exit(Some(1)); // A 失敗 → 停止
        let QueueAction::Start(spec) = q.resume() else {
            panic!("resume must return Start");
        };
        assert_eq!(spec.args[0], "run");
        assert!(matches!(q.state(), QueueState::Running { current: 1 }));
    }

    /// 正常系: 最終タスクの失敗での resume は残り無しと判断して完了する。
    #[test]
    fn test_queue_resume_on_last_failure_completes_queue() {
        let mut q = QueueExec::new(vec![queue_entry("A", "launch")]);
        let _ = q.start();
        let _ = q.on_exit(Some(1));
        assert!(matches!(q.resume(), QueueAction::QueueCompleted));
        assert!(matches!(q.state(), QueueState::Completed));
    }

    /// 正常系 (UC-4 表示): summary は i/N 進行とラベルを含む。
    #[test]
    fn test_queue_summary_shows_progress() {
        let mut q = QueueExec::new(vec![
            queue_entry("周回", "run"),
            queue_entry("起動", "launch"),
        ]);
        assert_eq!(q.summary(), "待機: 2 タスク");
        let _ = q.start();
        assert!(q.summary().contains("1/2"), "summary: {}", q.summary());
        assert!(q.summary().contains("周回"), "summary: {}", q.summary());
        let _ = q.on_exit(Some(0));
        assert!(q.summary().contains("2/2"), "summary: {}", q.summary());
        let _ = q.on_exit(Some(3));
        assert!(q.summary().contains("exit=3"), "summary: {}", q.summary());
        let _ = q.resume();
        assert_eq!(q.summary(), "完了 2/2");
    }

    /// 正常系 (UC-4 表示): エントリ行マーカは完了/実行中/待機/失敗を区別する。
    #[test]
    fn test_queue_entry_marker_progression() {
        let mut q = QueueExec::new(vec![queue_entry("A", "a"), queue_entry("B", "b")]);
        assert_eq!(q.entry_marker(0), "待機");
        let _ = q.start();
        assert_eq!(q.entry_marker(0), "実行中");
        assert_eq!(q.entry_marker(1), "待機");
        let _ = q.on_exit(Some(0));
        assert_eq!(q.entry_marker(0), "完了");
        assert_eq!(q.entry_marker(1), "実行中");
        let _ = q.on_exit(Some(1));
        assert_eq!(q.entry_marker(1), "失敗");
        let _ = q.resume();
        assert_eq!(q.entry_marker(0), "完了");
        assert_eq!(q.entry_marker(1), "完了");
    }

    /// エッジケース: 空キューの開始は拒否され状態は Pending のまま。
    #[test]
    fn test_queue_start_empty_is_rejected() {
        let mut q = QueueExec::new(Vec::new());
        assert!(matches!(q.start(), QueueAction::Noop));
        assert!(matches!(q.state(), QueueState::Pending));
        assert_eq!(q.total(), 0);
    }

    /// エッジケース: 実行中 (Pending 以外) での再開始は拒否される。
    #[test]
    fn test_queue_start_while_running_is_rejected() {
        let mut q = QueueExec::new(vec![queue_entry("A", "a")]);
        let _ = q.start();
        assert!(matches!(q.start(), QueueAction::Noop));
        assert!(matches!(q.state(), QueueState::Running { current: 0 }));
        // 失敗停止中・完了後も同様に拒否。
        let _ = q.on_exit(Some(1));
        assert!(matches!(q.start(), QueueAction::Noop));
        let _ = q.resume();
        assert!(matches!(q.start(), QueueAction::Noop));
    }

    /// エッジケース: 非零 exit は失敗停止 (Start を返さない = 自動継続禁止)。
    #[test]
    fn test_queue_failure_pauses_without_auto_continue() {
        let mut q = QueueExec::new(vec![queue_entry("A", "a"), queue_entry("B", "b")]);
        let _ = q.start();
        assert!(matches!(q.on_exit(Some(2)), QueueAction::WaitForExit));
        assert!(matches!(
            q.state(),
            QueueState::PausedAfterFailure {
                current: 0,
                exit_code: Some(2)
            }
        ));
    }

    /// エッジケース: exit code 不明 (None) も失敗停止扱い。
    #[test]
    fn test_queue_exit_none_code_pauses() {
        let mut q = QueueExec::new(vec![queue_entry("A", "a")]);
        let _ = q.start();
        let _ = q.on_exit(None);
        assert!(matches!(
            q.state(),
            QueueState::PausedAfterFailure {
                current: 0,
                exit_code: None
            }
        ));
    }

    /// エッジケース: 二重 Exit 観測 (失敗停止中の on_exit) は Noop。
    #[test]
    fn test_queue_on_exit_paused_is_noop() {
        let mut q = QueueExec::new(vec![queue_entry("A", "a")]);
        let _ = q.start();
        let _ = q.on_exit(Some(1));
        assert!(matches!(q.on_exit(Some(0)), QueueAction::Noop));
        assert!(matches!(q.state(), QueueState::PausedAfterFailure { .. }));
    }

    /// エッジケース: abort は任意状態から全破棄する (子の停止は呼び出し側)。
    #[test]
    fn test_queue_abort_discards_from_any_state() {
        let mut q = QueueExec::new(vec![queue_entry("A", "a"), queue_entry("B", "b")]);
        let _ = q.start();
        q.abort();
        assert!(matches!(q.state(), QueueState::Completed));
        assert!(q.entries().is_empty());
        assert!(q.is_aborted());
        assert_eq!(q.summary(), "中止");
        // Pending からの abort も終端化。
        let mut p = QueueExec::new(vec![queue_entry("A", "a")]);
        p.abort();
        assert!(matches!(p.state(), QueueState::Completed));
    }

    /// 正常系: TaskListState::queue_entries はチェック順にラベル付きで組む。
    #[test]
    fn test_task_list_state_builds_queue_entries_in_check_order() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let mut list = TaskListState::load(&root.join("templates/tasks")).unwrap();
        list.toggle("field_loop_pc").unwrap();
        list.toggle("launch").unwrap();
        let entries = list
            .queue_entries("anaden", "windows", None, &root)
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].label, "フィールド周回");
        assert_eq!(entries[0].spec.args[0], "run");
        assert_eq!(entries[1].label, "ゲーム起動");
        assert_eq!(entries[1].spec.args[0], "launch");
    }
}
