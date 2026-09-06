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
//!
//! Shard 3 (T3) はこの上に保存フロー ([`save_scenario`]) と Authoring ペイン埋め込み
//! 用 egui パネル ([`ScenarioPanel`]) を構築する (UC-1/UC-2 の GUI 配線)。

use std::path::{Path, PathBuf};

use anaden_core::{Goal, GoalError, StopCondition};
use anaden_vision::{PipelineManifest, TaskDef};
use image::DynamicImage;

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
    /// シナリオ名が保存先ディレクトリ名として不適 (パス区切り・`.`/`..` 等)。
    /// `templates/pipelines/<name>/` の `<name>` は単一パス要素でなければならない。
    #[error("scenario name `{name}` is not a safe directory name")]
    UnsafeName {
        /// 不適だったシナリオ名。
        name: String,
    },
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
    /// 検査内容: シナリオ名非空・ディレクトリ名として安全・TaskDef 1 件以上・
    /// タスク名一意・start_task が TaskDef 名前空間に存在・next 参照が解決可能・
    /// 各 Goal の [`Goal::validate`] 委譲・ROI が [`SCREEN_WIDTH`]x[`SCREEN_HEIGHT`]
    /// 画面内で有効サイズ。
    /// 出力順は決定論的 (名前 → TaskDef 存在 → 重複 → start_task → タスク毎 →
    /// ゴール毎)。
    #[must_use]
    pub fn validation_issues(&self) -> Vec<ScenarioValidationError> {
        let mut issues = Vec::new();

        if self.name.trim().is_empty() {
            issues.push(ScenarioValidationError::EmptyName);
        }
        // 保存先ディレクトリ名として安全か。file_name() が名前全体と一致すれば
        // パス区切りを含まない単一要素 (`.`/`..`/末尾区切りは不一致になる)。
        let name = self.name.trim();
        if !name.is_empty() && Path::new(name).file_name() != Some(std::ffi::OsStr::new(name)) {
            issues.push(ScenarioValidationError::UnsafeName {
                name: self.name.clone(),
            });
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

// ---- Shard 3 (T3): 保存フロー + Authoring ペイン埋め込みパネル (UC-1/UC-2) ----

/// シナリオ保存 (manifest + TaskDef 群 + ROI 由来テンプレート PNG) のエラー。
#[derive(Debug, thiserror::Error)]
pub enum ScenarioSaveError {
    /// エディタ状態のバリデーション不合格 ([`ScenarioEditorState::validate`])。
    #[error("scenario invalid: {0}")]
    Invalid(#[from] ScenarioValidationError),
    /// pipeline ディレクトリ作成失敗。
    #[error("pipeline dir create failed")]
    DirCreate(#[source] std::io::Error),
    /// ROI 追加タスクのテンプレート PNG 書き出し失敗。
    #[error("template PNG write failed")]
    PngWrite(#[source] image::ImageError),
    /// manifest / TaskDef TOML の保存失敗 (anaden-vision save ヘルパー)。
    #[error("pipeline save failed: {0}")]
    Vision(#[from] anaden_vision::TaskDefError),
}

/// 検証済みシナリオを `<pipelines_root>/<シナリオ名>/` へ保存する (UC-1 保存経路)。
///
/// 書き出し構成 (既存 pipeline ディレクトリと完全互換):
/// - `pipeline.toml` — manifest (start_task + goals)。[`anaden_vision::save_pipeline_manifest`]
/// - `<task>.toml` — 各 TaskDef。[`anaden_vision::save_task_def`]
/// - `<task>.png` — ROI から追加したタスクのテンプレート PNG 本体
///
/// `pngs` は「タスク追加時に確保した crop」のリストで、TaskDef 名前空間に残る
/// タスクのみ書き出す (削除済みタスクの crop は無視)。ROI 追加タスクの
/// `template` は追加時に `<name>.png` (pipeline dir 基準の裸相対) が設定済みの
/// ため、保存 TOML の template 参照と PNG 実体が一致する。
///
/// # Errors
/// - [`ScenarioSaveError::Invalid`]: バリデーション不合格 (1 バイトも書かない)。
/// - それ以外: 各書き出し段階の失敗 ([`ScenarioSaveError::DirCreate`] /
///   [`ScenarioSaveError::PngWrite`] / [`ScenarioSaveError::Vision`])。
///
/// 保存 → [`anaden_vision::load_pipeline`] / [`load_pipeline_manifest` 往復は
/// `tests/scenario_editor_tests.rs` (AC-1) で機械保証されている。
///
/// [`load_pipeline_manifest`]: anaden_vision::load_pipeline_manifest
pub fn save_scenario(
    state: &ScenarioEditorState,
    pngs: &[(String, DynamicImage)],
    pipelines_root: &Path,
) -> Result<PathBuf, ScenarioSaveError> {
    state.validate()?;
    let dir = pipelines_root.join(state.name.trim());
    std::fs::create_dir_all(&dir).map_err(ScenarioSaveError::DirCreate)?;
    for (name, img) in pngs {
        if !state.tasks.iter().any(|t| &t.name == name) {
            continue; // 削除済みタスクの crop は書かない
        }
        img.save(dir.join(format!("{name}.png")))
            .map_err(ScenarioSaveError::PngWrite)?;
    }
    anaden_vision::save_pipeline_manifest(&state.to_manifest(), &dir)?;
    for task in &state.tasks {
        anaden_vision::save_task_def(task, &dir.join(format!("{}.toml", task.name)))?;
    }
    Ok(dir)
}

/// StopCondition の UI 表示要約 (ゴール一覧行・豆腐なし ASCII + 日本語)。
fn goal_summary(stop: &StopCondition) -> String {
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

/// ゴール追加フォームの停止条件種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum GoalKind {
    /// 指定回数ループで停止。
    #[default]
    LoopCount,
    /// 指定秒数でタイムアウト停止。
    Timeout,
    /// 開始タスクのテンプレ一致で停止 (task 名は start_task を採用)。
    TemplateMatch,
}

impl GoalKind {
    /// UI コンボ表示ラベル。
    fn label(self) -> &'static str {
        match self {
            Self::LoopCount => "ループ回数",
            Self::Timeout => "タイムアウト秒",
            Self::TemplateMatch => "テンプレ一致(開始タスク)",
        }
    }

    /// 種別切替時の値入力既定値。
    fn default_value(self) -> &'static str {
        match self {
            Self::LoopCount => "10",
            Self::Timeout => "600",
            Self::TemplateMatch => "0.85",
        }
    }
}

/// 保存済みシナリオの記録 (UC-3 登録フローの対象)。
///
/// `save()` 成功時の実体 (保存先 dir + manifest の start_task) を保持する。
/// 以後の名前変更・編集が未保存でも、登録はこの保存済み実体に対して行う。
#[derive(Debug, Clone)]
struct SavedScenario {
    /// 保存済み pipeline ディレクトリ (絶対パス)。
    dir: PathBuf,
    /// 保存済み manifest の start_task。
    start_task: String,
}

/// Authoring ペイン埋め込みのシナリオ作成パネル (UC-1/UC-2)。
///
/// `strategy_ui::StrategyPanel` と同じ「純状態モデル + egui パネル」パターン。
/// 保持する編集データは [`ScenarioEditorState`]、保存は [`save_scenario`] へ
/// 委譲。描画は [`Self::ui`] を Authoring ペインの collapsing セクションから
/// 呼ぶ (app.rs は配線のみ)。
///
/// UC-3 (Shard 4): 保存済み pipeline をタスクへ登録・有効化するサブフローは
/// [`Self::ui_task_link`] (ドメインは `scenario_task_link`)。
pub struct ScenarioPanel {
    /// 編集中のシナリオ (純状態モデル)。フォームがフィールドを直接編集する。
    pub state: ScenarioEditorState,
    /// 保存先ルート (既定は workspace の `templates/pipelines`)。
    pipelines_root: PathBuf,
    /// ROI 追加タスクのテンプレ PNG 本体 (task 名 → crop)。保存時に書き出す。
    pending_pngs: Vec<(String, DynamicImage)>,
    /// UC-2 でファイル参照を割り当てた PNG の元絶対パス (task 名 → 絶対パス)。
    /// 保存直前に最終 pipeline dir 基準で再相対化する (シナリオ名変更耐性)。
    assigned_pngs: Vec<(String, PathBuf)>,
    /// ゴール追加フォーム: 停止条件種別。
    goal_kind: GoalKind,
    /// ゴール追加フォーム: 値 (回数 / 秒 / 信頼度)。
    goal_value: String,
    /// ゴール追加フォーム: ゴール名。
    goal_name: String,
    /// UC-3: 直近の保存成功実体 (未保存 = None。登録フローの対象)。
    saved: Option<SavedScenario>,
    /// UC-3 登録フォーム: task id (保存時にシナリオ名へ同期・編集可)。
    pub task_id_input: String,
    /// UC-3 登録フォーム: title (保存時にシナリオ名へ同期・編集可)。
    pub task_title_input: String,
    /// UC-3: 既存 stub タスク (implemented=false の pipeline_run) からの
    /// 紐付け先選択 (未選択 = None)。
    pub bind_selection: Option<String>,
}

impl ScenarioPanel {
    /// 保存先ルートを指定して構築する。
    #[must_use]
    pub fn new(pipelines_root: PathBuf) -> Self {
        Self {
            state: ScenarioEditorState::new("my_scenario"),
            pipelines_root,
            pending_pngs: Vec::new(),
            assigned_pngs: Vec::new(),
            goal_kind: GoalKind::default(),
            goal_value: GoalKind::default().default_value().to_string(),
            goal_name: "goal_1".to_string(),
            saved: None,
            task_id_input: String::new(),
            task_title_input: String::new(),
            bind_selection: None,
        }
    }

    /// 保存先 pipeline ディレクトリ (`<pipelines_root>/<シナリオ名>`)。
    #[must_use]
    pub fn pipeline_dir(&self) -> PathBuf {
        self.pipelines_root.join(self.state.name.trim())
    }

    /// 現在のROI候補タスク (TaskDef + crop PNG) を追加する
    /// (「+ 現在のROIをタスク追加」ボタンの実体)。同名タスク既存時は Err。
    pub fn add_candidate(&mut self, task: TaskDef, png: DynamicImage) -> Result<(), String> {
        if self.state.task(&task.name).is_some() {
            return Err(format!("同名タスクが既に存在します: {}", task.name));
        }
        let name = task.name.clone();
        self.state.add_task(task);
        self.pending_pngs.push((name, png));
        Ok(())
    }

    /// タスクを削除する (pending PNG・UC-2 参照も同期除去)。
    fn remove_task(&mut self, name: &str) {
        self.state.remove_task(name);
        self.pending_pngs.retain(|(n, _)| n != name);
        self.assigned_pngs.retain(|(n, _)| n != name);
    }

    /// 追加フォーム入力から Goal を構築する。種別ごとに値をパースし、不正入力は
    /// Err (fail-closed・黙って既定値へフォールバックしない)。値域検証
    /// (`target > 0` 等) は保存時の [`ScenarioEditorState::validate`] に委ねる。
    fn build_goal(&self) -> Result<Goal, String> {
        let name = if self.goal_name.trim().is_empty() {
            format!("goal_{}", self.state.goals.len() + 1)
        } else {
            self.goal_name.trim().to_string()
        };
        match self.goal_kind {
            GoalKind::LoopCount => {
                let target: u64 = self.goal_value.trim().parse().map_err(|_| {
                    format!("ゴール値は正の整数で入力してください: {}", self.goal_value)
                })?;
                Ok(Goal {
                    name,
                    stop: StopCondition::LoopCount { target },
                })
            }
            GoalKind::Timeout => {
                let secs: u64 = self.goal_value.trim().parse().map_err(|_| {
                    format!(
                        "ゴール値は正の整数 (秒) で入力してください: {}",
                        self.goal_value
                    )
                })?;
                Ok(Goal {
                    name,
                    stop: StopCondition::Timeout { secs },
                })
            }
            GoalKind::TemplateMatch => {
                let confidence: f32 = self.goal_value.trim().parse().map_err(|_| {
                    format!(
                        "ゴール値は信頼度 (0 < v <= 1) で入力してください: {}",
                        self.goal_value
                    )
                })?;
                let task = self.state.start_task.clone();
                if task.is_empty() {
                    return Err("テンプレ一致ゴールには開始タスクの設定が必要です".to_string());
                }
                Ok(Goal {
                    name,
                    stop: StopCondition::TemplateMatch { task, confidence },
                })
            }
        }
    }

    /// 現在の状態を保存する (「シナリオ保存」ボタンの実体)。
    /// UC-2 参照は最終 pipeline dir 基準で再相対化してから保存する。
    /// UC-3: 保存成功時は保存実体 (dir + start_task) を記録し、登録フォームの
    /// 既定値 (task id / title = シナリオ名) を同期する。
    pub fn save(&mut self, status: &mut String) {
        let dir = self.pipeline_dir();
        for (name, png) in self.assigned_pngs.clone() {
            self.state.assign_template(&name, &png, &dir);
        }
        match save_scenario(&self.state, &self.pending_pngs, &self.pipelines_root) {
            Ok(dir) => {
                *status = format!("シナリオ保存: {}", dir.display());
                self.saved = Some(SavedScenario {
                    start_task: self.state.start_task.clone(),
                    dir,
                });
                let name = self.state.name.trim().to_string();
                self.task_id_input = name.clone();
                self.task_title_input = name;
            }
            Err(e) => *status = format!("シナリオ保存失敗: {e}"),
        }
    }

    /// 直近の保存済み pipeline ディレクトリ (未保存 = None)。
    #[must_use]
    pub fn saved_pipeline(&self) -> Option<&Path> {
        self.saved.as_ref().map(|s| s.dir.as_path())
    }

    /// UC-3 (i): 保存済み pipeline を新規タスクとして登録・有効化する
    /// (「新規タスクとして登録・有効化」ボタンの実体。ドメインは
    /// `scenario_task_link::register_and_enable_task`)。
    /// 成功時は [`scenario_task_link::ScenarioPanelEvent::TaskEnabled`] を返す。
    pub fn register_as_new_task(
        &mut self,
        ctx: &crate::scenario_task_link::TaskLinkContext<'_>,
        status: &mut String,
    ) -> Option<crate::scenario_task_link::ScenarioPanelEvent> {
        let Some(saved) = self.saved.clone() else {
            *status = "タスク登録には先にシナリオを保存してください".to_string();
            return None;
        };
        match crate::scenario_task_link::register_and_enable_task(
            ctx.tasks_dir,
            ctx.root,
            &saved.dir,
            &self.task_id_input,
            &self.task_title_input,
            &saved.start_task,
        ) {
            Ok(def) => {
                *status = format!(
                    "タスク登録・有効化: {} [{}] (ホーム一覧へ反映)",
                    def.title, def.id
                );
                Some(crate::scenario_task_link::ScenarioPanelEvent::TaskEnabled {
                    message: status.clone(),
                })
            }
            Err(e) => {
                *status = format!("タスク登録失敗: {e}");
                None
            }
        }
    }

    /// UC-3 (ii): 選択中の既存 stub タスク (implemented=false の pipeline_run) へ
    /// 保存済み pipeline を紐付けて有効化する (「選択タスクへ紐付け・有効化」
    /// ボタンの実体。ドメインは `scenario_task_link::bind_and_enable_task`)。
    pub fn bind_stub_task(
        &mut self,
        ctx: &crate::scenario_task_link::TaskLinkContext<'_>,
        status: &mut String,
    ) -> Option<crate::scenario_task_link::ScenarioPanelEvent> {
        let Some(saved) = self.saved.clone() else {
            *status = "タスク紐付けには先にシナリオを保存してください".to_string();
            return None;
        };
        let Some(id) = self.bind_selection.clone() else {
            *status = "紐付け先の未実装タスクを選択してください".to_string();
            return None;
        };
        match crate::scenario_task_link::bind_and_enable_task(
            ctx.tasks_dir,
            ctx.root,
            &id,
            &saved.dir,
        ) {
            Ok(def) => {
                *status = format!(
                    "タスク有効化: {} [{}] (ホーム一覧へ反映)",
                    def.title, def.id
                );
                Some(crate::scenario_task_link::ScenarioPanelEvent::TaskEnabled {
                    message: status.clone(),
                })
            }
            Err(e) => {
                *status = format!("タスク有効化失敗: {e}");
                None
            }
        }
    }

    /// UC-3: 保存済み pipeline をタスクへ登録・有効化するセクションを描画する。
    ///
    /// 保存済みでない場合は何も描画しない (4 段階フローの (b)(c) は保存 (a) が前提)。
    /// 2 経路: (i) 新規タスクとして登録 (task id / title 編集可)・
    /// (ii) 既存の未実装タスク (`ctx.stubs`) へ紐付け+有効化。
    /// 成功時は [`scenario_task_link::ScenarioPanelEvent`] を返す
    /// (app.rs がホーム一覧を再読込して反映)。
    pub fn ui_task_link(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &crate::scenario_task_link::TaskLinkContext<'_>,
        status: &mut String,
    ) -> Option<crate::scenario_task_link::ScenarioPanelEvent> {
        // 未保存時はセクション自体を表示しない (イベントも無し)。
        let saved = self.saved.clone()?;
        let mut event = None;
        egui::CollapsingHeader::new("タスク登録・有効化")
            .default_open(true)
            .show(ui, |ui| {
                ui.label(format!("pipeline: {}", saved.dir.display()));
                ui.label(format!("start_task: {}", saved.start_task));
                ui.separator();
                // (i) 新規タスクとして登録 (task id 既定 = シナリオ名・title 編集可)。
                ui.horizontal(|ui| {
                    ui.label("task id:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.task_id_input).desired_width(120.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("title:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.task_title_input).desired_width(160.0),
                    );
                });
                if ui.button("新規タスクとして登録・有効化").clicked() {
                    event = self.register_as_new_task(ctx, status);
                }
                ui.separator();
                // (ii) 既存の未実装タスク (implemented=false の pipeline_run) へ紐付け。
                if ctx.stubs.is_empty() {
                    ui.weak("紐付け可能な未実装タスクがありません");
                } else {
                    ui.horizontal(|ui| {
                        ui.label("既存タスク:");
                        let selected = self
                            .bind_selection
                            .clone()
                            .unwrap_or_else(|| "(選択してください)".to_string());
                        egui::ComboBox::from_id_salt("scenario_bind_stub")
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                for stub in ctx.stubs {
                                    ui.selectable_value(
                                        &mut self.bind_selection,
                                        Some(stub.id.clone()),
                                        format!("{} [{}]", stub.title, stub.id),
                                    );
                                }
                            });
                        let can_bind = self.bind_selection.is_some();
                        ui.add_enabled_ui(can_bind, |ui| {
                            if ui.button("選択タスクへ紐付け・有効化").clicked() {
                                event = self.bind_stub_task(ctx, status);
                            }
                        });
                    });
                }
            });
        event
    }

    /// パネル本体を描画する。`candidate` は呼出側 (app.rs) が現在のROI・入力から
    /// 構築した追加候補タスク (None = スクショ/ROI 未確定で追加不可表示)。
    /// 失敗・結果は `status` へ書き出す (Authoring ペインのステータス行と共有)。
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        candidate: Option<(TaskDef, DynamicImage)>,
        status: &mut String,
    ) {
        ui.horizontal(|ui| {
            ui.label("名前:");
            ui.add(egui::TextEdit::singleline(&mut self.state.name).desired_width(120.0));
        });
        ui.label(format!("保存先: {}", self.pipeline_dir().display()));
        ui.separator();

        // --- タスク一覧 (名前一覧を先に clone して borrow を分離) ---
        let names: Vec<String> = self.state.tasks.iter().map(|t| t.name.clone()).collect();
        let mut removed: Option<String> = None;
        let mut pick_png: Option<String> = None;
        for name in &names {
            let Some(task) = self.state.task_mut(name) else {
                continue;
            };
            egui::CollapsingHeader::new(format!("タスク: {name}"))
                .default_open(false)
                .show(ui, |ui| {
                    ui.monospace(format!("template: {}", task.template.display()));
                    ui.add(egui::Slider::new(&mut task.threshold, 0.5..=0.99).text("閾値"));
                    // next 参照 (他タスク名のチェックボックスでON/OFF)。
                    let mut nexts = task.next.clone().unwrap_or_default();
                    let mut changed = false;
                    for other in &names {
                        if other == name {
                            continue;
                        }
                        let mut on = nexts.iter().any(|n| n == other);
                        if ui.checkbox(&mut on, format!("next: {other}")).changed() {
                            if on {
                                nexts.push(other.clone());
                            } else {
                                nexts.retain(|n| n != other);
                            }
                            changed = true;
                        }
                    }
                    if changed {
                        task.next = Some(nexts);
                    }
                    // UC-2: 既存 PNG (テンプレート作成ペイン成果物等) をファイル
                    // 参照として割り当てる (pipeline dir 基準の相対パスへ解決)。
                    if ui.small_button("テンプレPNG参照...").clicked() {
                        pick_png = Some(name.clone());
                    }
                });
            if ui.small_button(format!("[{name}] を削除")).clicked() {
                removed = Some(name.clone());
            }
        }
        if let Some(name) = removed {
            self.remove_task(&name);
        }
        // UC-2: ファイルダイアログで選んだ PNG を相対参照として割り当てる。
        // 元絶対パスも保持し、保存直前に最終 pipeline dir 基準で再相対化する。
        if let Some(name) = pick_png
            && let Some(png) = rfd::FileDialog::new()
                .add_filter("PNG image", &["png"])
                .pick_file()
        {
            let dir = self.pipeline_dir();
            self.assigned_pngs.retain(|(n, _)| n != &name);
            self.assigned_pngs.push((name.clone(), png.clone()));
            if self.state.assign_template(&name, &png, &dir) {
                *status = format!("テンプレート参照: {name} <- {}", png.display());
            }
        }

        // --- タスク追加 (現在のROI候補) ---
        match candidate {
            Some((task, crop)) => {
                if ui
                    .button(format!("+ 現在のROIをタスク追加: {}", task.name))
                    .clicked()
                    && let Err(e) = self.add_candidate(task, crop)
                {
                    *status = e;
                }
            }
            None => {
                ui.label("タスク追加にはスクリーンショットとROI確定が必要です");
            }
        }
        ui.separator();

        // --- 開始タスク選択 (TaskDef 名前空間から) ---
        ui.horizontal(|ui| {
            ui.label("開始タスク:");
            egui::ComboBox::from_id_salt("scenario_start_task")
                .selected_text(if self.state.start_task.is_empty() {
                    "(未設定)".to_string()
                } else {
                    self.state.start_task.clone()
                })
                .show_ui(ui, |ui| {
                    let mut picked: Option<String> = None;
                    for name in &names {
                        if ui
                            .selectable_label(self.state.start_task == *name, name.as_str())
                            .clicked()
                        {
                            picked = Some(name.clone());
                        }
                    }
                    if let Some(p) = picked {
                        self.state.set_start_task(&p);
                    }
                });
        });
        ui.separator();

        // --- ゴール一覧 + 追加フォーム ---
        ui.strong(format!("ゴール ({})", self.state.goals.len()));
        let mut remove_goal: Option<usize> = None;
        for (i, goal) in self.state.goals.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(format!("[{i}] {}", goal.name));
                ui.label(goal_summary(&goal.stop));
                if ui.small_button("削除").clicked() {
                    remove_goal = Some(i);
                }
            });
        }
        if let Some(i) = remove_goal {
            self.state.remove_goal(i);
        }
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("scenario_goal_kind")
                .selected_text(self.goal_kind.label())
                .show_ui(ui, |ui| {
                    for kind in [
                        GoalKind::LoopCount,
                        GoalKind::Timeout,
                        GoalKind::TemplateMatch,
                    ] {
                        ui.selectable_value(&mut self.goal_kind, kind, kind.label());
                    }
                });
            ui.label("値:");
            ui.add(egui::TextEdit::singleline(&mut self.goal_value).desired_width(50.0));
            ui.label("名前:");
            ui.add(egui::TextEdit::singleline(&mut self.goal_name).desired_width(80.0));
            if ui.small_button("+ ゴール追加").clicked() {
                match self.build_goal() {
                    Ok(goal) => self.state.add_goal(goal),
                    Err(e) => *status = e,
                }
            }
        });
        ui.separator();

        // --- 保存 (検証不合格時は全問題を一覧表示してボタン無効化) ---
        let issues = self.state.validation_issues();
        if issues.is_empty() {
            if ui.button("シナリオ保存").clicked() {
                self.save(status);
            }
        } else {
            ui.add_enabled_ui(false, |ui| {
                // 無効化表示のみ (クリックは起きない)。Response は未使用でよい。
                let _ = ui.button("シナリオ保存");
            });
            for issue in &issues {
                ui.colored_label(egui::Color32::from_rgb(220, 60, 60), issue.to_string());
            }
        }
    }
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
    fn validate_flags_unsafe_directory_name() {
        // パス区切り・`.`/`..` は保存先ディレクトリ名として不適。
        for bad in ["a/b", "..", ".", "a\\b", "abc/"] {
            let mut st = ScenarioEditorState::new(bad);
            st.add_task(task_def("A", None));
            assert!(
                st.validation_issues()
                    .contains(&ScenarioValidationError::UnsafeName {
                        name: bad.to_string()
                    }),
                "name {bad:?} must be flagged unsafe"
            );
        }
        // 通常の名前は UnsafeName を出さない。
        let mut ok = ScenarioEditorState::new("my_scenario");
        ok.add_task(task_def("A", None));
        assert!(
            !ok.validation_issues()
                .iter()
                .any(|i| matches!(i, ScenarioValidationError::UnsafeName { .. }))
        );
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

    /// UC-3 (Shard 4): save() 成功で保存実体 (dir) が記録され、登録フォームの
    /// 既定値 (task id / title = シナリオ名) が同期される。
    #[test]
    fn save_tracks_saved_scenario_and_syncs_register_form_defaults() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("pipelines");
        let mut panel = ScenarioPanel::new(root.clone());
        panel.state.name = "fishing2".to_string();
        panel.state.add_task(task_def("Start", None));
        panel.state.add_goal(loop_goal("g", 3));
        let mut status = String::new();

        assert!(panel.saved_pipeline().is_none(), "未保存時は None");
        panel.save(&mut status);
        assert!(status.contains("シナリオ保存"), "status: {status}");
        assert_eq!(
            panel.saved_pipeline(),
            Some(root.join("fishing2").as_path()),
            "保存実体を記録"
        );
        assert_eq!(panel.task_id_input, "fishing2");
        assert_eq!(panel.task_title_input, "fishing2");
    }
}
