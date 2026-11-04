use image::DynamicImage;
use serde::{Deserialize, Serialize};

use anaden_core::ScreenRegion;
use anaden_vision::{Action, StepOutcome, TaskDef, TaskDefError, roi_to_normalized, run_step};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputCommand {
    Tap { x: u32, y: u32 },
    Swipe { from: (u32, u32), to: (u32, u32) },
}

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
            let nroi = roi_to_normalized([roi.x, roi.y, roi.width, roi.height], norm_w, norm_h);
            let nr = ScreenRegion::new(nroi[0], nroi[1], nroi[2], nroi[3]);
            let (x, y) = nr.center();
            Some(InputCommand::Tap { x, y })
        }
        Action::Swipe { from, to } => {
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

pub fn advance_next(outcome: &StepOutcome) -> Option<String> {
    match outcome.action {
        Action::Stop => None,
        _ => outcome.next.first().cloned(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TickResult {
    pub command: Option<InputCommand>,
    pub next_current: Option<String>,
    pub matched_confidence: Option<f32>,
    pub matched_region: Option<ScreenRegion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineState {
    pub current: String,
}

impl PipelineState {
    pub fn new(current: impl Into<String>) -> Self {
        Self {
            current: current.into(),
        }
    }

    pub fn current(&self) -> &str {
        &self.current
    }

    pub fn set_current(&mut self, current: impl Into<String>) {
        self.current = current.into();
    }

    pub fn tick(
        &mut self,
        screenshot: &DynamicImage,
        tasks: &[TaskDef],
    ) -> Result<Option<TickResult>, TaskDefError> {
        let outcome = match run_step(tasks, screenshot, &self.current)? {
            Some(o) => o,
            None => return Ok(None),
        };
        let matched_region = outcome.matched_region;
        let matched_confidence = Some(outcome.matched_confidence);
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
        Ok(Some(TickResult {
            command,
            next_current,
            matched_confidence,
            matched_region: Some(matched_region),
        }))
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
        let result = state
            .tick(&screenshot, &tasks)
            .expect("tick must not error")
            .expect("should match");
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
        let result = state
            .tick(&screenshot, &tasks)
            .expect("tick must not error");
        assert!(result.is_none(), "no needle must yield Ok(None)");
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
        let result = state
            .tick(&screenshot, &tasks)
            .expect("tick must not error");
        assert!(result.is_none(), "unknown current must yield Ok(None)");
        assert_eq!(
            state.current(),
            "NoSuch",
            "current unchanged on unknown task"
        );
    }

    /// F34 (05-template-cache): テンプレート読込エラーは tick を Ok(None) (=NoMatch) に
    /// 潰さず Err で伝播すること。driver (run_once) はこれを停止系エラーとして扱い、
    /// リトライ候補の NoMatch と区別する。
    #[test]
    fn tick_missing_template_propagates_err_not_none() {
        let screenshot = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let tasks = vec![TaskDef {
            name: "Title".into(),
            state: "Title".into(),
            algorithm: anaden_vision::Algorithm::Ccoeff,
            template: PathBuf::from("/nonexistent/does_not_exist.png"),
            roi: None,
            threshold: 0.9,
            base: None,
            action: Some(Action::ClickSelf),
            next: None,
        }];

        let mut state = PipelineState::new("Title");
        let err = state
            .tick(&screenshot, &tasks)
            .expect_err("missing template must surface Err (not Ok(None)=NoMatch)");
        assert!(
            matches!(err, TaskDefError::TemplateLoadFailed { .. }),
            "error kind must be TemplateLoadFailed, got {err:?}"
        );
        assert_eq!(state.current(), "Title", "current unchanged on error");
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
        let result = state
            .tick(&screenshot, &tasks)
            .expect("tick must not error")
            .expect("should match");
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
        let result = state
            .tick(&screenshot, &tasks)
            .expect("tick must not error")
            .expect("should match");
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
