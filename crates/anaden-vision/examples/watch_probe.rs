use std::path::Path;

use anaden_vision::{ScreenScaler, load_watch_defs};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: watch_probe <capture.png> <pipeline_dir> [task_name]");
        std::process::exit(2);
    }
    let raw = image::open(&args[1]).expect("open capture");
    let dir = Path::new(&args[2]);
    let (normalized, crop) = ScreenScaler::new().normalize_capture(&raw);
    println!(
        "capture: {}x{} -> {}x{} (content {}x{} offset {},{})",
        raw.width(),
        raw.height(),
        normalized.width(),
        normalized.height(),
        crop.width,
        crop.height,
        crop.offset_x,
        crop.offset_y
    );
    let defs = load_watch_defs(dir).expect("load watch defs");
    let filter = args.get(3);
    let threshold_override: Option<f32> = args.get(4).and_then(|v| v.parse().ok());
    for def in defs {
        if filter.is_some_and(|name| name != &def.name) {
            continue;
        }
        let mut probe = def.clone();
        if let Some(thr) = threshold_override {
            probe.threshold = thr;
        }
        let template_root = Path::new("");
        match probe.detect(&normalized, template_root) {
            Ok(Some(m)) => println!(
                "MATCH {} conf={:.4} threshold={:.2} region=[{},{},{},{}]",
                probe.name,
                m.confidence.0,
                probe.threshold,
                m.region.x,
                m.region.y,
                m.region.width,
                m.region.height
            ),
            Ok(None) => println!("NO MATCH {} threshold={:.2}", probe.name, probe.threshold),
            Err(e) => println!("ERROR {} {e}", probe.name),
        }
    }
}
