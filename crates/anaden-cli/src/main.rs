//! Another Eden 自動操作ツールの CLI エントリポイント。

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use tracing::info;

use anaden_engine::{AutomationConfig, Orchestrator};

#[derive(Parser, Debug)]
#[command(name = "anaden", about = "Another Eden automation helper")]
struct Cli {
    /// ADB デバイスのシリアル番号または接続先
    #[arg(short, long, default_value = "localhost:5555")]
    device: String,

    /// テンプレート画像のディレクトリパス
    #[arg(short, long, default_value = "./templates/scenes")]
    templates: PathBuf,

    /// メインループの間隔（ミリ秒）
    #[arg(short, long, default_value_t = 500)]
    interval: u64,

    /// テンプレートマッチの信頼度閾値 (0.0〜1.0)
    #[arg(short, long, default_value_t = 0.85)]
    threshold: f32,

    /// 最大実行時間（秒）。0 で無制限。
    #[arg(long, default_value_t = 0)]
    timeout: u64,

    /// 設定ファイルのパス (TOML)
    #[arg(short, long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // ロギングの初期化
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "anaden=info".into()),
        )
        .init();

    let cli = Cli::parse();

    let config = if let Some(config_path) = cli.config {
        // 設定ファイルから読み込み
        let content = std::fs::read_to_string(&config_path)?;
        let mut cfg: AutomationConfig = toml::from_str(&content)?;
        // CLI 引数で上書き
        if cli.device != "localhost:5555" {
            cfg.device_serial = cli.device;
        }
        cfg
    } else {
        AutomationConfig {
            device_serial: cli.device,
            template_dir: cli.templates.to_string_lossy().to_string(),
            loop_interval_ms: cli.interval,
            confidence_threshold: cli.threshold,
            max_runtime_secs: cli.timeout,
            ..Default::default()
        }
    };

    info!("Starting anaden-helper with config: {:?}", config);

    let mut orchestrator = Orchestrator::new(config);

    let summary = orchestrator.run().await?;

    info!("Automation completed: {:?}", summary);
    println!("\n=== 実行結果 ===");
    println!("総ループ回数: {}", summary.total_loops);
    println!("実行時間: {:.1}秒", summary.elapsed_secs);
    println!("終了理由: {}", summary.termination_reason);
    println!("\n状態別滞在回数:");
    for (state, count) in &summary.state_counts {
        println!("  {}: {}", state, count);
    }

    Ok(())
}
