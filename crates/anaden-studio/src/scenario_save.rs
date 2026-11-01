use std::path::{Path, PathBuf};

use image::DynamicImage;

use crate::scenario_state::ScenarioEditorState;
use crate::scenario_validate::ScenarioValidationError;

#[derive(Debug, thiserror::Error)]
pub enum ScenarioSaveError {
    #[error("scenario invalid: {0}")]
    Invalid(#[from] ScenarioValidationError),
    #[error("pipeline dir create failed")]
    DirCreate(#[source] std::io::Error),
    #[error("template PNG write failed")]
    PngWrite(#[source] image::ImageError),
    #[error("pipeline save failed: {0}")]
    Vision(#[from] anaden_vision::TaskDefError),
    #[error(
        "pipeline dir already exists: {dir} (既存 pipeline の無警告上書きは禁止。ロードして編集するか別名を指定してください)"
    )]
    PipelineDirAlreadyExists { dir: PathBuf },
    #[error(
        "task file path collision: {path} is the write target of tasks `{first}` and `{second}` (stem 衝突。stem ≠ name の既存タスクと衝突する名前は使えません)"
    )]
    TaskFileCollision {
        path: PathBuf,
        first: String,
        second: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioSaveOutcome {
    pub dir: PathBuf,
    pub warnings: Vec<String>,
}

pub fn save_scenario_with_warnings(
    state: &ScenarioEditorState,
    pngs: &[(String, DynamicImage)],
    pipelines_root: &Path,
) -> Result<ScenarioSaveOutcome, ScenarioSaveError> {
    state.validate()?;
    let dir = pipelines_root.join(state.name.trim());
    let out_paths = task_out_paths(state, &dir);
    check_unique_out_paths(state, &out_paths)?;
    if dir.is_dir() && !is_owned_dir(state, &dir) {
        return Err(ScenarioSaveError::PipelineDirAlreadyExists { dir: dir.clone() });
    }
    std::fs::create_dir_all(&dir).map_err(ScenarioSaveError::DirCreate)?;
    for (name, img) in pngs {
        if !state.tasks.iter().any(|t| &t.name == name) {
            continue;
        }
        img.save(dir.join(format!("{name}.png")))
            .map_err(ScenarioSaveError::PngWrite)?;
    }
    anaden_vision::save_pipeline_manifest(&state.to_manifest(), &dir)?;
    for (task, path) in state.tasks.iter().zip(&out_paths) {
        anaden_vision::save_task_def(task, path)?;
    }
    sweep_removed_taskdefs(&dir, &out_paths);
    let warnings = template_quality_warnings(state, pngs, &dir);
    Ok(ScenarioSaveOutcome { dir, warnings })
}

pub fn save_scenario(
    state: &ScenarioEditorState,
    pngs: &[(String, DynamicImage)],
    pipelines_root: &Path,
) -> Result<PathBuf, ScenarioSaveError> {
    save_scenario_with_warnings(state, pngs, pipelines_root).map(|outcome| outcome.dir)
}

fn template_quality_warnings(
    state: &ScenarioEditorState,
    pngs: &[(String, DynamicImage)],
    dir: &Path,
) -> Vec<String> {
    let mut warnings = Vec::new();
    for task in &state.tasks {
        if let Some((_, img)) = pngs.iter().find(|(name, _)| name == &task.name) {
            if let Some(warning) = crate::library::template_structure_warning(img) {
                warnings.push(format!("{}: {warning}", task.name));
            }
            if let Some(warning) =
                crate::library::needle_roi_warning((img.width(), img.height()), task.roi)
            {
                warnings.push(format!("{}: {warning}", task.name));
            }
            continue;
        }
        let path = if task.template.is_absolute() {
            task.template.clone()
        } else {
            dir.join(&task.template)
        };
        if !path.is_file() {
            continue;
        }
        if let Ok((nw, nh)) = image::image_dimensions(&path)
            && let Some(warning) = crate::library::needle_roi_warning((nw, nh), task.roi)
        {
            warnings.push(format!("{}: {warning}", task.name));
        }
        let Ok(img) = image::open(&path) else {
            continue;
        };
        if let Some(warning) = crate::library::template_structure_warning(&img) {
            warnings.push(format!("{}: {warning}", task.name));
        }
    }
    warnings
}

fn task_out_paths(state: &ScenarioEditorState, dir: &Path) -> Vec<PathBuf> {
    state
        .tasks
        .iter()
        .map(|task| {
            let stem = state
                .loaded_task_files
                .iter()
                .find(|(n, _)| n == &task.name)
                .map(|(_, stem)| stem.as_str())
                .unwrap_or(task.name.as_str());
            dir.join(format!("{stem}.toml"))
        })
        .collect()
}

fn check_unique_out_paths(
    state: &ScenarioEditorState,
    out_paths: &[PathBuf],
) -> Result<(), ScenarioSaveError> {
    for (i, path_a) in out_paths.iter().enumerate() {
        for (j, path_b) in out_paths.iter().enumerate().skip(i + 1) {
            if path_eq_ignore_case(path_a, path_b) {
                let name = |idx: usize| {
                    state
                        .tasks
                        .get(idx)
                        .map_or_else(String::new, |t| t.name.clone())
                };
                return Err(ScenarioSaveError::TaskFileCollision {
                    path: path_b.clone(),
                    first: name(i),
                    second: name(j),
                });
            }
        }
    }
    Ok(())
}

fn path_eq_ignore_case(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_ascii_lowercase() == b.to_string_lossy().to_ascii_lowercase()
}

fn is_owned_dir(state: &ScenarioEditorState, dir: &Path) -> bool {
    state.loaded_from.as_deref().is_some_and(|src| {
        match (std::fs::canonicalize(src), std::fs::canonicalize(dir)) {
            (Ok(a), Ok(b)) => a == b,
            _ => src == dir,
        }
    })
}

fn sweep_removed_taskdefs(dir: &Path, written: &[PathBuf]) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let written_lower: Vec<String> = written
        .iter()
        .map(|w| w.to_string_lossy().to_ascii_lowercase())
        .collect();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_toml = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("toml"));
        let is_manifest = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == anaden_vision::PIPELINE_MANIFEST_FILENAME);
        let is_written = written_lower.contains(&path.to_string_lossy().to_ascii_lowercase());
        if is_toml && !is_manifest && !is_written {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use anaden_core::{Goal, StopCondition};
    use anaden_vision::{Action, Algorithm, TaskDef};
    use std::fs;

    fn task_def(name: &str, next: Option<Vec<&str>>) -> TaskDef {
        TaskDef {
            name: name.to_string(),
            state: "Field".to_string(),
            algorithm: Algorithm::Ccoeff,
            template: std::path::PathBuf::from(format!("{}.png", name.to_lowercase())),
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

    #[test]
    fn saved_scenario_roundtrips_through_anaden_vision_load() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("my_pipeline");
        fs::create_dir_all(&dir).expect("mkdir");

        let mut st = ScenarioEditorState::new("my_pipeline");
        st.add_task(task_def("Start", Some(vec!["End"])));
        st.add_task(TaskDef {
            roi: None,
            ..task_def("End", Some(vec![]))
        });
        st.add_goal(loop_goal("loop3", 3));
        st.add_goal(Goal {
            name: "combo".to_string(),
            stop: StopCondition::Any {
                conditions: vec![
                    StopCondition::TemplateMatch {
                        task: "End".to_string(),
                        confidence: 0.85,
                    },
                    StopCondition::Timeout { secs: 600 },
                ],
            },
        });
        st.validate().expect("scenario must be valid");

        let manifest = st.to_manifest();
        anaden_vision::save_pipeline_manifest(&manifest, &dir).expect("save manifest");
        for t in &st.tasks {
            anaden_vision::save_task_def(t, &dir.join(format!("{}.toml", t.name)))
                .expect("save task");
        }

        let loaded_manifest = anaden_vision::load_pipeline_manifest(&dir).expect("load manifest");
        assert_eq!(loaded_manifest, manifest);
        let defs = anaden_vision::load_pipeline(&dir).expect("load tasks");
        assert_eq!(defs.len(), 2, "pipeline.toml (manifest) must be skipped");
        let start = defs.iter().find(|d| d.name == "Start").expect("Start");
        assert_eq!(
            start.next.as_deref(),
            Some(&["End".to_string()][..]),
            "next chain survives round-trip"
        );
        assert!(start.template.is_absolute());
        assert!(
            start.template.ends_with("start.png"),
            "relative template preserved: {:?}",
            start.template
        );
        let end = defs.iter().find(|d| d.name == "End").expect("End");
        assert_eq!(end.roi, None);
        assert_eq!(end.action, Some(Action::ClickSelf));
    }

    fn dir_snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out: Vec<_> = fs::read_dir(dir)
            .expect("read_dir")
            .flatten()
            .map(|e| {
                let p = e.path();
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                (name, fs::read(&p).unwrap())
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn save_refuses_stem_collision_writing_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pipelines_root = tmp.path().join("pipelines");

        let mut st = ScenarioEditorState::new("field_hud");
        st.add_task(task_def("FieldHudWide", Some(vec![])));
        st.loaded_task_names = vec!["FieldHudWide".to_string()];
        st.loaded_task_files = vec![("FieldHudWide".to_string(), "FieldHudTr".to_string())];
        st.add_goal(loop_goal("loop3", 3));
        let dir = save_scenario(&st, &[], &pipelines_root).expect("initial save");
        let before = dir_snapshot(&dir);

        let mut st2 = crate::scenario_load::load_scenario_from_dir(&dir).expect("load");
        st2.add_task(task_def("FieldHudTr", Some(vec![])));
        assert!(
            st2.validate().is_ok(),
            "stem 衝突は既存バリデーションでは検出できないこと: {:?}",
            st2.validation_issues()
        );

        let err = save_scenario(&st2, &[], &pipelines_root).expect_err("must refuse");
        assert!(
            matches!(
                err,
                ScenarioSaveError::TaskFileCollision { ref path, .. }
                    if path.file_name().is_some_and(|n| n == "FieldHudTr.toml")
            ),
            "err: {err:?}"
        );
        assert_eq!(dir_snapshot(&dir), before, "1 バイトも書かない");
    }

    #[test]
    fn save_refuses_case_variant_stem_collision() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pipelines_root = tmp.path().join("pipelines");

        let mut st = ScenarioEditorState::new("s");
        st.add_task(task_def("StartPc", Some(vec![])));
        st.add_task(task_def("StartPcSub", Some(vec![])));
        st.loaded_task_names = vec!["StartPc".to_string(), "StartPcSub".to_string()];
        st.loaded_task_files = vec![
            ("StartPc".to_string(), "StartPc".to_string()),
            ("StartPcSub".to_string(), "startpc".to_string()),
        ];
        st.add_goal(loop_goal("g", 1));
        assert!(st.validate().is_ok());

        let err = save_scenario(&st, &[], &pipelines_root).expect_err("must refuse");
        assert!(
            matches!(err, ScenarioSaveError::TaskFileCollision { .. }),
            "err: {err:?}"
        );
        assert!(
            !pipelines_root.join("s").exists(),
            "保存先ディレクトリすら作らない"
        );
    }

    #[test]
    fn sweep_keeps_case_variant_of_written_task_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("p");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join("Alpha.toml"), b"a").expect("write");
        fs::write(dir.join("Beta.toml"), b"b").expect("write");

        sweep_removed_taskdefs(&dir, &[dir.join("beta.toml")]);

        assert!(
            dir.join("Beta.toml").exists(),
            "ケース違い stem は sweep されない"
        );
        assert!(
            !dir.join("Alpha.toml").exists(),
            "完全不一致の旧ファイルは sweep される"
        );
    }

    fn gradient_image(w: u32, h: u32) -> DynamicImage {
        let mut img = image::GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = ((x * 2 + y * 3) % 200) as u8;
                img.put_pixel(x, y, image::Luma([v]));
            }
        }
        DynamicImage::ImageLuma8(img)
    }

    fn flat_image(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageLuma8(image::GrayImage::from_pixel(w, h, image::Luma([250u8])))
    }

    #[test]
    fn save_scenario_with_warnings_flags_flat_pending_png_but_saves() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("pipelines");

        let mut st = ScenarioEditorState::new("warn_flat");
        st.add_task(task_def("Start", Some(vec![])));
        st.add_goal(loop_goal("g", 3));
        let pngs = vec![("Start".to_string(), flat_image(100, 50))];

        let outcome = save_scenario_with_warnings(&st, &pngs, &root).expect("save must succeed");
        assert_eq!(outcome.dir, root.join("warn_flat"));
        assert_eq!(
            outcome.warnings.len(),
            1,
            "無構造 pending PNG に警告 1 件: {:?}",
            outcome.warnings
        );
        assert!(
            outcome.warnings[0].contains("Start"),
            "警告は対象タスク名を含む: {}",
            outcome.warnings[0]
        );
        assert!(
            outcome.warnings[0].contains("認識不能"),
            "警告は認識不能の恐れを明示: {}",
            outcome.warnings[0]
        );
        assert!(outcome.dir.join("Start.png").exists());
    }

    #[test]
    fn save_scenario_with_warnings_structured_png_has_no_warning() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("pipelines");

        let mut st = ScenarioEditorState::new("warn_ok");
        st.add_task(task_def("Start", Some(vec![])));
        st.add_goal(loop_goal("g", 3));
        let pngs = vec![("Start".to_string(), gradient_image(100, 50))];

        let outcome = save_scenario_with_warnings(&st, &pngs, &root).expect("save must succeed");
        assert!(
            outcome.warnings.is_empty(),
            "構造ありテンプレートに警告を出さない: {:?}",
            outcome.warnings
        );
    }

    #[test]
    fn save_scenario_with_warnings_flags_flat_uc2_reference_png() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("pipelines");

        let lib_dir = tmp.path().join("lib");
        fs::create_dir_all(&lib_dir).expect("mkdir lib");
        let flat_ref = lib_dir.join("flat_ref.png");
        flat_image(64, 32).save(&flat_ref).expect("save flat ref");
        let structured_ref = lib_dir.join("structured_ref.png");
        gradient_image(64, 32)
            .save(&structured_ref)
            .expect("save structured ref");

        let mut st = ScenarioEditorState::new("warn_uc2");
        let mut flat_task = task_def("FlatRef", Some(vec![]));
        flat_task.template = flat_ref;
        let mut ok_task = task_def("OkRef", Some(vec![]));
        ok_task.template = structured_ref;
        st.add_task(flat_task);
        st.add_task(ok_task);
        st.add_goal(loop_goal("g", 1));

        let outcome = save_scenario_with_warnings(&st, &[], &root).expect("save must succeed");
        assert_eq!(
            outcome.warnings.len(),
            1,
            "無構造参照のみ警告 (構造あり参照は警告なし): {:?}",
            outcome.warnings
        );
        assert!(
            outcome.warnings[0].contains("FlatRef"),
            "警告は対象タスク名を含む: {}",
            outcome.warnings[0]
        );
    }

    #[test]
    fn save_scenario_with_warnings_skips_missing_reference_and_wrapper_keeps_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("pipelines");

        let mut st = ScenarioEditorState::new("warn_ghost");
        let mut ghost = task_def("Ghost", Some(vec![]));
        ghost.template = PathBuf::from("does_not_exist.png");
        st.add_task(ghost);
        st.add_goal(loop_goal("g", 1));

        let outcome = save_scenario_with_warnings(&st, &[], &root).expect("save must succeed");
        assert!(
            outcome.warnings.is_empty(),
            "参照先不在は警告しない: {:?}",
            outcome.warnings
        );

        st.loaded_from = Some(outcome.dir.clone());
        let dir = save_scenario(&st, &[], &root).expect("wrapper save");
        assert_eq!(dir, root.join("warn_ghost"));
    }

    #[test]
    fn save_scenario_with_warnings_flags_oversized_pending_png() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("pipelines");

        let mut st = ScenarioEditorState::new("warn_fit");
        let mut task = task_def("Big", Some(vec![]));
        task.roi = Some([10, 20, 60, 40]);
        st.add_task(task);
        st.add_goal(loop_goal("g", 3));
        let pngs = vec![("Big".to_string(), gradient_image(100, 50))];

        let outcome = save_scenario_with_warnings(&st, &pngs, &root).expect("save must succeed");
        assert_eq!(
            outcome.warnings.len(),
            1,
            "オーバーサイズ needle に警告 1 件 (構造ありなので stddev 警告は無し): {:?}",
            outcome.warnings
        );
        assert!(
            outcome.warnings[0].contains("Big"),
            "警告は対象タスク名を含む: {}",
            outcome.warnings[0]
        );
        assert!(
            outcome.warnings[0].contains("100x50") && outcome.warnings[0].contains("60x40"),
            "警告は needle/roi 両寸法を含む: {}",
            outcome.warnings[0]
        );
        assert!(
            outcome.warnings[0].contains("恒久 NoMatch"),
            "警告は恒久 NoMatch を明示: {}",
            outcome.warnings[0]
        );
        assert!(outcome.dir.join("Big.png").exists());
    }

    #[test]
    fn save_scenario_with_warnings_fitting_pending_png_has_no_roi_warning() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("pipelines");

        let mut st = ScenarioEditorState::new("warn_fit_ok");
        st.add_task(task_def("Fits", Some(vec![])));
        st.add_goal(loop_goal("g", 3));
        let pngs = vec![("Fits".to_string(), gradient_image(100, 50))];

        let outcome = save_scenario_with_warnings(&st, &pngs, &root).expect("save must succeed");
        assert!(
            outcome.warnings.is_empty(),
            "収まる needle に警告を出さない: {:?}",
            outcome.warnings
        );
    }

    #[test]
    fn save_scenario_with_warnings_fullscreen_roi_never_flags() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("pipelines");

        let mut st = ScenarioEditorState::new("warn_fit_full");
        let mut task = task_def("Full", Some(vec![]));
        task.roi = None;
        st.add_task(task);
        st.add_goal(loop_goal("g", 1));
        let pngs = vec![("Full".to_string(), gradient_image(4000, 2000))];

        let outcome = save_scenario_with_warnings(&st, &pngs, &root).expect("save must succeed");
        assert!(
            outcome.warnings.is_empty(),
            "roi None (全面) は needle/roi 警告の対象外: {:?}",
            outcome.warnings
        );
    }

    #[test]
    fn save_scenario_with_warnings_flags_oversized_uc2_reference_png() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("pipelines");

        let lib_dir = tmp.path().join("lib");
        fs::create_dir_all(&lib_dir).expect("mkdir lib");
        let wide_ref = lib_dir.join("wide_ref.png");
        gradient_image(138, 20)
            .save(&wide_ref)
            .expect("save wide ref");

        let mut st = ScenarioEditorState::new("warn_uc2_fit");
        let mut task = task_def("WideRef", Some(vec![]));
        task.template = wide_ref;
        task.roi = Some([8, 2, 121, 35]);
        st.add_task(task);
        st.add_goal(loop_goal("g", 1));

        let outcome = save_scenario_with_warnings(&st, &[], &root).expect("save must succeed");
        assert_eq!(
            outcome.warnings.len(),
            1,
            "roi を超える参照 PNG に警告 1 件 (構造ありなので stddev 警告は無し): {:?}",
            outcome.warnings
        );
        assert!(
            outcome.warnings[0].contains("WideRef")
                && outcome.warnings[0].contains("138x20")
                && outcome.warnings[0].contains("121x35"),
            "警告はタスク名と両寸法を含む: {}",
            outcome.warnings[0]
        );
    }
}
