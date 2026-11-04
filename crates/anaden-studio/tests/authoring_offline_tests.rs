#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use anaden_core::{Goal, StopCondition};
use anaden_studio::authoring_session::{AuthoringSession, GestureOutcome};
use anaden_studio::scenario_load::load_scenario_from_dir;
use anaden_vision::{Action, ScreenScaler};
use image::DynamicImage;

const LIVE_FRAME: &str = "live-field-probe.jpg";

const STEP1: ((u32, u32), [u32; 4]) = ((1007, 150), [865, 35, 285, 230]);
const STEP2: ((u32, u32), [u32; 4]) = ((933, 318), [878, 268, 110, 100]);

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("issue190")
        .join(name)
}

fn load_fixture() -> DynamicImage {
    let path = fixture(LIVE_FRAME);
    image::open(&path).unwrap_or_else(|e| panic!("fixture open {}: {e}", path.display()))
}

fn scripted_session() -> AuthoringSession {
    let mut s = AuthoringSession::new("authored_offline");
    s.set_threshold(0.80);

    s.push_frame(&load_fixture());
    match s.record_tap(STEP1.0).expect("tap accepted") {
        GestureOutcome::Pending => {}
        GestureOutcome::Confirmed { .. } => panic!("tap alone must not confirm"),
    }
    match s.record_region(STEP1.1).expect("region confirms") {
        GestureOutcome::Confirmed { .. } => {}
        GestureOutcome::Pending => panic!("both gestures must confirm"),
    }

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

#[test]
fn scripted_session_on_real_frames_saves_and_loads() {
    let session = scripted_session();
    assert_eq!(session.steps().len(), 2, "2 ステップ確定済み");
    assert!(
        session.warnings().is_empty(),
        "実フレーム (タイトル画面アートワーク) クロップに品質警告は出ない: {:?}",
        session.warnings()
    );

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

#[test]
fn authored_template_detects_in_runtime_normalized_space() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut session = scripted_session();
    let outcome = session.save(tmp.path()).expect("save");
    let st = load_scenario_from_dir(&outcome.dir).expect("load");
    let scaler = ScreenScaler::new();
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
