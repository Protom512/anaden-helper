//! 実演オーサリングモードの純状態機械 (Issue #190 Shard 1/3)。
//!
//! ゲーム画面 (PC版キャプチャ) を実際に動かしながら「どこをクリックするか
//! (タップ位置)」「どこらへんをイメージ認識するか (認識領域)」を積み上げて
//! 1 本のシナリオ ([`ScenarioEditorState`] → `templates/pipelines/<name>/`) へ
//! 落とし込む。egui 非依存の純モデルであり、ライブビュー描画・入力注入
//! (anaden-device) の配線は Shard 2 が、実機 E2E は Shard 3 が本モデルの上に
//! 構築する (`scenario_state` / `strategy_ui` と同じ「純モデル + パネル分離」
//! パターン)。
//!
//! ## ステップモデル
//!
//! 1 ステップ = 認識 (テンプレート + ROI) + 操作 (`click_self` = マッチ位置を
//! タップ)。作図中ステップは「タップ位置 (未確定)」と「認識領域 (未確定)」の
//! 2 ジェスチャを **どちらの順でも** 受け付け
//! ([`AuthoringSession::record_tap`] / [`AuthoringSession::record_region`]、
//! 後着ジェスチャで上書き可)、両方揃った時点で確定して [`AuthoringStep`]
//! (TaskDef + テンプレート crop) を生成する。
//!
//! 記録タップ位置は TaskDef に直接使われない検証アンカー扱い (action は
//! ClickSelf で実行時タップ座標はマッチ中心から導かれるため)。領域外タップは
//! 警告 (fail-visible) だが確定は妨げない。
//!
//! ## テンプレート品質 (fail-visible)
//!
//! 確定時のクロップに対し、既存 3 保存経路と同じ単一実装
//! (`crate::library::template_structure_warning` — 無構造 stddev 検証 /
//! `crate::library::needle_roi_warning` — needle/roi 収容検証) で品質警告を
//! 収集し、呼出側へ返す (保存はブロックしない)。クロップは記録領域と現行
//! フレームの共通部分 (クリップ済み) を ROI に使うため needle ≤ roi が常に
//! 成立し、needle/roi 警告は本モジュールの標準形では発火しない — チェック
//! 自体は save 経路 (`save_scenario_with_warnings`) でも再実行されるため、
//! シナリオフォームでの ROI 事後編集との不整合はそちらで予見される。
//!
//! ## 保存
//!
//! [`AuthoringSession::save`] は `save_scenario_with_warnings` へ完全委譲する
//! (二重実装禁止)。ステップの TaskDef チェーン (next 接続 + start_task) と
//! goal は [`AuthoringSession::to_scenario`] が残ステップから毎回導出する
//! (undo 後のチェーン再接続は追加実装不要)。

use std::path::Path;

use image::DynamicImage;

use anaden_core::{Goal, ScreenRegion, StopCondition};
use anaden_vision::TaskDef;

use crate::app_state_pipeline_task::{PipelineActionKind, pipeline_task_spec};
use crate::library::{needle_roi_warning, template_structure_warning};
use crate::scenario_save::{ScenarioSaveError, ScenarioSaveOutcome, save_scenario_with_warnings};
use crate::scenario_state::ScenarioEditorState;

/// 確定ステップの TaskDef 構築に使う認識方式文字列 (Ccoeff)。
const AUTHORING_METHOD: &str = "ccoeff";

/// TaskDef.state ラベルの既定値 (作成タブ STATE_OPTIONS の "field" と同一)。
const DEFAULT_STATE: &str = "field";

/// マッチ閾値の既定値 (作成タブの discrimination 無し既定 0.9 と同一)。
const DEFAULT_THRESHOLD: f32 = 0.9;

/// 未確定ジェスチャの組 (作図中ステップ)。両方揃うと確定する。
#[derive(Debug, Clone, Default)]
struct PendingStep {
    tap: Option<(u32, u32)>,
    region: Option<[u32; 4]>,
}

