#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::path::Path;

use anaden_engine::{OnFailure, RoutineDef, RoutineError, RoutineStep};

fn step(name: &str, pipeline_dir: &str) -> RoutineStep {
    RoutineStep {
        name: name.to_string(),
        pipeline_dir: pipeline_dir.to_string(),
        start_task: "T".to_string(),
        max_iters: 1,
        interval: 1,
        on_failure: OnFailure::Stop,
    }
}

fn def_with(steps: Vec<RoutineStep>) -> RoutineDef {
    RoutineDef {
        name: "daily".to_string(),
        steps,
    }
}

#[test]
fn empty_step_name_is_reported_with_index() {
    let def = def_with(vec![step("", "pipe")]);
    let err = def.validate(Path::new(".")).unwrap_err();
    match err {
        RoutineError::EmptyStepName { index } => assert_eq!(index, 0),
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn pipeline_dir_without_taskdefs_is_pipeline_load_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let empty = tmp.path().join("empty_pipe");
    std::fs::create_dir_all(&empty).unwrap();

    let def = def_with(vec![step("s", "empty_pipe")]);
    let err = def.validate(tmp.path()).unwrap_err();
    match err {
        RoutineError::PipelineLoadFailed { step, reason, .. } => {
            assert_eq!(step, "s");
            assert!(reason.contains("*.toml"), "reason: {reason}");
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[test]
fn pipeline_dir_with_broken_taskdef_is_pipeline_load_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let corrupt = tmp.path().join("corrupt_pipe");
    std::fs::create_dir_all(&corrupt).unwrap();
    std::fs::write(corrupt.join("bad.toml"), "not = valid = toml").unwrap();

    let def = def_with(vec![step("s", "corrupt_pipe")]);
    let err = def.validate(tmp.path()).unwrap_err();
    assert!(
        matches!(err, RoutineError::PipelineLoadFailed { ref step, .. } if step == "s"),
        "err: {err}"
    );
}
