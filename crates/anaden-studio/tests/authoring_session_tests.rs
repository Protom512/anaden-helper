#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use anaden_core::{Goal, StopCondition};
use anaden_studio::authoring_session::{AuthoringError, AuthoringSession, GestureOutcome};
use anaden_studio::scenario_load::load_scenario_from_dir;
use anaden_studio::scenario_save::ScenarioSaveError;
use anaden_studio::scenario_validate::ScenarioValidationError;
use anaden_vision::Action;
use image::{DynamicImage, GrayImage, Luma};

fn gradient_frame(w: u32, h: u32, seed: u32) -> DynamicImage {
    let mut img = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = ((x * 2 + y * 3 + seed) % 200) as u8;
            img.put_pixel(x, y, Luma([v]));
        }
    }
    DynamicImage::ImageLuma8(img)
}

fn flat_frame(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageLuma8(GrayImage::from_pixel(w, h, Luma([250u8])))
}

fn confirm_step(session: &mut AuthoringSession, tap: (u32, u32), region: [u32; 4]) -> Vec<String> {
    match session.record_tap(tap).expect("tap must be accepted") {
        GestureOutcome::Pending => {}
        GestureOutcome::Confirmed { .. } => panic!("tap alone must not confirm"),
    }
    match session.record_region(region).expect("region must confirm") {
        GestureOutcome::Confirmed { warnings } => warnings,
        GestureOutcome::Pending => panic!("both gestures present must confirm"),
    }
}

#[test]
fn tap_then_region_confirms_click_self_step() {
    let mut s = AuthoringSession::new("demo_tap");
    s.push_frame(&gradient_frame(400, 300, 0));

    let warnings = confirm_step(&mut s, (60, 45), [10, 20, 100, 50]);
    assert!(warnings.is_empty(), "warnings: {warnings:?}");

    assert_eq!(s.steps().len(), 1);
    let step = &s.steps()[0];
    assert_eq!(step.task.name, "AuthoredStep01");
    assert_eq!(step.task.roi, Some([10, 20, 100, 50]));
    assert_eq!(step.task.action, Some(Action::ClickSelf));
    assert_eq!(step.task.template.to_str(), Some("AuthoredStep01.png"));
    assert_eq!((step.template.width(), step.template.height()), (100, 50));
    assert_eq!(step.tap, (60, 45));
    assert!(s.warnings().is_empty());
}

#[test]
fn region_then_tap_confirms_in_reverse_order() {
    let mut s = AuthoringSession::new("demo_rev");
    s.push_frame(&gradient_frame(400, 300, 7));

    match s.record_region([0, 0, 80, 40]).expect("region accepted") {
        GestureOutcome::Pending => {}
        GestureOutcome::Confirmed { .. } => panic!("region alone must not confirm"),
    }
    assert_eq!(s.pending_region(), Some([0, 0, 80, 40]));
    match s.record_tap((40, 20)).expect("tap confirms") {
        GestureOutcome::Confirmed { warnings } => assert!(warnings.is_empty()),
        GestureOutcome::Pending => panic!("must confirm"),
    }
    assert_eq!(s.steps().len(), 1);
    assert_eq!(s.steps()[0].task.roi, Some([0, 0, 80, 40]));
    assert!(s.pending_tap().is_none() && s.pending_region().is_none());
}

#[test]
fn multiple_steps_chain_via_next_and_start_task() {
    let mut s = AuthoringSession::new("demo_chain");
    s.push_frame(&gradient_frame(400, 300, 0));
    confirm_step(&mut s, (60, 45), [10, 20, 100, 50]);
    s.push_frame(&gradient_frame(400, 300, 40));
    confirm_step(&mut s, (200, 100), [150, 80, 120, 60]);
    s.push_frame(&gradient_frame(400, 300, 80));
    confirm_step(&mut s, (300, 250), [280, 220, 90, 40]);

    let st = s.to_scenario();
    assert_eq!(
        st.task_names(),
        vec!["AuthoredStep01", "AuthoredStep02", "AuthoredStep03"]
    );
    assert_eq!(st.start_task, "AuthoredStep01");
    assert_eq!(st.tasks[0].next, Some(vec!["AuthoredStep02".to_string()]));
    assert_eq!(st.tasks[1].next, Some(vec!["AuthoredStep03".to_string()]));
    assert_eq!(st.tasks[2].next, Some(vec![]));
    assert!(
        st.validate().is_ok(),
        "issues: {:?}",
        st.validation_issues()
    );
}

