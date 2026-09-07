//! シナリオ編集ドメインの状態操作モデル (Issue #160 Shard 1 / UC-1+2 /
//! Issue #166 分割・Issue #173 で scenario_editor.rs から分割)。
//!
//! `templates/pipelines/<name>/` 配下の pipeline manifest (start_task + goals) と
//! TaskDef 群を GUI で作成・編集するための純状態モデルのうち、状態操作
//! (追加・削除・参照・manifest 変換) を担う。egui 非依存の状態操作層であり、
//! 描画パネルは `scenario_panel` が本モデルの上に構築する
//! (`strategy_ui` / `tasks` と同じ「純モデル + egui パネル分離」パターン)。
//!
//! - 保持データの schema 単一情報源は `anaden-vision` (`TaskDef` /
//!   `PipelineManifest`)。本モデルは `anaden_vision::Action` を直接保持するため
//!   既存 `app::PipelineActionKind` (ClickSelf/DoNothing/Stop のみ) を超える
//!   ClickRect/Swipe を含むフル Action 編集もフォーム側でそのまま可能。
//! - UC-2: テンプレート PNG (ライブラリ由来の絶対パス) を pipeline dir 基準の
//!   相対パス・フォワードスラッシュ形式へ解決する ([`resolve_template_reference`])。
//!
//! バリデーションは `scenario_validate`、保存 (save) は `scenario_save` へ
//! 分割し、旧パス互換の facade が `scenario_editor` (Issue #173)。

use std::path::{Path, PathBuf};

use anaden_core::{Goal, StopCondition};
use anaden_vision::{PipelineManifest, TaskDef};

/// シナリオ編集の純状態モデル (UC-1)。
///
/// 保持するのは「保存したい値」のみ。UI 入力バッファや egui 状態は持たず、
/// フォームパネルが本モデルのフィールドを直接編集する。
///
/// 本構造体自体は UI 編集状態のため `PartialEq` を持たない (等価比較が必要な
/// 箇所は `TaskDef` の `PartialEq` — UC-4 (Issue #160 Shard 5) で追加 — を
/// `tasks` ベクタ単位で使う)。
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
    /// UC-4 (Shard 5): ディスクからロードした元タスク名集合 (作成時検査の
    /// baseline)。命名規約
    /// ([`crate::scenario_validate::ScenarioValidationError::TaskNaming`]) と ROI
    /// 画面内検査
    /// ([`crate::scenario_validate::ScenarioValidationError::RoiOutOfBounds`]) は
    /// baseline 外の名前 (= 新規追加・リネーム後) にのみ適用する — 既存 pipeline
    /// の資産 (20:9 座標系 ROI 等の PC 1258x708 契約外データ) を編集なき保存で
    /// 弾かないため。新規シナリオでは空 (= 全タスクが検査対象)。
    pub loaded_task_names: Vec<String>,
    /// UC-4: ロード済みタスクの元 TOML ファイル名 (TaskDef name → ファイル stem)。
    /// [`crate::scenario_save::save_scenario`] は元ファイルへ書き戻す — 既存
    /// pipeline は stem ≠ name (例: `tap_bottom.toml` の name は
    /// `TapBottomStable`) のため、name で保存すると同一 TaskDef の重複ファイルが
    /// でき再 load でタスクが倍化する。対応エントリの無い (新規追加) タスクは
    /// `<name>.toml` へ保存される。リネームは「新規ファイルへ保存 + 旧ファイルの
    /// 掃除」として扱われる。
    pub loaded_task_files: Vec<(String, String)>,
    /// この編集状態の由来 pipeline ディレクトリ (ロード元、または直近の新規保存先)。
    ///
    /// [`crate::scenario_save::save_scenario`] の既存 dir 上書きガード
    /// ([`crate::scenario_save::ScenarioSaveError::PipelineDirAlreadyExists`]) が
    /// 「同一 dir への書き戻し (ロード編集・連続保存)」を許可するための所有権証明。
    /// [`crate::scenario_load::load_scenario_from_dir`] がロード元を設定し、
    /// [`crate::scenario_panel::ScenarioPanel::save`] が保存成功時に保存先へ更新する。
    /// 新規シナリオ (未保存) では `None` (= 既存 dir への保存は全て拒否)。
    pub loaded_from: Option<std::path::PathBuf>,
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
            loaded_task_names: Vec::new(),
            loaded_task_files: Vec::new(),
            loaded_from: None,
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

/// StopCondition の UI 表示要約 (ゴール一覧行・豆腐なし ASCII + 日本語)。
/// scenario_panel のゴール一覧描画から参照されるため pub(crate)。
pub(crate) fn goal_summary(stop: &StopCondition) -> String {
    match stop {
        StopCondition::LoopCount { target } => format!("ループ {target} 回"),
        StopCondition::TemplateMatch { task, confidence } => {
            format!("テンプレ {task} @ {confidence:.2}")
        }
        StopCondition::Timeout { secs } => format!("{secs} 秒"),
        StopCondition::All { .. } => "ALL 合成".to_string(),
        StopCondition::Any { .. } => "ANY 合成".to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::scenario_validate::ScenarioValidationError;
    use anaden_vision::{Action, Algorithm};

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
}
