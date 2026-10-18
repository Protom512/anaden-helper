//! テンプレートマッチ実測ベンチ（実機キャプチャ使用）。
//!
//! 実行:
//!   cargo run --release --example bench_match -p anaden-vision
//!
//! 旧 Android 実機キャプチャ（2400x1080・Issue #188 で取得経路は削除済み）を正規化（幅1280）し、
//! SseVisionEngine / CcoeffVisionEngine の match_template を各設定で30回計測、
//! 中央値(ms)を表出力する。
//!
//! 設定マトリクス: {sse,ccoeff} x {full, roi} x {downscale=1,2,4}
//!   - full: 正規化後全面（1280x576 相当）
//!   - roi : 中央 300x150 を事前 crop（探索空間を絞る効果の計測）
//!   - downscale: エンジン内部のダウンスケール倍率（1=原寸, 2=1/2, 4=1/4）
//!
//! テンプレートは同キャプチャから特徴的（輝度分散が大きい）領域を crop して作成。
//! ROI とテンプレートは base 座標系（正規化後）で定義するため、
//! 事前 crop による探索空間削減効果とテンプレートサイズ効果を実測できる。

use std::path::PathBuf;
use std::time::Instant;

use anaden_core::MatchConfidence;
use anaden_vision::{CcoeffVisionEngine, ScreenScaler, SseVisionEngine, VisionEngine};
use image::{DynamicImage, GrayImage};

/// 計測繰り返し回数。
const ITERS: usize = 30;

/// テンプレートサイズ（base 座標系）。
const TPL_W: u32 = 240;
const TPL_H: u32 = 80;
const TPL_W_SMALL: u32 = 80;
const TPL_H_SMALL: u32 = 80;

/// ROI（base 座標系、中央 300x150）。
const ROI_W: u32 = 300;
const ROI_H: u32 = 150;

/// キャプチャパス解決。コマンドライン引数がなければ既定の 3 枚を探す。
/// 1 枚も無ければ exit 1。
fn resolve_capture_paths(args: &[String]) -> Vec<PathBuf> {
    let default_paths = [
        "/tmp/real_shot1.png",
        "/tmp/real_shot2.png",
        "/tmp/real_shot3.png",
    ];
    let paths: Vec<PathBuf> = if args.len() > 1 {
        args[1..].iter().map(PathBuf::from).collect()
    } else {
        default_paths
            .iter()
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .collect()
    };
    if paths.is_empty() {
        eprintln!("No capture PNG found. Pass paths as args.");
        std::process::exit(1);
    }
    paths
}

/// scope 名 → haystack (full 全面 / roi 中央 crop) とその寸法。
fn scope_haystack(
    scope: &str,
    full_dyn: &DynamicImage,
    roi_dyn: &DynamicImage,
    bw: u32,
    bh: u32,
) -> (DynamicImage, u32, u32) {
    match scope {
        "full" => (full_dyn.clone(), bw, bh),
        "roi" => (roi_dyn.clone(), ROI_W, ROI_H),
        _ => unreachable!(),
    }
}

