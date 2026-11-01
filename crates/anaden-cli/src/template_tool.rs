mod plan_data;
mod plan_tool;

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
    Extract {
        screenshot: PathBuf,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        output: PathBuf,
    },
    Match {
        screenshot: PathBuf,
        template: PathBuf,
        #[arg(default_value_t = 0.95)]
        threshold: f32,
        #[arg(short, long, default_value_t = 4)]
        scale: u32,
    },
    Detect {
        screenshot: PathBuf,
        #[arg(short, long, default_value = "./templates/scenes")]
        templates: PathBuf,
        #[arg(short = 'c', long, default_value_t = 0.85)]
        threshold: f32,
    },
    RunPipeline {
        screenshot: PathBuf,
        pipeline_dir: PathBuf,
        start_task: String,
        #[arg(short, long)]
        algorithm: Option<String>,
    },
    PlanTenmei {
        #[arg(required = true)]
        chars: Vec<String>,
        #[arg(short, long, default_value = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/ad_tenmei.toml"))]
        data: PathBuf,
        #[arg(long, default_value = "VH")]
        difficulty: plan_data::Difficulty,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    PlanExpand {
        plan: PathBuf,
        #[arg(short, long, default_value = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/ad_tenmei.toml"))]
        data: PathBuf,
        #[arg(short, long, default_value = concat!(env!("CARGO_MANIFEST_DIR"), "/../../templates/pipelines/ticket_digest"))]
        pipeline: PathBuf,
        #[arg(short, long)]
        out: Option<PathBuf>,
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
        Commands::PlanTenmei {
            chars,
            data,
            difficulty,
            out,
        } => {
            plan_tool::run_plan_tenmei(&chars, &data, difficulty, out.as_deref())?;
        }
        Commands::PlanExpand {
            plan,
            data,
            pipeline,
            out,
        } => {
            plan_tool::run_plan_expand(&plan, &data, &pipeline.to_string_lossy(), out.as_deref())?;
        }
    }

    Ok(())
}

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

    let state = detector.detect_state(&screenshot);
    println!("\n── 投票判定結果 ──");
    println!("  判定: {:?}", state);
    println!("  閾値: {:.2}", threshold);

    Ok(())
}

fn run_pipeline(
    screenshot_path: &PathBuf,
    pipeline_dir: &PathBuf,
    start_task: &str,
    algorithm_override: Option<&str>,
) -> anyhow::Result<()> {
    let override_algo: Option<anaden_vision::Algorithm> = match algorithm_override {
        Some("sse") => Some(anaden_vision::Algorithm::Sse),
        Some("ccoeff") => Some(anaden_vision::Algorithm::Ccoeff),
        Some(other) => {
            anyhow::bail!("--algorithm は `sse` または `ccoeff` です（指定値: {other}）")
        }
        None => None,
    };

    let raw = image::open(screenshot_path)
        .map_err(|e| anyhow::anyhow!("スクリーンショット読込失敗 {:?}: {e}", screenshot_path))?;
    let (orig_w, orig_h) = (raw.width(), raw.height());
    let scaler = anaden_vision::ScreenScaler::new();
    let (screenshot, crop_info) = scaler.normalize_capture(&raw);
    let (norm_w, norm_h) = (screenshot.width(), screenshot.height());

    println!("📷 Screenshot: {}x{} {:?}", orig_w, orig_h, screenshot_path);
    println!(
        "📐 正規化: {}x{} → content {}x{} (offset {},{}) → {}x{} (720p基準/幅1280)",
        orig_w,
        orig_h,
        crop_info.width,
        crop_info.height,
        crop_info.offset_x,
        crop_info.offset_y,
        norm_w,
        norm_h,
    );

    let mut tasks = anaden_vision::load_pipeline(pipeline_dir)
        .map_err(|e| anyhow::anyhow!("パイプライン読込失敗 {:?}: {e}", pipeline_dir))?;
    println!(
        "📁 Pipeline: {} タスク読込 {:?} (開始: {})",
        tasks.len(),
        pipeline_dir,
        start_task,
    );

    if let Some(algo) = override_algo {
        for t in tasks.iter_mut() {
            if t.name == start_task {
                t.algorithm = algo;
            }
        }
    }

    let mut state = anaden_engine::PipelineState::new(start_task);
    let before = state.current().to_string();
    match state.tick(&screenshot, &tasks) {
        Ok(Some(result)) => {
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
        Ok(None) => {
            println!(
                "❌ 非マッチ: {} は現在の画面で検出されませんでした（閾値下または ROI 外）",
                before,
            );
        }
        Err(e) => {
            println!("⚠️ テンプレートエラー: {before}: {e}");
            anyhow::bail!("template error in task '{before}': {e}");
        }
    }

    Ok(())
}
