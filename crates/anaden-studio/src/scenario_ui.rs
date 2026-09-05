//! シナリオ編集ドメインモデル (Issue #160 Shard 1 / UC-1+2)。
//!
//! `templates/pipelines/<name>/` 配下の pipeline manifest (start_task + goals) と
//! TaskDef 群を GUI で作成・編集するための純状態モデル。egui 非依存の状態操作層
//! であり、描画パネルは後続シャードが本モデルの上に構築する
//! (`strategy_ui` / `tasks` と同じ「純モデル + egui パネル分離」パターン)。
//!
//! - 保持データの schema 単一情報源は `anaden-vision` (`TaskDef` /
//!   `PipelineManifest`)。本モデルは `anaden_vision::Action` を直接保持するため
//!   既存 `app::PipelineActionKind` (ClickSelf/DoNothing/Stop のみ) を超える
//!   ClickRect/Swipe を含むフル Action 編集もフォーム側でそのまま可能。
//! - 保存は `anaden_vision::save_task_def` / `save_pipeline_manifest` に委譲
//!   (保存 -> load 往復はテストで機械保証)。
//! - UC-2: テンプレート PNG (ライブラリ由来の絶対パス) を pipeline dir 基準の
//!   相対パス・フォワードスラッシュ形式へ解決する ([`resolve_template_reference`])。

use std::path::{Path, PathBuf};

use anaden_core::{Goal, GoalError, StopCondition};
use anaden_vision::{PipelineManifest, TaskDef};

/// ROI 検証基準の画面寸法 (raw-1258x708 PC キャプチャ空間)。
/// pipeline.rs テストの `assert_roi_within_1258x708` と同一契約。
pub const SCREEN_WIDTH: u32 = 1258;
pub const SCREEN_HEIGHT: u32 = 708;

/// シナリオ編集状態のバリデーションエラー。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ScenarioValidationError {
    /// シナリオ名が空 (保存先ディレクトリ名になれない)。
    #[error("scenario name must not be empty")]
    EmptyName,
    /// TaskDef が 1 つもない (manifest 単独では実行不能)。
    #[error("scenario must contain at least 1 task")]
    NoTasks,
    /// start_task が未設定。
    #[error("start_task must be set")]
    EmptyStartTask,
    /// start_task が TaskDef 名前空間に存在しない。
    #[error("start_task `{task}` does not match any TaskDef name")]
    UnknownStartTask {
        /// 不一致だった start_task 名。
        task: String,
    },
    /// TaskDef 名の重複 (名前空間が一意でないと lookup が曖昧になる)。
    #[error("duplicate task name `{name}`")]
    DuplicateTaskName {
        /// 重複していたタスク名。
        name: String,
    },
    /// next 参照が TaskDef 名前空間に存在しない。
    #[error("task `{task}`: next reference `{next}` does not match any TaskDef name")]
    UnresolvedNext {
        /// 参照元タスク名。
        task: String,
        /// 解決不能だった next 参照先。
        next: String,
    },
    /// Goal の不変量違反 (`Goal::validate` の委譲結果)。
    #[error("goal[{index}] `{goal_name}` invalid: {source}")]
    GoalInvalid {
        /// `goals` 内のインデックス。
        index: usize,
        /// ゴール名。
        goal_name: String,
        /// 委譲先 (`Goal::validate`) のエラー。
        #[source]
        source: GoalError,
    },
    /// ROI が画面外にはみ出す、または幅/高さが 0。
    #[error(
        "task `{task}`: roi {roi:?} exceeds screen {SCREEN_WIDTH}x{SCREEN_HEIGHT} or has zero size"
    )]
    RoiOutOfBounds {
        /// 対象タスク名。
        task: String,
        /// はみ出し/ゼロサイズだった ROI `[x, y, w, h]`。
        roi: [u32; 4],
    },
}

