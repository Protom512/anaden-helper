//! キャプチャ幾何ドリフト診断 (Issue #212 由来): 実ショットを複数経路の前処理に
//! 通し、roi 付き detect と roi なし全文探索 (最良 conf 含む) を突き合わせて出力する。
//!
//! 経路:
//!   A: raw (無前処理)
//!   B: crop_to_content_with_info のみ (素の黒帯クロップ)
//!   C: normalize_capture (本番前処理 = crop_to_canvas_with_info + normalize・engine と
//!      anaden-tool run-pipeline が使う単一関数)
//!   D: normalize のみ (旧 tool run-pipeline 相当・Issue #212 修正前の再現用)
//!
//! あわせて行平均輝度プロファイル (内容領域の特定) を出力する。
//!
//! usage (全文探索が重いので --release 推奨):
//!   cargo run --release -p anaden-vision --example issue212_staged -- <shot.png>

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

/// roi 付き (TOML どおり) と roi なし全文探索 (threshold=0 で最良 conf まで) を出力。
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

/// raw 画像の行平均輝度プロファイル (内容領域・未描画黒の特定用)。
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

    // A: raw 無前処理
    probe("A raw", &raw, &task);

    // B: 素の黒帯クロップ (キャンバス内部の黒も除去してしまう旧来の挙動)
    let (cropped, info) = crop_to_content_with_info(&raw);
    println!(
        "B crop_to_content: offset=({},{}) size={}x{}",
        info.offset_x, info.offset_y, info.width, info.height
    );
    probe("B crop-only", &cropped, &task);

    // C: 本番前処理 (normalize_capture = アスペクト保護付きクロップ + 1280 正規化)
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

    // D: normalize のみ (Issue #212 修正前の tool run-pipeline 相当)
    let tool_normalized = ScreenScaler::new().normalize(&raw);
    probe("D normalize-only (pre-#212 tool)", &tool_normalized, &task);
}
