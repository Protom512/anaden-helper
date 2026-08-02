//! 宣言的パイプラインの純粋実行層。
//!
//! [`anaden_vision::run_step`] を駆動し、action から入力コマンド([`InputCommand`])への変換と
//! next への状態遷移([`PipelineState::tick`])を行う。デバイス IO・async・ADB 文字列は一切持たず、
//! 入力([`anaden_vision::TaskDef`]・[`image::DynamicImage`]・現在タスク名)だけに依存する純粋層。
//!
//! 実デバイス発火([`InputCommand`] -> [`anaden_core::InputAction`] 変換 + InputExecutor::execute)は
//! 本モジュールの範囲外。caller は [`PipelineState::tick`] の戻り値 [`TickResult`] を消費して
//! ループを駆動する。

use image::DynamicImage;
use serde::{Deserialize, Serialize};

use anaden_core::ScreenRegion;
use anaden_vision::{Action, StepOutcome, TaskDef, roi_to_normalized, run_step};

/// デバイスへ発火すべき入力コマンド（ピクセル座標）。
///
/// **座標空間**: 全バリアントの座標は **normalize 後1280空間**（黒帯除去後描画領域を
/// 幅1280基準へ正規化した空間）に統一される。実デバイス生画像（黒帯込み）への逆変換は
/// 後段の [`crate::pipeline_driver::rescale_command`] が [`anaden_vision::CropInfo`] を使って行う。
///
/// - [`Action::ClickSelf`] / [`Action::ClickRect`] は [`InputCommand::Tap`]、
///   [`Action::Swipe`] は [`InputCommand::Swipe`] へ 1:1 対応する。
///
/// duration_ms は現状 [`Action::Swipe`] にパラメータが無いため持たない。後段の発火層が
/// デフォルト値を埋める。将来 [`Action::Swipe`] に duration が増えたらフィールド追加で拡張する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputCommand {
    /// 指定座標をタップ。
    Tap { x: u32, y: u32 },
    /// `from` から `to` へスワイプ。
    Swipe { from: (u32, u32), to: (u32, u32) },
}

/// アクションから入力コマンドへ変換する。全座標は **normalize 後空間**（screenshot と同じ
/// 座標空間。PC版16:9なら1280x720、20:9端末なら1280x576 等）へ統一される。
///
/// 座標空間の整合（C1 修正）:
/// - [`Action::ClickSelf`] は `matched_region` の中心をタップ。`matched_region` は
///   [`run_step`] が screenshot（=normalize後空間）と同じ空間で返すため、**既に同空間**。
///   そのまま `Tap` へ。`matched_region` が [`None`] の場合は [`None`]（安全側: 発火しない）。
/// - [`Action::ClickRect`] の `roi`・[`Action::Swipe`] の `from`/`to` は TOML 定義上
///   **raw-1258x708 空間**。これを [`roi_to_normalized`] で `(norm_w, norm_h)` 空間へ変換してから
///   中心を取り、`Tap`/`Swipe` へ載せる。detect（[`run_step`]）が ROI/needle をスケールするのと
///   **同一の `(norm_w, norm_h)`** を渡すことで、ClickRect 座標が matched_region と同じ空間に揃う
///   （従来は raw-1258 のまま `InputCommand` に入り、空間不整合でタップが外れていた）。
/// - [`Action::DoNothing`] / [`Action::Stop`] は入力コマンドではないため [`None`]。
///
/// `action` は参照で受け Clone 回避。戻り値 [`Option<InputCommand>`]: [`None`] は
/// 「この tick では入力を発火しない」を意味し、caller は状態遷移だけ進める。
pub fn action_to_command(
    action: &Action,
    matched_region: Option<ScreenRegion>,
    norm_w: u32,
    norm_h: u32,
) -> Option<InputCommand> {
    match action {
        Action::ClickSelf => match matched_region {
            Some(r) => {
                let (x, y) = r.center();
                Some(InputCommand::Tap { x, y })
            }
            None => None,
        },
        Action::ClickRect { roi } => {
            // roi は raw-1258 空間 → (norm_w, norm_h) 空間へ変換して中心を取る。
            let nroi = roi_to_normalized([roi.x, roi.y, roi.width, roi.height], norm_w, norm_h);
            let nr = ScreenRegion::new(nroi[0], nroi[1], nroi[2], nroi[3]);
            let (x, y) = nr.center();
            Some(InputCommand::Tap { x, y })
        }
        Action::Swipe { from, to } => {
            // from/to も raw-1258 空間 → (norm_w, norm_h) 空間へ変換して中心を取る。
            let nfrom =
                roi_to_normalized([from.x, from.y, from.width, from.height], norm_w, norm_h);
            let nto = roi_to_normalized([to.x, to.y, to.width, to.height], norm_w, norm_h);
            Some(InputCommand::Swipe {
                from: ScreenRegion::new(nfrom[0], nfrom[1], nfrom[2], nfrom[3]).center(),
                to: ScreenRegion::new(nto[0], nto[1], nto[2], nto[3]).center(),
            })
        }
        Action::DoNothing => None,
        Action::Stop => None,
    }
}

