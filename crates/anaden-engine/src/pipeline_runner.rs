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

use anaden_core::ScreenRegion;
use anaden_vision::{Action, StepOutcome, TaskDef, run_step};

/// デバイスへ発火すべき入力コマンド（ピクセル座標）。
///
/// 座標モデルは [`anaden_core::ScreenPoint`] とデバイス層 `input tap x y` / `input swipe` の
/// `u32` ピクセルと一致。[`Action::ClickSelf`] / [`Action::ClickRect`] は [`InputCommand::Tap`]、
/// [`Action::Swipe`] は [`InputCommand::Swipe`] へ 1:1 対応する。
///
/// duration_ms は現状 [`Action::Swipe`] にパラメータが無いため持たない。後段の発火層が
/// デフォルト値を埋める。将来 [`Action::Swipe`] に duration が増えたらフィールド追加で拡張する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputCommand {
    /// 指定座標をタップ。
    Tap { x: u32, y: u32 },
    /// `from` から `to` へスワイプ。
    Swipe { from: (u32, u32), to: (u32, u32) },
}

/// アクションから入力コマンドへ変換する。
///
/// - [`Action::ClickSelf`] は `matched_region` の中心をタップ。`matched_region` が [`None`] の場合は
///   クリック位置を決定できないため [`None`]（安全側: 発火しない）。
/// - [`Action::ClickRect`] は `roi` 自身の中心をタップ（`matched_region` は無視する）。
/// - [`Action::Swipe`] は `from`/`to` 各々の中心を使う。
/// - [`Action::DoNothing`] / [`Action::Stop`] は入力コマンドではないため [`None`]。
///
/// `action` は参照で受け Clone 回避。戻り値 [`Option<InputCommand>`]: [`None`] は
/// 「この tick では入力を発火しない」を意味し、caller は状態遷移だけ進める。
pub fn action_to_command(
    action: &Action,
    matched_region: Option<ScreenRegion>,
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
            let (x, y) = roi.center();
            Some(InputCommand::Tap { x, y })
        }
        Action::Swipe { from, to } => Some(InputCommand::Swipe {
            from: from.center(),
            to: to.center(),
        }),
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickResult {
    /// 発火すべき入力コマンド。[`None`] はこの tick で入力無し。
    pub command: Option<InputCommand>,
    /// 遷移先タスク名。[`None`] は停止 or 待機（caller が別判断）。
    pub next_current: Option<String>,
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
        let command = action_to_command(&outcome.action, Some(outcome.matched_region));
        let next_current = advance_next(&outcome);
        if let Some(next) = &next_current {
            self.current = next.clone();
        }
        Some(TickResult {
            command,
            next_current,
        })
    }
}

#[cfg(test)]
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
        }
    }

    // ---- (A) action_to_command の全ケース ----

    #[test]
    fn click_self_with_region_taps_center() {
        let action = Action::ClickSelf;
        let region = ScreenRegion::new(100, 200, 80, 60);
        assert_eq!(
            action_to_command(&action, Some(region)),
            Some(InputCommand::Tap { x: 140, y: 230 })
        );
    }

    #[test]
    fn click_self_without_region_returns_none() {
        let action = Action::ClickSelf;
        assert_eq!(action_to_command(&action, None), None);
    }

    #[test]
    fn click_rect_taps_roi_center_ignoring_matched_region() {
        let action = Action::ClickRect {
            roi: ScreenRegion::new(520, 320, 240, 80),
        };
        // matched_region を与えても roi 優先であることを確認。
        let matched = Some(ScreenRegion::new(0, 0, 10, 10));
        assert_eq!(
            action_to_command(&action, matched),
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
            action_to_command(&action, None),
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
            action_to_command(&action, Some(ScreenRegion::new(10, 10, 10, 10))),
            None
        );
    }

    #[test]
    fn stop_returns_none() {
        let action = Action::Stop;
        assert_eq!(
            action_to_command(&action, Some(ScreenRegion::new(10, 10, 10, 10))),
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
    // pipeline.rs の run_step_setup 手法を再利用する。キャンバスは FULL_W×FULL_H。

    const FULL_W: u32 = 320;
    const FULL_H: u32 = 180;

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
        let needle = gradient_needle(40, 40);
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
        let needle = gradient_needle(40, 40);
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
        let needle = gradient_needle(40, 40);
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
        let needle = gradient_needle(40, 40);
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
        let needle = gradient_needle(40, 40);
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
}