/// エンジン名 → 計測 (median_ms, confidence)。閾値は 0.0 固定 (最良マッチを
/// 常に返させる = 計測の安定化)。
fn measure_engine(engine: &str, hay: &DynamicImage, needle: &DynamicImage, ds: u32) -> (f64, f32) {
    let thr = MatchConfidence::new(0.0);
    match engine {
        "sse" => {
            let eng = SseVisionEngine::new(anaden_vision::TemplateMatcher::new(thr, ds));
            measure(&eng, hay, needle, ITERS)
        }
        "ccoeff" => {
            let eng = CcoeffVisionEngine::new(thr, ds);
            measure(&eng, hay, needle, ITERS)
        }
        _ => unreachable!(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // 合成 fixture モード (05-template-cache evidence 採取)。実キャプチャ不要で
    // TaskDef::detect のサイクル時間を計測する。詳細は run_synthetic_bench の doc。
    if args.get(1).map(String::as_str) == Some("synthetic") {
        run_synthetic_bench();
        return;
    }

    let paths = resolve_capture_paths(&args);
    let path0 = paths[0].to_string_lossy().to_string();
    println!("# anaden-vision bench_match");
    println!("# capture[0]: {}", path0);
    println!("# iters per cell: {}", ITERS);

    // 1) キャプチャ読込 + 正規化（幅1280 base 座標系）。
    let scaler = ScreenScaler::new();
    let raw = image::open(&paths[0]).expect("open capture");
    let raw_w = raw.width();
    let raw_h = raw.height();
    let base = scaler.normalize(&raw);
    let bw = base.width();
    let bh = base.height();
    println!(
        "# raw: {}x{}  base(normalized): {}x{}",
        raw_w, raw_h, bw, bh
    );

    // 2) 特徴的テンプレートをデータ駆動で選択:
    //    base 画像全体をグレースケールの窓分散で走査し、最も分散が大きい領域を
    //    採用する（平坦領域でない＝マッチングに意味のある特徴を含む）。
    let base_gray = base.to_luma8();
    let big_tpl = pick_high_variance_crop(&base_gray, TPL_W, TPL_H);
    let small_tpl = pick_high_variance_crop(&base_gray, TPL_W_SMALL, TPL_H_SMALL);

    let big_dyn = DynamicImage::ImageLuma8(big_tpl.clone());
    let small_dyn = DynamicImage::ImageLuma8(small_tpl.clone());

    println!(
        "# templates: big={}x{} small={}x{} (high-variance crops from base image)",
        TPL_W, TPL_H, TPL_W_SMALL, TPL_H_SMALL
    );

    // 3) full / roi の haystack を用意。
    //    full: base 全面。roi: 中央 ROI_W x ROI_H を事前 crop。
    let roi_x = bw.saturating_sub(ROI_W) / 2;
    let roi_y = bh.saturating_sub(ROI_H) / 2;
    let roi_crop = crop_luma(&base, roi_x, roi_y, ROI_W, ROI_H);
    let roi_dyn = DynamicImage::ImageLuma8(roi_crop);

    // base 全面の haystack（エンジン内部は to_luma8 を呼ぶので RGB でも可）。
    let full_dyn = base.clone();

    println!(
        "# haystacks: full={}x{}  roi={}x{} (crop @ base {},{} downscaled inside engine)",
        bw, bh, ROI_W, ROI_H, roi_x, roi_y
    );
    println!(
        "# warning: ROI template may exceed ROI haystack at downscale>=4; such cells report N/A."
    );

    // 4) 設定マトリクスで計測。
    let engines = ["sse", "ccoeff"];
    let scopes = ["full", "roi"];
    let downscales = [1u32, 2u32, 4u32];

    // 結果格納（findings 用）。
    let mut results: Vec<Row> = Vec::new();

    println!();
    println!(
        "{:<8} {:<6} {:>10} {:>14} {:>14}",
        "engine", "scope", "downscale", "median_ms", "confidence"
    );
    println!("{}", "-".repeat(56));

    for &engine in &engines {
        for &scope in &scopes {
            // scope ごとの haystack を決定。
            let (hay, hay_w, hay_h) = scope_haystack(scope, &full_dyn, &roi_dyn, bw, bh);
            for &ds in &downscales {
                // テンプレートサイズが downscale 後に haystack を超える場合は N/A。
                // big テンプレで判定（small は通る場合も big で代表）。
                let eff_hw = hay_w / ds;
                let eff_hh = hay_h / ds;
                let eff_tw = TPL_W / ds;
                let eff_th = TPL_H / ds;
                let row = if eff_tw >= eff_hw || eff_th >= eff_hh || eff_hw == 0 || eff_hh == 0 {
                    Row {
                        engine: engine.to_string(),
                        scope: scope.to_string(),
                        downscale: ds,
                        template_size: format!("{}x{}", TPL_W, TPL_H),
                        median_ms: f64::NAN,
                        confidence: "N/A (tpl>hay)".to_string(),
                    }
                } else {
                    let (median_ms, conf) = measure_engine(engine, &hay, &big_dyn, ds);
                    Row {
                        engine: engine.to_string(),
                        scope: scope.to_string(),
                        downscale: ds,
                        template_size: format!("{}x{}", TPL_W, TPL_H),
                        median_ms,
                        confidence: format!("{:.4}", conf),
                    }
                };
                print_row(&row);
                results.push(row);
            }
        }
    }

    // 5) small テンプレ(80x80)の代表セルも追加計測（テンプレートサイズ効果の比較）。
    println!();
    println!(
        "# supplementary: small template {}x{} (downscale=2)",
        TPL_W_SMALL, TPL_H_SMALL
    );
    println!(
        "{:<8} {:<6} {:>10} {:>14} {:>14}",
        "engine", "scope", "downscale", "median_ms", "confidence"
    );
    println!("{}", "-".repeat(56));
    for &engine in &engines {
        for &scope in &scopes {
            let (hay, _hay_w, _hay_h) = scope_haystack(scope, &full_dyn, &roi_dyn, bw, bh);
            let ds = 2u32;
            let (median_ms, conf) = measure_engine(engine, &hay, &small_dyn, ds);
            let row = Row {
                engine: engine.to_string(),
                scope: scope.to_string(),
                downscale: ds,
                template_size: format!("{}x{}", TPL_W_SMALL, TPL_H_SMALL),
                median_ms,
                confidence: format!("{:.4}", conf),
            };
            print_row(&row);
            results.push(row);
        }
    }

    // JSON 風サマリを stderr に出力（パース容易）。
    eprintln!("---RESULTS_JSON---");
    eprintln!("[");
    for (i, r) in results.iter().enumerate() {
        let med = if r.median_ms.is_nan() {
            "null".to_string()
        } else {
            format!("{:.4}", r.median_ms)
        };
        eprintln!(
            "  {{\"engine\":\"{}\",\"scope\":\"{}\",\"downscale\":{},\"template_size\":\"{}\",\"median_ms\":{},\"confidence\":\"{}\"}}{}",
            r.engine,
            r.scope,
            r.downscale,
            r.template_size,
            med,
            r.confidence,
            if i + 1 < results.len() { "," } else { "" }
        );
    }
    eprintln!("]");
}

struct Row {
    engine: String,
    scope: String,
    downscale: u32,
    template_size: String,
    median_ms: f64,
    confidence: String,
}

fn print_row(r: &Row) {
    let med = if r.median_ms.is_nan() {
        "N/A".to_string()
    } else {
        format!("{:.4}", r.median_ms)
    };
    println!(
        "{:<8} {:<6} {:>10} {:>14} {:>14}",
        r.engine, r.scope, r.downscale, med, r.confidence
    );
}

/// エンジンで match_template を `iters` 回実行し中央値(ms)と代表 confidence を返す。
fn measure(
    eng: &dyn VisionEngine,
    hay: &DynamicImage,
    needle: &DynamicImage,
    iters: usize,
) -> (f64, f32) {
    // warmup（1回）でキャッシュ/JIT を安定化。
    let warm = eng.match_template(hay, needle);
    let mut samples: Vec<f64> = Vec::with_capacity(iters);
    let mut conf = 0.0f32;
    for _ in 0..iters {
        let t0 = Instant::now();
        let m = eng.match_template(hay, needle);
        let dt = t0.elapsed().as_secs_f64() * 1000.0;
        samples.push(dt);
        if let Some(mr) = m {
            conf = mr.confidence.0;
        }
    }
    let _ = warm;
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = samples.len() / 2;
    let median = samples[mid];
    (median, conf)
}

/// グレースケール画像上で (w,h) の窓を走査し、窓内分散が最大となる領域を返す。
/// 高分散＝エッジ/テクスチャ豊富＝テンプレートとして意味あり。
fn pick_high_variance_crop(gray: &GrayImage, w: u32, h: u32) -> GrayImage {
    let (gw, gh) = gray.dimensions();
    let mut best_x = 0u32;
    let mut best_y = 0u32;
    let mut best_var = -1.0f64;

    // 高速化のため積分図で窓和・窓二乗和を O(1) 取得。
    let mut sum = vec![0u64; ((gw + 1) * (gh + 1)) as usize];
    let mut sum2 = vec![0u64; ((gw + 1) * (gh + 1)) as usize];
    for y in 0..gh {
        for x in 0..gw {
            let v = gray.get_pixel(x, y)[0] as u64;
            let idx = ((y + 1) * (gw + 1) + (x + 1)) as usize;
            let up = sum[(y * (gw + 1) + (x + 1)) as usize];
            let left = sum[((y + 1) * (gw + 1) + x) as usize];
            let ul = sum[(y * (gw + 1) + x) as usize];
            sum[idx] = up + left - ul + v;
            let up2 = sum2[(y * (gw + 1) + (x + 1)) as usize];
            let left2 = sum2[((y + 1) * (gw + 1) + x) as usize];
            let ul2 = sum2[(y * (gw + 1) + x) as usize];
            sum2[idx] = up2 + left2 - ul2 + v * v;
        }
    }

    let n = (w * h) as f64;
    let stride = (gw + 1) as usize;
    for y in (0..gh.saturating_sub(h)).step_by(8) {
        for x in (0..gw.saturating_sub(w)).step_by(8) {
            let xs = x as usize;
            let ys = y as usize;
            let xe = xs + w as usize;
            let ye = ys + h as usize;
            let s = sum[ye * stride + xe] - sum[ys * stride + xe] - sum[ye * stride + xs]
                + sum[ys * stride + xs];
            let s2 = sum2[ye * stride + xe] - sum2[ys * stride + xe] - sum2[ye * stride + xs]
                + sum2[ys * stride + xs];
            let mean = s as f64 / n;
            let var = s2 as f64 / n - mean * mean;
            if var > best_var {
                best_var = var;
                best_x = x;
                best_y = y;
            }
        }
    }

    crop_luma_gray(gray, best_x, best_y, w, h)
}

/// DynamicImage から (x,y,w,h) のグレースケール crop を返す。
fn crop_luma(img: &DynamicImage, x: u32, y: u32, w: u32, h: u32) -> GrayImage {
    let g = img.to_luma8();
    crop_luma_gray(&g, x, y, w, h)
}

fn crop_luma_gray(gray: &GrayImage, x: u32, y: u32, w: u32, h: u32) -> GrayImage {
    let mut out = GrayImage::new(w, h);
    for j in 0..h {
        for i in 0..w {
            let p = gray.get_pixel(x + i, y + j)[0];
            out.put_pixel(i, j, image::Luma([p]));
        }
    }
    out
}

// ---- 合成 fixture ベンチ (05-template-cache: detect サイクル時間の evidence 採取) ----
//
// 実行:
//   cargo run --release --example bench_match -p anaden-vision -- synthetic
//
// 実キャプチャに依存せず、合成 needle テンプレート + 合成フレームで
// [`anaden_vision::TaskDef::detect`] のサイクル時間を計測するモード。
// テンプレート decode + needle リサイズ + ccoeff 前処理のキャッシュ効果
// (F13/F17/F19) を before/after で比較するための生データを出力する。
//
// セル構成 (どちらも ccoeff / threshold 0.9 / frame 1258x708 = スケール恒等):
//   - full-cycle: needle 120x120 + スラック ROI [604,223,160,180] (実運用形状。
//     走査主導でテンプレ読込は数 % の寄与)
//   - load-path : needle 400x300 + 同寸 ROI [100,100,400,300] (走査 1 定点。
//     decode+resize+ccoeff 前処理の寄与を分離して可視化)
//
// 各セルで (1 + SYN_ITERS) 回 detect し、初回 (コールド = 読込+前処理を含む) と
// 残りの中央値/平均/最小を生出力する。フレームは needle を LCG 決定論位置に埋め込んだ
// ものを事前生成する (計測中の画像生成コストを排除)。全 detect がマッチすることを
// fail-loud で検証する (偽 green の計測を許さない)。

/// 合成ベンチの反復回数 (初回コールド分を含まず)。
const SYN_ITERS: usize = 60;
/// 合成フレーム寸法 (raw-1258x708 = スケール恒等にするため)。
const SYN_W: u32 = 1258;
const SYN_H: u32 = 708;

fn run_synthetic_bench() {
    let dir = std::env::temp_dir().join("anaden_bench_match_synthetic");
    std::fs::create_dir_all(&dir).expect("create bench dir");

    println!("# anaden-vision bench_match (synthetic mode)");
    println!("# frame: {SYN_W}x{SYN_H} (scale-identity)  iters per cell: {SYN_ITERS} (+1 cold)");
    println!(
        "# cells: full-cycle = scan-dominated (realistic), load-path = decode/resize/preprocess-dominated"
    );
    println!();

    let cells: [(&str, u32, u32, [u32; 4]); 2] = [
        ("full-cycle", 120, 120, [604, 223, 160, 180]),
        ("load-path", 400, 300, [100, 100, 400, 300]),
    ];
    for (label, tw, th, roi) in cells {
        bench_detect_cell(&dir, label, tw, th, roi);
    }
    println!();
    println!(
        "# note: 'cold' is the first detect per cell (template decode + Lanczos3 resize +\n\
         # ccoeff preprocess included). Compare before/after runs of the same binary args:\n\
         # pre-cache builds show median ~= cold; cached builds show median << cold."
    );
}

/// 1 セル分の detect サイクル計測。テンプレート PNG とフレーム群を合成して
/// (1 + SYN_ITERS) 回 detect し、所要時間 (ms) を生出力する。
fn bench_detect_cell(dir: &std::path::Path, label: &str, tw: u32, th: u32, roi: [u32; 4]) {
    // 1) 合成テンプレート (勾配 = 構造あり, denomT != 0) を書き出す。
    let tpl_path = dir.join(format!("synth_{label}_{tw}x{th}.png"));
    gradient_pattern(tw, th)
        .save(&tpl_path)
        .expect("save synthetic template");

    // 2) フレームを事前生成: needle を ROI 内の決定論 (LCG) 位置に埋め込む。
    let frames: Vec<DynamicImage> = (0..=SYN_ITERS)
        .map(|i| {
            let [rx, ry, rw, rh] = roi;
            let slack_x = (rw.saturating_sub(tw)) as u64;
            let slack_y = (rh.saturating_sub(th)) as u64;
            let ox = rx + (lcg(i as u64) % (slack_x + 1)) as u32;
            let oy = ry + (lcg(i as u64 + 1) % (slack_y + 1)) as u32;
            DynamicImage::ImageLuma8(embed_needle(
                SYN_W,
                SYN_H,
                &gradient_pattern(tw, th),
                ox,
                oy,
            ))
        })
        .collect();

    // 3) TaskDef (ccoeff / threshold 0.9 / 絶対パステンプレート)。
    let task = anaden_vision::TaskDef {
        name: format!("Synth{label}"),
        state: "Bench".into(),
        algorithm: anaden_vision::Algorithm::Ccoeff,
        template: tpl_path.clone(),
        roi: Some(roi),
        threshold: 0.9,
        base: None,
        action: None,
        next: None,
    };

    // 4) 計測: 全 detect がマッチすることを fail-loud 検証しつつサンプル採取。
    let mut cold_ms = f64::NAN;
    let mut samples: Vec<f64> = Vec::with_capacity(SYN_ITERS);
    for (i, frame) in frames.iter().enumerate() {
        let t0 = Instant::now();
        let m = task
            .detect(frame, std::path::Path::new(""))
            .expect("detect must not error");
        let dt = t0.elapsed().as_secs_f64() * 1000.0;
        assert!(
            m.is_some(),
            "synthetic embed must match at iter {i} (label={label})"
        );
        if i == 0 {
            cold_ms = dt;
        } else {
            samples.push(dt);
        }
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = samples[samples.len() / 2];
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let min = samples[0];
    println!(
        "{label:<12} needle={tw}x{th} roi={roi:?} cold={cold_ms:.3}ms median={median:.3}ms \
         mean={mean:.3}ms min={min:.3}ms n={} (ccoeff thr=0.90)",
        samples.len()
    );
}

/// `(x+y) mod 64` の勾配パターン (pipeline.rs テスト準拠。denomT != 0 を保証)。
fn gradient_pattern(w: u32, h: u32) -> GrayImage {
    let mut img = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = ((x + y) % 64) as u8;
            img.put_pixel(x, y, image::Luma([v]));
        }
    }
    img
}

/// `w x h` の背景 `128` 上に needle を `(ox, oy)` に埋め込んだ画像。
fn embed_needle(w: u32, h: u32, needle: &GrayImage, ox: u32, oy: u32) -> GrayImage {
    let mut img = GrayImage::from_pixel(w, h, image::Luma([128u8]));
    for y in 0..needle.height() {
        for x in 0..needle.width() {
            let p = needle.get_pixel(x, y)[0];
            img.put_pixel(ox + x, oy + y, image::Luma([p]));
        }
    }
    img
}

/// 決定論的な LCG (合成フレームの埋込位置を決める。乱数 crate 非依存)。
fn lcg(seed: u64) -> u64 {
    seed.wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407)
}
