//! シナリオ作成 egui パネル (Issue #160 Shard 3 T3 / Issue #166 分割)。
//!
//! [`ScenarioPanel`] は Authoring ペイン埋め込みのシナリオ作成・保存・タスク登録
//! UI。`strategy_ui::StrategyPanel` と同じ「純状態モデル + egui パネル」パターン
//! で、保持する編集データは [`crate::scenario_editor::ScenarioEditorState`]、
//! 保存は [`crate::scenario_editor::save_scenario`] へ委譲。描画は
//! [`ScenarioPanel::ui`] を Authoring ペインの collapsing セクションから呼ぶ
//! (app_ui.rs は配線のみ)。
//!
//! UC-3 (Shard 4): 保存済み pipeline をタスクへ登録・有効化するサブフローは
//! [`ScenarioPanel::ui_task_link`] (ドメインは `scenario_task_link`)。
//! 呼び出し元互換のため scenario_ui が本モジュールの公開アイテムを re-export する。

use std::path::{Path, PathBuf};

use anaden_core::{Goal, StopCondition};
use anaden_vision::TaskDef;
use image::DynamicImage;

use crate::scenario_editor::{ScenarioEditorState, goal_summary, save_scenario};

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
    /// 保存先ルート (既定は workspace の `templates/pipelines`。
    /// UC-4 ロード編集モードではロード元 dir の親に固定される)。
    pipelines_root: PathBuf,
    /// UC-4 (Shard 5): 新規作成モードの保存先ルート。「既存 pipeline を開く」と
    /// `pipelines_root` がロード元へ移動するため、新規モードへ戻る際の復元元。
    default_root: PathBuf,
    /// UC-4: 「既存 pipeline を開く」コンボの選択中 pipeline 名 (未選択 = None)。
    open_selection: Option<String>,
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
            default_root: pipelines_root.clone(),
            pipelines_root,
            open_selection: None,
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

    /// UC-4 (Shard 5): 既存 pipeline ディレクトリをエディタへロードする
    /// (「開く」ボタンの実体。ドメインは [`crate::scenario_load::load_scenario_from_dir`])。
    ///
    /// ロード後は (a) 既定の保存先を **ロード元 dir に固定** する
    /// (`pipelines_root` = dir の親。シナリオ名 = dir 名のため
    /// [`Self::pipeline_dir`] がロード元と一致する)、(b) 編集用の未確定データ
    /// (pending/参照 PNG・保存実体・登録フォーム) をリセットする。ロード済み
    /// TaskDef の template は load_pipeline が絶対化済みで、保存時
    /// `save_task_def` が同じ dir 基準で再相対化するため元の相対参照へ戻る。
    ///
    /// シナリオ名を書き換えて保存すると別ディレクトリへの新規保存
    /// (save-as) になる — その場合 template 参照は新しい dir 基準で再相対化
    /// されるが PNG 実体はコピーされない点は既知の制約。
    ///
    /// 戻り値はロード成否。失敗理由は `status` へ。
    pub fn open_existing(&mut self, dir: &Path, status: &mut String) -> bool {
        match crate::scenario_load::load_scenario_from_dir(dir) {
            Ok(state) => {
                if let Some(parent) = dir.parent() {
                    self.pipelines_root = parent.to_path_buf();
                }
                let task_count = state.tasks.len();
                self.state = state;
                self.pending_pngs.clear();
                self.assigned_pngs.clear();
                self.saved = None;
                self.open_selection = None;
                self.task_id_input.clear();
                self.task_title_input.clear();
                self.bind_selection = None;
                *status = format!(
                    "pipeline ロード: {} (タスク {task_count} 件・保存先はロード元に固定)",
                    dir.display()
                );
                true
            }
            Err(e) => {
                *status = format!("pipeline ロード失敗: {e}");
                false
            }
        }
    }

    /// UC-4: 新規作成モードへ戻る (ロード編集状態を破棄し、保存先を既定ルートへ
    /// 復元する。「新規シナリオ」ボタンの実体)。
    pub fn new_scenario(&mut self, status: &mut String) {
        self.pipelines_root = self.default_root.clone();
        self.state = ScenarioEditorState::new("my_scenario");
        self.pending_pngs.clear();
        self.assigned_pngs.clear();
        self.saved = None;
        self.open_selection = None;
        self.task_id_input.clear();
        self.task_title_input.clear();
        self.bind_selection = None;
        *status = "新規シナリオ作成モードへ戻りました".to_string();
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
                // 保存成功 = この編集状態は保存先 pipeline の所有者 (連続保存が
                // PipelineDirAlreadyExists ガードを通るための所有権証明)。
                self.state.loaded_from = Some(dir.clone());
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
    /// 成功時は [`crate::scenario_task_link::ScenarioPanelEvent::TaskEnabled`] を返す。
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
    /// 成功時は [`crate::scenario_task_link::ScenarioPanelEvent`] を返す
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
        // --- UC-4 (Shard 5): 既存 pipeline を開く / 新規作成モード切替 ---
        // コンボは既定ルート (workspace `templates/pipelines`) 直下の pipeline
        // ディレクトリ列挙。rfd のフォルダ参照は列挙外の場所を開く経路。
        ui.horizontal(|ui| {
            let names = crate::scenario_load::list_pipeline_dirs(&self.default_root);
            let selected = self
                .open_selection
                .clone()
                .unwrap_or_else(|| "(選択してください)".to_string());
            egui::ComboBox::from_id_salt("scenario_open_pipeline")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for n in &names {
                        ui.selectable_value(&mut self.open_selection, Some(n.clone()), n.as_str());
                    }
                });
            let open_dir = self
                .open_selection
                .clone()
                .map(|n| self.default_root.join(n));
            ui.add_enabled_ui(open_dir.is_some(), |ui| {
                if ui.button("既存 pipeline を開く").clicked()
                    && let Some(dir) = &open_dir
                {
                    self.open_existing(dir, status);
                }
            });
            if ui.button("フォルダ参照...").clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                self.open_existing(&dir, status);
            }
            if ui.button("新規シナリオ").clicked() {
                self.new_scenario(status);
            }
        });
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

    /// テスト用 TaskDef (scenario_editor テストと同構成)。
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

    /// UC-4: open_existing でロード → 保存先がロード元 dir に固定され、
    /// 編集なし保存で同 dir へ書き戻る。new_scenario で既定ルートへ復元。
    #[test]
    fn open_existing_fixes_save_target_and_new_scenario_restores_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("pipelines");
        // ロード対象 pipeline を先に保存しておく (name ≠ file stem の検証も兼ね、
        // 手書きで stem ファイルを置く)。
        let dir = root.join("fishing2");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(
            dir.join("fishing_start.toml"),
            "name = \"FishingStartPc\"\nstate = \"Field\"\nalgorithm = \"ccoeff\"\n\
             template = \"fishing.png\"\nroi = [10, 20, 100, 50]\nthreshold = 0.8\n",
        )
        .expect("write");

        let mut panel = ScenarioPanel::new(root.clone());
        panel.state.name = "other".to_string();
        let mut status = String::new();
        assert!(panel.open_existing(&dir, &mut status), "status: {status}");
        assert!(status.contains("pipeline ロード"), "status: {status}");
        assert_eq!(panel.state.name, "fishing2", "シナリオ名 = dir 名");
        assert_eq!(panel.state.task_names(), vec!["FishingStartPc"]);
        assert_eq!(panel.state.start_task, "FishingStartPc");
        assert_eq!(panel.pipeline_dir(), dir, "既定の保存先 = ロード元 dir");
        assert!(panel.saved_pipeline().is_none(), "ロード直後は未保存");

        // 編集なし保存 → 同一 dir へ。stem ファイル (fishing_start.toml) へ書き戻り、
        // name ファイル (FishingStartPc.toml) は作られない (重複 TaskDef 防止)。
        panel.save(&mut status);
        assert_eq!(panel.saved_pipeline(), Some(dir.as_path()));
        assert!(
            dir.join("fishing_start.toml").exists(),
            "元 stem ファイルへ書き戻す"
        );
        assert!(
            !dir.join("FishingStartPc.toml").exists(),
            "name ファイルを二重に作らない"
        );
        let defs = anaden_vision::load_pipeline(&dir).expect("reload");
        assert_eq!(defs.len(), 1, "TaskDef が倍化しない");
        assert_eq!(defs[0].name, "FishingStartPc");

        // 新規シナリオへ戻ると既定ルート・空状態へ復元。
        panel.new_scenario(&mut status);
        assert_eq!(panel.pipeline_dir(), root.join("my_scenario"));
        assert!(panel.state.tasks.is_empty());
        assert!(panel.state.loaded_task_names.is_empty());
        assert!(panel.saved_pipeline().is_none());
    }
}
