//! テンプレート抽出・マッチング検証ツール。
//!
//! 使い方:
//! ```text
//!   cargo run --bin anaden-tool -- extract <screenshot> <x> <y> <w> <h> <output>
//!   cargo run --bin anaden-tool -- match <screenshot> <template> <threshold>
//!   cargo run --bin anaden-tool -- detect <screenshot> [--templates <dir>]
//!   cargo run --bin anaden-tool -- run-pipeline <screenshot> <pipeline_dir> <start_task> [--algorithm sse|ccoeff]
//! ```
//!
//! (Issue #188: ADB 経路の capture / launch / record / explore サブコマンドは
//!  Android サポート削除に伴い廃止)

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "anaden-tool", about = "Template extraction and matching tool")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    // (Issue #188: Android (ADB) 経路の Capture / Explore / Launch / Record
    //  サブコマンドは削除済み)
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
    /// テンプレートマッチングのテスト（単一テンプレート）
    Match {
        /// スクリーンショット画像
        screenshot: PathBuf,
        /// テンプレート画像
        template: PathBuf,
        /// 信頼度閾値 (0.0〜1.0)
        #[arg(default_value_t = 0.95)]
        threshold: f32,
        /// ダウンスケール倍率（例: 4 で 1/4 に縮小してマッチング）
        #[arg(short, long, default_value_t = 4)]
        scale: u32,
    },
    /// 全テンプレートで投票判定（SceneDetector を使用）
    Detect {
        /// スクリーンショット画像
        screenshot: PathBuf,
        /// テンプレートディレクトリ
        #[arg(short, long, default_value = "./templates/scenes")]
        templates: PathBuf,
        /// 信頼度閾値
        #[arg(short = 'c', long, default_value_t = 0.85)]
        threshold: f32,
    },
    /// 宣言的パイプラインを1ステップ実行（認識→コマンド表示→次タスク）。発火しない。
    RunPipeline {
        /// スクリーンショット画像（元解像度PNG）
        screenshot: PathBuf,
        /// `*.toml` を格納したパイプラインディレクトリ
        pipeline_dir: PathBuf,
        /// 開始タスク名（PipelineState の初期 current）
        start_task: String,
        /// algorithm 上書き（`sse` または `ccoeff`）。未指定時は TOML の algorithm を尊重
        #[arg(short, long)]
        algorithm: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
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

            println!(
                "Screenshot: {}x{}",
                haystack_orig.width(),
                haystack_orig.height()
            );
            println!(
                "Template:   {}x{}",
                needle_orig.width(),
                needle_orig.height()
            );
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

            println!(
                "Scaled screenshot: {}x{}",
                haystack.width(),
                haystack.height()
            );
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
            println!(
                "Best match (scaled): ({}, {}) → (original): ({}, {})",
                best_x, best_y, orig_x, orig_y
            );
            println!("SSE={:.6} Confidence={:.4}", best_sse, confidence);

            if confidence >= threshold {
                println!(
                    "✅ MATCH (confidence {:.2}% >= threshold {:.2}%)",
                    confidence * 100.0,
                    threshold * 100.0
                );
            } else {
                println!(
                    "❌ NO MATCH (confidence {:.2}% < threshold {:.2}%)",
                    confidence * 100.0,
                    threshold * 100.0
                );
            }
        }
        Commands::Detect {
            screenshot,
            templates,
            threshold,
        } => {
            run_detect(&screenshot, &templates, threshold)?;
        }
        Commands::RunPipeline {
            screenshot,
            pipeline_dir,
            start_task,
            algorithm,
        } => {
            run_pipeline(
                &screenshot,
                &pipeline_dir,
                &start_task,
                algorithm.as_deref(),
            )?;
        }
    }

    Ok(())
}

