//! Issue #190 Shard 3/3 — スクリプト模擬オーサリングセッションの実演 example。
//!
//! GUI 実演オーサリング (Shard 1 の `AuthoringSession` + Shard 2 のパネル) を
//! スクリプトテーブルで模擬駆動し、1 本のシナリオを保存する:
//!
//! ```text
//! cargo run -p anaden-studio --example authoring_demo [frames_dir]
//! ```
//!
//! - `frames_dir` (既定 `.omc/logs/run-issue-190-e2e/live-frames`): 実機 E2E 直前の
//!   PC 版キャプチャ (読み取り専用 probe `probe_windows_capture` 採取の
//!   `live-probe-01.png`)。E2E 実行時点の実画面から録ることで、保存シナリオが
//!   実行時の画面レイアウトと一致する (run-180 旧フレームは HUD レイアウトが
//!   現行と異なり offline 検証で NoMatch になったため不使用 — 調整 1 回の記録)。
//! - スクリプトテーブル ([`SCRIPT`]): フレーム → タップ座標 + 認識領域 の列。
//!   各エントリが `push_frame` → `record_tap` / `record_region` → ステップ確定に対応。
//! - 保存先は `.omc/logs/run-issue-190-e2e/authored-<name>/` (repo の
//!   `templates/` には書かない — テンプレートバンク監査テストと干渉させない)。
//!
//! ## 座標空間 (TaskDef 実行契約との整合)
//!
//! セッションへ push するフレームは **raw-1258x708 空間** (PC 版 GetClientRect 実測 =
//! `anaden-vision` scale.rs の `PC_CLIENT_*_MEASURED` と同一契約) へスケール済みで
//! ある。TaskDef の roi/テンプレートはこの空間で定義される前提で `detect` が
//! `roi_to_normalized` / `needle_to_normalized` で実行時キャプチャ (1280 基準) へ
//! 動的スケールするため、この空間で録ることで保存シナリオが `anaden run` の
//! 実行経路 (capture → 黒帯クロップ → 1280 正規化 → detect) と正確に整合する
//! (raw 1917x1078 / 1955x1100 等キャプチャ寸法が変わってもdetect が動的スケールする)。
//!
//! ## 出力
//!
//! ステップ概要 (タスク名・roi・タップアンカー・テンプレ寸法)・テンプレート品質
//! 警告 (stddev / needle-roi・fail-visible)・保存先と E2E 実行コマンド例。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anaden_studio::authoring_session::{AuthoringSession, GestureOutcome};
use anaden_studio::scenario_validate::{SCREEN_HEIGHT, SCREEN_WIDTH};
use anyhow::{Context, Result, bail};
use image::DynamicImage;
use image::imageops::FilterType;

/// スクリプトテーブルの 1 エントリ (= 1 ステップ)。
struct ScriptStep {
    /// frames_dir 内のフレームファイル名。
    frame: &'static str,
    /// ステップの概要 (何をクリックするか)。
    desc: &'static str,
    /// 記録タップ位置 (raw-1258x708 空間・検証アンカー)。
    tap: (u32, u32),
    /// 認識領域 (raw-1258x708 空間 `[x, y, w, h]`)。クロップがテンプレートになる。
    region: [u32; 4],
}

/// 実演スクリプト (E2E 直前の実機キャプチャに対する模擬オーサリング)。
///
/// 事後確定: probe フレーム (`live-probe-01.png`) は **タイトル画面** だった
/// (フィールド HUD ではない。当初の「ミニマップ UI」解釈は誤り)。両ステップの
/// テンプレートはタイトル画面右上の水彩アートワーク領域クロップであり、
/// クリックは不活性 (画面反応なし = FiredUnverified 扱い)。
/// run-180 での発火実績 (`TapHudTrPc` がミニマップ領域で 3 発火・クリック後も
/// テンプレート残存) と同種の「マッチはするが反応しない」不活性クリック対象。
/// - Step 1: タイトル右上アートワーク (version 表記周辺。キャプチャ時点で静的)。
/// - Step 2: その直下のアートワーク領域。next 空 (終端)。
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

/// フレーム資産の既定ディレクトリ (E2E 直前の読み取り専用 probe 採取先)。
const DEFAULT_FRAMES_DIR: &str = ".omc/logs/run-issue-190-e2e/live-frames";

/// シナリオ保存ルート (repo の templates/ 外・監査テストと干渉しない)。
const SAVE_ROOT: &str = ".omc/logs/run-issue-190-e2e";

/// シナリオ名 (保存先 `<SAVE_ROOT>/authored-<name>/`)。
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

    // フレームは同一ファイルを複数ステップで使いうるためキャッシュする。
    let mut frame_cache: HashMap<String, DynamicImage> = HashMap::new();
    let mut session = AuthoringSession::new(&format!("authored-{SCENARIO_NAME}"));
    // 閾値は実機発火実績のある PC pipeline (login 0.80 / field_loop_pc 0.80) に揃える。
    // セッション既定 0.9 に対し、1258 空間テンプレを 1280 正規化キャプチャへ一致させる
    // 再サンプル (Lanczos3→Triangle) で conf が低下する実績 (TapHudTrPc コメント) による。
    session.set_threshold(0.80);

    // Step 1 の前に「誤タップ → 確定 → undo」を実演する (オーサリング操作の一部として)。
    // 模擬とはいえ実フローと同じ API 経路 (record_tap/record_region/undo) を通す。
    if let Some(first) = SCRIPT.first() {
        let frame = load_authored_frame(&frames_dir, first.frame, &mut frame_cache)?;
        session.push_frame(&frame);
        // 誤った領域で確定してしまう → 1 ステップ確定 → 取り消し。
        let bad_region = [0, 0, 40, 30];
        match session.record_tap((10, 10))? {
            GestureOutcome::Pending => {}
            GestureOutcome::Confirmed { .. } => {}
        }
        match session.record_region(bad_region)? {
            GestureOutcome::Confirmed { .. } => {}
            GestureOutcome::Pending => {}
        }
        let undone = session.undo();
        println!("undo 実演 (誤ステップ取り消し): 取り消した={undone}");
        // undo 後の番号再利用を明示 (steps.len()+1 採番のため衝突しない)。
    }

    for (i, step) in SCRIPT.iter().enumerate() {
        let frame = load_authored_frame(&frames_dir, step.frame, &mut frame_cache)?;
        session.push_frame(&frame);
        // タップ → 領域 の順で記録 (逆順も可・セッションは順非依存)。
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

    // 保存 (repo の templates/ 外へ)。既存 dir は所有権ガードで拒否される
    // (再実行時は .omc/logs/run-issue-190-e2e/authored-* を手動削除)。
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

/// フレームを読み、黒帯クロップ + raw-1258x708 空間へスケールして返す。
///
/// クロップは `anaden run` の実行経路 (capture → `crop_to_content` → 正規化) と
/// 同一の前処理。`raw → cropped → 1258x708` の寸法遷移も診断用に印字する。
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
    let cropped = anaden_vision::crop_to_content(&raw);
    let (cw, ch) = (cropped.width(), cropped.height());
    let authored = cropped.resize_exact(SCREEN_WIDTH, SCREEN_HEIGHT, FilterType::Lanczos3);
    println!(
        "  frame {file}: raw {rw}x{rh} -> crop {cw}x{ch} -> authored {SCREEN_WIDTH}x{SCREEN_HEIGHT}"
    );
    cache.insert(file.to_string(), authored.clone());
    Ok(authored)
}
