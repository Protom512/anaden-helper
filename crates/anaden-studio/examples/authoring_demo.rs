use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anaden_studio::authoring_session::{AuthoringSession, GestureOutcome};
use anaden_studio::scenario_validate::{SCREEN_HEIGHT, SCREEN_WIDTH};
use anyhow::{Context, Result, bail};
use image::DynamicImage;
use image::imageops::FilterType;

struct ScriptStep {
    frame: &'static str,
    desc: &'static str,
    tap: (u32, u32),
    region: [u32; 4],
}

const SCRIPT: &[ScriptStep] = &[
    ScriptStep {
        frame: "live-probe-01.png",
        desc: "タイトル画面右上のアートワーク領域をクリック (不活性)",
        tap: (1007, 150),
        region: [865, 35, 285, 230],
    },
    ScriptStep {
        frame: "live-probe-01.png",
        desc: "その直下のアートワーク領域をクリック (不活性)",
        tap: (933, 318),
        region: [878, 268, 110, 100],
    },
];

const DEFAULT_FRAMES_DIR: &str = ".omc/logs/run-issue-190-e2e/live-frames";

const SAVE_ROOT: &str = ".omc/logs/run-issue-190-e2e";

const SCENARIO_NAME: &str = "field-hud-demo";

fn main() -> Result<()> {
    let frames_dir: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_FRAMES_DIR.to_string())
        .into();
    if !frames_dir.is_dir() {
        bail!(
            "frames dir not found: {} (既定 {DEFAULT_FRAMES_DIR} は E2E 直前の\
             読み取り専用 probe 採取先。パスを引数で指定してください)",
            frames_dir.display()
        );
    }

    println!("== Issue #190 スクリプト模擬オーサリング ==");
    println!("frames dir: {}", frames_dir.display());

    let mut frame_cache: HashMap<String, DynamicImage> = HashMap::new();
    let mut session = AuthoringSession::new(&format!("authored-{SCENARIO_NAME}"));
    session.set_threshold(0.80);

    if let Some(first) = SCRIPT.first() {
        let frame = load_authored_frame(&frames_dir, first.frame, &mut frame_cache)?;
        session.push_frame(&frame);
        let _ = session.record_tap((10, 10))?;
        let _ = session.record_region([0, 0, 40, 30])?;
        let undone = session.undo();
        println!("undo 実演 (誤ステップ取り消し): 取り消した={undone}");
    }

    run_script_steps(&mut session, &frames_dir, &mut frame_cache)?;

    let root = Path::new(SAVE_ROOT);
    match session.save(root) {
        Ok(outcome) => {
            println!("保存先: {}", outcome.dir.display());
            for warning in &outcome.warnings {
                println!("保存時テンプレ品質警告: {warning}");
            }
        }
        Err(e) => {
            bail!(
                "save failed: {e} (再実行時は {}/authored-{SCENARIO_NAME} を削除してください)",
                root.display()
            );
        }
    }

    println!(
        "E2E 実行例: cargo run -p anaden-cli -- run --target windows \
         {SAVE_ROOT}/authored-{SCENARIO_NAME} AuthoredStep01 \
         --max-iters 10 --interval 2 --recover-launch false \
         --evidence-run-id run-issue-190-e2e"
    );
    Ok(())
}

fn run_script_steps(
    session: &mut AuthoringSession,
    frames_dir: &Path,
    frame_cache: &mut HashMap<String, DynamicImage>,
) -> Result<()> {
    for (i, step) in SCRIPT.iter().enumerate() {
        let frame = load_authored_frame(frames_dir, step.frame, frame_cache)?;
        session.push_frame(&frame);
        let tap_outcome = session.record_tap(step.tap)?;
        let confirmed = match tap_outcome {
            GestureOutcome::Pending => match session.record_region(step.region)? {
                GestureOutcome::Confirmed { warnings } => Some(warnings),
                GestureOutcome::Pending => None,
            },
            GestureOutcome::Confirmed { warnings } => Some(warnings),
        };
        let Some(step_warnings) = confirmed else {
            bail!(
                "script step {} did not confirm (internal inconsistency)",
                i + 1
            );
        };
        let confirmed_step = session
            .steps()
            .last()
            .context("confirmed step must exist")?;
        println!(
            "[{}/{}] {} — frame={} task={} roi={:?} tap=({},{}) template={}x{}",
            i + 1,
            SCRIPT.len(),
            step.desc,
            step.frame,
            confirmed_step.task.name,
            confirmed_step.task.roi,
            step.tap.0,
            step.tap.1,
            confirmed_step.template.width(),
            confirmed_step.template.height(),
        );
        for warning in &step_warnings {
            println!("    警告: {warning}");
        }
    }
    Ok(())
}

fn load_authored_frame(
    frames_dir: &Path,
    file: &str,
    cache: &mut HashMap<String, DynamicImage>,
) -> Result<DynamicImage> {
    if let Some(cached) = cache.get(file) {
        return Ok(cached.clone());
    }
    let path = frames_dir.join(file);
    let raw =
        image::open(&path).with_context(|| format!("frame open failed: {}", path.display()))?;
    let (rw, rh) = (raw.width(), raw.height());
    let (cropped, _crop_info) = anaden_vision::crop_to_canvas_with_info(&raw);
    let (cw, ch) = (cropped.width(), cropped.height());
    let authored = cropped.resize_exact(SCREEN_WIDTH, SCREEN_HEIGHT, FilterType::Lanczos3);
    println!(
        "  frame {file}: raw {rw}x{rh} -> crop {cw}x{ch} -> authored {SCREEN_WIDTH}x{SCREEN_HEIGHT}"
    );
    cache.insert(file.to_string(), authored.clone());
    Ok(authored)
}
