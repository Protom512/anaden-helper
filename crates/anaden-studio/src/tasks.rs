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
//! - enable_task (Issue #160 UC-3): task TOML の implemented フリップ +
//!   pipeline_dir 紐付け書き戻し (コメント保全の外科的行編集・load 検証は
//!   fail-closed)。

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
    /// タスク有効化: ファイル内 `id` が期待と一致しない (誤ファイル書き換え防止)。
    #[error("task id mismatch in {path}: expected {expected:?}, file declares {actual:?}")]
    IdMismatch {
        expected: String,
        actual: String,
        path: PathBuf,
    },
    /// タスク有効化: `pipeline_run` 以外は pipeline 紐付け対象外。
    #[error("task {id:?} is not kind = \"pipeline_run\" and cannot be linked to a pipeline")]
    NotPipelineRun { id: String },
    /// タスク有効化: pipeline_dir が load 不可 (manifest 無し・TaskDef パース不能)。
    #[error("pipeline not loadable: {dir}: {reason}")]
    PipelineNotLoadable { dir: String, reason: String },
    /// タスク有効化: pipeline_dir に TaskDef TOML が 1 つも無い。
    #[error("pipeline has no TaskDef TOMLs: {0}")]
    PipelineNoTaskDefs(String),
    /// タスク有効化: pipeline_dir 文字列が TOML 基本文字列として書き込めない。
    #[error("pipeline_dir {dir:?} cannot be written as a TOML string value")]
    InvalidPipelineDir { dir: String },
    /// タスク有効化: 外科的行編集に失敗 (アンカー欠損・編集結果の検証不一致)。
    #[error("surgical edit of task file {path} failed: {reason}")]
    EditFailed { path: PathBuf, reason: String },
    /// タスクファイルへの書き戻し IO 失敗。
    #[error("failed to write task file {path}: {source}")]
    Write {
        path: PathBuf,
        source: Box<std::io::Error>,
    },
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

// ---- Issue #154 Shard 2 (UC-3): タスク TOML 設定の GUI 可視 (読み取り専用表示モデル) ----

/// 1 タスク定義の GUI 詳細表示モデル (UC-3: 何をするか・引数の可視化)。
///
/// pipeline TOML schema (TaskDef 契約) は一切変更しない — 読み取り専用の
/// 可視化専用構造。実引数プレビューは [`spawn_args`] と同一の解決結果
/// (実行と表示の単一情報源)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDetailView {
    /// タスク定義 ID。
    pub id: String,
    /// 表示ラベル (title)。
    pub title: String,
    /// kind 表示文字列 ([`TaskKind::as_str`])。
    pub kind: &'static str,
    /// pipeline 実行時の対象ディレクトリ (root 結合済みパス文字列)。
    pub pipeline_dir: Option<String>,
    /// 開始タスク名 (未宣言時は [`resolve_start_task`] の解決結果)。
    pub start_task: Option<String>,
    /// 実引数プレビュー (子プロセスへ実際に渡される引数列)。
    pub spawn_args: Vec<String>,
    /// 「未実装」ラベル表示対象 (implemented = false または引数解決不能)。
    pub unimplemented: bool,
    /// 「未実装」ラベルの理由 (fail-closed 表示用)。
    pub unimplemented_reason: Option<String>,
}

impl TaskDetailView {
    /// 引数プレビューの 1 行表示 (スペース結合)。
    #[must_use]
    pub fn args_preview(&self) -> String {
        self.spawn_args.join(" ")
    }
}

/// タスク定義から GUI 詳細表示モデルを組み立てる純関数 (UC-3)。
///
/// - `start_task` は未宣言時に [`resolve_start_task`] の解決結果を反映
///   ([`spawn_args`] と同じ解決 — 実行と表示で同一の引数になる)。
/// - `implemented = false`、または実行に必要な引数が解決不能 (pipeline_dir
///   実在せず start_task 不明) の場合は `unimplemented = true` + 理由を含む
///   (fail-closed: 嘘の実行可能表示をしない)。
#[must_use]
pub fn task_detail_view(
    def: &TaskDefinition,
    target: &str,
    serial: Option<&str>,
    root: &Path,
) -> TaskDetailView {
    let pipeline_dir = def
        .pipeline_dir
        .as_ref()
        .map(|d| root.join(d).to_string_lossy().into_owned());
    let start_task = match def.kind {
        TaskKind::LaunchSubcommand => None, // 実行に start_task を使用しない
        TaskKind::PipelineRun => match &def.start_task {
            Some(s) => Some(s.clone()),
            None => def
                .pipeline_dir
                .as_ref()
                .map(|d| root.join(d))
                .and_then(|abs| resolve_start_task(&abs)),
        },
    };
    let spawn_args = spawn_args(def, target, serial, root);
    let unimplemented_reason = if !def.implemented {
        Some("implemented = false (未実装タスク)".to_string())
    } else if spawn_args.is_empty() {
        Some("実行引数を解決できません (pipeline_dir/start_task 不明)".to_string())
    } else {
        None
    };
    TaskDetailView {
        id: def.id.clone(),
        title: def.title.clone(),
        kind: def.kind.as_str(),
        pipeline_dir,
        start_task,
        spawn_args,
        unimplemented: unimplemented_reason.is_some(),
        unimplemented_reason,
    }
}