/// 確定済みの実演ステップ (認識 + 操作の 1 単位)。
#[derive(Debug, Clone)]
pub struct AuthoringStep {
    /// ステップの TaskDef (name/state/algorithm/template/roi/threshold/action)。
    /// template は pipeline dir 基準の裸相対 `<name>.png`。
    pub task: TaskDef,
    /// テンプレート PNG 本体 (確定時点の現行フレームから roi クロップした画像)。
    /// [`AuthoringSession::save`] が `<name>.png` として書き出す。
    pub template: DynamicImage,
    /// 記録されたタップ位置 (検証アンカー)。実行時のタップ座標は ClickSelf
    /// アクションのマッチ中心から導かれるため TaskDef には使われない。
    pub tap: (u32, u32),
}

/// ジェスチャ記録の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GestureOutcome {
    /// ジェスチャを記録したがステップは未確定 (もう一方のジェスチャ待ち)。
    Pending,
    /// 両ジェスチャが揃いステップを確定した。`warnings` はそのステップの
    /// テンプレート品質・タップ位置警告 (fail-visible・確定は済んでいる)。
    Confirmed {
        /// 確定ステップの警告 (対象タスク名接頭付き)。
        warnings: Vec<String>,
    },
}

/// ジェスチャ記録・ステップ確定のエラー。いずれも呼出時点のペンディング
/// 状態へ戻す (atomic — 部分適用なし)。
#[derive(Debug, thiserror::Error)]
pub enum AuthoringError {
    /// 両ジェスチャが揃ったが現行フレームが未取得 (クロップ不能)。
    #[error("no current frame: push a frame before completing a step")]
    NoFrame,
    /// 認識領域の幅または高さが 0。
    #[error("roi {roi:?} has zero width or height")]
    RoiEmpty {
        /// 拒否された ROI `[x, y, w, h]`。
        roi: [u32; 4],
    },
    /// 認識領域が現行フレームと交差しない (クロップ対象が空)。
    #[error("roi {roi:?} does not intersect the current frame ({fw}x{fh})")]
    RoiOutOfFrame {
        /// 拒否された ROI `[x, y, w, h]`。
        roi: [u32; 4],
        /// 現行フレームの幅。
        fw: u32,
        /// 現行フレームの高さ。
        fh: u32,
    },
    /// TaskDef 構築ヘルパ (`pipeline_task_spec`) が方式文字列を拒否した
    /// (AUTHORING_METHOD が既知方式から外れた場合の fail-closed)。
    #[error("internal: task build failed for method `{method}`")]
    TaskBuild {
        /// 拒否された方式文字列。
        method: &'static str,
    },
}

/// 実演オーサリングセッション (Issue #190 操作モデルの純状態機械)。
///
/// ライブビューのフレーム流入 (`push_frame`) と 2 種ジェスチャ
/// (`record_tap` / `record_region`) を受け付け、確定ステップを積み上げる。
/// ステップの TaskDef 名は `AuthoredStep01` ... の自動連番 (命名規約
/// PascalCase 適合・undo で該当番号を再利用)。
#[derive(Debug, Clone)]
pub struct AuthoringSession {
    /// シナリオ名 (= 保存先 `templates/pipelines/<name>/` ディレクトリ名)。
    name: String,
    /// TaskDef.state ラベル (以降の確定ステップに適用)。
    state: String,
    /// マッチ閾値 (以降の確定ステップに適用)。
    threshold: f32,
    /// シナリオのゴール (終端条件)。to_scenario で付与される。
    goal: Goal,
    /// 現行フレーム (確定時のクロップソース)。
    frame: Option<DynamicImage>,
    /// 作図中ステップの未確定ジェスチャ。
    pending: PendingStep,
    /// 確定済みステップ (記録順)。
    steps: Vec<AuthoringStep>,
    /// 確定時に収集した警告の累積 (fail-visible・GUI status 表示用)。
    warnings: Vec<String>,
    /// 直近の保存先 (連続保存の所有権証明 → to_scenario の loaded_from)。
    saved_to: Option<std::path::PathBuf>,
}