#[test]
fn undo_removes_last_step_and_rechains() {
    let mut s = AuthoringSession::new("demo_undo");
    s.push_frame(&gradient_frame(400, 300, 0));
    confirm_step(&mut s, (60, 45), [10, 20, 100, 50]);
    s.push_frame(&gradient_frame(400, 300, 40));
    confirm_step(&mut s, (200, 100), [150, 80, 120, 60]);
    assert_eq!(s.steps().len(), 2);

    assert!(s.undo());
    let st = s.to_scenario();
    assert_eq!(st.task_names(), vec!["AuthoredStep01"]);
    assert_eq!(st.tasks[0].next, Some(vec![]), "チェーン再接続");

    assert!(s.undo());
    assert_eq!(s.steps().len(), 0);
    assert!(!s.undo(), "空セッションの undo は false");
    confirm_step(&mut s, (60, 45), [10, 20, 100, 50]);
    assert_eq!(s.steps()[0].task.name, "AuthoredStep01");
}

#[test]
fn to_scenario_carries_name_start_task_and_goal() {
    let mut s = AuthoringSession::new("demo_goal");
    s.push_frame(&gradient_frame(400, 300, 0));
    confirm_step(&mut s, (60, 45), [10, 20, 100, 50]);

    let default_goal = Goal {
        name: "authoring_timeout".to_string(),
        stop: StopCondition::Timeout { secs: 600 },
    };
    let st = s.to_scenario();
    assert_eq!(st.name, "demo_goal");
    assert_eq!(st.start_task, "AuthoredStep01");
    assert_eq!(st.goals, vec![default_goal]);

    s.set_goal(Goal {
        name: "loop3".to_string(),
        stop: StopCondition::LoopCount { target: 3 },
    });
    assert_eq!(
        s.to_scenario().goals,
        vec![Goal {
            name: "loop3".to_string(),
            stop: StopCondition::LoopCount { target: 3 },
        }]
    );
}

#[test]
fn save_roundtrips_through_load_scenario_from_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("pipelines");

    let mut s = AuthoringSession::new("demo_saved");
    s.push_frame(&gradient_frame(400, 300, 0));
    confirm_step(&mut s, (60, 45), [10, 20, 100, 50]);
    s.push_frame(&gradient_frame(400, 300, 40));
    confirm_step(&mut s, (200, 100), [150, 80, 120, 60]);

    let outcome = s.save(&root).expect("save");
    assert_eq!(outcome.dir, root.join("demo_saved"));
    assert!(
        outcome.warnings.is_empty(),
        "warnings: {:?}",
        outcome.warnings
    );
    assert!(outcome.dir.join("AuthoredStep01.png").exists());
    assert!(outcome.dir.join("AuthoredStep01.toml").exists());

    let loaded = load_scenario_from_dir(&outcome.dir).expect("load");
    assert_eq!(loaded.name, "demo_saved");
    assert_eq!(loaded.start_task, "AuthoredStep01");
    assert_eq!(
        loaded.task_names(),
        vec!["AuthoredStep01", "AuthoredStep02"]
    );
    for (task, roi) in [
        (&loaded.tasks[0], [10, 20, 100, 50]),
        (&loaded.tasks[1], [150, 80, 120, 60]),
    ] {
        assert_eq!(task.roi, Some(roi));
        assert_eq!(task.action, Some(Action::ClickSelf));
        let (nw, nh) = image::image_dimensions(&task.template).expect("template resolves");
        assert_eq!((nw, nh), (roi[2], roi[3]));
    }
    assert_eq!(
        loaded.tasks[0].next,
        Some(vec!["AuthoredStep02".to_string()])
    );
    assert_eq!(loaded.tasks[1].next, Some(vec![]));
    assert_eq!(
        loaded.goals,
        vec![Goal {
            name: "authoring_timeout".to_string(),
            stop: StopCondition::Timeout { secs: 600 },
        }]
    );

    s.push_frame(&gradient_frame(400, 300, 80));
    confirm_step(&mut s, (300, 250), [280, 220, 90, 40]);
    s.save(&root).expect("consecutive save");
    let reloaded = load_scenario_from_dir(&outcome.dir).expect("reload");
    assert_eq!(reloaded.task_names().len(), 3);
}

#[test]
fn tap_outside_region_warns_but_confirms() {
    let mut s = AuthoringSession::new("edge_tap");
    s.push_frame(&gradient_frame(400, 300, 0));

    let warnings = confirm_step(&mut s, (390, 290), [10, 20, 100, 50]);
    assert_eq!(warnings.len(), 1, "warnings: {warnings:?}");
    assert!(
        warnings[0].contains("AuthoredStep01") && warnings[0].contains("領域"),
        "警告はタスク名と領域外を明示: {}",
        warnings[0]
    );
    assert_eq!(s.steps().len(), 1, "確定は妨げない");
    assert_eq!(s.warnings().len(), 1, "セッション警告へ累積");
}

