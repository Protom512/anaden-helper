//! One-off helper (Issue #12 Branch A): resize `title_pc_probe.png` (captured at
//! 1918x1048 RGBA — the PrintWindow size, not operator-controllable) to PC RAW
//! 1258x708 (same Triangle path as the E2E test) and extract the small
//! sub-templates at the real-probe ROIs. Also writes the resized probe for
//! anaden-studio use (so the operator can drag at 1258x708 space).
//!
//! Re-derivation: `cargo run -p anaden-vision --example extract_pc_title_templates`.
//! Inputs: `templates/captures/title_pc_probe.png`. Outputs:
//! `templates/scenes/title_pc/{version_label,title_logo_corner}.png` +
//! `templates/captures/title_pc_probe_1258x708.png` (gitignored, regenerable).
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

    // operator-dragged ROI on the original 1918x1048: version_label x=1086,y=12,w=184,h=52
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

    // title_logo_corner: a *small* stable sub-feature (<=130px each side) inside the
    // operator-dragged logo band, NOT the full 932x405 band. The full band exceeds
    // the small-template ceiling (roi[2]<=130 && roi[3]<=130) and is background-diff
    // sensitive (TASKS.md:30-33). find_logo_corner_subfeature.rs scanned the band for
    // the highest texture-energy 120x120 window; the winner [624,263,120,120] sits on
    // the title glyphs (stable, high contrast). This is DEFER-option (b): re-crop into
    // a stable small feature. We re-crop from the SAME 1258x708 probe so the template
    // is guaranteed to be in the test's coordinate space.
    let (lx, ly, lw, lh) = (624u32, 263u32, 120u32, 120u32);
    eprintln!("title_logo_corner ROI @1258x708: [{lx},{ly},{lw},{lh}] (small sub-feature)");
    let lc = p.crop_imm(lx, ly, lw, lh);
    lc.save("templates/scenes/title_pc/title_logo_corner.png")
        .expect("save title_logo_corner.png");
    eprintln!("wrote templates/scenes/title_pc/title_logo_corner.png ({lw}x{lh})");
}
