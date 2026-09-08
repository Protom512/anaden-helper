//! シナリオ保存 pipeline → タスク登録・有効化のドメイン (Issue #160 UC-3 / Shard 4)。
//!
//! MAA interface.json モデル (設計ノート A) の 4 段階フロー (a) pipeline 作成
//! (save_scenario) → (b) task エントリ登録 (`templates/tasks/<id>.toml` 生成) →
//! (c) 有効化 ([`crate::tasks::enable_task`]) → (d) queue 追加 (既存ホーム一覧)
//! のうち (b)(c) を担う純ドメイン。egui 非依存。task = pipeline への名前参照
//! (MAA taskItem の entry と同型) であり pipeline 本体とは完全分離。
//!
//! - fail-closed: 同名 task TOML 既存時は上書き拒否、task id の安全性検証
//!   (register・bind 両経路・Issue #180)、書き込み前に
//!   [`TaskDefinition::parse_toml`] で完全検証、有効化は
//!   [`crate::tasks::enable_task`] の pipeline load 検証に委譲。
//! - TOML 書き込み中断・有効化失敗時は登録した TOML を補償削除し部分状態を
//!   残さない (Issue #180)。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::tasks::{self, TaskDefinition, TaskError, TaskKind};

/// タスク登録・有効化に関するエラー。
#[derive(Debug, Error)]
pub enum ScenarioTaskError {
    /// task id が task TOML ファイル名として不適 (空・パス区切り・Windows 予約文字等)。
    #[error("task id `{id}` is not a safe task file name")]
    UnsafeTaskId { id: String },
    /// 同名 task TOML が既存 (上書き拒否・fail-closed)。
    #[error("task already exists (overwrite refused): {0}")]
    TaskAlreadyExists(PathBuf),
    /// pipeline ディレクトリが workspace root 配下ではない (相対化不能)。
    #[error("pipeline dir {dir} is not a subdirectory of the workspace root")]
    NotRootRelative { dir: String },
    /// 生成した task TOML が書き込み前検証に不合格 (通常は起きない・生成バグ検出)。
    #[error("generated task TOML failed pre-write validation: {0}")]
    InvalidGenerated(#[source] TaskError),
    /// task TOML 書き込み IO 失敗。
    #[error("failed to write task file {path}")]
    Write {
        path: PathBuf,
        source: Box<std::io::Error>,
    },
    /// 紐付け先 task TOML が存在しない。
    #[error("task not found: {0}")]
    TaskNotFound(PathBuf),
    /// 有効化 (enable_task) の失敗。pipeline load 不能・id 不一致等は本 variant で伝播。
    #[error(transparent)]
    Enable(#[from] TaskError),
}

/// 既存 stub タスク (implemented = false の pipeline_run) の選択肢表示モデル。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StubTaskOption {
    /// タスク定義 ID (= TOML ファイル名規約 `<id>.toml`)。
    pub id: String,
    /// 表示ラベル (title)。
    pub title: String,
}

/// タスク定義一覧から stub 選択肢 (未実装 pipeline_run) を導出する純関数。
#[must_use]
pub fn stub_options(defs: &[TaskDefinition]) -> Vec<StubTaskOption> {
    defs.iter()
        .filter(|d| d.kind == TaskKind::PipelineRun && !d.implemented)
        .map(|d| StubTaskOption {
            id: d.id.clone(),
            title: d.title.clone(),
        })
        .collect()
}

/// パネルが呼出側 (app.rs) へ通知するイベント (UC-3 配線)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScenarioPanelEvent {
    /// タスク登録・有効化に成功 (ホーム一覧の再読込が必要)。`message` は再読込後にも
    /// status へ再設定する成功メッセージ (再読込が status を上書きするため持ち回す)。
    TaskEnabled { message: String },
}

/// `ui_task_link` 描画に必要な app.rs 側コンテキスト (配線データ)。
pub struct TaskLinkContext<'a> {
    /// workspace ルート (pipeline_dir の相対化基準)。
    pub root: &'a Path,
    /// task TOML ディレクトリ (既定 `templates/tasks`)。
    pub tasks_dir: &'a Path,
    /// 既存 stub タスク選択肢 (implemented=false の pipeline_run)。
    pub stubs: &'a [StubTaskOption],
}

