//! Issue #190 Shard 3/3 — スクリプト模擬オーサリングのオフライン統合テスト
//! (受け入れ基準 2「保存→load→実行」のオフライン側)。
//!
//! 実フレーム資産 (E2E 直前の読み取り専用 probe が採取した PC 版キャプチャを
//! `crop_to_content` + Lanczos3 で raw-1258x708 空間へ縮小した JPEG fixture) で
//! 模擬セッションを駆動し、
//!
//! 1. セッション確定 (tap + region → click_self ステップ)
//! 2. 保存 (`templates/` 外の tempdir — テンプレートバンク監査テストと干渉しない)
//! 3. `load_scenario_from_dir` によるロード
//! 4. 保存テンプレートで recorded frame に `TaskDef::detect` が match
//!
//! までを検証する。4 は 2 つの座標空間で行う:
//! - **authoring 空間そのまま** (1258x708): detect の ROI/needle スケールが恒等。
//! - **runtime 空間** (`ScreenScaler::normalize` で 1280 基準へ): `anaden run` の
//!   実行経路 (capture → 黒帯クロップ → 1280 正規化 → detect) と同一の変換。
//!   保存テンプレート (1258 空間) が 1280 空間キャプチャへ一致することで、
//!   実機 E2E (発火) の前提条件をオフラインで機械保証する。
//!
//! スクリプトは `examples/authoring_demo.rs` の SCRIPT と同値 (タイトル画面右上
//! アートワーク領域 2 箇所。E2E 直前の実画面から録った幾何。事後確定で probe
//! フレームはフィールド HUD ではなくタイトル画面だった — テンプレは水彩
//! アートワークのクロップ。run-180 旧フレームは HUD レイアウトが現行と異なり
//! offline 検証で NoMatch になったため差し替えた)。

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use anaden_core::{Goal, StopCondition};
use anaden_studio::authoring_session::{AuthoringSession, GestureOutcome};
use anaden_studio::scenario_load::load_scenario_from_dir;
use anaden_vision::{Action, ScreenScaler};
use image::DynamicImage;

/// E2E 実機キャプチャ由来の fixture ファイル名 (1258x708 JPEG)。
const LIVE_FRAME: &str = "live-field-probe.jpg";

/// Step 1・Step 2 (タイトル画面右上アートワーク 2 箇所) のスクリプト値。
/// example の SCRIPT と同値 (tap アンカー + 認識領域)。
const STEP1: ((u32, u32), [u32; 4]) = ((1007, 150), [865, 35, 285, 230]);
const STEP2: ((u32, u32), [u32; 4]) = ((933, 318), [878, 268, 110, 100]);

/// fixture ルート (tests/fixtures/issue190)。
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("issue190")
        .join(name)
}

/// fixture フレームを読む (fail-loud: 欠損時は open エラーで即 panic)。
fn load_fixture() -> DynamicImage {
    let path = fixture(LIVE_FRAME);
    image::open(&path).unwrap_or_else(|e| panic!("fixture open {}: {e}", path.display()))
}

/// 模擬セッション: example の SCRIPT と同値の 2 ステップを確定させたセッションを
/// 返す (Step1・Step2 = タイトル画面右上アートワーク 2 箇所。同一実フレームから録る)。
fn scripted_session() -> AuthoringSession {
    let mut s = AuthoringSession::new("authored_offline");
    s.set_threshold(0.80);

    // Step 1: タップ→領域 の順。
    s.push_frame(&load_fixture());
    match s.record_tap(STEP1.0).expect("tap accepted") {
        GestureOutcome::Pending => {}
        GestureOutcome::Confirmed { .. } => panic!("tap alone must not confirm"),
    }
    match s.record_region(STEP1.1).expect("region confirms") {
        GestureOutcome::Confirmed { .. } => {}
        GestureOutcome::Pending => panic!("both gestures must confirm"),
    }

    // Step 2: 領域→タップ の逆順 (セッションは順非依存)。
    s.push_frame(&load_fixture());
    match s.record_region(STEP2.1).expect("region accepted") {
        GestureOutcome::Pending => {}
        GestureOutcome::Confirmed { .. } => panic!("region alone must not confirm"),
    }
    match s.record_tap(STEP2.0).expect("tap confirms") {
        GestureOutcome::Confirmed { .. } => {}
        GestureOutcome::Pending => panic!("both gestures must confirm"),
    }
    s
}