/// シナリオ編集の純状態モデル (UC-1)。
///
/// 保持するのは「保存したい値」のみ。UI 入力バッファや egui 状態は持たず、
/// フォームパネルが本モデルのフィールドを直接編集する。
///
/// `TaskDef` が `PartialEq` 非実装のため本構造体も `PartialEq` を持たない
/// (等価比較はフィールド単位で行う)。
#[derive(Debug, Clone)]
pub struct ScenarioEditorState {
    /// シナリオ名 (= `templates/pipelines/<name>/` ディレクトリ名)。
    pub name: String,
    /// 最初に実行する TaskDef 名。
    pub start_task: String,
    /// ゴール (終端条件) リスト。StopCondition 3 種 + All/Any 合成を保持。
    pub goals: Vec<Goal>,
    /// TaskDef 編集リスト (name/state/algorithm/template/roi/threshold/action/next)。
    pub tasks: Vec<TaskDef>,
}

impl ScenarioEditorState {
    /// 空のシナリオ (task/goal なし) を作成する。
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            start_task: String::new(),
            goals: Vec::new(),
            tasks: Vec::new(),
        }
    }

    /// TaskDef を追加する。start_task が空なら最初のタスクを採用する
    /// (新規シナリオ作成の定石フロー。2 件目以降は既存 start_task を維持)。
    pub fn add_task(&mut self, task: TaskDef) {
        if self.start_task.is_empty() {
            self.start_task = task.name.clone();
        }
        self.tasks.push(task);
    }

    /// 指定名の TaskDef を削除する。
    ///
    /// 削除対象が start_task だった場合、残り先頭のタスクへ繋ぎ替える
    /// (タスクが空なら start_task を空に戻す)。戻り値は削除成否。
    pub fn remove_task(&mut self, name: &str) -> bool {
        let Some(pos) = self.tasks.iter().position(|t| t.name == name) else {
            return false;
        };
        let removed = self.tasks.remove(pos);
        if self.start_task == removed.name {
            self.start_task = self
                .tasks
                .first()
                .map_or_else(String::new, |t| t.name.clone());
        }
        true
    }

    /// start_task を設定する。TaskDef 名前空間に存在しない名前は無視して false。
    pub fn set_start_task(&mut self, name: &str) -> bool {
        if self.task(name).is_some() {
            self.start_task = name.to_string();
            true
        } else {
            false
        }
    }

    /// 指定名の TaskDef への参照。
    #[must_use]
    pub fn task(&self, name: &str) -> Option<&TaskDef> {
        self.tasks.iter().find(|t| t.name == name)
    }

    /// 指定名の TaskDef への可変参照 (フォームでの直接編集用)。
    pub fn task_mut(&mut self, name: &str) -> Option<&mut TaskDef> {
        self.tasks.iter_mut().find(|t| t.name == name)
    }

    /// TaskDef 名前空間 (ドロップダウン/next 参照候補表示用)。
    #[must_use]
    pub fn task_names(&self) -> Vec<&str> {
        self.tasks.iter().map(|t| t.name.as_str()).collect()
    }

    /// ゴールを追加する。
    pub fn add_goal(&mut self, goal: Goal) {
        self.goals.push(goal);
    }

    /// `goals[index]` を削除する。範囲外は false。
    pub fn remove_goal(&mut self, index: usize) -> bool {
        if index < self.goals.len() {
            self.goals.remove(index);
            true
        } else {
            false
        }
    }

    /// `goals[index]` の停止条件を差し替える (ゴール名は保持)。範囲外は false。
    pub fn update_goal_stop(&mut self, index: usize, stop: StopCondition) -> bool {
        if let Some(goal) = self.goals.get_mut(index) {
            goal.stop = stop;
            true
        } else {
            false
        }
    }

    /// UC-2: ライブラリ PNG (絶対パス) を pipeline dir 基準の相対参照として
    /// タスクへ割り当てる ([`resolve_template_reference`]参照)。
    /// 戻り値は対象タスクが存在して割り当てたか。
    pub fn assign_template(&mut self, task_name: &str, png: &Path, pipeline_dir: &Path) -> bool {
        let reference = resolve_template_reference(png, pipeline_dir);
        match self.task_mut(task_name) {
            Some(task) => {
                task.template = PathBuf::from(reference);
                true
            }
            None => false,
        }
    }

    /// 現在の状態から pipeline manifest へ変換する (バリデーションは呼出側の責務)。
    #[must_use]
    pub fn to_manifest(&self) -> PipelineManifest {
        PipelineManifest {
            start_task: self.start_task.clone(),
            goals: self.goals.clone(),
        }
    }

    /// 全バリデーション問題を収集して返す (フォームで全件一覧表示する用途)。
    ///
    /// 検査内容: シナリオ名非空・TaskDef 1 件以上・タスク名一意・start_task が
    /// TaskDef 名前空間に存在・next 参照が解決可能・各 Goal の [`Goal::validate`]
    /// 委譲・ROI が [`SCREEN_WIDTH`]x[`SCREEN_HEIGHT`] 画面内で有効サイズ。
    /// 出力順は決定論的 (名前 → TaskDef 存在 → 重複 → start_task → タスク毎 →
    /// ゴール毎)。
    #[must_use]
    pub fn validation_issues(&self) -> Vec<ScenarioValidationError> {
        let mut issues = Vec::new();

        if self.name.trim().is_empty() {
            issues.push(ScenarioValidationError::EmptyName);
        }
        if self.tasks.is_empty() {
            issues.push(ScenarioValidationError::NoTasks);
        }

        // タスク名一意性 (名前空間整合)。二件目以降の出現を報告する。
        for (i, task) in self.tasks.iter().enumerate() {
            if self.tasks[..i].iter().any(|t| t.name == task.name) {
                issues.push(ScenarioValidationError::DuplicateTaskName {
                    name: task.name.clone(),
                });
            }
        }

        if self.start_task.trim().is_empty() {
            issues.push(ScenarioValidationError::EmptyStartTask);
        } else if !self.tasks.iter().any(|t| t.name == self.start_task) {
            issues.push(ScenarioValidationError::UnknownStartTask {
                task: self.start_task.clone(),
            });
        }

        for task in &self.tasks {
            if let Some(nexts) = &task.next {
                for next in nexts {
                    if !self.tasks.iter().any(|t| &t.name == next) {
                        issues.push(ScenarioValidationError::UnresolvedNext {
                            task: task.name.clone(),
                            next: next.clone(),
                        });
                    }
                }
            }
            if let Some(roi) = task.roi
                && !roi_within_screen(roi)
            {
                issues.push(ScenarioValidationError::RoiOutOfBounds {
                    task: task.name.clone(),
                    roi,
                });
            }
        }

        for (index, goal) in self.goals.iter().enumerate() {
            if let Err(source) = goal.validate() {
                issues.push(ScenarioValidationError::GoalInvalid {
                    index,
                    goal_name: goal.name.clone(),
                    source,
                });
            }
        }

        issues
    }

    /// 最初のバリデーション問題を返す ([`Goal::validate`] と同じ契約)。
    ///
    /// # Errors
    /// 1 つでも問題があればその最初の [`ScenarioValidationError`]。
    pub fn validate(&self) -> Result<(), ScenarioValidationError> {
        match self.validation_issues().into_iter().next() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// ROI `[x, y, w, h]` が画面内で有効サイズか (pipeline.rs テストと同一契約)。
fn roi_within_screen(roi: [u32; 4]) -> bool {
    let [x, y, w, h] = roi;
    w > 0 && h > 0 && x.saturating_add(w) <= SCREEN_WIDTH && y.saturating_add(h) <= SCREEN_HEIGHT
}

/// UC-2: テンプレート PNG パスを pipeline dir 基準の相対パス
/// (フォワードスラッシュ区切り) へ解決する。
///
/// - 相対パス入力はセパレータ正規化のみ。
/// - 絶対パスは共通祖先接頭辞を求め、`..` 遷移 + 残差で相対化する
///   (`templates/pipelines/fishing` 基準の `../field_loop_pc/hud_tr.png` 形式)。
/// - 共通祖先なし (Windows ドライブ違い等) は絶対・フォワードスラッシュ形式を返す。
///
/// `anaden_vision::save_task_def` の保存時相対化と対称のレキシカル変換
/// (シンボリックリンク解決はしない)。
#[must_use]
pub fn resolve_template_reference(png: &Path, pipeline_dir: &Path) -> String {
    let forward = |p: &Path| p.to_string_lossy().replace('\\', "/");
    if !png.is_absolute() || pipeline_dir.as_os_str().is_empty() {
        return forward(png);
    }
    let target: Vec<_> = png.components().collect();
    let base: Vec<_> = pipeline_dir.components().collect();
    let common = target
        .iter()
        .zip(base.iter())
        .take_while(|(t, b)| t == b)
        .count();
    // 先頭 (Prefix/RootDir) 不一致 = 共通祖先なし → 絶対パス保持。
    if common == 0 {
        return forward(png);
    }
    let mut rel = PathBuf::new();
    for _ in common..base.len() {
        rel.push("..");
    }
    for component in &target[common..] {
        rel.push(component.as_os_str());
    }
    if rel.as_os_str().is_empty() {
        return forward(png);
    }
    forward(&rel)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use anaden_vision::{Action, Algorithm};
    use std::fs;

    /// テスト用 TaskDef (roi=[10,20,100,50]・threshold=0.8・click_self)。
    fn task_def(name: &str, next: Option<Vec<&str>>) -> TaskDef {
        TaskDef {
            name: name.to_string(),
            state: "Field".to_string(),
            algorithm: Algorithm::Ccoeff,
            template: PathBuf::from(format!("{}.png", name.to_lowercase())),
            roi: Some([10, 20, 100, 50]),
            threshold: 0.8,
            base: None,
            action: Some(Action::ClickSelf),
            next: next.map(|v| v.iter().map(|s| s.to_string()).collect()),
        }
    }

    fn loop_goal(name: &str, target: u64) -> Goal {
        Goal {
            name: name.to_string(),
            stop: StopCondition::LoopCount { target },
        }
    }

    /// OS ごとの workspace 風ルート (`/work/...` or `C:\work\...`)。
    fn pipelines_root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\work\templates\pipelines")
        } else {
            PathBuf::from("/work/templates/pipelines")
        }
    }

    #[test]
    fn new_state_starts_empty_and_reports_no_tasks() {
        let st = ScenarioEditorState::new("neko_nikki");
        assert!(st.tasks.is_empty());
        assert!(st.goals.is_empty());
        assert_eq!(st.start_task, "");
        assert!(matches!(
            st.validate(),
            Err(ScenarioValidationError::NoTasks)
        ));
    }

    #[test]
    fn add_task_adopts_first_task_as_start_and_keeps_it() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("First", None));
        assert_eq!(st.start_task, "First");
        st.add_task(task_def("Second", None));
        assert_eq!(st.start_task, "First");
        assert_eq!(st.task_names(), vec!["First", "Second"]);
    }

    #[test]
    fn remove_task_fixes_up_start_task() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        st.add_task(task_def("B", None));
        assert!(st.remove_task("A"));
        assert_eq!(st.start_task, "B");
        assert_eq!(st.task_names(), vec!["B"]);
        assert!(st.remove_task("B"));
        assert_eq!(st.start_task, "");
        assert!(!st.remove_task("B"));
    }

    #[test]
    fn set_start_task_accepts_only_known_names() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        assert!(!st.set_start_task("Ghost"));
        assert_eq!(st.start_task, "A");
        st.start_task = String::new();
        assert!(st.set_start_task("A"));
        assert_eq!(st.start_task, "A");
    }

    #[test]
    fn task_mut_enables_full_action_editing_including_swipe() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        let Some(t) = st.task_mut("A") else {
            panic!("task A must exist");
        };
        t.action = Some(Action::Swipe {
            from: anaden_core::ScreenRegion::new(100, 100, 30, 30),
            to: anaden_core::ScreenRegion::new(200, 300, 30, 30),
        });
        t.threshold = 0.7;
        let t = st.task("A").expect("A");
        assert!(matches!(t.action, Some(Action::Swipe { .. })));
        assert!((t.threshold - 0.7).abs() < 1e-6);
        assert!(st.task_mut("Ghost").is_none());
    }

    #[test]
    fn goal_editing_preserves_name_and_remove_goal() {
        let mut st = ScenarioEditorState::new("s");
        st.add_goal(loop_goal("g", 5));
        assert!(st.update_goal_stop(0, StopCondition::Timeout { secs: 60 }));
        assert_eq!(st.goals[0].name, "g");
        assert_eq!(st.goals[0].stop, StopCondition::Timeout { secs: 60 });
        assert!(!st.update_goal_stop(9, StopCondition::LoopCount { target: 1 }));
        assert!(st.remove_goal(0));
        assert!(st.goals.is_empty());
        assert!(!st.remove_goal(0));
    }

    #[test]
    fn validate_ok_for_complete_scenario() {
        let mut st = ScenarioEditorState::new("fishing2");
        st.add_task(task_def("Start", Some(vec!["Loop"])));
        // roi: None (= 全面) も有効。
        st.add_task(TaskDef {
            roi: None,
            ..task_def("Loop", Some(vec!["Start"]))
        });
        st.add_goal(loop_goal("loop50", 50));
        st.add_goal(Goal {
            name: "any".to_string(),
            stop: StopCondition::Any {
                conditions: vec![StopCondition::Timeout { secs: 3600 }],
            },
        });
        assert!(st.validation_issues().is_empty());
        assert!(st.validate().is_ok());
    }

    #[test]
    fn validate_flags_empty_name_no_tasks_empty_start() {
        let st = ScenarioEditorState::new("   ");
        let issues = st.validation_issues();
        assert!(issues.contains(&ScenarioValidationError::EmptyName));
        assert!(issues.contains(&ScenarioValidationError::NoTasks));
        assert!(issues.contains(&ScenarioValidationError::EmptyStartTask));
    }

    #[test]
    fn validate_flags_unknown_start_task() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        st.start_task = "Ghost".to_string();
        let issues = st.validation_issues();
        assert!(issues.contains(&ScenarioValidationError::UnknownStartTask {
            task: "Ghost".to_string()
        }));
    }

    #[test]
    fn validate_flags_unresolved_next_reference() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("Start", Some(vec!["Missing"])));
        let issues = st.validation_issues();
        assert!(issues.contains(&ScenarioValidationError::UnresolvedNext {
            task: "Start".to_string(),
            next: "Missing".to_string()
        }));
    }

    #[test]
    fn validate_flags_duplicate_task_names() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        st.add_task(task_def("A", None));
        let issues = st.validation_issues();
        assert!(
            issues.contains(&ScenarioValidationError::DuplicateTaskName {
                name: "A".to_string()
            })
        );
    }

    #[test]
    fn validate_delegates_to_goal_validate() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        st.add_goal(loop_goal("bad", 0));
        let issues = st.validation_issues();
        let expected = ScenarioValidationError::GoalInvalid {
            index: 0,
            goal_name: "bad".to_string(),
            source: GoalError::NonPositive { field: "target" },
        };
        assert!(issues.contains(&expected));
    }

    #[test]
    fn validate_flags_roi_out_of_bounds_and_zero_size() {
        let mut st = ScenarioEditorState::new("s");
        let mut edge = task_def("Edge", None);
        edge.roi = Some([1200, 600, 200, 200]);
        st.add_task(edge);
        let mut zero = task_def("Zero", None);
        zero.roi = Some([10, 10, 0, 40]);
        st.add_task(zero);
        let issues = st.validation_issues();
        assert!(issues.contains(&ScenarioValidationError::RoiOutOfBounds {
            task: "Edge".to_string(),
            roi: [1200, 600, 200, 200]
        }));
        assert!(issues.contains(&ScenarioValidationError::RoiOutOfBounds {
            task: "Zero".to_string(),
            roi: [10, 10, 0, 40]
        }));
    }

    #[test]
    fn resolve_template_reference_inside_pipeline_dir_is_bare_relative() {
        let dir = pipelines_root().join("fishing");
        let png = dir.join("scenes").join("title.png");
        assert_eq!(resolve_template_reference(&png, &dir), "scenes/title.png");
    }

    #[test]
    fn resolve_template_reference_from_sibling_dir_yields_parent_jump() {
        let root = pipelines_root();
        let png = root.join("field_loop_pc").join("hud_tr.png");
        let dir = root.join("fishing");
        assert_eq!(
            resolve_template_reference(&png, &dir),
            "../field_loop_pc/hud_tr.png"
        );
    }

    #[test]
    fn resolve_template_reference_normalizes_separator_for_relative_input() {
        let rel = if cfg!(windows) {
            "scenes\\title.png"
        } else {
            "scenes/title.png"
        };
        assert_eq!(
            resolve_template_reference(Path::new(rel), Path::new("ignored")),
            "scenes/title.png"
        );
    }

    #[cfg(windows)]
    #[test]
    fn resolve_template_reference_without_common_ancestor_keeps_absolute() {
        let png = Path::new(r"D:\library\title.png");
        let dir = Path::new(r"C:\work\templates\pipelines\my");
        assert_eq!(resolve_template_reference(png, dir), "D:/library/title.png");
    }

    #[test]
    fn assign_template_sets_forward_slash_relative_reference() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("my_pipeline");
        let png = dir.join("scenes").join("btn.png");
        let mut st = ScenarioEditorState::new("my_pipeline");
        st.add_task(task_def("Start", Some(vec![])));
        assert!(st.assign_template("Start", &png, &dir));
        let t = st.task("Start").expect("Start");
        assert_eq!(t.template, PathBuf::from("scenes/btn.png"));
        assert!(!st.assign_template("Ghost", &png, &dir));
    }

    #[test]
    fn to_manifest_carries_start_task_and_goals() {
        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("A", None));
        st.add_goal(loop_goal("g", 7));
        let m = st.to_manifest();
        assert_eq!(m.start_task, "A");
        assert_eq!(m.goals, vec![loop_goal("g", 7)]);
    }

    // AC-1 機械保証: エディタ状態 -> anaden-vision save -> load 往復。
    #[test]
    fn saved_scenario_roundtrips_through_anaden_vision_load() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("my_pipeline");
        fs::create_dir_all(&dir).expect("mkdir");

        let mut st = ScenarioEditorState::new("my_pipeline");
        st.add_task(task_def("Start", Some(vec!["End"])));
        st.add_task(TaskDef {
            roi: None,
            ..task_def("End", Some(vec![]))
        });
        st.add_goal(loop_goal("loop3", 3));
        st.add_goal(Goal {
            name: "combo".to_string(),
            stop: StopCondition::Any {
                conditions: vec![
                    StopCondition::TemplateMatch {
                        task: "End".to_string(),
                        confidence: 0.85,
                    },
                    StopCondition::Timeout { secs: 600 },
                ],
            },
        });
        st.validate().expect("scenario must be valid");

        let manifest = st.to_manifest();
        anaden_vision::save_pipeline_manifest(&manifest, &dir).expect("save manifest");
        for t in &st.tasks {
            anaden_vision::save_task_def(t, &dir.join(format!("{}.toml", t.name)))
                .expect("save task");
        }

        let loaded_manifest = anaden_vision::load_pipeline_manifest(&dir).expect("load manifest");
        assert_eq!(loaded_manifest, manifest);
        let defs = anaden_vision::load_pipeline(&dir).expect("load tasks");
        assert_eq!(defs.len(), 2, "pipeline.toml (manifest) must be skipped");
        let start = defs.iter().find(|d| d.name == "Start").expect("Start");
        assert_eq!(
            start.next.as_deref(),
            Some(&["End".to_string()][..]),
            "next chain survives round-trip"
        );
        assert!(start.template.is_absolute());
        assert!(
            start.template.ends_with("start.png"),
            "relative template preserved: {:?}",
            start.template
        );
        let end = defs.iter().find(|d| d.name == "End").expect("End");
        assert_eq!(end.roi, None);
        assert_eq!(end.action, Some(Action::ClickSelf));
    }
}