/// マッチ結果から次のタスク名を決める。
///
/// - [`Action::Stop`] は next の有無に関わらず [`None`]（停止指示）。
/// - それ以外は `outcome.next[0]` を返す。next が空（終端タスク）なら [`None`]。
///
/// 純粋: `outcome` の参照のみ、副作用なし。caller は戻り値で `current` を置き換える。
pub fn advance_next(outcome: &StepOutcome) -> Option<String> {
    match outcome.action {
        Action::Stop => None,
        _ => outcome.next.first().cloned(),
    }
}

/// 1 tick の結果。
///
/// `command` は発火すべき入力コマンド（無ければ [`None`]）。
/// `next_current` は遷移先タスク名。停止・待機（next 空・Stop）の場合は [`None`]。
///
/// `next_current` は caller のログ/デバッグ用参照情報。実際の `current` 更新は
/// [`PipelineState::tick`] 内で行うため、caller は戻り値をそのまま消費してよい。
///
/// `matched_confidence` / `matched_region` はマッチしたテンプレートの信頼度と領域。
/// Issue #37 T4: 宣言的ゴールの TemplateMatch(UC-2) 評価が `last_match` を構築するために
/// 必要。`run_step` の [`StepOutcome`][anaden_vision::StepOutcome] が保持する値をここへ伝播する。
#[derive(Debug, Clone, PartialEq)]
pub struct TickResult {
    /// 発火すべき入力コマンド。[`None`] はこの tick で入力無し。
    pub command: Option<InputCommand>,
    /// 遷移先タスク名。[`None`] は停止 or 待機（caller が別判断）。
    pub next_current: Option<String>,
    /// マッチしたテンプレートの信頼度（0.0..=1.0）。
    /// テンプレ未マッチ時は [`None`] だが、本構造体はマッチ成功時のみ構築されるため
    /// 実運用上は常に [`Some`]。UC-2 評価用。
    pub matched_confidence: Option<f32>,
    /// マッチしたテンプレート領域（スクリーンショット元解像度座標）。
    /// UC-2 評価用（`GoalStatusContext::last_match` の region 要素）。
    pub matched_region: Option<ScreenRegion>,
}

/// パイプラインの実行状態ホルダ。現在タスク名だけを持つ最小の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineState {
    /// 現在のタスク名。
    pub current: String,
}

impl PipelineState {
    /// 現在タスク名を指定して生成。
    pub fn new(current: impl Into<String>) -> Self {
        Self {
            current: current.into(),
        }
    }

    /// 現在のタスク名への参照。
    pub fn current(&self) -> &str {
        &self.current
    }

    /// 現在のタスク名を強制設定する。
    ///
    /// [`Self::tick`] は next[0] へ自動遷移するが、発火後検証で対象が残存した場合など
    /// caller が current を発火前へ巻き戻したいときに使う(アンドゥ用途)。
    /// 通常のループ駆動では使わない(tick が current を管理する)。
    pub fn set_current(&mut self, current: impl Into<String>) {
        self.current = current.into();
    }

