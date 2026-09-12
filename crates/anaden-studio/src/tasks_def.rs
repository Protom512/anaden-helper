//! タスク定義 TOML ドメイン (Issue #170: tasks.rs 分割)。
//!
//! `templates/tasks/*.toml` の定義読み込み・パース・検証と、タスク有効化
//! (task TOML 書き戻し) を担う。
//!
//! - TaskDefinition: `templates/tasks/*.toml` の 1 タスク定義。
//! - TaskKind: 異種実行経路の分岐 (`LaunchSubcommand` = `anaden launch` standalone、
//!   `PipelineRun` = `anaden run <pipeline_dir>` )。
//! - fail-closed: TOML 欠損・不正 kind はエラー。`implemented = false` は選択不可
//!   (グレー表示) — 嘘の動作可能表示は禁止 (CEO 確定)。
//! - enable_task (Issue #160 UC-3): task TOML の implemented フリップ +
//!   pipeline_dir 紐付け書き戻し (コメント保全の外科的行編集・load 検証は
//!   fail-closed)。
//!
//! チェック順キュー組み立て・逐次実行の状態機械は [`crate::tasks_queue`] へ、
//! 表示モデル純関数は [`crate::tasks_view`] へ、TOML 外科的行編集の純粋関数は
//! [`crate::tasks_toml`] へ分割済み。[`crate::tasks`] はこれらの公開シンボルを
//! re-export する facade である (呼び出し元パス不変)。

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::tasks_toml::{edit_task_toml_source, normalize_pipeline_dir_rel};

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
/// (`tasks_toml::edit_task_toml_source`)。
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

/// タスク定義テスト共有フィクスチャ ([`crate::tasks_queue`] のテストからも利用)。
#[cfg(test)]
pub(crate) mod test_util {
    pub(crate) const LAUNCH_TOML: &str = r#"
id = "launch"
title = "ゲーム起動"
kind = "launch_subcommand"
implemented = true
"#;

    pub(crate) const FIELD_LOOP_TOML: &str = r#"
id = "field_loop_pc"
title = "フィールド周回"
kind = "pipeline_run"
implemented = true
pipeline_dir = "templates/pipelines/field_loop_pc"
start_task = "start"
"#;

    pub(crate) const LOGIN_TOML: &str = r#"
id = "login"
title = "ログイン"
kind = "pipeline_run"
implemented = false
pipeline_dir = "templates/pipelines/login"
"#;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use test_util::{FIELD_LOOP_TOML, LAUNCH_TOML};

    // ---- 正常系: 定義のパース・ディレクトリ読み込み ----

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

    // ---- Issue #144 Task 3: 定義一覧の実ファイル検証 ----

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

    /// タスク定義一覧 TOML (リポジトリ実ファイル 8 件) から全定義をパースできる。
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
        for known in ["field_loop_pc", "launch", "login", "nav_to_field_pc"] {
            assert!(selectable.contains(&known), "{known} must stay selectable");
        }
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

    /// 既存タスク TOML (リポジトリ実ファイル 8 件中 pipeline_run 7 件) は有効化の
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
