//! エンドツーエップ認識ループの実機実測ベンチ。
//!
//! 実行（--release 必須）:
//!   cargo run --release --example bench_loop -p anaden-vision
//!   cargo run --release --example bench_loop -p anaden-vision -- <serial>
//!
//! 実機 ADB（Pixel 7a 等）で「認識→次アクションまでの1ループ」の各工程を
//! std::time::Instant で各30回計測し中央値(ms)を算出する。性能仕様は
//! 「認識から次アクションまで1秒以内」。
//!
//! 工程:
//!   (a) capture : adb -s <serial> exec-out screencap -p で PNG バイト取得まで
//!   (b) normalize : 取得画像を ScreenScaler で 720p(幅1280) 正規化
//!   (c) recognition : SseVisionEngine / CcoeffVisionEngine の match_template
//!                     設定 {sse,ccoeff} x {full, roi(中央300x150)} x {downscale=1,2,4}
//!   (d) tap : adb -s <serial> shell input tap <x> <y>（無害な画面端）
//!
//! テンプレートは同キャプチャの中央 240x80 を crop して作る（ベンチ用途の
//! 純粋なマッチング時間計測が目的のため、マッチの正しさは問わない）。
//! ROI 設定では中央 300x150 を事前 crop して探索空間を絞る。

use std::process::Command;
use std::time::Instant;

use anaden_core::MatchConfidence;
use anaden_vision::{CcoeffVisionEngine, ScreenScaler, SseVisionEngine, VisionEngine};
use image::{DynamicImage, GenericImageView};

/// 計測繰り返し回数（中央値算出用）。
const ITERS: usize = 30;

/// テンプレートサイズ（base 座標系・キャプチャ中央から crop）。
const TPL_W: u32 = 240;
const TPL_H: u32 = 80;

/// ROI（base 座標系・中央 300x150 を事前 crop）。
const ROI_W: u32 = 300;
const ROI_H: u32 = 150;

/// 無害なタップ座標（base 座標系 → 端末座標へ逆変換して入力）。
/// 画面中央付近の目立たない点。
const TAP_X_BASE: u32 = 640;
const TAP_Y_BASE: u32 = 360;