/// task id が task TOML ファイル名として安全か。
///
/// - 単一パス要素 (パス区切り・`.`/`..` 拒否) — `ScenarioValidationError::UnsafeName`
///   と同一契約 (`file_name()` 一致検査)。
/// - Windows 予約ファイル名文字 (`<>:"|?*`)・backslash・制御文字も拒否
///   (書き込み IO 事故・OS 間非移植の未然防止)。
fn is_safe_task_id(id: &str) -> bool {
    !id.is_empty()
        && Path::new(id).file_name() == Some(OsStr::new(id))
        && !id
            .chars()
            .any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\') || c.is_control())
}

/// pipeline ディレクトリを repo root 相対・forward slash 形式へ解決する。
///
/// [`crate::scenario_ui::resolve_template_reference`] を再用し、結果が
/// 絶対パス・`..` 遷移含み・空の場合は [`ScenarioTaskError::NotRootRelative`]
/// で fail-closed する。受け入れるのは **root 相対の任意パス**
/// (セパレータは forward slash) であって、`templates/pipelines/<name>` は
/// 慣例上の代表例にすぎない — ディレクトリ prefix としては検証しない
/// (doc-code 整合・Issue #180: prefix 検証の追加は既存 task TOML の受け付け
/// を変える動作変更になるため非スコープ)。
fn pipeline_dir_rel(pipeline_dir: &Path, root: &Path) -> Result<String, ScenarioTaskError> {
    let rel = crate::scenario_ui::resolve_template_reference(pipeline_dir, root);
    let unsafe_rel =
        rel.is_empty() || Path::new(&rel).is_absolute() || rel.split('/').any(|seg| seg == "..");
    if unsafe_rel {
        return Err(ScenarioTaskError::NotRootRelative {
            dir: pipeline_dir.display().to_string(),
        });
    }
    Ok(rel)
}

/// 生成する task TOML のシリアライズ専用構造 (stub 状態: implemented = false)。
///
/// フィールド順 = 出力 TOML のキー順 (既存 `templates/tasks/*.toml` と同一順)。
/// 有効化は [`crate::tasks::enable_task`] が行うため生成時点では必ず stub 状態。
#[derive(serde::Serialize)]
struct GeneratedTaskToml<'a> {
    id: &'a str,
    title: &'a str,
    kind: &'a str,
    implemented: bool,
    pipeline_dir: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_task: Option<&'a str>,
}

/// task TOML ソースを生成する (ヘッダコメント付き・人間可読)。
///
/// 既存 `templates/tasks/*.toml` の形式 (由来コメント 2 行 + キー順) に合わせる。
/// コメント内 title は制御文字を空白化して改行混入を防ぐ (値側は serializer がエスケープ)。
fn generate_task_toml(
    id: &str,
    title: &str,
    pipeline_dir_rel: &str,
    start_task: &str,
) -> Result<String, ScenarioTaskError> {
    let comment_title: String = title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let start = if start_task.trim().is_empty() {
        None
    } else {
        Some(start_task.trim())
    };
    let body = toml::to_string(&GeneratedTaskToml {
        id,
        title,
        kind: "pipeline_run",
        implemented: false,
        pipeline_dir: pipeline_dir_rel,
        start_task: start,
    })
    .map_err(|e| {
        ScenarioTaskError::InvalidGenerated(TaskError::EditFailed {
            path: PathBuf::from(format!("{id}.toml")),
            reason: format!("serialization failed: {e}"),
        })
    })?;
    Ok(format!(
        "# {comment_title} タスク (Issue #160 シナリオ作成 GUI から自動登録)。\n\
         # 実行経路: `anaden run {pipeline_dir_rel}`。\n{body}"
    ))
}