/// `detect` サブコマンド: 全テンプレートで投票判定
fn run_detect(
    screenshot_path: &PathBuf,
    template_dir: &PathBuf,
    threshold: f32,
) -> anyhow::Result<()> {
    let screenshot = image::open(screenshot_path)?;
    println!(
        "📷 Screenshot: {}x{} {:?}",
        screenshot.width(),
        screenshot.height(),
        screenshot_path
    );

    // テンプレート読み込み
    let mut store = anaden_vision::TemplateStore::new();
    if template_dir.exists() {
        let count = store.load_from_directory(template_dir)?;
        println!("📁 Loaded {} templates from {:?}", count, template_dir);
    } else {
        anyhow::bail!("Template directory not found: {:?}", template_dir);
    }

    if store.is_empty() {
        anyhow::bail!("No templates loaded");
    }

    let detector = anaden_vision::SceneDetector::with_defaults(store);
    let threshold_conf = anaden_core::MatchConfidence::new(threshold);

    // 各テンプレートの個別マッチ結果を表示
    println!("\n── 個別テンプレート結果 ──");
    for template in detector.template_list() {
        let best = detector.match_single_template(&screenshot, template);
        match best {
            Some(m) => {
                let mark = if m.confidence.exceeds_threshold(&threshold_conf) {
                    "✅"
                } else {
                    "❌"
                };
                println!(
                    "  {} {:?} conf={:.4} ({}) at ({},{})",
                    mark, template.state, m.confidence.0, template.name, m.region.x, m.region.y,
                );
            }
            None => {
                println!("  ⬜ {:?} no match ({})", template.state, template.name);
            }
        }
    }

    // 投票判定結果
    let state = detector.detect_state(&screenshot);
    println!("\n── 投票判定結果 ──");
    println!("  判定: {:?}", state);
    println!("  閾値: {:.2}", threshold);

    Ok(())
}

/// `run-pipeline` サブコマンド: 宣言的パイプラインを1ステップ実行。
///
/// 範囲: 1ステップのみ。`tick` は1回呼んで結果を表示して終了（ライブループ・発火しない）。
/// screenshot は `ScreenScaler` で幅1280へ正規化してから tick に渡す
/// （`TaskDef::detect` は roi を 720p基準座標の画素座標として直接 crop する前提のため）。
fn run_pipeline(
    screenshot_path: &PathBuf,
    pipeline_dir: &PathBuf,
    start_task: &str,
    algorithm_override: Option<&str>,
) -> anyhow::Result<()> {
    // algorithm 上書き文字列を Algorithm へ解決（未指定・不正値は None）
    let override_algo: Option<anaden_vision::Algorithm> = match algorithm_override {
        Some("sse") => Some(anaden_vision::Algorithm::Sse),
        Some("ccoeff") => Some(anaden_vision::Algorithm::Ccoeff),
        Some(other) => {
            anyhow::bail!("--algorithm は `sse` または `ccoeff` です（指定値: {other}）")
        }
        None => None,
    };

    // 1. スクリーンショット読込 + 正規化
    let raw = image::open(screenshot_path)
        .map_err(|e| anyhow::anyhow!("スクリーンショット読込失敗 {:?}: {e}", screenshot_path))?;
    let (orig_w, orig_h) = (raw.width(), raw.height());
    let scaler = anaden_vision::ScreenScaler::new();
    let screenshot = scaler.normalize(&raw);
    let (norm_w, norm_h) = (screenshot.width(), screenshot.height());

    println!("📷 Screenshot: {}x{} {:?}", orig_w, orig_h, screenshot_path);
    println!(
        "📐 正規化: {}x{} → {}x{} (720p基準/幅1280)",
        orig_w, orig_h, norm_w, norm_h,
    );

    // 2. パイプライン読込
    let mut tasks = anaden_vision::load_pipeline(pipeline_dir)
        .map_err(|e| anyhow::anyhow!("パイプライン読込失敗 {:?}: {e}", pipeline_dir))?;
    println!(
        "📁 Pipeline: {} タスク読込 {:?} (開始: {})",
        tasks.len(),
        pipeline_dir,
        start_task,
    );

    // algorithm 上書き: start_task の TaskDef.algorithm を差し替え
    if let Some(algo) = override_algo {
        for t in tasks.iter_mut() {
            if t.name == start_task {
                t.algorithm = algo;
            }
        }
    }

    // 3. 1ステップ実行。tick は内部で current を next へ更新するため、表示用に退避。
    let mut state = anaden_engine::PipelineState::new(start_task);
    let before = state.current().to_string();
    match state.tick(&screenshot, &tasks) {
        Some(result) => {
            let command_str = match result.command {
                Some(anaden_engine::InputCommand::Tap { x, y }) => format!("Tap({x},{y})"),
                Some(anaden_engine::InputCommand::Swipe { from, to }) => {
                    format!("Swipe({:?}→{:?})", from, to)
                }
                None => "なし(DoNothing/Stop)".to_string(),
            };
            let next_str = result.next_current.as_deref().unwrap_or("なし");
            println!(
                "✅ マッチ: {} / 入力: {} / 次: {}",
                before, command_str, next_str,
            );
        }
        None => {
            println!(
                "❌ 非マッチ: {} は現在の画面で検出されませんでした（閾値下または ROI 外）",
                before,
            );
        }
    }

    Ok(())
}
