//! 一時検証ヘルパ: 入力PNGを ScreenScaler と同じ方法(幅1280・Triangle)で正規化して保存。
//! run-pipeline の内部正規化と同一結果を得るため、テンプレ crop 元画像を作る。
use anaden_vision::ScreenScaler;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: normalize_shot <input.png> <output.png>");
        std::process::exit(2);
    }
    let inp = &args[1];
    let out = &args[2];
    let img = image::open(inp).expect("open input");
    let (ow, oh) = (img.width(), img.height());
    let scaler = ScreenScaler::new();
    let n = scaler.normalize(&img);
    n.save(out).expect("save output");
    println!("normalize: {}x{} -> {}x{}", ow, oh, n.width(), n.height());
}
