#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use anaden_core::{Goal, StopCondition};
use anaden_studio::scenario_load::load_scenario_from_dir;
use anaden_studio::scenario_task_link::{ScenarioTaskError, bind_and_enable_task};
use anaden_studio::scenario_ui::{ScenarioEditorState, ScenarioSaveError, save_scenario};
use anaden_studio::tasks_def::{TaskError, enable_task, load_task_definitions};
use anaden_vision::{Action, Algorithm, TaskDef};
use image::{DynamicImage, GrayImage, Luma};

fn task_def(name: &str) -> TaskDef {
    TaskDef {
        name: name.to_string(),
        state: "Field".to_string(),
        algorithm: Algorithm::Ccoeff,
        template: PathBuf::from(format!("{name}.png")),
        roi: Some([10, 20, 100, 50]),
        threshold: 0.8,
        base: None,
        action: Some(Action::ClickSelf),
        next: Some(vec![]),
    }
}

fn minimal_state(name: &str) -> ScenarioEditorState {
    let mut st = ScenarioEditorState::new(name);
    st.add_task(task_def("Start"));
    st.add_goal(Goal {
        name: "loop3".to_string(),
        stop: StopCondition::LoopCount { target: 3 },
    });
    st
}

fn solid_image(w: u32, h: u32, v: u8) -> DynamicImage {
    DynamicImage::ImageLuma8(GrayImage::from_pixel(w, h, Luma([v])))
}

#[test]
fn task_dir_with_unreadable_toml_entry_reports_read_error() {
    let tmp = tempfile::tempdir().unwrap();
    let entry = tmp.path().join("zz.toml");
    std::fs::create_dir_all(&entry).unwrap();

    let err = load_task_definitions(tmp.path()).unwrap_err();
    assert!(
        matches!(err, TaskError::Read { ref path, .. } if path == &entry),
        "err: {err:?}"
    );
}

#[test]
fn task_dir_with_malformed_toml_reports_parse_error() {
    let tmp = tempfile::tempdir().unwrap();
    let toml_path = tmp.path().join("broken.toml");
    std::fs::write(&toml_path, "not = valid = toml").unwrap();

    let err = load_task_definitions(tmp.path()).unwrap_err();
    assert!(
        matches!(err, TaskError::Parse { ref path, .. } if path == &toml_path),
        "err: {err:?}"
    );
}

#[test]
fn enable_task_on_missing_file_reports_read_error() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("ghost.toml");

    let err = enable_task(&missing, "ghost", "pipe", tmp.path()).unwrap_err();
    assert!(matches!(err, TaskError::Read { .. }), "err: {err:?}");
}

#[test]
fn save_onto_file_root_reports_dir_create_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let root_file = tmp.path().join("not_a_dir");
    std::fs::write(&root_file, b"x").unwrap();

    let err = save_scenario(&minimal_state("pipe"), &[], &root_file).unwrap_err();
    assert!(
        matches!(err, ScenarioSaveError::DirCreate(_)),
        "err: {err:?}"
    );
}

#[test]
fn save_writeback_onto_directory_shaped_png_target_reports_png_write_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("pipelines");
    save_scenario(&minimal_state("pipe"), &[], &root).expect("create pipeline");

    let dir = root.join("pipe");
    let st = load_scenario_from_dir(&dir).expect("load back");
    std::fs::create_dir_all(dir.join("Start.png")).unwrap();

    let pngs = vec![("Start".to_string(), solid_image(100, 50, 128))];
    let err = save_scenario(&st, &pngs, &root).unwrap_err();
    assert!(
        matches!(err, ScenarioSaveError::PngWrite(_)),
        "err: {err:?}"
    );
}

#[test]
fn bind_with_missing_task_toml_reports_task_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let tasks_dir = root.join("templates/tasks");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    let pipeline = root.join("templates/pipelines/pipe");
    std::fs::create_dir_all(&pipeline).unwrap();

    let err = bind_and_enable_task(&tasks_dir, root, "ghost_id", &pipeline).unwrap_err();
    assert!(
        matches!(
            err,
            ScenarioTaskError::TaskNotFound(ref p)
                if *p == tasks_dir.join("ghost_id.toml")
        ),
        "err: {err:?}"
    );
}