fn main() {
    // シリアル解決。引数がなければ adb devices から device 状態の先頭を使用。
    let args: Vec<String> = std::env::args().collect();
    let serial: String = if args.len() > 1 {
        args[1].clone()
    } else {
        match pick_first_device() {
            Some(s) => s,
            None => {
                eprintln!("No ADB device found. Pass serial as arg.");
                std::process::exit(1);
            }
        }
    };
    println!("[device] serial = {}", serial);

    // ---------- (a) capture ----------
    println!("\n[capture] adb exec-out screencap -p x{} ...", ITERS);
    let mut capture_times: Vec<f64> = Vec::with_capacity(ITERS);
    let mut last_png: Vec<u8> = Vec::new();
    for _ in 0..ITERS {
        let t = Instant::now();
        let png = adb_screencap(&serial);
        let dt = t.elapsed().as_secs_f64() * 1000.0;
        capture_times.push(dt);
        if png.is_empty() {
            eprintln!("capture returned empty bytes. Is the device unlocked?");
            std::process::exit(1);
        }
        last_png = png;
    }
    let capture_ms = median(&capture_times);
    println!("  capture median = {:.2} ms", capture_ms);

    // PNG → DynamicImage
    let raw_img = match image::load_from_memory_with_format(&last_png, image::ImageFormat::Png) {
        Ok(img) => img,
        Err(e) => {
            eprintln!("failed to decode captured PNG: {}", e);
            std::process::exit(1);
        }
    };
    println!(
        "  raw capture dims = {}x{} ({} bytes)",
        raw_img.width(),
        raw_img.height(),
        last_png.len()
    );

    // ---------- (b) normalize ----------
    println!("\n[normalize] ScreenScaler → 720p(幅1280) x{} ...", ITERS);
    let scaler = ScreenScaler::new();
    let mut normalize_times: Vec<f64> = Vec::with_capacity(ITERS);
    let mut normalized: Option<DynamicImage> = None;
    for _ in 0..ITERS {
        let t = Instant::now();
        let out = scaler.normalize(&raw_img);
        let dt = t.elapsed().as_secs_f64() * 1000.0;
        normalize_times.push(dt);
        normalized = Some(out);
    }
    let normalize_ms = median(&normalize_times);
    let normalized = normalized.unwrap();
    println!(
        "  normalize median = {:.2} ms (→ {}x{})",
        normalize_ms,
        normalized.width(),
        normalized.height()
    );

    // テンプレート生成: 正規化後全面の中央 240x80 を crop。
    let (nw, nh) = normalized.dimensions();
    let tx = nw.saturating_sub(TPL_W) / 2;
    let ty = nh.saturating_sub(TPL_H) / 2;
    let tpl_full = normalized.crop_imm(tx, ty, TPL_W, TPL_H);
    // ROI: 正規化後全面の中央 300x150 を事前 crop。
    let rx = nw.saturating_sub(ROI_W) / 2;
    let ry = nh.saturating_sub(ROI_H) / 2;
    let roi_img = normalized.crop_imm(rx, ry, ROI_W, ROI_H);
    // ROI 用テンプレート: ROI 中央 240x80（ROI が十分大きいことを前提）。
    let rtx = ROI_W.saturating_sub(TPL_W) / 2;
    let rty = ROI_H.saturating_sub(TPL_H) / 2;
    let tpl_roi = roi_img.crop_imm(rtx, rty, TPL_W, TPL_H);

    // ---------- (c) recognition ----------
    println!("\n[recognition] match_template x{} per config ...", ITERS);
    // 閾値は 0 にして「マッチするか」で early-return させず純粋な全走査時間を測る。
    let thr = MatchConfidence(0.0);

    let mut recognition_results: Vec<RecognitionResult> = Vec::new();
    let configs: &[(EngineKind, ScopeKind)] = &[
        (EngineKind::Sse, ScopeKind::Full),
        (EngineKind::Sse, ScopeKind::Roi),
        (EngineKind::Ccoeff, ScopeKind::Full),
        (EngineKind::Ccoeff, ScopeKind::Roi),
    ];
    for &(engine, scope) in configs {
        for &ds in &[1u32, 2u32, 4u32] {
            let (haystack, needle) = match scope {
                ScopeKind::Full => (&normalized, &tpl_full),
                ScopeKind::Roi => (&roi_img, &tpl_roi),
            };
            let mut times: Vec<f64> = Vec::with_capacity(ITERS);
            // JIT ウォームアップ2回（計測外）。
            for _ in 0..2 {
                let _ = run_match(engine, thr, ds, haystack, needle);
            }
            for _ in 0..ITERS {
                let t = Instant::now();
                let _ = run_match(engine, thr, ds, haystack, needle);
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            let med = median(&times);
            println!(
                "  {:>6} / {:>4} / ds={} : median = {:8.2} ms",
                engine_name(engine),
                scope_name(scope),
                ds,
                med
            );
            recognition_results.push(RecognitionResult {
                engine: engine_name(engine).to_string(),
                scope: scope_name(scope).to_string(),
                downscale: ds as i32,
                median_ms: med,
            });
        }
    }

    // ---------- (d) tap ----------
    // base 座標 → 端末座標へ逆変換。端末幅 = raw_img.width()。
    let device_w = raw_img.width();
    let tap_x = scaler.from_base(device_w, TAP_X_BASE);
    let tap_y = scaler.from_base(device_w, TAP_Y_BASE);
    println!(
        "\n[tap] adb shell input tap {} {} x{} (base {},{} → device {},{})",
        tap_x, tap_y, ITERS, TAP_X_BASE, TAP_Y_BASE, tap_x, tap_y
    );
    let mut tap_times: Vec<f64> = Vec::with_capacity(ITERS);
    for _ in 0..ITERS {
        let t = Instant::now();
        let st = adb_tap(&serial, tap_x, tap_y);
        let dt = t.elapsed().as_secs_f64() * 1000.0;
        if !st.success() {
            eprintln!("tap command failed with status {:?}", st.code());
        }
        tap_times.push(dt);
    }
    let tap_ms = median(&tap_times);
    println!("  tap median = {:.2} ms", tap_ms);

    // ---------- end-to-end まとめ ----------
    println!("\n[end-to-end] capture+normalize+recognition+tap (median 合計)");
    for r in &recognition_results {
        let total = capture_ms + normalize_ms + r.median_ms + tap_ms;
        let meets = total < 1000.0;
        println!(
            "  {} / {} / ds={} : total = {:8.2} ms  {}",
            r.engine,
            r.scope,
            r.downscale,
            total,
            if meets { "< 1000ms OK" } else { ">= 1000ms NG" }
        );
    }

    // 機械可読サマリ（最終行）。
    println!(
        "\n===SUMMARY=== capture={:.2} normalize={:.2} tap={:.2}",
        capture_ms, normalize_ms, tap_ms
    );
    for r in &recognition_results {
        let total = capture_ms + normalize_ms + r.median_ms + tap_ms;
        println!(
            "===RESULT=== {} {} ds={} med={:.2} total={:.2} meets={}",
            r.engine,
            r.scope,
            r.downscale,
            r.median_ms,
            total,
            total < 1000.0
        );
    }
}

#[derive(Clone, Copy)]
enum EngineKind {
    Sse,
    Ccoeff,
}
#[derive(Clone, Copy)]
enum ScopeKind {
    Full,
    Roi,
}

struct RecognitionResult {
    engine: String,
    scope: String,
    downscale: i32,
    median_ms: f64,
}

fn run_match(
    engine: EngineKind,
    threshold: MatchConfidence,
    ds: u32,
    haystack: &DynamicImage,
    needle: &DynamicImage,
) -> Option<anaden_vision::MatchResult> {
    match engine {
        EngineKind::Sse => {
            let matcher = anaden_vision::TemplateMatcher::new(threshold, ds);
            let eng = SseVisionEngine::new(matcher);
            eng.match_template(haystack, needle)
        }
        EngineKind::Ccoeff => {
            let eng = CcoeffVisionEngine::new(threshold, ds);
            eng.match_template(haystack, needle)
        }
    }
}

fn engine_name(e: EngineKind) -> &'static str {
    match e {
        EngineKind::Sse => "sse",
        EngineKind::Ccoeff => "ccoeff",
    }
}

