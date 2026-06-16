//! 検証ヘルパ: haystack(実キャプチャ)を ScreenScaler で1280正規化し、
//! 720p基準テンプレ needle で CcoeffVisionEngine(TM_CCOEFF_NORMED) マッチング。
//! run-pipeline の内部挙動と同一条件で、座標・スコアを実値出力する。
//! ROI 指定時は haystack を crop してからマッチ(roi_to_region と同等)。
use anaden_core::{MatchConfidence, ScreenRegion};
use anaden_vision::{CcoeffVisionEngine, ScreenScaler, VisionEngine};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: probe_ccoeff <haystack.png> <template.png> <threshold> [roi x,y,w,h]");
        std::process::exit(2);
    }
    let raw = image::open(&args[1]).expect("open haystack");
    let needle = image::open(&args[2]).expect("open template");
    let thr: f32 = args[3].parse().expect("threshold f32");

    let scaler = ScreenScaler::new();
    let mut haystack = scaler.normalize(&raw);
    let (ow, oh) = (raw.width(), raw.height());
    println!("haystack: {}x{} -> {}x{} (720p/1280)", ow, oh, haystack.width(), haystack.height());
    println!("template: {}x{} (720p-space)", needle.width(), needle.height());

    let mut offset = (0u32, 0u32);
    if let Some(roi_str) = args.get(4) {
        let parts: Vec<u32> = roi_str.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if parts.len() == 4 {
            let r = ScreenRegion::new(parts[0], parts[1], parts[2], parts[3]);
            offset = (r.x, r.y);
            let hw = haystack.width();
            let hh = haystack.height();
            let x2 = (r.x + r.width).min(hw);
            let y2 = (r.y + r.height).min(hh);
            let xs = r.x.min(hw);
            let ys = r.y.min(hh);
            if x2 > xs && y2 > ys {
                haystack = haystack.crop_imm(xs, ys, x2 - xs, y2 - ys);
                println!("roi crop: [{},{},{},{}] -> {}x{} (offset {},{})", r.x, r.y, r.width, r.height, haystack.width(), haystack.height(), offset.0, offset.1);
            }
        }
    }

    let engine = CcoeffVisionEngine::threshold_only(MatchConfidence::new(thr));
    match engine.match_template(&haystack, &needle) {
        Some(m) => {
            let r = &m.region;
            println!(
                "MATCH: conf={:.4} region=[{},{},{},{}] (roi offset +{},{})",
                m.confidence.0, r.x, r.y, r.width, r.height, offset.0, offset.1
            );
            println!("abs-center(720p): ({},{})", r.x + r.width / 2 + offset.0, r.y + r.height / 2 + offset.1);
        }
        None => println!("NO MATCH (below threshold {:.2})", thr),
    }
}
