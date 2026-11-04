use anaden_vision::ScreenScaler;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let img = image::open(&args[1]).expect("open");
    let scaler = ScreenScaler::new();
    let n = scaler.normalize(&img).to_rgba8();
    let (w, h) = (n.width(), n.height());
    let bs = 8u32;
    let mut best: Vec<(u32, u32, u64)> = vec![];
    for by in (0..120u32).step_by(bs as usize) {
        for bx in (1080..w).step_by(bs as usize) {
            let mut sum = 0u64;
            let mut cnt = 0u64;
            for y in by..(by + bs).min(h) {
                for x in bx..(bx + bs).min(w) {
                    let p = n.get_pixel(x, y);
                    let lum = (p[0] as u64 + p[1] as u64 + p[2] as u64) / 3;
                    sum += lum;
                    cnt += 1;
                }
            }
            let mean = sum / cnt.max(1);
            if mean > 100 {
                best.push((bx, by, mean));
            }
        }
    }
    best.sort_by_key(|x| std::cmp::Reverse(x.2));
    eprintln!("=== bright blocks in top-right (x>1080, y<120) ===");
    for (bx, by, mean) in best.iter().take(20) {
        eprintln!("  block px({},{}) size8 mean_lum={}", bx, by, mean);
    }
}
