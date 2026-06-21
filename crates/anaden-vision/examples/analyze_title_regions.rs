//! 一時解析ヘルパ: title シーンの固定テクスチャ領域(点滅アニメ非依存)を決定論的に検出する。
//!
//! T3 (title_pc 小テンプレ化) のための導出ツール。`analyze_menu_bar.rs` の
//! 決定論的列分散手法を title コールドスタート画面へ再利用する。
//!
//! ユースケース:
//! - title 画面は "Tap to Start" (正規化 (930,488) 周辺) の点滅アニメーションで
//!   フレーム非安定。テンプレ認識は点滅に巻き込まれない固定テクスチャ部分
//!   (version/copyright 表示帯, title ロゴ角) で行う必要がある。
//! - 本ツールは与えた title キャプチャ上で列分散(縦縞エッジ強度)を計算し、
//!   「描画あり連続 run」= 固定テクスチャ領域候補を列挙する。人間がその中から
//!   アニメ非依存な ROI を選んで小テンプレ PNG を自己クロップする。
//!
//! usage: analyze_title_regions <input.png> <y0,y1> [x_min x_max]
//!   y0,y1 : 安定帯として解析する縦方向範囲(例: version 帯 660..708)
//!   x_min,x_max : 解析する横方向範囲(省略時 0..width)
//!
//! 出力: 各 run の [x_start,x_end, w] と中心 x を stdout へ。PNG は出さない
//! (ROI 確定後は crop_region.rs で自己クロップする)。
use image::GenericImageView;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: analyze_title_regions <input.png> <y0,y1> [x_min x_max]");
        std::process::exit(2);
    }
    let inp = &args[1];
    let band: Vec<u32> = args[2]
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    if band.len() != 2 {
        eprintln!("band must be y0,y1");
        std::process::exit(2);
    }
    let (y0, y1) = (band[0], band[1]);
    let img = image::open(inp).expect("open input");
    let (w, h) = img.dimensions();
    if y1 > h || y0 >= y1 {
        eprintln!("band {y0}..{y1} invalid for image {w}x{h}");
        std::process::exit(2);
    }
    let gray = img.to_luma8();

    let xs: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0).min(w);
    let xe: u32 = args
        .get(4)
        .and_then(|s| s.parse().ok())
        .unwrap_or(w)
        .max(xs + 1)
        .min(w);

    let band_h = (y1 - y0) as usize;

    // 列分散: 縦縞エッジ(文字ストローク等の描画)を持つ列を高分散として検出。
    let mut col_var: Vec<u64> = Vec::with_capacity((xe - xs) as usize);
    for x in xs..xe {
        let mut vals: Vec<u8> = Vec::with_capacity(band_h);
        for y in y0..y1 {
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
    // 3-tap smoothing で微細ノイズを抑える(analyze_menu_bar.rs と同じ手法)。
    let sm: Vec<f64> = (0..col_var.len())
        .map(|i| {
            let a = col_var[i.saturating_sub(1)];
            let b = col_var[i];
            let c = col_var[(i + 1).min(col_var.len() - 1)];
            (a + b + c) as f64 / 3.0
        })
        .collect();

    // 描画あり連続 run(閾値 max*0.10)。
    let thr = max_var * 0.10;
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
    // 小テンプレ基準: 幅 30..=120 程度の安定 run を候補とする(点滅領域のような大領域は除外しないが明示)。
    println!("image: {w}x{h} band=y{y0}..y{y1} x={xs}..{xe} max_var={max_var:.0} thr={thr:.0}");
    println!("runs (描画あり連続区間):");
    for (s, e) in &runs {
        let cw = e - s;
        let tag = if (30..=120).contains(&cw) {
            "SUBTEMPLATE_CANDIDATE"
        } else {
            ""
        };
        println!("  x={s}..{e} w={cw} center={} {tag}", (s + e) / 2);
    }
    if runs.is_empty() {
        println!("  (no runs detected in this band)");
    }
}
