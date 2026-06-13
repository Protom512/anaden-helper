//! テンプレート抽出・マッチング検証ツール。
//!
//! 使い方:
//!   cargo run --bin anaden-tool -- capture <serial> <output_dir>
//!   cargo run --bin anaden-tool -- extract <screenshot> <x> <y> <w> <h> <output>
//!   cargo run --bin anaden-tool -- match <screenshot> <template> <threshold>
//!   cargo run --bin anaden-tool -- launch <serial>

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use image::GenericImageView;

#[derive(Parser)]
#[command(name = "anaden-tool", about = "Template extraction and matching tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// キャプチャした画面を保存する
    Capture {
        /// ADB デバイスシリアル
        serial: String,
        /// 出力先パス
        #[arg(default_value = "templates/captures/latest.png")]
        output: PathBuf,
    },
    /// スクリーンショットからテンプレート領域を抽出する
    Extract {
        /// 元画像パス
        screenshot: PathBuf,
        /// X座標
        x: u32,
        /// Y座標
        y: u32,
        /// 幅
        width: u32,
        /// 高さ
        height: u32,
        /// 出力先パス
        output: PathBuf,
    },
    /// テンプレートマッチングのテスト
    Match {
        /// スクリーンショット画像
        screenshot: PathBuf,
        /// テンプレート画像
        template: PathBuf,
        /// 信頼度閾値 (0.0〜1.0)
        #[arg(default_value_t = 0.85)]
        threshold: f32,
        /// ダウンスケール倍率（例: 4 で 1/4 に縮小してマッチング）
        #[arg(short, long, default_value_t = 4)]
        scale: u32,
    },
    /// アナザーエデンを起動する
    Launch {
        /// ADB デバイスシリアル
        serial: String,
    },
    /// 連続キャプチャ（指定秒数ごとに保存）
    Record {
        /// ADB デバイスシリアル
        serial: String,
        /// 出力ディレクトリ
        #[arg(default_value = "templates/captures")]
        output_dir: PathBuf,
        /// キャプチャ間隔（秒）
        #[arg(default_value_t = 2)]
        interval: u64,
        /// キャプチャ枚数
        #[arg(default_value_t = 10)]
        count: usize,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Capture { serial, output } => {
            let png_data = run_adb_exec_out(&serial, "screencap -p")?;
            let img = image::load_from_memory(&png_data)?;
            img.save(&output)?;
            println!("Captured: {}x{} → {:?}", img.width(), img.height(), output);
        }
        Commands::Extract {
            screenshot,
            x,
            y,
            width,
            height,
            output,
        } => {
            let img = image::open(&screenshot)?;
            let sub_img = img.crop_imm(x, y, width, height);
            sub_img.save(&output)?;
            println!(
                "Extracted ({},{}) {}x{} → {:?}",
                x, y, width, height, output
            );
        }
        Commands::Match {
            screenshot,
            template,
            threshold,
            scale,
        } => {
            let haystack_orig = image::open(&screenshot)?;
            let needle_orig = image::open(&template)?;

            println!("Screenshot: {}x{}", haystack_orig.width(), haystack_orig.height());
            println!("Template:   {}x{}", needle_orig.width(), needle_orig.height());
            println!("Scale:      1/{}", scale);

            // ダウンスケール
            let haystack = haystack_orig.resize_exact(
                haystack_orig.width() / scale,
                haystack_orig.height() / scale,
                image::imageops::FilterType::Triangle,
            );
            let needle = needle_orig.resize_exact(
                needle_orig.width() / scale,
                needle_orig.height() / scale,
                image::imageops::FilterType::Triangle,
            );

            println!("Scaled screenshot: {}x{}", haystack.width(), haystack.height());
            println!("Scaled template:   {}x{}", needle.width(), needle.height());

            let haystack_gray = haystack.to_luma8();
            let needle_gray = needle.to_luma8();

            if needle_gray.width() > haystack_gray.width()
                || needle_gray.height() > haystack_gray.height()
            {
                anyhow::bail!("Template larger than screenshot");
            }

            let result = imageproc::template_matching::match_template(
                &haystack_gray,
                &needle_gray,
                imageproc::template_matching::MatchTemplateMethod::SumOfSquaredErrorsNormalized,
            );

            // 最小値（最良マッチ）を見つける
            let mut best_x = 0u32;
            let mut best_y = 0u32;
            let mut best_sse = f32::MAX;

            for y in 0..result.height() {
                for x in 0..result.width() {
                    let val = result.get_pixel(x, y)[0];
                    if val < best_sse {
                        best_sse = val;
                        best_x = x;
                        best_y = y;
                    }
                }
            }

            // 元の解像度の座標に逆変換
            let orig_x = best_x * scale;
            let orig_y = best_y * scale;
            let confidence = 1.0 - best_sse;
            println!("Best match (scaled): ({}, {}) → (original): ({}, {})", best_x, best_y, orig_x, orig_y);
            println!("SSE={:.6} Confidence={:.4}", best_sse, confidence);

            if confidence >= threshold {
                println!("✅ MATCH (confidence {:.2}% >= threshold {:.2}%)", confidence * 100.0, threshold * 100.0);
            } else {
                println!("❌ NO MATCH (confidence {:.2}% < threshold {:.2}%)", confidence * 100.0, threshold * 100.0);
            }
        }
        Commands::Launch { serial } => {
            let output = std::process::Command::new("adb")
                .args(["-s", &serial, "shell", "am start", "-n", "net.wrightflyer.anothereden/net.wrightflyer.toybox.AppActivity"])
                .output()?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            println!("{}", stdout.trim());
        }
        Commands::Record {
            serial,
            output_dir,
            interval,
            count,
        } => {
            std::fs::create_dir_all(&output_dir)?;
            for i in 0..count {
                let filename = format!("capture_{:03}.png", i);
                let path = output_dir.join(&filename);
                let png_data = run_adb_exec_out(&serial, "screencap -p")?;
                let img = image::load_from_memory(&png_data)?;
                img.save(&path)?;
                println!("[{}/{}] Saved: {:?}", i + 1, count, path);
                if i < count - 1 {
                    std::thread::sleep(std::time::Duration::from_secs(interval));
                }
            }
            println!("Done: {} captures saved to {:?}", count, output_dir);
        }
    }

    Ok(())
}

fn run_adb_exec_out(serial: &str, command: &str) -> anyhow::Result<Vec<u8>> {
    let output = std::process::Command::new("adb")
        .args(["-s", serial, "exec-out", command])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ADB exec-out failed: {}", stderr);
    }

    Ok(output.stdout)
}