#[test]
fn flat_frame_collects_unstructured_template_warning() {
    let mut s = AuthoringSession::new("edge_flat");
    s.push_frame(&flat_frame(400, 300));

    let warnings = confirm_step(&mut s, (60, 45), [10, 20, 100, 50]);
    assert_eq!(warnings.len(), 1, "warnings: {warnings:?}");
    assert!(
        warnings[0].contains("認識不能"),
        "警告は認識不能の恐れを明示: {}",
        warnings[0]
    );
    assert_eq!(s.steps().len(), 1, "保存と同様にブロックしない");
}

#[test]
fn empty_session_save_is_rejected_with_existing_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("pipelines");
    let mut s = AuthoringSession::new("edge_empty");

    let err = s.save(&root).expect_err("must refuse empty session");
    assert!(
        matches!(
            err,
            ScenarioSaveError::Invalid(ScenarioValidationError::NoTasks)
        ),
        "err: {err:?}"
    );
    assert!(!root.join("edge_empty").exists(), "1 バイトも書かない");
}

#[test]
fn zero_size_region_is_rejected_atomically() {
    let mut s = AuthoringSession::new("edge_zero");
    s.push_frame(&gradient_frame(400, 300, 0));
    match s.record_tap((60, 45)).expect("tap accepted") {
        GestureOutcome::Pending => {}
        GestureOutcome::Confirmed { .. } => panic!("tap alone must not confirm"),
    }

    let err = s
        .record_region([10, 20, 0, 50])
        .expect_err("must reject zero size");
    assert!(matches!(err, AuthoringError::RoiEmpty { roi } if roi == [10, 20, 0, 50]));
    assert!(s.pending_region().is_none(), "拒否時は未記録");
    assert_eq!(s.pending_tap(), Some((60, 45)), "先行タップは保持");

    match s.record_region([10, 20, 100, 50]).expect("region confirms") {
        GestureOutcome::Confirmed { warnings } => assert!(warnings.is_empty()),
        GestureOutcome::Pending => panic!("must confirm"),
    }
    assert_eq!(s.steps().len(), 1);
}

#[test]
fn confirm_without_frame_fails_and_reverts_gesture() {
    let mut s = AuthoringSession::new("edge_noframe");
    match s
        .record_tap((60, 45))
        .expect("tap accepted (no confirm without region)")
    {
        GestureOutcome::Pending => {}
        GestureOutcome::Confirmed { .. } => panic!("must stay pending"),
    }

    let err = s
        .record_region([10, 20, 100, 50])
        .expect_err("must fail without frame");
    assert!(matches!(err, AuthoringError::NoFrame));
    assert!(s.pending_region().is_none(), "失敗ジェスチャは破棄");
    assert_eq!(s.pending_tap(), Some((60, 45)), "先行タップは保持");

    s.push_frame(&gradient_frame(400, 300, 0));
    match s.record_region([10, 20, 100, 50]).expect("region confirms") {
        GestureOutcome::Confirmed { .. } => {}
        GestureOutcome::Pending => panic!("must confirm"),
    }
    assert_eq!(s.steps().len(), 1);
}

#[test]
fn region_fully_outside_frame_is_rejected() {
    let mut s = AuthoringSession::new("edge_out");
    s.push_frame(&gradient_frame(400, 300, 0));

    let err = s
        .record_region([500, 0, 50, 50])
        .expect_err("must reject outside frame");
    assert!(
        matches!(err, AuthoringError::RoiOutOfFrame { roi, fw, fh } if roi == [500, 0, 50, 50]
            && fw == 400 && fh == 300),
        "err: {err:?}"
    );
    assert!(s.pending_region().is_none());
}

#[test]
fn partially_outside_region_is_clipped_with_warning() {
    let mut s = AuthoringSession::new("edge_clip");
    s.push_frame(&gradient_frame(400, 300, 0));

    let warnings = confirm_step(&mut s, (380, 290), [350, 250, 100, 100]);
    assert_eq!(warnings.len(), 1, "warnings: {warnings:?}");
    assert!(
        warnings[0].contains("クリップ"),
        "警告はクリップを明示: {}",
        warnings[0]
    );
    let step = &s.steps()[0];
    assert_eq!(step.task.roi, Some([350, 250, 50, 50]));
    assert_eq!((step.template.width(), step.template.height()), (50, 50));
}