fn scope_name(s: ScopeKind) -> &'static str {
    match s {
        ScopeKind::Full => "full",
        ScopeKind::Roi => "roi",
    }
}

/// adb exec-out screencap -p の標準出力（PNG バイト列）を取得する。
fn adb_screencap(serial: &str) -> Vec<u8> {
    let out = Command::new("adb")
        .args(["-s", serial, "exec-out", "screencap", "-p"])
        .output();
    match out {
        Ok(o) => o.stdout,
        Err(e) => {
            eprintln!("adb screencap failed: {}", e);
            Vec::new()
        }
    }
}

/// adb shell input tap の完了を待つ（ブロッキング）。
fn adb_tap(serial: &str, x: u32, y: u32) -> std::process::ExitStatus {
    Command::new("adb")
        .args([
            "-s",
            serial,
            "shell",
            "input",
            "tap",
            &x.to_string(),
            &y.to_string(),
        ])
        .status()
        .unwrap_or_else(|e| {
            eprintln!("adb tap failed: {}", e);
            // 失敗を示すダミー status は作れないので終了。
            std::process::exit(1);
        })
}

/// adb devices の出力から最初の device 行のシリアルを返す。
fn pick_first_device() -> Option<String> {
    let out = Command::new("adb").args(["devices"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        if let Some(serial) = it.next() {
            if it.next() == Some("device") {
                return Some(serial.to_string());
            }
        }
    }
    None
}

fn median(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    let mut v: Vec<f64> = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}