    /// 1ステップ認識を実行し、コマンド変換 + next 遷移を行う。
    ///
    /// 内部で [`run_step`]（現在タスク名で [`TaskDef`] を検索 → detect）を呼ぶ。
    /// 戻り値 [`Option<TickResult>`]:
    /// - マッチ成功 → [`Some`]([`TickResult`])。`command` は action から変換、`next_current` は next[0]。
    ///   `next_current` が [`Some`] なら `current` をそこへ更新する。
    /// - 非マッチ・閾値下・ROI 外・テンプレ欠落・未知タスク名 → [`None`]（`current` は変更せず）。
    ///
    /// `screenshot`/`tasks` は借用参照。変更するのは `current` のみ（純粋計算 + 状態遷移）。
    pub fn tick(&mut self, screenshot: &DynamicImage, tasks: &[TaskDef]) -> Option<TickResult> {
        let outcome = run_step(tasks, screenshot, &self.current)?;
        let matched_region = outcome.matched_region;
        let matched_confidence = Some(outcome.matched_confidence);
        // ClickRect/Swipe の roi/from/to を screenshot（=normalize後空間）と同じ空間へ揃えるため、
        // その寸法を action_to_command へ渡す（detect の roi_to_normalized と同一 norm_w/norm_h）。
        let command = action_to_command(
            &outcome.action,
            Some(matched_region),
            screenshot.width(),
            screenshot.height(),
        );
        let next_current = advance_next(&outcome);
        if let Some(next) = &next_current {
            self.current = next.clone();
        }
        Some(TickResult {
            command,
            next_current,
            matched_confidence,
            matched_region: Some(matched_region),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use image::{DynamicImage, GrayImage, Luma};
    use std::path::PathBuf;

    /// `(x+y) mod 64` の勾配パターン（pipeline.rs テスト準拠）。
    fn gradient_needle(w: u32, h: u32) -> GrayImage {
        let mut img = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = ((x + y) % 64) as u8;
                img.put_pixel(x, y, Luma([v]));
            }
        }
        img
    }

    /// 背景の上に needle を `(ox, oy)` に埋め込んだ画像。
    fn embed(
        haystack_w: u32,
        haystack_h: u32,
        needle: &GrayImage,
        ox: u32,
        oy: u32,
        bg: u8,
    ) -> GrayImage {
        let mut img = GrayImage::from_pixel(haystack_w, haystack_h, Luma([bg]));
        for y in 0..needle.height() {
            for x in 0..needle.width() {
                let p = needle.get_pixel(x, y)[0];
                img.put_pixel(ox + x, oy + y, Luma([p]));
            }
        }
        img
    }

    fn luma_dyn(img: GrayImage) -> DynamicImage {
        DynamicImage::ImageLuma8(img)
    }

    /// ダミー領域（advance_next テストでは値不問）。
    fn dummy_region() -> ScreenRegion {
        ScreenRegion::new(0, 0, 1, 1)
    }

    /// advance_next テスト用の StepOutcome を構築するヘルパ。
    fn outcome(action: Action, next: Vec<&str>) -> StepOutcome {
        StepOutcome {
            matched_task: "T".into(),
            action,
            next: next.into_iter().map(String::from).collect(),
            matched_region: dummy_region(),
            matched_confidence: 0.95,
        }
    }

    // ---- (A) action_to_command の全ケース ----
    //
    // norm_w/norm_h は ClickRect/Swipe の roi 変換先空間の寸法。ここでは PC版 raw-1258x708 空間
    // （=PC_CLIENT_*_MEASURED）を渡し恒等変換させ、roi がそのまま中心計算へ渡るようにする。
    // ClickSelf は matched_region が既に同空間なので norm 値は結果に影響しない。

    #[test]
    fn click_self_with_region_taps_center() {
        let action = Action::ClickSelf;
        let region = ScreenRegion::new(100, 200, 80, 60);
        assert_eq!(
            action_to_command(&action, Some(region), FULL_W, FULL_H),
            Some(InputCommand::Tap { x: 140, y: 230 })
        );
    }

    #[test]
    fn click_self_without_region_returns_none() {
        let action = Action::ClickSelf;
        assert_eq!(action_to_command(&action, None, FULL_W, FULL_H), None);
    }

    #[test]
    fn click_rect_taps_roi_center_ignoring_matched_region() {
        let action = Action::ClickRect {
            roi: ScreenRegion::new(520, 320, 240, 80),
        };
        // matched_region を与えても roi 優先であることを確認。
        let matched = Some(ScreenRegion::new(0, 0, 10, 10));
        assert_eq!(
            action_to_command(&action, matched, FULL_W, FULL_H),
            Some(InputCommand::Tap { x: 640, y: 360 })
        );
    }

    #[test]
    fn swipe_centers_from_to() {
        let action = Action::Swipe {
            from: ScreenRegion::new(100, 500, 40, 40),
            to: ScreenRegion::new(100, 100, 40, 40),
        };
        assert_eq!(
            action_to_command(&action, None, FULL_W, FULL_H),
            Some(InputCommand::Swipe {
                from: (120, 520),
                to: (120, 120),
            })
        );
    }

    #[test]
    fn do_nothing_returns_none() {
        let action = Action::DoNothing;
        assert_eq!(
            action_to_command(
                &action,
                Some(ScreenRegion::new(10, 10, 10, 10)),
                FULL_W,
                FULL_H
            ),
            None
        );
    }

    #[test]
    fn stop_returns_none() {
        let action = Action::Stop;
        assert_eq!(
            action_to_command(
                &action,
                Some(ScreenRegion::new(10, 10, 10, 10)),
                FULL_W,
                FULL_H
            ),
            None
        );
    }

    // ---- (B) advance_next ----

    #[test]
    fn advance_returns_first_next() {
        let out = outcome(Action::ClickSelf, vec!["LoadGame", "Menu"]);
        assert_eq!(advance_next(&out), Some("LoadGame".to_string()));
    }

    #[test]
    fn advance_empty_next_returns_none() {
        let out = outcome(Action::ClickSelf, vec![]);
        assert_eq!(advance_next(&out), None);
    }

    #[test]
    fn advance_stop_returns_none_even_with_next() {
        let out = outcome(Action::Stop, vec!["Next"]);
        assert_eq!(advance_next(&out), None);
    }

    #[test]
    fn advance_do_nothing_advances() {
        let out = outcome(Action::DoNothing, vec!["X"]);
        assert_eq!(advance_next(&out), Some("X".to_string()));
    }

    // ---- (D) PipelineState::new/current ----

    #[test]
    fn state_new_and_current() {
        let s = PipelineState::new("Title");
        assert_eq!(s.current(), "Title");
    }

    // ---- (C) tick: 画像合成を通す統合テスト ----
    //
    // 新スケールモデルでは detect は needle/ROI を screenshot 寸法へ動的スケールする。
    // needle_to_normalized / roi_to_normalized を恒等（sx=sy=1.0）にするため、キャンバスは
    // PC版実測 raw-1258x708（=PC_CLIENT_*_MEASURED）。これで embed で needle を直接埋め込んでも
    // detect が needle をスケールせず、埋め込み位置・サイズが保存されたままマッチする。
    // needle は NEEDLE_PX に小さく保ち、全面 ccoeff 走査（O(N·M)）を単発テストで実用的時間に抑える。

    const FULL_W: u32 = 1258;
    const FULL_H: u32 = 708;
    /// tick 統合テストの needle サイズ（全面走査コスト抑制のため小さく）。
    const NEEDLE_PX: u32 = 16;

    /// テンプレPNGを tempdir に保存し、絶対パスを返す。tempdir は .keep() で永続化する。
    fn write_template_persisted(needle: &GrayImage) -> PathBuf {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("needle.png");
        needle.save(&p).expect("save png");
        let _persisted = tmp.keep();
        p
    }

    #[test]
    fn tick_match_emits_command_and_advances() {
        // ClickRect は matched_region 非依存。needle を含む screenshot でマッチさせ、
        // roi 中心を Tap する + next[0] へ current が進むことを検証する。
        let needle = gradient_needle(NEEDLE_PX, NEEDLE_PX);
        let screenshot = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        let tpl = write_template_persisted(&needle);

        let tasks = vec![TaskDef {
            name: "Title".into(),
            state: "Title".into(),
            algorithm: anaden_vision::Algorithm::Ccoeff,
            template: tpl,
            roi: None,
            threshold: 0.9,
            base: None,
            action: Some(Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            }),
            next: Some(vec!["LoadGame".into()]),
        }];

        let mut state = PipelineState::new("Title");
        let result = state.tick(&screenshot, &tasks).expect("should match");
        assert_eq!(
            result.command,
            Some(InputCommand::Tap { x: 640, y: 360 }),
            "ClickRect roi center"
        );
        assert_eq!(result.next_current, Some("LoadGame".to_string()));
        assert_eq!(state.current(), "LoadGame", "current advanced to next[0]");
    }

