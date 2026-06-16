//! 指定領域の明るいブロックを探す(領域指定版)
//! usage: find_bright2 <img> <x0> <y0> <x1> <y1>
use anaden_vision::ScreenScaler;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 6 {
        eprintln!("usage: find_bright2 <img> <x0> <y0> <x1> <y1>");
        std::process::exit(2);
    }
    let img = image::open(&args[1]).expect("open");
    let scaler = ScreenScaler::new();
    let n = scaler.normalize(&img).to_rgba8();
    let x0: u32 = args[2].parse().unwrap();
    let y0: u32 = args[3].parse().unwrap();
    let x1: u32 = args[4].parse().unwrap();
    let y1: u32 = args[5].parse().unwrap();
    let bs = 8u32;
    let mut best: Vec<(u32, u32, u64)> = vec![];
    for by in (y0..y1).step_by(bs as usize) {
        for bx in (x0..x1).step_by(bs as usize) {
            let mut sum = 0u64;
            let mut cnt = 0u64;
            for y in by..(by + bs).min(n.height()) {
                for x in bx..(bx + bs).min(n.width()) {
                    let p = n.get_pixel(x, y);
                    let lum = (p[0] as u64 + p[1] as u64 + p[2] as u64) / 3;
                    sum += lum;
                    cnt += 1;
                }
            }
            best.push((bx, by, sum / cnt.max(1)));
        }
    }
    best.sort_by(|a, b| b.2.cmp(&a.2));
    eprintln!("=== brightest blocks in [{},{},{},{}] ===", x0, y0, x1, y1);
    for (bx, by, mean) in best.iter().take(15) {
        eprintln!("  px({},{}) lum={}", bx, by, mean);
    }
}