/// 保存済み pipeline を新規タスクとして登録 (b) して有効化 (c) する。
///
/// - task id は `<id>.toml` のファイル名になる。既存時は
///   [`ScenarioTaskError::TaskAlreadyExists`] (上書き拒否・1 バイトも書かない)。
/// - `title` が空の場合は task id を採用する (フォーム既定値のフォールバック)。
/// - 生成 TOML は stub 状態 (`implemented = false` + pipeline_dir + start_task 宣言)
///   で書き出し直後に [`crate::tasks::enable_task`] でフリップする — 有効化の
///   fail-closed 検証 (pipeline load 可能・id 一致) を新規経路にも適用する。
/// - TOML 書き込み中断 (部分ファイル残留)・有効化失敗時は登録した TOML を
///   補償削除する (部分状態を残さない・Issue #180。削除自体の失敗は stub
///   (選択不可) が残るのみで無害なため伝播しない)。
///
/// # Errors
/// 上記 fail-closed 条件いずれかで [`ScenarioTaskError`]。
pub fn register_and_enable_task(
    tasks_dir: &Path,
    root: &Path,
    pipeline_dir: &Path,
    task_id: &str,
    title: &str,
    start_task: &str,
) -> Result<TaskDefinition, ScenarioTaskError> {
    let id = task_id.trim();
    if !is_safe_task_id(id) {
        return Err(ScenarioTaskError::UnsafeTaskId {
            id: task_id.to_string(),
        });
    }
    let title = if title.trim().is_empty() {
        id
    } else {
        title.trim()
    };
    let dir_rel = pipeline_dir_rel(pipeline_dir, root)?;
    let task_path = tasks_dir.join(format!("{id}.toml"));
    if task_path.exists() {
        return Err(ScenarioTaskError::TaskAlreadyExists(task_path));
    }
    let source = generate_task_toml(id, title, &dir_rel, start_task)?;
    // 書き込み前の完全検証 (fail-closed): 生成ソースが正当な stub 定義であること。
    TaskDefinition::parse_toml(&source, &task_path).map_err(ScenarioTaskError::InvalidGenerated)?;
    std::fs::create_dir_all(tasks_dir).map_err(|source| ScenarioTaskError::Write {
        path: tasks_dir.to_path_buf(),
        source: Box::new(source),
    })?;
    if let Err(source) = std::fs::write(&task_path, &source) {
        // 補償削除 (Issue #180): 呼出前は task_path が存在しなかった (冒頭の
        // exists 検査で保証) ため、write 中断で残留した部分ファイルはすべて
        // 本呼出の産物。削除して部分状態を残さない (削除失敗は stub 残留のみ)。
        let _ = std::fs::remove_file(&task_path);
        return Err(ScenarioTaskError::Write {
            path: task_path,
            source: Box::new(source),
        });
    }
    match tasks::enable_task(&task_path, id, &dir_rel, root) {
        Ok(def) => Ok(def),
        Err(e) => {
            // 補償削除: 本呼出で登録した TOML のみを戻す。
            let _ = std::fs::remove_file(&task_path);
            Err(ScenarioTaskError::Enable(e))
        }
    }
}