/// 実フレームでの模擬セッション → 保存 → load まで。
/// ステップ構成 (next チェーン・start_task・action・閾値) とテンプレート品質
/// (警告ゼロ = 構造あり + needle 収容) を検証する。
#[test]
fn scripted_session_on_real_frames_saves_and_loads() {
    let session = scripted_session();
    assert_eq!(session.steps().len(), 2, "2 ステップ確定済み");
    assert!(
        session.warnings().is_empty(),
        "実フレーム (タイトル画面アートワーク) クロップに品質警告は出ない: {:?}",
        session.warnings()
    );

    // 保存 (tempdir = repo templates/ 外)。
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut session = session;
    let outcome = session
        .save(tmp.path())
        .expect("save must succeed with 2 confirmed steps");
    assert_eq!(outcome.dir, tmp.path().join("authored_offline"));
    assert!(
        outcome.warnings.is_empty(),
        "保存時テンプレ品質警告ゼロ: {:?}",
        outcome.warnings
    );

    // ロード: チェーン・start_task・action・閾値が復元される。
    let st = load_scenario_from_dir(&outcome.dir).expect("load must succeed");
    assert_eq!(st.name, "authored_offline");
    assert_eq!(st.start_task, "AuthoredStep01");
    assert_eq!(st.task_names(), vec!["AuthoredStep01", "AuthoredStep02"]);
    let step1 = st.task("AuthoredStep01").expect("step1");
    assert_eq!(
        step1.next.as_deref(),
        Some(&["AuthoredStep02".to_string()][..])
    );
    assert_eq!(step1.roi, Some(STEP1.1));
    assert_eq!(step1.threshold, 0.80);
    assert_eq!(step1.action, Some(Action::ClickSelf));
    let step2 = st.task("AuthoredStep02").expect("step2");
    assert_eq!(step2.next.as_deref(), Some(&[][..]));
    assert_eq!(step2.action, Some(Action::ClickSelf));
    assert_eq!(
        st.goals,
        vec![Goal {
            name: "authoring_timeout".to_string(),
            stop: StopCondition::Timeout { secs: 600 },
        }],
        "既定ゴール (10 分タイムアウト安全弁) が保存される"
    );
}

/// 保存したテンプレートで recorded frame に detect が match する (authoring 空間)。
///
/// テンプレートクロップは record 時点のフレームそのものから取られているため
/// 恒等スケール (1258x708) ではマッチ中心が記録タップアンカーと一致する。
#[test]
fn authored_template_detects_on_recorded_frame() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut session = scripted_session();
    let outcome = session.save(tmp.path()).expect("save");
    let st = load_scenario_from_dir(&outcome.dir).expect("load");
    let frame = load_fixture();

    for (task_name, (tap, _region)) in [("AuthoredStep01", STEP1), ("AuthoredStep02", STEP2)] {
        let task = st.task(task_name).expect("task");
        let m = task
            .detect(&frame, &outcome.dir)
            .unwrap_or_else(|e| panic!("detect {task_name}: {e}"))
            .unwrap_or_else(|| panic!("detect {task_name} must match its recorded frame"));
        assert!(
            m.confidence.0 >= task.threshold,
            "{task_name}: conf {:.4} >= threshold {:.2}",
            m.confidence.0,
            task.threshold
        );
        let (cx, cy) = m.region.center();
        let (dx, dy) = (cx.abs_diff(tap.0), cy.abs_diff(tap.1));
        assert!(
            dx <= 24 && dy <= 24,
            "{task_name}: match center ({cx},{cy}) must be near tap anchor {tap:?} \
             (roi とテンプレが同一クロップのため中心は roi 中心に一致する)"
        );
        println!("{task_name}: conf={:.4} center=({cx},{cy})", m.confidence.0);
    }
}

/// 保存テンプレートが runtime 空間 (1280 基準正規化) のキャプチャへも match する。
///
/// `anaden run` は capture → `crop_to_content` → `ScreenScaler::normalize` (1280 基準)
/// → `TaskDef::detect` の順で流す。detect は roi/needle を raw-1258 空間から
/// 正規化後寸法へ動的スケールするため、この検証が実機 E2E 発火の前提条件を
/// オフラインで保証する (実機未検証時の Done 証拠 2 の本体)。
#[test]
fn authored_template_detects_in_runtime_normalized_space() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut session = scripted_session();
    let outcome = session.save(tmp.path()).expect("save");
    let st = load_scenario_from_dir(&outcome.dir).expect("load");
    let scaler = ScreenScaler::new();
    // runtime と同一の正規化: 1280 基準 (1258x708 → 1280x722)。
    let normalized = scaler.normalize(&load_fixture());
    assert_eq!(normalized.width(), 1280, "normalize は 1280 基準");

    for (task_name, (tap, _region)) in [("AuthoredStep01", STEP1), ("AuthoredStep02", STEP2)] {
        let task = st.task(task_name).expect("task");
        let m = task
            .detect(&normalized, &outcome.dir)
            .unwrap_or_else(|e| panic!("detect {task_name}: {e}"))
            .unwrap_or_else(|| panic!("detect {task_name} must match in runtime normalized space"));
        assert!(
            m.confidence.0 >= task.threshold,
            "{task_name}: runtime-space conf {:.4} >= threshold {:.2} \
             (1258→1280 再サンプルでの劣化が閾値内であること)",
            m.confidence.0,
            task.threshold
        );
        // マッチ中心は 1280 空間。タップアンカー (1258 空間) と比較するには
        // 正規化後寸法へ拡大して許容誤差を見る (中心ずれ < 2%)。
        let (cx, cy) = m.region.center();
        let expected = (
            (tap.0 as f32 * normalized.width() as f32 / 1258.0) as u32,
            (tap.1 as f32 * normalized.height() as f32 / 708.0) as u32,
        );
        assert!(
            cx.abs_diff(expected.0) <= 26 && cy.abs_diff(expected.1) <= 26,
            "{task_name}: runtime match center ({cx},{cy}) near scaled anchor {expected:?}"
        );
        println!(
            "{task_name}: runtime conf={:.4} center=({cx},{cy})",
            m.confidence.0
        );
    }
}