    #[test]
    fn tick_no_match_returns_none_and_keeps_current() {
        // 背景のみ（needle 無）→ run_step None → tick None。current 変更なし。
        let screenshot = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let needle = gradient_needle(NEEDLE_PX, NEEDLE_PX);
        let tpl = write_template_persisted(&needle);

        let tasks = vec![TaskDef {
            name: "Title".into(),
            state: "Title".into(),
            algorithm: anaden_vision::Algorithm::Ccoeff,
            template: tpl,
            roi: None,
            threshold: 0.9,
            base: None,
            action: Some(Action::ClickSelf),
            next: Some(vec!["LoadGame".into()]),
        }];

        let mut state = PipelineState::new("Title");
        let result = state.tick(&screenshot, &tasks);
        assert!(result.is_none(), "no needle must yield None");
        assert_eq!(state.current(), "Title", "current unchanged on no match");
    }

    #[test]
    fn tick_unknown_current_returns_none() {
        let needle = gradient_needle(NEEDLE_PX, NEEDLE_PX);
        let screenshot = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        let tpl = write_template_persisted(&needle);

        let tasks = vec![TaskDef {
            name: "Title".into(),
            state: "Title".into(),
            algorithm: anaden_vision::Algorithm::Ccoeff,
            template: tpl,
            roi: None,
            threshold: 0.9,
            base: None,
            action: Some(Action::ClickSelf),
            next: None,
        }];

        let mut state = PipelineState::new("NoSuch");
        let result = state.tick(&screenshot, &tasks);
        assert!(result.is_none(), "unknown current must yield None");
        assert_eq!(
            state.current(),
            "NoSuch",
            "current unchanged on unknown task"
        );
    }

