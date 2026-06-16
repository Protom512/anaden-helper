//! 一時検証ヘルパ: 入力PNG(既に正規化済想定)の指定矩形を crop して保存。
//! usage: crop_region <input.png> <output.png> <x,y,w,h>
use image::DynamicImage;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: crop_region <input.png> <output.png> <x,y,w,h>");
        std::process::exit(2);
    }
    let inp = &args[1];
    let out = &args[2];
    let parts: Vec<u32> = args[3]
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    if parts.len() != 4 {
        eprintln!("roi must be x,y,w,h");
        std::process::exit(2);
    }
    let img = image::open(inp).expect("open input");
    let (x, y, w, h) = (parts[0], parts[1], parts[2], parts[3]);
    let cropped = img.crop_imm(x, y, w, h);
    let c: &DynamicImage = &cropped;
    let _ = c;
    cropped.save(Path::new(out)).expect("save output");
    println!("crop: [{},{},{},{}] -> {}x{} saved", x, y, w, h, cropped.width(), cropped.height());
}
