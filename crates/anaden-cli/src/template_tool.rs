//! テンプレート抽出・マッチング検証ツール。
//!
//! 使い方:
//!   cargo run --bin anaden-tool -- capture <serial> <output_dir>
//!   cargo run --bin anaden-tool -- extract <screenshot> <x> <y> <w> <h> <output>
//!   cargo run --bin anaden-tool -- match <screenshot> <template> <threshold>
//!   cargo run --bin anaden-tool -- detect <screenshot> [--templates <dir>]
//!   cargo run --bin anaden-tool -- launch <serial>
//!   cargo run --bin anaden-tool -- record <serial> [options]
//!   cargo run --bin anaden-tool -- explore <serial> [options]
//!   cargo run --bin anaden-tool -- run-pipeline <screenshot> <pipeline_dir> <start_task> [--algorithm sse|ccoeff]

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
    /// 探索的テンプレート自動収集（キャプチャ→グループ化→抽出→検証→保存）
    Explore {
        /// ADB デバイスシリアル
        serial: String,
        /// テンプレート出力ディレクトリ
        #[arg(short, long, default_value = "./templates/scenes")]
        output: PathBuf,
        /// キャプチャ間隔（秒）
        #[arg(short = 'i', long, default_value_t = 3)]
        interval: u64,
        /// 収集時間（秒）。0で無制限（Ctrl+C で停止）
        #[arg(short, long, default_value_t = 60)]
        duration: u64,
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
        Commands::Explore {
            serial,
            output,
            interval,
            duration,
        } => {
            run_explore(&serial, &output, interval, duration)?;
        }
        Commands::Launch { serial } => {
            let output = std::process::Command::new("adb")
                .args([
                    "-s",
                    &serial,
                    "shell",
                    "am start",
                    "-n",
                    "net.wrightflyer.anothereden/net.wrightflyer.toybox.AppActivity",
                ])
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

/// `explore` サブコマンド: 探索的テンプレート自動収集
fn run_explore(
    serial: &str,
    output_dir: &PathBuf,
    interval: u64,
    duration: u64,
) -> anyhow::Result<()> {
    println!("🔍 探索的テンプレート収集開始");
    println!("   デバイス: {}", serial);
    println!("   間隔: {}秒", interval);
    println!(
        "   時間: {}秒{}",
        duration,
        if duration == 0 { " (無制限)" } else { "" }
    );
    println!("   出力: {:?}", output_dir);
    println!();
    println!("💡 ゲームを操作して色々な画面を表示してください。");
    println!("   ツールが自動で画面をグループ化し、テンプレートを抽出します。");
    println!();

    std::fs::create_dir_all(output_dir)?;

    // Phase 1: キャプチャ
    let start = std::time::Instant::now();
    let mut captures: Vec<image::DynamicImage> = Vec::new();

    loop {
        if duration > 0 && start.elapsed().as_secs() >= duration {
            println!("⏱ 指定時間に到達。キャプチャ終了。");
            break;
        }

        match run_adb_exec_out(serial, "screencap -p") {
            Ok(png_data) => match image::load_from_memory(&png_data) {
                Ok(img) => {
                    let elapsed = start.elapsed().as_secs();
                    let sim = if let Some(last) = captures.last() {
                        anaden_vision::compute_similarity(last, &img)
                    } else {
                        1.0
                    };

                    let change_mark = if sim < 0.80 {
                        "🔄 画面変化!"
                    } else if sim < 0.95 {
                        "〜 遷移中..."
                    } else {
                        "  安定"
                    };

                    println!(
                        "[{:3}s] キャプチャ {} ({}x{}) 類似度={:.3} {}",
                        elapsed,
                        captures.len() + 1,
                        img.width(),
                        img.height(),
                        sim,
                        change_mark,
                    );

                    captures.push(img);
                }
                Err(e) => {
                    eprintln!("⚠ 画像デコード失敗: {}", e);
                }
            },
            Err(e) => {
                eprintln!("⚠ ADB キャプチャ失敗: {}", e);
            }
        }

        std::thread::sleep(std::time::Duration::from_secs(interval));
    }

    if captures.len() < 3 {
        anyhow::bail!(
            "キャプチャ数が少なすぎます ({}枚)。ゲームを操作しながら再度実行してください。",
            captures.len()
        );
    }

    println!("\n📊 {} 枚キャプチャ完了。グループ化中...", captures.len());

    // Phase 2: グループ化
    let groups = anaden_vision::group_captures(captures);
    println!("   → {} グループに分割", groups.len());

    for g in &groups {
        println!("   グループ {}: {} 枚", g.index, g.captures.len());
    }

    // 十分なグループがあるか確認
    let usable_groups: Vec<_> = groups.iter().filter(|g| g.captures.len() >= 3).collect();

    if usable_groups.is_empty() {
        anyhow::bail!(
            "十分なキャプチャがあるグループがありません。\
            各画面で最低3秒間静止してください。"
        );
    }

    if usable_groups.len() < 2 {
        println!("\n⚠ グループが1つだけです。他グループとの特異性検証ができません。");
        println!("  テンプレート候補を抽出しますが、誤検出のリスクがあります。");
        println!("  より確実な結果を得るには、複数の異なる画面をキャプチャしてください。");
    }

    // Phase 3 & 4: テンプレート抽出 → 検証 → 保存
    println!("\n🔧 テンプレート抽出と検証中...");

    let results = anaden_vision::collect_templates(&groups);

    if results.is_empty() {
        println!("\n❌ 検証を通過したテンプレートがありません。");
        println!("  原因: 画面の変動が大きすぎる、または複数画面間で差が少ない");
        return Ok(());
    }

    // 結果を保存
    let mut total_saved = 0;
    for (group_idx, verified) in &results {
        let group_dir = output_dir.join(format!("group_{:03}", group_idx));
        std::fs::create_dir_all(&group_dir)?;

        // 代表画像を保存
        let group = groups.iter().find(|g| g.index == *group_idx).unwrap();
        let representative_path = group_dir.join("_representative.png");
        group.captures[0].save(&representative_path)?;

        // 検証通過テンプレートを保存
        for (i, v) in verified.iter().enumerate() {
            let filename = format!("template_{:02}.png", i);
            let path = group_dir.join(&filename);
            v.image.save(&path)?;
            total_saved += 1;

            println!(
                "  ✅ {}/{}: 位置({},{}) 自グループ最低={:.3} 他グループ最高={:.3}",
                group_dir.file_name().unwrap().to_string_lossy(),
                filename,
                v.position.0,
                v.position.1,
                v.own_best_confidence,
                v.other_worst_confidence,
            );
        }
    }

    println!(
        "\n✅ 完了: {} グループから {} テンプレートを保存",
        results.len(),
        total_saved
    );
    println!("   保存先: {:?}", output_dir);
    println!();
    println!("📌 次のステップ:");
    println!("   1. 各 group_XXX/ フォルダの _representative.png を目視確認");
    println!("   2. 内容に合わせてフォルダ名を変更 (例: group_000 → title, group_001 → field)");
    println!("   3. detect コマンドで動作確認:");

    for (group_idx, _) in &results {
        println!(
            "      cargo run --bin anaden-tool -- detect templates/captures/test.png --templates {}/group_{:03}",
            output_dir.display(),
            group_idx
        );
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