    #[test]
    fn tick_stop_returns_no_command_and_none_next() {
        let needle = gradient_needle(NEEDLE_PX, NEEDLE_PX);
        let screenshot = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        let tpl = write_template_persisted(&needle);

        let tasks = vec![TaskDef {
            name: "Title".into(),
            state: "Title".into(),
            algorithm: anaden_vision::Algorithm::Ccoeff,
            template: tpl,
            roi: None,
            threshold: 0.9,
            base: None,
            action: Some(Action::Stop),
            // next があっても Stop は next_current=None。
            next: Some(vec!["Ignored".into()]),
        }];

        let mut state = PipelineState::new("Title");
        let result = state.tick(&screenshot, &tasks).expect("should match");
        assert_eq!(result.command, None, "Stop emits no command");
        assert_eq!(result.next_current, None, "Stop yields no next");
        assert_eq!(state.current(), "Title", "current unchanged on Stop");
    }

    #[test]
    fn click_self_uses_matched_region_in_tick() {
        // needle を (150,75) に埋め、ClickSelf で tick すると Tap が matched_region の中心
        // （マッチ左上 + needle wh/2 = (150+20, 75+20) 付近）になることをレンジ検証する。
        let needle = gradient_needle(NEEDLE_PX, NEEDLE_PX);
        let screenshot = luma_dyn(embed(FULL_W, FULL_H, &needle, 150, 75, 128));
        let tpl = write_template_persisted(&needle);

        let tasks = vec![TaskDef {
            name: "Title".into(),
            state: "Title".into(),
            algorithm: anaden_vision::Algorithm::Ccoeff,
            template: tpl,
            roi: None,
            threshold: 0.9,
            base: None,
            action: Some(Action::ClickSelf),
            next: Some(vec!["LoadGame".into()]),
        }];

        let mut state = PipelineState::new("Title");
        let result = state.tick(&screenshot, &tasks).expect("should match");
        let tap = result
            .command
            .expect("ClickSelf with matched region must emit a Tap");
        match tap {
            InputCommand::Tap { x, y } => {
                assert!(
                    (148..=172).contains(&x),
                    "tap.x near matched center: got {x}"
                );
                assert!((73..=97).contains(&y), "tap.y near matched center: got {y}");
            }
            other => panic!("expected Tap, got {other:?}"),
        }
        assert_eq!(state.current(), "LoadGame");
    }

    // ---- (F) InputCommand の serde Deserialize 境界検証（E0277 回帰ガード） ----
    //
    // LoopOutcome.fired_commands: Vec<InputCommand> が serde::Deserialize 境界を満たすには、
    // InputCommand 自体が Deserialize でなければならない（E0277）。本モジュールは JSON 等の
    // フォーマットクレート(serde_json/bincode)に依存しないため、汎用デシリアライザを要求する
    // 関数へ型を渡すことで「Deserialize 境界を満たすこと」を静的に検証する。
    // T-fix-1 で #[derive(Deserialize)] を外すとこれらのテストはコンパイルエラーとなる。

    fn _require_deserialize<'de, T>()
    where
        T: serde::Deserialize<'de>,
    {
    }

    #[test]
    fn input_command_satisfies_serde_deserialize_bound() {
        // E0277 の直接の再発防止: 型レベルで Deserialize<'de> を要求する関数へ渡す。
        _require_deserialize::<InputCommand>();
    }

    #[test]
    fn input_command_vec_satisfies_serde_deserialize_bound() {
        // Vec<InputCommand> は LoopOutcome.fired_commands の実型。
        // これが Deserialize 可能であることが E0277 解消の直接の要件。
        _require_deserialize::<Vec<InputCommand>>();
    }

    #[test]
    fn input_command_satisfies_serde_serialize_bound() {
        // Serialize は既存。両 derive が揃っていることを型レベルで担保。
        fn _require_serialize<T>()
        where
            T: serde::Serialize,
        {
        }
        _require_serialize::<InputCommand>();
    }
}
