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
    pub fn spawn_specs(
        &self,
        program: &str,
        target: &str,
        serial: Option<&str>,
        root: &Path,
    ) -> Result<Vec<crate::childproc::SpawnSpec>, TaskError> {
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
                Ok(crate::childproc::SpawnSpec::new(program, args))
            })
            .collect()
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
}
