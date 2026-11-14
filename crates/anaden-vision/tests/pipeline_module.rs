use std::fs;
use std::path::Path;

use anaden_vision::{Algorithm, Pipeline, TaskDef};
use image::{DynamicImage, GrayImage, Luma};

fn gradient(w: u32, h: u32) -> GrayImage {
    let mut img = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            img.put_pixel(x, y, Luma([((x + y) % 64) as u8]));
        }
    }
    img
}

fn embed(w: u32, h: u32, needle: &GrayImage, ox: u32, oy: u32) -> DynamicImage {
    let mut img = GrayImage::from_pixel(w, h, Luma([128]));
    for y in 0..needle.height() {
        for x in 0..needle.width() {
            img.put_pixel(ox + x, oy + y, *needle.get_pixel(x, y));
        }
    }
    DynamicImage::ImageLuma8(img)
}

fn write_task(dir: &Path, file: &str, body: &str) {
    fs::write(dir.join(file), body).expect("write task toml");
}

fn task_row(name: &str, template: &str, roi: &str) -> String {
    format!(
        "name = \"{name}\"\n\
         state = \"field\"\n\
         algorithm = \"ccoeff\"\n\
         template = \"{template}\"\n\
         roi = {roi}\n\
         threshold = 0.9\n\
         action = {{ type = \"do_nothing\" }}\n\
         next = []\n"
    )
}

#[test]
fn load_owns_manifest_and_watch_layer() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    let watch = dir.join("watch");
    fs::create_dir(&watch).expect("watch dir");
    write_task(
        dir,
        "main.toml",
        &task_row("Main", "main.png", "[0, 0, 0, 0]"),
    );
    write_task(
        &watch,
        "probe.toml",
        &task_row("Probe", "probe.png", "[0, 0, 0, 0]"),
    );
    fs::write(
        dir.join("pipeline.toml"),
        "start_task = \"Main\"\nwatch = [\"Probe\"]\n",
    )
    .expect("manifest");

    let pipeline = Pipeline::load(dir).expect("load");

    assert_eq!(pipeline.tasks().len(), 1);
    assert_eq!(pipeline.tasks()[0].name(), "Main");
    assert_eq!(pipeline.watch_tasks().len(), 1);
    assert_eq!(pipeline.watch_tasks()[0].name(), "Probe");
    assert_eq!(pipeline.watch_names(), ["Probe".to_string()]);
    assert_eq!(pipeline.start_task(), Some("Main"));

    assert!(pipeline.task("Probe").is_some());
    assert_eq!(
        pipeline.defs().map(|d| d.name.as_str()).collect::<Vec<_>>(),
        ["Main"]
    );
}

#[test]
fn step_resolves_relative_template_against_pipeline_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    let needle = gradient(40, 40);
    DynamicImage::ImageLuma8(needle.clone())
        .save(dir.join("main.png"))
        .expect("save template");
    write_task(
        dir,
        "main.toml",
        &task_row("Main", "main.png", "[0, 0, 0, 0]"),
    );

    let pipeline = Pipeline::load(dir).expect("load");
    let screenshot = embed(1258, 708, &needle, 600, 340);

    let hit = pipeline
        .step(&screenshot, "Main")
        .expect("step")
        .expect("must match");

    assert_eq!(hit.matched_task, "Main");
    assert!(
        hit.matched_confidence > 0.9,
        "conf={}",
        hit.matched_confidence
    );
    assert_eq!(hit.next, Vec::<String>::new());
    assert!(pipeline
        .step(&screenshot, "Unknown")
        .expect("step")
        .is_none());
}

#[test]
fn save_task_def_writes_relative_template_and_reloads_absolute() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path();
    let needle = gradient(20, 20);
    let png = dir.join("tpl.png");
    DynamicImage::ImageLuma8(needle)
        .save(&png)
        .expect("save png");

    let def = TaskDef {
        name: "T".to_string(),
        state: "field".to_string(),
        algorithm: Algorithm::Ccoeff,
        template: png.clone(),
        roi: None,
        threshold: 0.8,
        base: None,
        action: None,
        next: None,
    };
    anaden_vision::save_task_def(&def, &dir.join("T.toml")).expect("save");

    let body = fs::read_to_string(dir.join("T.toml")).expect("read saved");
    assert!(
        body.contains("template = \"tpl.png\""),
        "saved body must be relative:\n{body}"
    );
    assert!(body.contains("threshold = 0.8"), "body:\n{body}");

    let loaded = anaden_vision::load_pipeline(dir).expect("reload");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].template, png);
}
