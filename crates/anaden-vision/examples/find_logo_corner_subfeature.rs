use image::GrayImage;
use image::imageops::FilterType;

fn window_energy(p: &GrayImage, x: u32, y: u32, win: u32) -> u64 {
    let mut energy: u64 = 0;
    for yy in y..y + win {
        for xx in x..x + win - 1 {
            let a = p.get_pixel(xx, yy).0[0] as i32;
            let b = p.get_pixel(xx + 1, yy).0[0] as i32;
            energy += (a - b).unsigned_abs() as u64;
        }
    }
    for yy in y..y + win - 1 {
        for xx in x..x + win {
            let a = p.get_pixel(xx, yy).0[0] as i32;
            let b = p.get_pixel(xx, yy + 1).0[0] as i32;
            energy += (a - b).unsigned_abs() as u64;
        }
    }
    energy
}

fn main() {
    let probe =
        image::open("templates/captures/title_pc_probe.png").expect("open title_pc_probe.png");
    let p = probe
        .resize_exact(1258, 708, FilterType::Triangle)
        .to_luma8();

    let (bx0, by0, bx1, by1) = (164u32, 63u32, 1096u32, 468u32);
    let win = 120u32;

    let mut best = (0u64, 0u32, 0u32);
    let mut confs = Vec::new();
    for y in (by0..by1.saturating_sub(win)).step_by(20) {
        for x in (bx0..bx1.saturating_sub(win)).step_by(20) {
            let energy = window_energy(&p, x, y, win);
            confs.push((energy, x, y));
            if energy > best.0 {
                best = (energy, x, y);
            }
        }
    }
    confs.sort_by_key(|b| std::cmp::Reverse(b.0));
    eprintln!("top-5 highest-energy {win}x{win} windows in logo band:");
    for (e, x, y) in confs.iter().take(5) {
        eprintln!("  energy={e} roi=[{x},{y},{win},{win}]");
    }
    let (_, x, y) = best;
    eprintln!("BEST title_logo_corner sub-feature roi = [{x},{y},{win},{win}]");
}