/// 選択キュー内の実行順位置ラベル (例: 「実行順 2/3」)。未選択は None (UC-3)。
#[must_use]
pub fn queue_position_label(selected_ids: &[String], id: &str) -> Option<String> {
    let pos = selected_ids.iter().position(|s| s == id)?;
    Some(format!("実行順 {}/{}", pos + 1, selected_ids.len()))
}

/// 選択キューの実行順表示行 (UC-3: チェック順に 1. 2. 3. ... と番号付き)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueOrderRow {
    /// 実行順 (1-based・チェック順)。
    pub position: usize,
    /// タスク定義 ID。
    pub id: String,
    /// 表示ラベル (title)。
    pub title: String,
    /// 実行可能 (false = 未実装のためグレー表示)。
    pub runnable: bool,
}

/// 選択キューから実行順表示行列を組み立てる純関数 (UC-3)。
///
/// 通常 UI では未実装タスクは選択不可能だが、不整合時にもグレー表示用の
/// `runnable = false` 行として残す (fail-closed 表示)。未知の ID は実行時
/// [`TaskQueue::build`] が fail-closed で拒否するため表示では除外する。
#[must_use]
pub fn queue_order_rows(selected_ids: &[String], defs: &[TaskDefinition]) -> Vec<QueueOrderRow> {
    selected_ids
        .iter()
        .enumerate()
        .filter_map(|(i, id)| {
            defs.iter().find(|d| &d.id == id).map(|def| QueueOrderRow {
                position: i + 1,
                id: def.id.clone(),
                title: def.title.clone(),
                runnable: def.implemented,
            })
        })
        .collect()
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

    /// 選択済みタスク ID 一覧 (チェック順 = 実行順序・UC-3 表示用)。
    pub fn selected_ids(&self) -> &[String] {
        self.queue.selected_ids()
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

// ---- Issue #160 Shard 2 (UC-3): タスク有効化 API (task TOML 書き戻し) ----

/// タスク TOML の `implemented` を `true` へフリップし、`pipeline_dir` 紐付けを
/// 書き戻して [`TaskDefinition`] を返す (UC-3: 作成 pipeline をタスクに紐付け、
/// ホーム一覧で選択可能にする)。
///
/// # 設計制約 (コメント保全)
///
/// 既存タスク TOML は先頭に由来コメント (issue 参照等) を持つため
/// `toml::to_string` による全再生成はコメントを落とす。本関数は
/// **implemented 行・pipeline_dir 行のみを外科的に置換する行編集**を行い、
/// コメント行・インラインコメント・キー順・CRLF 改行をそのまま保存する
/// ([`edit_task_toml_source`])。
///
/// # fail-closed (嘘の動作可能表示禁止)
///
/// - ファイル内 `id` が `expected_id` と不一致なら 1 バイトも書き換えない。
/// - `kind = "launch_subcommand"` は pipeline 紐付け対象外として拒否。
/// - `root.join(pipeline_dir_rel)` が load 不可 (manifest `pipeline.toml` 無し /
///   TaskDef ゼロ / TaskDef パース不能) なら有効化を拒否。
/// - 編集後ソースを書き込み前に [`TaskDefinition::parse_toml`] で完全検証し、
///   parse 不能・期待と異なる場合は書き込まない。
///
/// `pipeline_dir_rel` は TOML 規約 (repo root 相対・forward slash) で書き戻す。
/// Windows 区切りの backslash は forward slash へ正規化する。
pub fn enable_task(
    task_path: &Path,
    expected_id: &str,
    pipeline_dir_rel: &str,
    root: &Path,
) -> Result<TaskDefinition, TaskError> {
    let dir_value = normalize_pipeline_dir_rel(pipeline_dir_rel)?;
    let source = std::fs::read_to_string(task_path).map_err(|source| TaskError::Read {
        path: task_path.to_path_buf(),
        source: Box::new(source),
    })?;
    // RawTaskDefinition (緩い検証) で読む: pipeline_dir 未宣言の pipeline_run は
    // 通常 [`TaskDefinition::parse_toml`] が MissingPipelineDir で拒否するが、
    // 有効化は「pipeline_dir をこれから書き込む」操作のため kind 検証までで止める。
    let raw: RawTaskDefinition = toml::from_str(&source).map_err(|source| TaskError::Parse {
        path: task_path.to_path_buf(),
        source: Box::new(source),
    })?;
    if raw.id != expected_id {
        return Err(TaskError::IdMismatch {
            expected: expected_id.to_string(),
            actual: raw.id,
            path: task_path.to_path_buf(),
        });
    }
    if TaskKind::parse(&raw.kind, task_path)? != TaskKind::PipelineRun {
        return Err(TaskError::NotPipelineRun {
            id: expected_id.to_string(),
        });
    }
    // fail-closed: 紐付け先 pipeline が load 可能 (manifest + TaskDef 1 件以上) か。
    let abs_dir = root.join(&dir_value);
    anaden_vision::load_pipeline_manifest(&abs_dir).map_err(|e| {
        TaskError::PipelineNotLoadable {
            dir: dir_value.clone(),
            reason: e.to_string(),
        }
    })?;
    if anaden_vision::load_pipeline(&abs_dir)
        .map_err(|e| TaskError::PipelineNotLoadable {
            dir: dir_value.clone(),
            reason: e.to_string(),
        })?
        .is_empty()
    {
        return Err(TaskError::PipelineNoTaskDefs(dir_value));
    }
    // 外科的行編集 → 書き込み前の完全検証 → 書き戻し。
    let edited = edit_task_toml_source(&source, &dir_value, task_path)?;
    let def = TaskDefinition::parse_toml(&edited, task_path)?;
    if !def.implemented || def.pipeline_dir.as_deref() != Some(Path::new(&dir_value)) {
        return Err(TaskError::EditFailed {
            path: task_path.to_path_buf(),
            reason: format!(
                "edited source must declare implemented = true and pipeline_dir = {dir_value:?}"
            ),
        });
    }
    std::fs::write(task_path, &edited).map_err(|source| TaskError::Write {
        path: task_path.to_path_buf(),
        source: Box::new(source),
    })?;
    Ok(def)
}

/// pipeline_dir 相対パスを TOML 値として書ける形へ正規化する。
///
/// - 前後の空白を除去し、backslash 区切りを TOML 規約の forward slash へ変える。
/// - 空文字・`"`・改行を含む場合は TOML 基本文字列に埋め込めないため
///   [`TaskError::InvalidPipelineDir`] で fail-closed。
fn normalize_pipeline_dir_rel(rel: &str) -> Result<String, TaskError> {
    let normalized = rel.trim().replace('\\', "/");
    if normalized.is_empty()
        || normalized.contains('"')
        || normalized.contains('\n')
        || normalized.contains('\r')
    {
        return Err(TaskError::InvalidPipelineDir {
            dir: rel.to_string(),
        });
    }
    Ok(normalized)
}

/// タスク TOML ソースへ「implemented = true フリップ」「pipeline_dir 書き戻し」を
/// 最小変更の行編集で適用する。
///
/// コメント行・未編集行はバイト単位で素通しし、対象キー行は値部分のみ置換する
/// (インラインコメント・行末改行を保存)。未宣言キーは `kind` 行をアンカーに
/// その直後へ (implemented → pipeline_dir の順で) 挿入する。
fn edit_task_toml_source(
    source: &str,
    pipeline_dir_rel: &str,
    path: &Path,
) -> Result<String, TaskError> {
    let newline = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut lines: Vec<String> = Vec::new();
    let mut kind_at: Option<usize> = None;
    let mut implemented_at: Option<usize> = None;
    let mut pipeline_dir_at: Option<usize> = None;

    for line in source.split_inclusive('\n') {
        if kind_at.is_none() && is_key_line(line, "kind") {
            lines.push(line.to_string());
            kind_at = Some(lines.len() - 1);
        } else if implemented_at.is_none() && is_key_line(line, "implemented") {
            let replaced =
                replace_key_value(line, "implemented", "true").unwrap_or_else(|| line.to_string());
            lines.push(replaced);
            implemented_at = Some(lines.len() - 1);
        } else if pipeline_dir_at.is_none() && is_key_line(line, "pipeline_dir") {
            let replaced =
                replace_key_value(line, "pipeline_dir", &format!("\"{pipeline_dir_rel}\""))
                    .unwrap_or_else(|| line.to_string());
            lines.push(replaced);
            pipeline_dir_at = Some(lines.len() - 1);
        } else {
            lines.push(line.to_string());
        }
    }

    let Some(kind_index) = kind_at else {
        return Err(TaskError::EditFailed {
            path: path.to_path_buf(),
            reason: "no `kind` line to anchor the edit".to_string(),
        });
    };
    if implemented_at.is_none() {
        ensure_terminated(&mut lines, kind_index, newline);
        lines.insert(kind_index + 1, format!("implemented = true{newline}"));
        implemented_at = Some(kind_index + 1);
    }
    if pipeline_dir_at.is_none() {
        // implemented を挿入済みなら必ず Some (直上で代入)、既存行のみの
        // 場合も kind 行以降のアンカーへ挿入する。
        let anchor = implemented_at.unwrap_or(kind_index);
        ensure_terminated(&mut lines, anchor, newline);
        lines.insert(
            anchor + 1,
            format!("pipeline_dir = \"{pipeline_dir_rel}\"{newline}"),
        );
    }
    Ok(lines.concat())
}

/// 行がトップレベルの `key = ...` 行か (コメント行は除外・前方空白は許容)。
fn is_key_line(line: &str, key: &str) -> bool {
    let t = line.trim_start();
    !t.starts_with('#')
        && t.strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
}

/// `key = <旧値> [# コメント]` 行の値部分のみを `new_value` へ置換する
/// (インデント・インラインコメント (直前空白込み)・行末改行を保存)。
/// `key = ...` 行でない場合は `None`。
fn replace_key_value(line: &str, key: &str, new_value: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    let after_eq = trimmed.strip_prefix(key)?.trim_start().strip_prefix('=')?;
    let (terminator, value_part) = split_line_terminator(after_eq);
    let comment_start = match find_comment_start(value_part) {
        // コメント直前の空白 (整形) も含めて保全する。
        Some(i) => value_part[..i].trim_end().len(),
        None => value_part.trim_end().len(),
    };
    let comment = value_part.get(comment_start..).unwrap_or("");
    Some(format!("{indent}{key} = {new_value}{comment}{terminator}"))
}

/// 行末の改行 (CRLF/LF/無し) を分離する。
fn split_line_terminator(s: &str) -> (&str, &str) {
    if let Some(rest) = s.strip_suffix("\r\n") {
        ("\r\n", rest)
    } else if let Some(rest) = s.strip_suffix('\n') {
        ("\n", rest)
    } else {
        ("", s)
    }
}

/// 値部分内のインラインコメント開始位置 (`#` の位置) を返す。
/// TOML 基本文字列の引用符内の `#` は無視する。
fn find_comment_start(s: &str) -> Option<usize> {
    let mut in_quotes = false;
    let mut escaped = false;
    for (i, ch) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return Some(i),
            _ => {}
        }
    }
    None
}