impl AuthoringSession {
    /// 空のオーサリングセッションを作成する。
    ///
    /// 既定値: state = "field"・threshold = 0.9・goal = 10 分タイムアウト
    /// (実演シナリオの安全弁。[`Self::set_goal`] で差し替え可)。
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            state: DEFAULT_STATE.to_string(),
            threshold: DEFAULT_THRESHOLD,
            goal: Goal {
                name: "authoring_timeout".to_string(),
                stop: StopCondition::Timeout { secs: 600 },
            },
            frame: None,
            pending: PendingStep::default(),
            steps: Vec::new(),
            warnings: Vec::new(),
            saved_to: None,
        }
    }

    /// シナリオ名。
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 以降の確定ステップに適用する TaskDef.state ラベルを設定する
    /// (既定 "field"。確定済みステップには影響しない)。
    pub fn set_state(&mut self, state: &str) {
        self.state = state.to_string();
    }

    /// 以降の確定ステップに適用するマッチ閾値を設定する
    /// (既定 0.9。作成タブの discrimination 導出と同じ 0.5..=0.99 にクランプ)。
    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold.clamp(0.5, 0.99);
    }

    /// シナリオのゴール (終端条件) を差し替える (既定: 10 分タイムアウト)。
    pub fn set_goal(&mut self, goal: Goal) {
        self.goal = goal;
    }

    /// 現行フレームを更新する (ライブビュー由来)。ステップ確定時のクロップは
    /// 常にこの最新フレームから行う。
    pub fn push_frame(&mut self, frame: &DynamicImage) {
        self.frame = Some(frame.clone());
    }

    /// 未確定のタップ位置ジェスチャ (もう一方の認識領域待ちかの判定用)。
    #[must_use]
    pub fn pending_tap(&self) -> Option<(u32, u32)> {
        self.pending.tap
    }

    /// 未確定の認識領域ジェスチャ。
    #[must_use]
    pub fn pending_region(&self) -> Option<[u32; 4]> {
        self.pending.region
    }

    /// タップ位置を記録する (未確定タップの上書き = 最後のタップを採用)。
    ///
    /// 認識領域が既に記録済みならその場でステップを確定する
    /// ([`GestureOutcome::Confirmed`])。確定に失敗した場合はタップを破棄して
    /// 呼出前の状態へ戻す (atomic)。
    ///
    /// # Errors
    /// [`AuthoringError`] — 確定に必要な現行フレームが無い等。
    pub fn record_tap(&mut self, pos: (u32, u32)) -> Result<GestureOutcome, AuthoringError> {
        let prev = self.pending.tap;
        self.pending.tap = Some(pos);
        match self.try_confirm() {
            Ok(outcome) => Ok(outcome),
            Err(e) => {
                self.pending.tap = prev;
                Err(e)
            }
        }
    }

    /// 認識領域を記録する (未確定領域の上書き = 最後のドラッグを採用)。
    ///
    /// タップ位置が既に記録済みならその場でステップを確定する
    /// ([`GestureOutcome::Confirmed`])。確定に失敗した場合は領域を破棄して
    /// 呼出前の状態へ戻す (atomic)。
    ///
    /// # Errors
    /// [`AuthoringError`] — 幅/高さ 0・現行フレームとの交差なし・確定に必要な
    /// フレームが無い等。
    pub fn record_region(&mut self, roi: [u32; 4]) -> Result<GestureOutcome, AuthoringError> {
        let [_, _, w, h] = roi;
        if w == 0 || h == 0 {
            return Err(AuthoringError::RoiEmpty { roi });
        }
        if let Some(frame) = self.frame.as_ref()
            && clamp_region_to_frame(roi, frame.width(), frame.height()).is_none()
        {
            return Err(AuthoringError::RoiOutOfFrame {
                roi,
                fw: frame.width(),
                fh: frame.height(),
            });
        }
        let prev = self.pending.region;
        self.pending.region = Some(roi);
        match self.try_confirm() {
            Ok(outcome) => Ok(outcome),
            Err(e) => {
                self.pending.region = prev;
                Err(e)
            }
        }
    }

    /// 直近の操作を取り消す。
    ///
    /// 未確定ジェスチャ (片方だけ記録済みのタップ/領域) を優先してクリアし、
    /// 無ければ最終確定ステップを取り消す。チェーン (next 接続・start_task) は
    /// [`Self::to_scenario`] が残ステップから毎回導出するため、取り消し後の
    /// 再接続は自動的。戻り値は何かを取り消せたか。
    pub fn undo(&mut self) -> bool {
        if self.pending.tap.is_some() || self.pending.region.is_some() {
            self.pending = PendingStep::default();
            return true;
        }
        self.steps.pop().is_some()
    }

    /// 確定済みステップ一覧 (記録順)。
    #[must_use]
    pub fn steps(&self) -> &[AuthoringStep] {
        &self.steps
    }

    /// 確定時に収集した警告の累積 (fail-visible。各要素は対象タスク名接頭付き)。
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// 現在のステップ群からシナリオ編集状態へ変換する。
    ///
    /// - 各ステップの TaskDef を記録順に next 接続 (i → i+1、末尾は空)。
    /// - start_task は先頭ステップ ([`ScenarioEditorState::add_task`] の採用)。
    /// - goal を 1 件付与 (既定 or [`Self::set_goal`])。
    /// - 直近保存先 (`saved_to`) を `loaded_from` に反映し、同一 dir への
    ///   連続保存を所有権ガードが許可する。
    #[must_use]
    pub fn to_scenario(&self) -> ScenarioEditorState {
        let mut st = ScenarioEditorState::new(&self.name);
        for (index, step) in self.steps.iter().enumerate() {
            let mut task = step.task.clone();
            task.next = Some(
                self.steps
                    .get(index + 1)
                    .map_or_else(Vec::new, |next| vec![next.task.name.clone()]),
            );
            st.add_task(task);
        }
        st.add_goal(self.goal.clone());
        st.loaded_from = self.saved_to.clone();
        st
    }

    /// セッションを `<pipelines_root>/<name>/` へ保存する。
    ///
    /// [`crate::scenario_save::save_scenario_with_warnings`] への完全委譲
    /// (二重実装禁止): 各ステップのテンプレート crop を `<task>.png` として
    /// 書き出し、manifest + TaskDef TOML を保存し、テンプレート品質警告を
    /// [`ScenarioSaveOutcome::warnings`] として返す。空セッションは保存先
    /// バリデーション (NoTasks) で拒否される。
    ///
    /// 保存に成功すると直近保存先を記録し、以降の同一 dir への連続保存が
    /// `PipelineDirAlreadyExists` で弾かれないようにする
    /// (`ScenarioPanel::save` と同一契約。このため `&mut self`)。
    ///
    /// # Errors
    /// [`ScenarioSaveError`] — バリデーション不合格 (空セッション含む)・
    /// 既存 dir 上書きガード・書き出し失敗 (委譲先の契約そのまま)。
    pub fn save(
        &mut self,
        pipelines_root: &Path,
    ) -> Result<ScenarioSaveOutcome, ScenarioSaveError> {
        let state = self.to_scenario();
        let pngs: Vec<(String, DynamicImage)> = self
            .steps
            .iter()
            .map(|step| (step.task.name.clone(), step.template.clone()))
            .collect();
        let outcome = save_scenario_with_warnings(&state, &pngs, pipelines_root)?;
        self.saved_to = Some(outcome.dir.clone());
        Ok(outcome)
    }

    /// 両ジェスチャが揃っている場合にステップを確定する。
    ///
    /// 現行フレームから ROI クロップ (フレーム境界面でクリップ) を生成し、
    /// テンプレート品質警告 (stddev / needle-roi) とタップ位置警告を収集して
    /// [`AuthoringStep`] を積む。確定に成功した時点でペンディングをクリアする。
    fn try_confirm(&mut self) -> Result<GestureOutcome, AuthoringError> {
        let (Some(tap), Some(region)) = (self.pending.tap, self.pending.region) else {
            return Ok(GestureOutcome::Pending);
        };
        let Some(frame) = self.frame.as_ref() else {
            return Err(AuthoringError::NoFrame);
        };
        // crop_imm は境界外 ROI でパニックするため、必ずフレームとの共通部分へ
        // クランプしてから切り出す (ライブラリコードで panic させない)。
        let Some(clamped) = clamp_region_to_frame(region, frame.width(), frame.height()) else {
            return Err(AuthoringError::RoiOutOfFrame {
                roi: region,
                fw: frame.width(),
                fh: frame.height(),
            });
        };
        let [cx, cy, cw, ch] = clamped;
        let crop = frame.crop_imm(cx, cy, cw, ch);
        let mut warnings = Vec::new();
        if clamped != region {
            warnings.push(format!(
                "警告: 認識領域 {roi:?} が現行フレーム ({fw}x{fh}) 外にはみ出したため \
                 {cw}x{ch} へクリップしました",
                roi = region,
                fw = frame.width(),
                fh = frame.height(),
            ));
        }
        // 品質警告: 既存保存経路と同じ単一実装経由 (stddev / needle-roi)。
        if let Some(warning) = template_structure_warning(&crop) {
            warnings.push(warning);
        }
        if let Some(warning) = needle_roi_warning((crop.width(), crop.height()), Some(clamped)) {
            warnings.push(warning);
        }
        if !region_contains(clamped, tap) {
            warnings.push(format!(
                "警告: 記録タップ位置 ({tx}, {ty}) が認識領域 {clamped:?} の外です \
                 (実行時タップはマッチ中心から導かれるため確定しますが、意図した \
                 UI 要素とは別の領域を認識している恐れがあります)",
                tx = tap.0,
                ty = tap.1,
            ));
        }
        let name = self.next_step_name();
        let Some(task) = pipeline_task_spec(
            &name,
            &self.state,
            AUTHORING_METHOD,
            ScreenRegion::new(cx, cy, cw, ch),
            self.threshold,
            PipelineActionKind::ClickSelf,
        ) else {
            return Err(AuthoringError::TaskBuild {
                method: AUTHORING_METHOD,
            });
        };
        let step_warnings: Vec<String> = warnings.iter().map(|w| format!("{name}: {w}")).collect();
        self.steps.push(AuthoringStep {
            task,
            template: crop,
            tap,
        });
        self.warnings.extend(step_warnings.iter().cloned());
        self.pending = PendingStep::default();
        Ok(GestureOutcome::Confirmed {
            warnings: step_warnings,
        })
    }

    /// 次ステップの TaskDef 名 (AuthoredStep 連番・2 桁ゼロ埋め)。
    ///
    /// 命名規約 (`task_name_issue`) 適合: 語幹 `AuthoredStep` は大文字 2 つ
    /// (複語名) のため末尾連番検査の対象外。undo は末尾ステップのみを取り
    /// 消すため `steps.len() + 1` の番号は常に未使用 (衝突なし)。
    fn next_step_name(&self) -> String {
        format!("AuthoredStep{:02}", self.steps.len() + 1)
    }
}

/// ROI をフレームとの共通部分へクランプする。
///
/// 幅/高さ 0、またはフレームと交差しない (x ≥ fw / y ≥ fh) 場合は `None`。
fn clamp_region_to_frame(roi: [u32; 4], fw: u32, fh: u32) -> Option<[u32; 4]> {
    let [x, y, w, h] = roi;
    if w == 0 || h == 0 || x >= fw || y >= fh {
        return None;
    }
    Some([x, y, w.min(fw - x), h.min(fh - y)])
}

/// 点が ROI 内にあるか (境界含まず・right/bottom は外側)。
fn region_contains(roi: [u32; 4], pos: (u32, u32)) -> bool {
    let [x, y, w, h] = roi;
    pos.0 >= x && pos.1 >= y && pos.0 - x < w && pos.1 - y < h
}
