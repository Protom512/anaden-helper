//! 2枚の正規化済み画面の差分を取り、フレーム間で安定(差分小)なブロックを列挙。
//! テンプレ候補領域をデータ駆動で特定するための検証ヘルパ。
use anaden_vision::ScreenScaler;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: stable_region <a.png> <b.png> [tile=64] [top=10]");
        std::process::exit(2);
    }
    let a = image::open(&args[1]).unwrap().to_rgb8();
    let b = image::open(&args[2]).unwrap().to_rgb8();
    let tile: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(64);
    let topn: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(15);
    assert_eq!(a.dimensions(), b.dimensions(), "dim mismatch");
    let (w, h) = a.dimensions();
    // 各タイルの平均輝度(テクスチャ指標=輝度分散)と、2枚間の差分平均を計算。
    // 安定かつテクスチャ豊富(認識しやすい)なタイルを探す。
    let mut rows: Vec<(f32, u32, u32, f32, f32)> = Vec::new();
    let _ = ScreenScaler::new(); // scale参照解決(未使用警告回避)
    let mut ty = 0;
    while ty < h {
        let mut tx = 0;
        while tx < w {
            let tw = tile.min(w - tx);
            let th = tile.min(h - ty);
            let mut diff_sum = 0u64;
            let mut lum_sum = 0u64;
            let mut lum_sq = 0u64;
            let n = (tw * th) as u64;
            for yy in 0..th {
                for xx in 0..tw {
                    let pa = a.get_pixel(tx + xx, ty + yy);
                    let pb = b.get_pixel(tx + xx, ty + yy);
                    let la = (pa.0[0] as u32 + pa.0[1] as u32 + pa.0[2] as u32) / 3;
                    let lb = (pb.0[0] as u32 + pb.0[1] as u32 + pb.0[2] as u32) / 3;
                    diff_sum += (la as i64 - lb as i64).unsigned_abs() as u64;
                    lum_sum += la as u64;
                    lum_sq += (la as u64) * (la as u64);
                }
            }
            let mean = lum_sum as f32 / n as f32;
            let var = (lum_sq as f32 / n as f32) - mean * mean; // テクスチャ量
            let diff_avg = diff_sum as f32 / n as f32; // フレーム間差分(小さいほど安定)
            // score: 安定(diff小) かつ テクスチャ豊富(var大)
            let score = -diff_avg + var * 0.05;
            rows.push((score, tx, ty, diff_avg, var));
            // unpack later
            let _ = (tw, th);
            tx += tile;
        }
        ty += tile;
    }
    // score降順でtopN
    rows.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap());
    println!("tile={} top{} (score desc):", tile, topn);
    println!("rank  x    y    diff_avg  lumvar");
    for (i, (_, x, y, diff, var)) in rows.iter().take(topn).enumerate() {
        println!(
            "{:>3}   {:>4} {:>4}  {:>7.2}  {:>8.1}",
            i + 1,
            x,
            y,
            diff,
            var
        );
    }
}