/// `lines[index]` が改行終端でない場合 (ファイル末尾行への挿入時) は改行を補う。
fn ensure_terminated(lines: &mut [String], index: usize, newline: &str) {
    if let Some(line) = lines.get_mut(index)
        && !line.ends_with('\n')
    {
        line.push_str(newline);
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

    /// テスト用独立オラクル: ソースから implemented 行の真偽値を生テキスト走査で
    /// 読む (load_task_definitions と異なる経路での照合用)。
    fn raw_implemented_flag(source: &str) -> Option<bool> {
        source.lines().find_map(|l| {
            let t = l.trim_start();
            if t.starts_with('#') {
                return None;
            }
            let rest = t.strip_prefix("implemented")?;
            let value = rest.trim_start().strip_prefix('=')?.trim();
            match value {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            }
        })
    }

    /// タスク定義一覧 TOML (リポジトリ実ファイル 9 件) から全定義をパースできる。
    ///
    /// selectable 期待値は各ファイルの implemented 行の生走査 (独立オラクル) からの
    /// 辞書導出とする (Issue #160 T4): UC-3 有効化でファイルの implemented が
    /// フリップしても本テストが偽 RED しない構造化。既知 implemented 5 タスクの
    /// superset 不変チェックは残し、迂回 regress を検出する。
    #[test]
    fn test_repo_task_definitions_all_parse() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let tasks_dir = root.join("templates/tasks");
        let defs = load_task_definitions(&tasks_dir).unwrap();
        let all_ids = [
            "field_loop_pc",
            "fishing",
            "launch",
            "login",
            "nav_to_field_pc",
            "neko_nikki",
            "roguelike",
            "ticket_digest",
            "worldmap_loop",
        ];
        let ids: Vec<&str> = defs.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, all_ids);

        let selectable: Vec<&str> = defs
            .iter()
            .filter(|d| d.is_selectable())
            .map(|d| d.id.as_str())
            .collect();
        for id in all_ids {
            let source = std::fs::read_to_string(tasks_dir.join(format!("{id}.toml"))).unwrap();
            let file_implemented = raw_implemented_flag(&source)
                .unwrap_or_else(|| panic!("{id}.toml has no parsable `implemented` line"));
            let def = defs
                .iter()
                .find(|d| d.id == id)
                .unwrap_or_else(|| panic!("{id} missing from loaded defs"));
            assert_eq!(
                def.is_selectable(),
                file_implemented,
                "{id}: is_selectable must reflect the file's implemented flag"
            );
        }
        // 既知 implemented セット (superset 不変 — 有効化で増えても壊れない)。
        for known in [
            "field_loop_pc",
            "launch",
            "login",
            "nav_to_field_pc",
            "worldmap_loop",
        ] {
            assert!(selectable.contains(&known), "{known} must stay selectable");
        }
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

    // ---- Issue #154 Shard 2 (UC-3): 表示モデル純関数 ----

    /// 正常系: launch_subcommand の詳細ビュー (windows・serial は使わない)。
    #[test]
    fn test_detail_view_launch_subcommand() {
        let def = TaskDefinition::parse_toml(LAUNCH_TOML, Path::new("launch.toml")).unwrap();
        let view = task_detail_view(&def, "windows", Some("ignored"), Path::new("/root"));
        assert_eq!(view.id, "launch");
        assert_eq!(view.title, "ゲーム起動");
        assert_eq!(view.kind, "launch_subcommand");
        assert_eq!(view.pipeline_dir, None);
        assert_eq!(view.start_task, None);
        assert_eq!(view.args_preview(), "launch --target windows");
        assert!(!view.unimplemented);
        assert_eq!(view.unimplemented_reason, None);
    }

    /// 正常系: launch_subcommand (android) は serial 引数をプレビューに含む。
    #[test]
    fn test_detail_view_launch_android_serial_preview() {
        let def = TaskDefinition::parse_toml(LAUNCH_TOML, Path::new("launch.toml")).unwrap();
        let view = task_detail_view(&def, "android", Some("localhost:5555"), Path::new("/root"));
        assert_eq!(
            view.args_preview(),
            "launch --target android localhost:5555"
        );
        assert!(!view.unimplemented);
    }

    /// 正常系: pipeline_run 宣言済み start_task の詳細ビュー
    /// (pipeline_dir は root 結合済み・宣言値がそのまま使われる)。
    #[test]
    fn test_detail_view_pipeline_run_declared_start_task() {
        let def =
            TaskDefinition::parse_toml(FIELD_LOOP_TOML, Path::new("field_loop_pc.toml")).unwrap();
        let view = task_detail_view(&def, "windows", None, Path::new("/root"));
        assert_eq!(view.kind, "pipeline_run");
        let dir = view.pipeline_dir.as_deref().unwrap();
        assert!(
            dir.ends_with("templates/pipelines/field_loop_pc")
                || dir.ends_with("templates\\pipelines\\field_loop_pc"),
            "dir: {dir}"
        );
        assert_eq!(view.start_task.as_deref(), Some("start"));
        assert_eq!(view.spawn_args.len(), 5);
        assert_eq!(view.spawn_args.last().map(String::as_str), Some("start"));
        assert!(!view.unimplemented);
    }

    /// 正常系: start_task 未宣言タスクは resolve_start_task の解決結果を
    /// 詳細ビューへ反映する (リポジトリ実 pipeline で結合検証)。
    #[test]
    fn test_detail_view_pipeline_run_resolves_undeclared_start_task() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let src = r#"
id = "nav_to_field_pc"
title = "マップ移動"
kind = "pipeline_run"
implemented = true
pipeline_dir = "templates/pipelines/nav_to_field_pc"
"#;
        let def = TaskDefinition::parse_toml(src, Path::new("nav_to_field_pc.toml")).unwrap();
        let view = task_detail_view(&def, "windows", None, &root);
        assert_eq!(view.start_task.as_deref(), Some("field_hud_top"));
        assert_eq!(
            view.spawn_args.last().map(String::as_str),
            Some("field_hud_top")
        );
        assert!(!view.unimplemented);
    }

    /// 正常系 (UC-3): 実行順位置ラベルはチェック順位置と総数を含む。
    #[test]
    fn test_queue_position_label_uses_check_order() {
        let selected = vec!["b".to_string(), "a".to_string(), "c".to_string()];
        assert_eq!(
            queue_position_label(&selected, "b").as_deref(),
            Some("実行順 1/3")
        );
        assert_eq!(
            queue_position_label(&selected, "a").as_deref(),
            Some("実行順 2/3")
        );
        assert_eq!(queue_position_label(&selected, "zzz"), None);
    }

    /// 正常系 (UC-3): 実行順リスト行はチェック順に 1 始まりで番号付き。
    #[test]
    fn test_queue_order_rows_numbered_in_check_order() {
        let defs = parse_all(&[
            ("launch.toml", LAUNCH_TOML),
            ("field_loop_pc.toml", FIELD_LOOP_TOML),
        ]);
        // チェック順: field_loop_pc → launch (定義順と逆にチェック)。
        let selected = vec!["field_loop_pc".to_string(), "launch".to_string()];
        let rows = queue_order_rows(&selected, &defs);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].position, 1);
        assert_eq!(rows[0].title, "フィールド周回");
        assert_eq!(rows[1].position, 2);
        assert_eq!(rows[1].title, "ゲーム起動");
        assert!(rows.iter().all(|r| r.runnable));
    }

    /// エッジケース: 未実装タスク (implemented = false) は「未実装」ラベル用
    /// データを持つ (引数自体は解決可能でも implemented 優先)。
    #[test]
    fn test_detail_view_unimplemented_task_carries_label_data() {
        let defs = parse_all(&[("login.toml", LOGIN_TOML)]);
        let view = task_detail_view(&defs[0], "windows", None, Path::new("/root"));
        assert!(view.unimplemented);
        let reason = view.unimplemented_reason.as_deref().unwrap();
        assert!(reason.contains("implemented = false"), "reason: {reason}");
    }

    /// エッジケース: pipeline_dir が実在せず start_task も未宣言なら引数解決不能
    /// として fail-closed 表示 (実行時は queue_entries が拒否する)。
    #[test]
    fn test_detail_view_unresolvable_pipeline_fail_closed() {
        let src = r#"
id = "ghost"
title = "G"
kind = "pipeline_run"
implemented = true
pipeline_dir = "templates/pipelines/nonexistent-xyz"
"#;
        let def = TaskDefinition::parse_toml(src, Path::new("ghost.toml")).unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let view = task_detail_view(&def, "windows", None, &root);
        assert!(view.spawn_args.is_empty());
        assert_eq!(view.start_task, None);
        assert!(view.unimplemented);
        let reason = view.unimplemented_reason.as_deref().unwrap();
        assert!(reason.contains("解決"), "reason: {reason}");
    }

    /// エッジケース: 不整合 (未実装タスクが選択済み) でも行はグレー表示用に
    /// runnable = false で残る (fail-closed 表示)。
    #[test]
    fn test_queue_order_rows_flags_unimplemented_for_gray() {
        let defs = parse_all(&[("launch.toml", LAUNCH_TOML), ("login.toml", LOGIN_TOML)]);
        let selected = vec!["login".to_string(), "launch".to_string()];
        let rows = queue_order_rows(&selected, &defs);
        assert_eq!(rows.len(), 2);
        assert!(!rows[0].runnable);
        assert_eq!(rows[0].title, "ログイン");
        assert!(rows[1].runnable);
    }

    /// エッジケース: 未知の選択 ID は表示から除外する (実行時 build が
    /// fail-closed で拒否するため表示側で番号がずれても誤実行はない)。
    #[test]
    fn test_queue_order_rows_skips_unknown_selected_id() {
        let defs = parse_all(&[("launch.toml", LAUNCH_TOML)]);
        let selected = vec!["ghost".to_string(), "launch".to_string()];
        let rows = queue_order_rows(&selected, &defs);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "launch");
        // 元の選択順位置を維持 (歯抜け番号 — 隠蔽しない)。
        assert_eq!(rows[0].position, 2);
    }

    // ---- Issue #160 Shard 2 (UC-3): タスク有効化 API (enable_task) ----

    /// 有効化検証用の最小 pipeline fixture: manifest (pipeline.toml) + TaskDef 1 件。
    /// テンプレート画像は遅延読込のため実ファイル不要。
    fn write_min_pipeline(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("pipeline.toml"), "start_task = \"stub_start\"\n").unwrap();
        std::fs::write(
            dir.join("stub_start.toml"),
            "name = \"stub_start\"\nstate = \"Field\"\nalgorithm = \"ccoeff\"\ntemplate = \"stub.png\"\n",
        )
        .unwrap();
    }

    /// CRLF 改行・冒頭コメント付きの未実装 pipeline_run タスク TOML を書く
    /// (リポジトリ実 neko_nikki.toml と同構造)。
    fn write_unimplemented_task(tasks_dir: &Path) -> PathBuf {
        std::fs::create_dir_all(tasks_dir).unwrap();
        let src = "# ねこにっきタスクのスケルトン (Issue #154)。\r\n\
                   # implemented = false: 未実装。グレー表示・チェック不可。\r\n\
                   id = \"neko_nikki\"\r\n\
                   title = \"ねこにっき\"\r\n\
                   kind = \"pipeline_run\"\r\n\
                   implemented = false\r\n\
                   pipeline_dir = \"templates/pipelines/neko_nikki\"\r\n";
        let path = tasks_dir.join("neko_nikki.toml");
        std::fs::write(&path, src).unwrap();
        path
    }

    /// 正常系: 有効化は implemented を flip し pipeline_dir を書き戻し、
    /// 冒頭コメント・CRLF 改行を保全したまま parse 可能な TOML を残す。
    #[test]
    fn test_enable_task_flips_implemented_and_keeps_comments_crlf() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("templates").join("tasks");
        let task_path = write_unimplemented_task(&tasks_dir);
        write_min_pipeline(
            &tmp.path()
                .join("templates")
                .join("pipelines")
                .join("neko_nikki"),
        );

        let def = enable_task(
            &task_path,
            "neko_nikki",
            "templates/pipelines/neko_nikki",
            tmp.path(),
        )
        .unwrap();
        assert!(def.implemented);
        assert_eq!(
            def.pipeline_dir.as_deref(),
            Some(Path::new("templates/pipelines/neko_nikki"))
        );
        assert_eq!(def.kind, TaskKind::PipelineRun);

        let after = std::fs::read_to_string(&task_path).unwrap();
        // 冒頭コメントが保全されている (toml::to_string 全再生成でない保証)。
        assert!(
            after.starts_with("# ねこにっきタスクのスケルトン (Issue #154)。\r\n"),
            "comments dropped:\n{after}"
        );
        assert!(after.contains("# implemented = false: 未実装。グレー表示・チェック不可。\r\n"));
        // implemented 行のみフリップ・CRLF 保全。
        assert!(after.contains("kind = \"pipeline_run\"\r\nimplemented = true\r\n"));
        assert!(
            after.contains("pipeline_dir = \"templates/pipelines/neko_nikki\"\r\n"),
            "after:\n{after}"
        );
        // 書き戻し後に再パース可能で、選択可能になっている。
        let defs = load_task_definitions(&tasks_dir).unwrap();
        let neko = defs
            .iter()
            .find(|d| d.id == "neko_nikki")
            .unwrap_or_else(|| panic!("neko_nikki missing after rewrite"));
        assert!(neko.is_selectable());
    }

    /// エッジケース: ファイル内 id と期待 id が不一致なら 1 バイトも書き換えない。
    #[test]
    fn test_enable_task_rejects_id_mismatch_and_keeps_file() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        let task_path = write_unimplemented_task(&tasks_dir);
        write_min_pipeline(&tmp.path().join("pipe"));
        let before = std::fs::read_to_string(&task_path).unwrap();
        let err = enable_task(&task_path, "roguelike", "pipe", tmp.path()).unwrap_err();
        assert!(matches!(err, TaskError::IdMismatch { .. }), "got {err:?}");
        assert_eq!(std::fs::read_to_string(&task_path).unwrap(), before);
    }

    /// エッジケース: launch_subcommand は pipeline 紐付け対象外として拒否。
    #[test]
    fn test_enable_task_rejects_launch_subcommand() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        let task_path = tasks_dir.join("launch.toml");
        std::fs::write(
            &task_path,
            "# ゲーム起動\r\nid = \"launch\"\r\ntitle = \"起動\"\r\nkind = \"launch_subcommand\"\r\nimplemented = true\r\n",
        )
        .unwrap();
        write_min_pipeline(&tmp.path().join("pipe"));
        let before = std::fs::read_to_string(&task_path).unwrap();
        let err = enable_task(&task_path, "launch", "pipe", tmp.path()).unwrap_err();
        assert!(
            matches!(err, TaskError::NotPipelineRun { .. }),
            "got {err:?}"
        );
        assert_eq!(std::fs::read_to_string(&task_path).unwrap(), before);
    }

    /// エッジケース (fail-closed): pipeline_dir が load 不可 (ディレクトリ無し /
    /// manifest 無し) なタスクの有効化は拒否し、ファイルは未変更のまま。
    #[test]
    fn test_enable_task_rejects_pipeline_without_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        let task_path = write_unimplemented_task(&tasks_dir);

        // (a) pipeline ディレクトリ自体が存在しない。
        let err = enable_task(&task_path, "neko_nikki", "pipelines/ghost", tmp.path()).unwrap_err();
        assert!(
            matches!(err, TaskError::PipelineNotLoadable { .. }),
            "got {err:?}"
        );

        // (b) ディレクトリはあるが manifest (pipeline.toml) が無い。
        let pipe = tmp.path().join("pipelines").join("neko_nikki");
        std::fs::create_dir_all(&pipe).unwrap();
        std::fs::write(
            pipe.join("stub_start.toml"),
            "name = \"stub_start\"\nstate = \"Field\"\nalgorithm = \"ccoeff\"\ntemplate = \"stub.png\"\n",
        )
        .unwrap();
        let err =
            enable_task(&task_path, "neko_nikki", "pipelines/neko_nikki", tmp.path()).unwrap_err();
        assert!(
            matches!(err, TaskError::PipelineNotLoadable { .. }),
            "got {err:?}"
        );

        let after = std::fs::read_to_string(&task_path).unwrap();
        assert!(after.contains("implemented = false\r\n"), "after:\n{after}");
    }

    /// エッジケース (fail-closed): manifest のみで TaskDef がゼロの pipeline への
    /// 有効化は拒否する。
    #[test]
    fn test_enable_task_rejects_pipeline_with_zero_taskdefs() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        let task_path = write_unimplemented_task(&tasks_dir);
        let pipe = tmp.path().join("pipe");
        std::fs::create_dir_all(&pipe).unwrap();
        std::fs::write(pipe.join("pipeline.toml"), "start_task = \"stub_start\"\n").unwrap();
        let err = enable_task(&task_path, "neko_nikki", "pipe", tmp.path()).unwrap_err();
        assert!(
            matches!(err, TaskError::PipelineNoTaskDefs(_)),
            "got {err:?}"
        );
        assert!(
            std::fs::read_to_string(&task_path)
                .unwrap()
                .contains("implemented = false")
        );
    }

    /// エッジケース: implemented 行・pipeline_dir 行が無い TOML でも kind 行を
    /// アンカーに挿入して有効化できる (explicit > implicit)。
    #[test]
    fn test_enable_task_inserts_missing_implemented_and_pipeline_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        let task_path = tasks_dir.join("bare.toml");
        std::fs::write(
            &task_path,
            "id = \"bare\"\ntitle = \"B\"\nkind = \"pipeline_run\"\n",
        )
        .unwrap();
        write_min_pipeline(&tmp.path().join("pipe"));
        let def = enable_task(&task_path, "bare", "pipe", tmp.path()).unwrap();
        assert!(def.implemented);
        assert_eq!(def.pipeline_dir.as_deref(), Some(Path::new("pipe")));
        let after = std::fs::read_to_string(&task_path).unwrap();
        assert!(
            after
                .contains("kind = \"pipeline_run\"\nimplemented = true\npipeline_dir = \"pipe\"\n"),
            "after:\n{after}"
        );
    }

    /// エッジケース: implemented 行のインラインコメントは値フリップ後も保全する。
    #[test]
    fn test_enable_task_preserves_inline_comment_on_implemented_line() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        let task_path = tasks_dir.join("c.toml");
        std::fs::write(
            &task_path,
            "id = \"c\"\ntitle = \"C\"\nkind = \"pipeline_run\"\nimplemented = false  # 有効化待ち\npipeline_dir = \"old\"\n",
        )
        .unwrap();
        write_min_pipeline(&tmp.path().join("pipe"));
        let def = enable_task(&task_path, "c", "pipe", tmp.path()).unwrap();
        assert!(def.implemented);
        assert_eq!(def.pipeline_dir.as_deref(), Some(Path::new("pipe")));
        let after = std::fs::read_to_string(&task_path).unwrap();
        assert!(
            after.contains("implemented = true  # 有効化待ち\n"),
            "after:\n{after}"
        );
        assert!(after.contains("pipeline_dir = \"pipe\"\n"));
    }

    /// 正常系: backslash 区切りの pipeline_dir は TOML 規約 (forward slash) へ
    /// 正規化して書き戻す。
    #[test]
    fn test_enable_task_normalizes_backslash_path_to_forward_slash() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        let task_path = write_unimplemented_task(&tasks_dir);
        write_min_pipeline(&tmp.path().join("pipe").join("sub"));
        let def = enable_task(&task_path, "neko_nikki", "pipe\\sub", tmp.path()).unwrap();
        assert_eq!(def.pipeline_dir.as_deref(), Some(Path::new("pipe/sub")));
        let after = std::fs::read_to_string(&task_path).unwrap();
        assert!(
            after.contains("pipeline_dir = \"pipe/sub\"\r\n"),
            "after:\n{after}"
        );
    }

    /// エッジケース: TOML 値にできない pipeline_dir (引用符含み) は拒否。
    #[test]
    fn test_enable_task_rejects_unwritable_pipeline_dir_value() {
        let tmp = tempfile::tempdir().unwrap();
        let tasks_dir = tmp.path().join("tasks");
        let task_path = write_unimplemented_task(&tasks_dir);
        write_min_pipeline(&tmp.path().join("pipe"));
        let err = enable_task(&task_path, "neko_nikki", "pipe\"quote", tmp.path()).unwrap_err();
        assert!(
            matches!(err, TaskError::InvalidPipelineDir { .. }),
            "got {err:?}"
        );
        assert!(
            std::fs::read_to_string(&task_path)
                .unwrap()
                .contains("implemented = false")
        );
    }

    /// 既存タスク TOML (リポジトリ実ファイル 9 件中 pipeline_run 8 件) は有効化の
    /// 書き戻し後も parse 可能・全コメント行が保全されていることの固定
    /// (設計制約: toml::to_string 全再生成はコメントを落とすため行編集を採る)。
    #[test]
    fn test_enable_task_repo_tomls_stay_parseable_with_comments() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let repo_tasks = root.join("templates").join("tasks");
        // launch (launch_subcommand) は pipeline 紐付け対象外のため除外。
        let pipeline_run_ids = [
            "field_loop_pc",
            "fishing",
            "login",
            "nav_to_field_pc",
            "neko_nikki",
            "roguelike",
            "ticket_digest",
            "worldmap_loop",
        ];
        for id in pipeline_run_ids {
            let tmp = tempfile::tempdir().unwrap();
            let tasks_dir = tmp.path().join("tasks");
            std::fs::create_dir_all(&tasks_dir).unwrap();
            write_min_pipeline(&tmp.path().join("pipe"));
            let task_path = tasks_dir.join(format!("{id}.toml"));
            std::fs::copy(repo_tasks.join(format!("{id}.toml")), &task_path).unwrap();
            let original = std::fs::read_to_string(&task_path).unwrap();

            let def = enable_task(&task_path, id, "pipe", tmp.path())
                .unwrap_or_else(|e| panic!("enable failed for {id}: {e}"));
            assert!(def.implemented, "{id}");
            assert_eq!(def.pipeline_dir.as_deref(), Some(Path::new("pipe")), "{id}");

            let after = std::fs::read_to_string(&task_path).unwrap();
            for line in original.lines().filter(|l| l.trim_start().starts_with('#')) {
                assert!(after.contains(line), "{id}: comment line lost: {line}");
            }
            let defs = load_task_definitions(&tasks_dir).unwrap();
            let enabled = defs
                .iter()
                .find(|d| d.id == id)
                .unwrap_or_else(|| panic!("{id} missing after rewrite"));
            assert!(enabled.is_selectable(), "{id}");
        }
    }
}
