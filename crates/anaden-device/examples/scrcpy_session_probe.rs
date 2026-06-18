//! ScrcpySession(video+control) の実機検証プローブ。
//!
//! やること:
//!   1. ScrcpySession::start でサーバ起動(nohup bg)〜video+control 2ソケット確立
//!   2. video フレームが到着するか確認(capture)
//!   3. 安全座標へ tap() 送信、前後のフレームを比較して内容変化を判定
//!
//! 使い方:
//!   cargo run --release -p anaden-device --features anaden-device/capture-scrcpy \
//!     --example scrcpy_session_probe -- --serial 33291JEHN27041

use std::time::{Duration, Instant};

use anaden_device::{AdbClient, ScrcpySession, ScrcpySessionConfig};
use image::GenericImageView;

#[tokio::main]
async fn main() {
    let mut serial =
        std::env::var("ANADEN_SERIAL").unwrap_or_else(|_| "33291JEHN27041".to_string());
    let mut x: u32 = 540;
    let mut y: u32 = 170;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--serial" => {
                serial = args.get(i + 1).cloned().unwrap_or(serial);
                i += 2;
            }
            "--x" => {
                x = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(x);
                i += 2;
            }
            "--y" => {
                y = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(y);
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    println!("=== ScrcpySession probe ===");
    println!("serial={} tap=({},{})", serial, x, y);

    let client = AdbClient::new(&serial);
    let config = ScrcpySessionConfig::default();

    let start = Instant::now();
    let session = match ScrcpySession::start(client, config).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("FAILED start: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "session established in {:?} (control_ready={})",
        start.elapsed(),
        session.control_ready()
    );

    // (2) video フレーム到着確認
    println!("waiting for first video frame...");
    let first = match tokio::time::timeout(Duration::from_secs(15), session.capture()).await {
        Ok(Ok(img)) => img,
        Ok(Err(e)) => {
            eprintln!("FAILED capture: {e}");
            std::process::exit(2);
        }
        Err(_) => {
            eprintln!("FAILED: first frame timeout(15s)");
            std::process::exit(2);
        }
    };
    println!(
        "first video frame: {}x{}",
        first.dimensions().0,
        first.dimensions().1
    );

    // フレームの「指紋」: 中央帯を縮小して比較しやすいシグネチャにする。
    // 自然変動(アニメ・エフェクト)とタッチ由来のシーン遷移を見分けるため、
    // タップ前後に複数フレームを取得して指紋のばらつきを比較する。
    let fingerprint = |img: &image::DynamicImage| -> Vec<u8> {
        let rgb = img.to_rgb8();
        let (w, h) = (rgb.width(), rgb.height());
        // 16x16 グリッドへ縮小サンプリング
        let mut sig = Vec::with_capacity(16 * 16 * 3);
        for gy in 0..16 {
            for gx in 0..16 {
                let x0 = (w as usize * gx) / 16;
                let y0 = (h as usize * gy) / 16;
                let p = rgb.get_pixel(x0 as u32, y0 as u32);
                sig.push(p.0[0]);
                sig.push(p.0[1]);
                sig.push(p.0[2]);
            }
        }
        sig
    };
    let sig_dist = |a: &[u8], b: &[u8]| -> usize {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (*x as isize - *y as isize).unsigned_abs() as usize)
            .sum()
    };

    // タップ前のベースライン: 5フレーム取得してばらつき(自然変動)を計測
    println!("collecting pre-tap baseline (5 frames)...");
    let mut pre_sigs = Vec::new();
    for _ in 0..5 {
        if let Ok(img) = session.capture().await {
            pre_sigs.push(fingerprint(&img));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let baseline_variance = if pre_sigs.len() >= 2 {
        let mut max_d = 0usize;
        for i in 0..pre_sigs.len() {
            for j in (i + 1)..pre_sigs.len() {
                max_d = max_d.max(sig_dist(&pre_sigs[i], &pre_sigs[j]));
            }
        }
        max_d
    } else {
        0
    };
    println!(
        "pre-tap baseline max variance (natural motion) = {}",
        baseline_variance
    );

    let pre_last = pre_sigs
        .last()
        .cloned()
        .unwrap_or_else(|| fingerprint(&first));

    // (3) tap 送信
    println!("sending tap ({},{}) via control socket...", x, y);
    if let Err(e) = session.tap(x, y) {
        eprintln!("FAILED tap: {e}");
        std::process::exit(3);
    }
    println!("tap sent OK");

    // 効果待ち + 事後フレーム取得
    tokio::time::sleep(Duration::from_millis(1500)).await;
    println!("collecting post-tap frames (5)...");
    let mut post_sigs = Vec::new();
    for _ in 0..5 {
        if let Ok(img) = session.capture().await {
            post_sigs.push(fingerprint(&img));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // 判定: post フレーム群が pre ベースラインから、自然変動幅を超えて離れているか。
    let mut max_change = 0usize;
    for ps in &post_sigs {
        let d = sig_dist(&pre_last, ps);
        if d > max_change {
            max_change = d;
        }
    }
    println!("post-tap max change vs pre-last = {}", max_change);

    // 閾値: 自然変動幅の 3 倍、か最低 400 (16x16x3=768byte のうちある程度の変化)
    let threshold = (baseline_variance * 3).max(400);
    let changed = max_change > threshold;
    println!(
        "threshold={} (3x baseline or 400 min) => {}",
        threshold,
        if changed {
            "SCREEN CHANGED (content)"
        } else {
            "no decisive change"
        }
    );

    println!("\n=== SUMMARY ===");
    println!("video_frames_ok=true");
    println!("control_ready={}", session.control_ready());
    println!("content_changed={}", changed);
    println!(
        "baseline_variance={} max_change={}",
        baseline_variance, max_change
    );

    drop(session);
    println!("session dropped (server cleanup).");
}