/// 既存 stub タスク (`implemented = false` の pipeline_run) へ pipeline を
/// 紐付けて有効化する (ルート (ii): (b) スキップで enable_task のみ)。
///
/// task id は [`register_and_enable_task`] と対称に
/// [`ScenarioTaskError::UnsafeTaskId`] で検証する (Issue #180: 公開 API の
/// 引数がそのまま `tasks_dir.join` へ入るため、不正 id はパス構築の前段で
/// 拒否する)。それ以外の検証 (id 一致・kind・pipeline load 可能・外科的行編集の
/// 往復確認) はすべて [`crate::tasks::enable_task`] に委譲する — 有効化の
/// 単一情報源。
///
/// # Errors
/// [`ScenarioTaskError::UnsafeTaskId`] (不正 task id)、
/// [`ScenarioTaskError::TaskNotFound`] (TOML 無し) または
/// [`ScenarioTaskError::Enable`] (enable_task の fail-closed 検証失敗)。
pub fn bind_and_enable_task(
    tasks_dir: &Path,
    root: &Path,
    task_id: &str,
    pipeline_dir: &Path,
) -> Result<TaskDefinition, ScenarioTaskError> {
    let id = task_id.trim();
    if !is_safe_task_id(id) {
        return Err(ScenarioTaskError::UnsafeTaskId {
            id: task_id.to_string(),
        });
    }
    let dir_rel = pipeline_dir_rel(pipeline_dir, root)?;
    let task_path = tasks_dir.join(format!("{id}.toml"));
    if !task_path.exists() {
        return Err(ScenarioTaskError::TaskNotFound(task_path));
    }
    tasks::enable_task(&task_path, id, &dir_rel, root).map_err(ScenarioTaskError::Enable)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::scenario_ui::ScenarioEditorState;
    use anaden_core::{Goal, StopCondition};
    use anaden_vision::{Action, Algorithm, TaskDef};
    use std::fs;

    /// テスト用 TaskDef (scenario_ui テストと同構成)。
    fn task_def(name: &str) -> TaskDef {
        TaskDef {
            name: name.to_string(),
            state: "Field".to_string(),
            algorithm: Algorithm::Ccoeff,
            template: std::path::PathBuf::from(format!("{name}.png")),
            roi: Some([10, 20, 100, 50]),
            threshold: 0.8,
            base: None,
            action: Some(Action::ClickSelf),
            next: Some(vec![]),
        }
    }

    /// 検証済みシナリオを `root/templates/pipelines/<name>/` へ保存し、
    /// その pipeline dir を返す ((a) 段階の再現)。
    fn saved_pipeline(root: &Path, name: &str) -> PathBuf {
        let pipelines_root = root.join("templates/pipelines");
        let mut st = ScenarioEditorState::new(name);
        st.add_task(task_def("Start"));
        st.add_task(task_def("Loop"));
        st.task_mut("Start").unwrap().next = Some(vec!["Loop".to_string()]);
        st.add_goal(Goal {
            name: "loop3".to_string(),
            stop: StopCondition::LoopCount { target: 3 },
        });
        crate::scenario_ui::save_scenario(&st, &[], &pipelines_root).expect("save scenario")
    }

    /// 正常系 1: 新規登録 → TOML 往復 (parse_toml) で is_selectable == true。
    #[test]
    fn register_generates_enabled_selectable_task_toml() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        let pipeline = saved_pipeline(root, "fishing2");

        let def = register_and_enable_task(
            &tasks_dir,
            root,
            &pipeline,
            "fishing2",
            "つり (新規)",
            "Start",
        )
        .expect("register");

        assert!(def.implemented && def.is_selectable());
        assert_eq!(def.kind, TaskKind::PipelineRun);
        assert_eq!(
            def.pipeline_dir.as_deref(),
            Some(Path::new("templates/pipelines/fishing2"))
        );
        assert_eq!(def.start_task.as_deref(), Some("Start"));
        // ディスク上の TOML はヘッダコメント付きで parse_toml 往復可能。
        let source = fs::read_to_string(tasks_dir.join("fishing2.toml")).expect("read");
        assert!(
            source.starts_with('#'),
            "人間可読ヘッダコメント必須:\n{source}"
        );
        let reparsed =
            TaskDefinition::parse_toml(&source, Path::new("fishing2.toml")).expect("reparse");
        assert!(reparsed.implemented && reparsed.is_selectable());
        assert_eq!(reparsed.id, "fishing2");
        assert_eq!(reparsed.title, "つり (新規)");
    }

    /// 正常系 2: 既存 stub への紐付け有効化 → implemented=true + pipeline_dir 書き戻り。
    #[test]
    fn bind_enables_existing_stub_writing_back_pipeline_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        fs::create_dir_all(&tasks_dir).expect("mkdir");
        let pipeline = saved_pipeline(root, "fishing2");
        // 既存 stub (由来コメント・インラインコメント付き = テンプレ実物形式)。
        fs::write(
            tasks_dir.join("neko_nikki.toml"),
            "# ねこにっきタスクのスケルトン (Issue #154)。\n\
             # implemented = false: 未実装。グレー表示・チェック不可。\n\
             id = \"neko_nikki\"\n\
             title = \"ねこにっき\"\n\
             kind = \"pipeline_run\"\n\
             implemented = false\n\
             pipeline_dir = \"templates/pipelines/neko_nikki\"\n",
        )
        .expect("write stub");

        let def = bind_and_enable_task(&tasks_dir, root, "neko_nikki", &pipeline).expect("bind");

        assert!(def.implemented && def.is_selectable());
        assert_eq!(
            def.pipeline_dir.as_deref(),
            Some(Path::new("templates/pipelines/fishing2"))
        );
        let source = fs::read_to_string(tasks_dir.join("neko_nikki.toml")).expect("read");
        assert!(source.contains("implemented = true"), "フリップ: {source}");
        assert!(
            source.contains("templates/pipelines/fishing2"),
            "pipeline_dir 書き戻り: {source}"
        );
        assert!(
            source.starts_with("# ねこにっきタスクのスケルトン"),
            "由来コメントは外科的編集で保全: {source}"
        );
    }

    /// エッジ 1: 同名 task TOML 既存 → 上書き拒否 + 既存ファイルのバイト不変。
    #[test]
    fn register_refuses_existing_task_toml_leaving_file_untouched() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        fs::create_dir_all(&tasks_dir).expect("mkdir");
        let pipeline = saved_pipeline(root, "fishing2");
        let existing = tasks_dir.join("fishing2.toml");
        let original = "# 既存定義 (絶対に書き換えさせない)。\n\
                        id = \"fishing2\"\n\
                        title = \"既存\"\n\
                        kind = \"launch_subcommand\"\n\
                        implemented = true\n";
        fs::write(&existing, original).expect("write existing");

        let err =
            register_and_enable_task(&tasks_dir, root, &pipeline, "fishing2", "新規", "Start")
                .expect_err("must refuse");

        assert!(
            matches!(err, ScenarioTaskError::TaskAlreadyExists(ref p) if *p == existing),
            "err: {err:?}"
        );
        assert_eq!(
            fs::read_to_string(&existing).expect("read"),
            original,
            "既存ファイルは 1 バイトも変わらない"
        );
    }

    /// エッジ 2: 不正 task id (パス区切り・予約文字・空) → 拒否。root 外 pipeline → 拒否。
    #[test]
    fn register_rejects_unsafe_task_id_and_outside_root_pipeline() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        let pipeline = saved_pipeline(root, "fishing2");

        // パス区切り・`.`/`..`・空・Windows 予約文字・backslash は UnsafeTaskId。
        for bad in ["a/b", "..", "", "a\\b", "a:b", "a<b", "a\"b"] {
            let err = register_and_enable_task(&tasks_dir, root, &pipeline, bad, "t", "Start")
                .expect_err("must reject");
            assert!(
                matches!(err, ScenarioTaskError::UnsafeTaskId { .. }),
                "bad {bad:?}: {err:?}"
            );
        }
        // root 配下でない pipeline (共通祖先なし → 絶対パス化) は拒否。
        let outside = if cfg!(windows) {
            PathBuf::from(r"D:\elsewhere\pipeline")
        } else {
            PathBuf::from("/elsewhere/pipeline")
        };
        let err =
            bind_and_enable_task(&tasks_dir, root, "neko_nikki", &outside).expect_err("reject");
        assert!(
            matches!(err, ScenarioTaskError::NotRootRelative { .. }),
            "err: {err:?}"
        );
    }

    /// minor-1 (Issue #180): bind_and_enable_task も task id を検証する
    /// (register_and_enable_task と対称)。不正 id (パス区切り・予約文字・空) は
    /// `tasks_dir.join` される前に UnsafeTaskId で拒否される。
    #[test]
    fn bind_rejects_unsafe_task_id_before_path_join() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        fs::create_dir_all(&tasks_dir).expect("mkdir");
        let pipeline = saved_pipeline(root, "fishing2");

        for bad in ["a/b", "..", "", "a\\b", "a:b", "a<b", "a\"b"] {
            let err =
                bind_and_enable_task(&tasks_dir, root, bad, &pipeline).expect_err("must reject");
            assert!(
                matches!(err, ScenarioTaskError::UnsafeTaskId { .. }),
                "bad {bad:?}: {err:?}"
            );
        }
        // tasks_dir には何も書かれていない (検査はパス構築後の IO より前段)。
        assert!(
            fs::read_dir(&tasks_dir).expect("read_dir").next().is_none(),
            "不正 id では一切の副作用を残さない"
        );
    }

    /// minor-2 (Issue #180): TOML write 失敗も補償削除対象。id としては安全
    /// (単一パス要素・予約文字なし) だがファイル名が FS のパス成分長上限
    /// (255 文字) を超える id で write を失敗させ、部分ファイルを含む残留
    /// ゼロを検証する (部分書き込みそのものは std::fs を差し替えられない
    /// ため再現不可 — write 失敗経路の補償挙動を実パスで確認)。
    #[test]
    fn register_compensates_task_toml_on_write_failure() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        let pipeline = saved_pipeline(root, "fishing2");
        let long_id = "A".repeat(300); // `<id>.toml` = 304 文字 > 255

        let err =
            register_and_enable_task(&tasks_dir, root, &pipeline, &long_id, "長い名前", "Start")
                .expect_err("write must fail");

        assert!(
            matches!(err, ScenarioTaskError::Write { .. }),
            "err: {err:?}"
        );
        let residue: Vec<_> = fs::read_dir(&tasks_dir)
            .expect("tasks_dir exists (create_dir_all 成功後)")
            .collect();
        assert!(
            residue.is_empty(),
            "write 失敗時は部分ファイルを含め残留ゼロ: {residue:?}"
        );
    }

    /// minor-2 (create 系エラーパス): tasks_dir 自体が既存ファイルなら
    /// create_dir_all が失敗する。前提資産 (そのファイル) は 1 バイトも変わらない。
    #[test]
    fn register_write_failure_keeps_preexisting_tasks_dir_file_intact() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        fs::create_dir_all(root.join("templates")).expect("mkdir");
        fs::write(&tasks_dir, b"not a directory").expect("prepare file");
        let pipeline = saved_pipeline(root, "fishing2");

        let err =
            register_and_enable_task(&tasks_dir, root, &pipeline, "fishing2", "新規", "Start")
                .expect_err("must fail");

        assert!(
            matches!(err, ScenarioTaskError::Write { .. }),
            "err: {err:?}"
        );
        assert_eq!(
            fs::read(&tasks_dir).expect("read"),
            b"not a directory",
            "前提資産は 1 バイトも変わらない"
        );
    }

    /// エッジ 3: pipeline が load 不能 → enable_task エラーが伝播 + 補償削除。
    #[test]
    fn register_propagates_enable_error_and_cleans_up_created_toml() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        // pipeline を保存しない (= manifest 無しで load 不能)。
        let ghost = root.join("templates/pipelines/ghost");

        let err =
            register_and_enable_task(&tasks_dir, root, &ghost, "ghost_task", "ゴースト", "Start")
                .expect_err("must fail");

        assert!(
            matches!(
                err,
                ScenarioTaskError::Enable(TaskError::PipelineNotLoadable { .. })
            ),
            "err: {err:?}"
        );
        assert!(
            !tasks_dir.join("ghost_task.toml").exists(),
            "有効化失敗時は登録 TOML を補償削除 (部分状態を残さない)"
        );
    }

    /// 補助: stub_options は未実装 pipeline_run のみを抽出する。
    #[test]
    fn stub_options_filter_unimplemented_pipeline_run_only() {
        let parse =
            |src: &str, name: &str| TaskDefinition::parse_toml(src, Path::new(name)).unwrap();
        let defs = [
            parse(
                "id = \"launch\"\ntitle = \"起動\"\nkind = \"launch_subcommand\"\nimplemented = true\n",
                "launch.toml",
            ),
            parse(
                "id = \"stub\"\ntitle = \"スタブ\"\nkind = \"pipeline_run\"\nimplemented = false\npipeline_dir = \"x\"\n",
                "stub.toml",
            ),
            parse(
                "id = \"done\"\ntitle = \"実装済\"\nkind = \"pipeline_run\"\nimplemented = true\npipeline_dir = \"x\"\n",
                "done.toml",
            ),
        ];
        let stubs = stub_options(&defs);
        assert_eq!(stubs.len(), 1);
        assert_eq!(stubs[0].id, "stub");
        assert_eq!(stubs[0].title, "スタブ");
    }

    /// 補助: 空タイトルは task id へフォールバック (フォーム既定値欠落耐性)。
    #[test]
    fn register_uses_task_id_as_title_when_title_empty() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let tasks_dir = root.join("templates/tasks");
        let pipeline = saved_pipeline(root, "fishing2");

        let def = register_and_enable_task(&tasks_dir, root, &pipeline, "fishing2", "   ", "Start")
            .expect("register");
        assert_eq!(def.title, "fishing2");
    }
}
