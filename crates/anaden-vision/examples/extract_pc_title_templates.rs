use image::GenericImageView;
use image::imageops::FilterType;

fn main() {
    let probe =
        image::open("templates/captures/title_pc_probe.png").expect("open title_pc_probe.png");
    eprintln!("probe dims: {:?}", probe.dimensions());
    let p = probe.resize_exact(1258, 708, FilterType::Triangle);
    p.save("templates/captures/title_pc_probe_1258x708.png")
        .expect("save resized probe");
    eprintln!(
        "wrote templates/captures/title_pc_probe_1258x708.png (1258x708) \
         — open THIS in anaden-studio (coordinates are already PC RAW space)"
    );

    let sx = 1258.0_f64 / 1918.0;
    let sy = 708.0_f64 / 1048.0;
    let (vx, vy, vw, vh) = (
        (1086.0 * sx).round() as u32,
        (12.0 * sy).round() as u32,
        (184.0 * sx).round() as u32,
        (52.0 * sy).round() as u32,
    );
    eprintln!("version_label ROI @1258x708: [{vx},{vy},{vw},{vh}]");
    let vl = p.crop_imm(vx, vy, vw, vh);
    vl.save("templates/scenes/title_pc/version_label.png")
        .expect("save version_label.png");
    eprintln!("wrote templates/scenes/title_pc/version_label.png ({vw}x{vh})");

    let (lx, ly, lw, lh) = (624u32, 263u32, 120u32, 120u32);
    eprintln!("title_logo_corner ROI @1258x708: [{lx},{ly},{lw},{lh}] (small sub-feature)");
    let lc = p.crop_imm(lx, ly, lw, lh);
    lc.save("templates/scenes/title_pc/title_logo_corner.png")
        .expect("save title_logo_corner.png");
    eprintln!("wrote templates/scenes/title_pc/title_logo_corner.png ({lw}x{lh})");
}
