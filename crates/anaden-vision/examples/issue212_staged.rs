use std::path::Path;

use anaden_vision::{MatchResult, ScreenScaler, TaskDef, crop_to_content_with_info, load_pipeline};

fn workspace_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn login_task() -> TaskDef {
    let dir = workspace_root()
        .join("templates")
        .join("pipelines")
        .join("login");
    let tasks = load_pipeline(&dir).expect("load login pipeline");
    tasks
        .into_iter()
        .find(|t| t.name == "LoginTapTitlePc")
        .expect("LoginTapTitlePc exists")
}

fn fmt_match(r: &Option<MatchResult>) -> String {
    match r {
        Some(m) => format!(
            "conf={:.4} at ({},{}) {}x{}",
            m.confidence.0, m.region.x, m.region.y, m.region.width, m.region.height
        ),
        None => "None".to_string(),
    }
}

fn probe(label: &str, frame: &image::DynamicImage, task: &TaskDef) {
    let (w, h) = (frame.width(), frame.height());
    let with_roi = task.detect(frame, Path::new("")).expect("detect with roi");
    let mut full = task.clone();
    full.roi = None;
    full.threshold = 0.0;
    let best = full.detect(frame, Path::new("")).expect("detect best-conf");
    println!(
        "[{label}] frame={w}x{h} roi={:?} thr={}: {} | best(full-scan): {}",
        task.roi,
        task.threshold,
        fmt_match(&with_roi),
        fmt_match(&best)
    );
}

fn row_profile(raw: &image::DynamicImage) {
    let rgb = raw.to_rgb8();
    let (w, h) = rgb.dimensions();
    let lum = |p: &image::Rgb<u8>| 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
    let row_mean = |y: u32| (0..w).map(|x| lum(rgb.get_pixel(x, y))).sum::<f64>() / w as f64;
    let first = (0..h).find(|&y| row_mean(y) >= 8.0);
    let last = (0..h).rev().find(|&y| row_mean(y) >= 8.0);
    println!(
        "profile: rows with mean>=8: first={first:?} last={last:?} (h={h}; bottom black rows = {})",
        match last {
            Some(l) => h - 1 - l,
            None => h,
        }
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("usage: issue212_staged <shot.png>");
        std::process::exit(2);
    }
    let raw = image::open(&args[1]).expect("open shot");
    println!("shot: {}x{} {:?}", raw.width(), raw.height(), args[1]);

    row_profile(&raw);

    let task = login_task();

    probe("A raw", &raw, &task);

    let (cropped, info) = crop_to_content_with_info(&raw);
    println!(
        "B crop_to_content: offset=({},{}) size={}x{}",
        info.offset_x, info.offset_y, info.width, info.height
    );
    probe("B crop-only", &cropped, &task);

    let (normalized, crop_info) = ScreenScaler::new().normalize_capture(&raw);
    println!(
        "C normalize_capture: content {}x{} (offset {},{}) -> {}x{}",
        crop_info.width,
        crop_info.height,
        crop_info.offset_x,
        crop_info.offset_y,
        normalized.width(),
        normalized.height()
    );
    probe("C normalize_capture (production)", &normalized, &task);

    let tool_normalized = ScreenScaler::new().normalize(&raw);
    probe("D normalize-only (pre-#212 tool)", &tool_normalized, &task);
}
