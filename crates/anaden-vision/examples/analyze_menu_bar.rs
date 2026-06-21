//! 一時解析ヘルパ: menu_pc_probe.png の下部 7 アイコン(右クラスタ)を決定論的に検出し、
//! 各アイコンの [x,y,w,h] ROI (RAW 1258x708 空間) と RGBA 自己クロップ PNG を出力。
//! usage: analyze_menu_bar <input.png> <out_dir> <n0,n1,...,n6>
use image::{DynamicImage, GenericImageView, RgbaImage};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: analyze_menu_bar <input.png> <out_dir> <n0,n1,...,n6>");
        std::process::exit(2);
    }
    let inp = &args[1];
    let out_dir = &args[2];
    let names: Vec<&str> = args[3].split(',').map(|s| s.trim()).collect();
    if names.len() != 7 {
        eprintln!("need exactly 7 names, got {}", names.len());
        std::process::exit(2);
    }
    let img = image::open(inp).expect("open input");
    let (w, h) = img.dimensions();
    let gray = img.to_luma8();
    eprintln!("image: {w}x{h}");

    // バー帯 y=579..652 (行スコア解析で特定済み)。上下マージン込みで y=579,h=73 を採用。
    let bar_y: u32 = 579;
    let bar_h: u32 = 73;
    let y_start = bar_y;
    let y_end = bar_y + bar_h;
    let band_h = (y_end - y_start) as usize;

    // 右クラスタ x=640..1258 の列分散を計算。
    let xs: u32 = 640;
    let xe: u32 = w;
    let mut col_var: Vec<u64> = Vec::new();
    for x in xs..xe {
        let mut vals: Vec<u8> = Vec::with_capacity(band_h);
        for y in y_start..y_end {
            vals.push(gray.get_pixel(x, y).0[0]);
        }
        let mean = vals.iter().map(|v| *v as u64).sum::<u64>() as f64 / vals.len() as f64;
        let var: u64 = vals
            .iter()
            .map(|v| {
                let d = *v as f64 - mean;
                (d * d) as u64
            })
            .sum();
        col_var.push(var);
    }
    let max_var = *col_var.iter().max().unwrap_or(&1) as f64;
    let sm: Vec<f64> = (0..col_var.len())
        .map(|i| {
            let a = col_var[i.saturating_sub(1)];
            let b = col_var[i];
            let c = col_var[(i + 1).min(col_var.len() - 1)];
            (a + b + c) as f64 / 3.0
        })
        .collect();

    // 「描画あり」連続 run を抽出(閾値 max*0.12)。
    let thr = max_var * 0.12;
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < sm.len() {
        if sm[i] > thr {
            let s = i;
            while i < sm.len() && sm[i] > thr {
                i += 1;
            }
            runs.push((xs as usize + s, xs as usize + i));
        } else {
            i += 1;
        }
    }
    // 幅 >=55 の run だけをアイコンとみなす(境界アーティファクトの w<30 を除外)。
    let icons: Vec<(usize, usize)> = runs.into_iter().filter(|(s, e)| e - s >= 55).collect();
    let centers: Vec<u32> = icons.iter().map(|(s, e)| ((*s + *e) / 2) as u32).collect();
    eprintln!("icon runs(>=55px): {} centers={:?}", icons.len(), centers);
    if centers.len() != 7 {
        panic!("expected 7 icons, detected {}", centers.len());
    }

    // 各アイコン ROI: 検出 run の x 範囲をそのまま使う(隙間を含めない)。
    std::fs::create_dir_all(out_dir).expect("mkdir out_dir");
    println!("BAR: y={bar_y} h={bar_h}");
    for (k, (s, e)) in icons.iter().enumerate() {
        let name = names[k];
        let rx = *s as u32;
        let rw = (*e - *s) as u32;
        let roi = [rx, bar_y, rw, bar_h];
        println!("{name}: {roi:?}");

        // RGBA 自己クロップ PNG 保存。
        let mut sub: RgbaImage = image::ImageBuffer::new(rw, bar_h);
        for yy in 0..bar_h {
            for xx in 0..rw {
                let p = img.get_pixel(rx + xx, bar_y + yy);
                sub.put_pixel(xx, yy, p);
            }
        }
        let out_path = Path::new(out_dir).join(format!("{name}.png"));
        sub.save(&out_path).expect("save crop");
        eprintln!("  saved {out_path:?} ({rw}x{bar_h})");
    }
    let _ = DynamicImage::new_luma8(1, 1);
}
