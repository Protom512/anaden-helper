use std::path::Path;

use anaden_vision::{ScreenScaler, load_pipeline};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let fixture = args
        .next()
        .ok_or("usage: probe_matrix <fixture.png> <dir>...")?;
    let raw = image::open(&fixture)?;
    let (normalized, crop) = ScreenScaler::new().normalize_capture(&raw);
    println!(
        "fixture={fixture} raw={}x{} normalized={}x{} crop_offset=({}, {})",
        raw.width(),
        raw.height(),
        normalized.width(),
        normalized.height(),
        crop.offset_x,
        crop.offset_y
    );
    for dir in args {
        let tasks = load_pipeline(Path::new(&dir))?;
        println!("dir={dir} tasks={}", tasks.len());
        for task in tasks {
            let matched = task.detect(&normalized, Path::new(""))?;
            match matched {
                Some(m) => println!(
                    "  {:32} conf={:.4} thr={:.2} region=[{},{},{},{}]",
                    task.name,
                    m.confidence.0,
                    task.threshold,
                    m.region.x,
                    m.region.y,
                    m.region.width,
                    m.region.height
                ),
                None => println!("  {:32} nomatch thr={:.2}", task.name, task.threshold),
            }
        }
    }
    Ok(())
}
