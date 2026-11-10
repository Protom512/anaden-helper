use std::error::Error;
use std::path::PathBuf;

use image::imageops::FilterType;

fn parse_usize(s: &str) -> Result<usize, String> {
    s.parse::<usize>()
        .map_err(|_| format!("invalid number: {s}"))
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 8 {
        return Err(
            "usage: cut_needle <screenshot.png> <x> <y> <w> <h> <out_w> <out_h> <output.png>\
             \nCrops the raw-pixel box and resizes to the raw-1258 coordinate space \
             (the needle storage space), with the production needle filter (Lanczos3)."
                .into(),
        );
    }
    let screenshot = PathBuf::from(&args[0]);
    let x = parse_usize(&args[1])?;
    let y = parse_usize(&args[2])?;
    let w = parse_usize(&args[3])?;
    let h = parse_usize(&args[4])?;
    let out_w = parse_usize(&args[5])?;
    let out_h = parse_usize(&args[6])?;
    let output = PathBuf::from(&args[7]);

    let img = image::open(&screenshot)?;
    if x + w > img.width() as usize || y + h > img.height() as usize {
        return Err(format!(
            "box ({x},{y},{w},{h}) exceeds screenshot {}x{}",
            img.width(),
            img.height()
        )
        .into());
    }
    let needle = img
        .crop_imm(x as u32, y as u32, w as u32, h as u32)
        .resize_exact(out_w as u32, out_h as u32, FilterType::Lanczos3);
    needle.save_with_format(&output, image::ImageFormat::Png)?;
    println!(
        "cut_needle: {} ({},{} {}x{}) -> {} ({}x{}, 1258-space)",
        screenshot.display(),
        x,
        y,
        w,
        h,
        output.display(),
        out_w,
        out_h
    );
    Ok(())
}
