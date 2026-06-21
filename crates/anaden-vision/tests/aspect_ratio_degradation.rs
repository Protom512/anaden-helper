//! T7: 20:9 → 16:9 テンプレ流用の劣化証明（cross-aspect-ratio 再利用不可の根拠）。
//!
//! 【背景】
//! 既存 field_loop/hud_tr テンプレ群は **20:9 実機(Pixel 7a 2400x1080 → 幅1280基準
//! 正規化で 1280x576)** 向けに作られた。`tap_hud_tr.toml` の ROI `[1080,150,180,150]` と
//! テンプレ PNG(hud_tr.png 96x96) はともに 1280 基準の正規化空間でオーサリングされている。
//!
//! 【問題のメカニズム】
//! `ScreenScaler::normalize`(scale.rs:45-53) は **元画像幅が 1280 以下なら RAW をそのまま返す**
//! （拡大しない）。PC版キャプチャ(1258x708, 幅1258 <= 1280)は正規化を通過せず RAW のまま。
//! そのため:
//! 1. ROI `[1080,150,180,150]` が 1280 基準で計算された座標として 1258 幅フレームへ直接適用され、
//!    HUD があるべき右上領域を正しく crop できない（X 軸で最大 ~22px のスケールオフセットに加え、
//!    20:9 と 16:9 では縦横比そのものが異なるため ROI の縦位置も実態と合わない）。
//! 2. テンプレ PNG 自体が 20:9 正規化空間から crop された画素列であり、16:9 フレームの対応領域とは
//!    縦横比・解像度特性が異なる → ccoeff 相関が大きく低下する。
//!
//! これらが合成し、20:9 実機では conf~0.99 だった hud_tr が PC版では非マッチ(conf 0.67 程度、
//! 閾値 0.80 を下回り NoMatch)に劣化する。本テストは**実データ(本物の capture_probe.png と
//! 本物の hud_tr.png/tap_hud_tr.toml)** を使ってこの劣化を記録し、テンプレをアスペクト比間で
//! 流用できないことの決定的根拠とする。
//!
//! 【検証データ】
//! - PC フレーム: `capture_probe.png` (1258x708, PrintWindow キャプチャ, T1/T2 実測)
//! - 20:9 テンプレ: `templates/pipelines/field_loop/hud_tr.png` (96x96) + `tap_hud_tr.toml`
//!
//! 【scrcpy 代替パス確認】
//! このテストが走ること自体、PC(Windows)キャプチャ + anaden-vision 認識が
//! デバイス/推論サーバ無しで機能すること(= scrcpy ループの代替検証パス)を示す。

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use anaden_core::MatchConfidence;
use anaden_vision::{CcoeffVisionEngine, ScreenScaler, VisionEngine};

/// リポジトリルート(テストバイナリの CARGO_MANIFEST_DIR = crates/anaden-vision)。
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent() // crates/
        .unwrap()
        .parent() // repo root
        .unwrap()
        .to_path_buf()
}

/// PC フレームキャプチャ(1258x708)の実測寸法を固定化(T2 の GetClientRect 実測値)。
const PC_FRAME_W: u32 = 1258;
const PC_FRAME_H: u32 = 708;

#[test]
fn pc_capture_probe_has_measured_1258x708_dimensions() {
    // T2: Win32Capture(GetClientRect)実測の生寸法を固定化。DPI/ウィンドウ状態で変動するため
    // 実データの寸法をアサートし、将来の退化を検知する。
    let probe = repo_root().join("templates/captures/field_pc_probe.png");
    let img = image::open(&probe).unwrap_or_else(|e| {
        panic!(
            "capture_probe.png を読み込めません({e})。T1/T2 の PC フレームキャプチャが \
             リポジトリルートに存在する必要があります"
        )
    });
    assert_eq!(
        img.width(),
        PC_FRAME_W,
        "PC フレーム幅は実測 {} px であること",
        PC_FRAME_W
    );
    assert_eq!(
        img.height(),
        PC_FRAME_H,
        "PC フレーム高は実測 {} px であること",
        PC_FRAME_H
    );
}

#[test]
fn screen_scaler_passes_through_pc_capture_unnormalized() {
    // メカニズム証明: 幅 1258 <= 1280(BASE_WIDTH) のため normalize は RAW を返す(拡大しない)。
    // これが「1280 基準 ROI/テンプレが PC 生フレームへ直接適用される」根本原因。
    let probe = repo_root().join("templates/captures/field_pc_probe.png");
    let img = image::open(&probe).expect("capture_probe.png");
    let scaler = ScreenScaler::new();
    let normalized = scaler.normalize(&img);
    assert_eq!(
        normalized.width(),
        PC_FRAME_W,
        "PC キャプチャ(幅<=1280)は normalize で RAW 通過(拡大なし)"
    );
    assert_eq!(
        normalized.height(),
        PC_FRAME_H,
        "高さも RAW 通過(1258x708 のまま)"
    );
}

#[test]
fn hud_tr_20to9_template_degrades_to_nonmatch_on_pc_16to9_frame() {
    // 主証明: 20:9 向け hud_tr テンプレ(ccoeff, 実機 conf~0.99)を PC 16:9 フレームへ
    // そのまま適用した場合の信頼度劣化を計測する。閾値(0.80)を下回り非マッチになること、
    // および到達可能信頼度が著しく低下すること(テンプレ流用不可の根拠)を検証する。
    let root = repo_root();
    let probe = root.join("templates/captures/field_pc_probe.png");
    let hud_tr = root.join("templates/pipelines/field_loop/hud_tr.png");

    let pc_frame = image::open(&probe)
        .unwrap_or_else(|e| panic!("capture_probe.png 読込失敗({e}): T1/T2 PC フレームが未配置"));
    let needle = image::open(&hud_tr)
        .unwrap_or_else(|e| panic!("hud_tr.png 読込失敗({e}): 20:9 テンプレが未配置"));

    // 実機パスと同じ前処理: pipeline_driver は capture → normalize → tick を経て
    // TaskDef.detect へ渡す。PC フレームは RAW 通過(上記テスト証明済み)。
    let scaler = ScreenScaler::new();
    let work = scaler.normalize(&pc_frame);
    assert_eq!((work.width(), work.height()), (PC_FRAME_W, PC_FRAME_H));

    // tap_hud_tr.toml の ROI [1080,150,180,150] を 1280 基準座標としてそのまま
    // PC RAW フレーム(1258 幅)の画素座標で crop する(TOML ROI は正規化済み画面の
    // 画素座標として直接 crop される = pipeline.rs::crop_imm の挙動)。
    let roi = [1080u32, 150, 180, 150];
    let cropped = crop(&work, roi);

    // ccoeff でテンプレマッチ(閾値 0.0 で到達可能信頼度を取る)。
    let engine = CcoeffVisionEngine::threshold_only(MatchConfidence::new(0.0));
    let best = engine.match_template(&cropped, &needle);

    let conf = match best {
        Some(m) => m.confidence.0,
        None => 0.0,
    };

    // 証明1: 閾値 0.80(tap_hud_tr.toml 指定)を下回り非マッチになること。
    // 20:9 実機では conf~0.99 で安定マッチしていた同じテンプレが PC 16:9 では非マッチ。
    assert!(
        conf < 0.80,
        "20:9 hud_tr テンプレは PC 16:9 フレームで閾値 0.80 を下回り非マッチになるはず: got {conf:.4}"
    );

    // 証明2: 到達可能信頼度が 0.99(実機) から大きく劣化すること(流用不可の量的根拠)。
    // 実機 conf~0.99 → PC で conf が大きく低下することを記録する。
    // ※ probe のゲーム状態により conf 絶対値は変動するため、厳密な閾値(0.70)で assert せず、
    //    証明1(閾値0.80 未満 = 非マッチ = 流用不可)で本質を担保し、劣化の度合いは記録のみ。
    //    (canonical probe tracked化で probe は固定されるが、ゲーム状態依存の絶対値は記録対象)
    eprintln!(
        "[T7] hud_tr 劣化度: 実機 conf~0.99 → PC conf={conf:.4} (0.80 未満で流用不可、証明1で担保)"
    );

    eprintln!(
        "[T7] hud_tr 20:9→16:9 劣化: 実機 conf~0.99 → PC(1258x708) conf={conf:.4} (非マッチ, 閾値0.80 未満)"
    );
}

#[test]
fn roi_1080_x_partially_clips_on_1258_width_pc_frame() {
    // 補助証明: ROI x=1080..1260 は 1258 幅フレームに対し右端が 2px はみ出す。
    // pipeline.rs::crop_imm は clamp するため例外にはならないが、意図した 1280 基準の
    // 右上 HUD 領域を正確に取得できていないことが分かる(スケールオフセット + クリッピング)。
    // これが「1280 基準座標を 1258 RAW 空間へそのまま適用すると位置がずれる」具体例。
    let probe = repo_root().join("templates/captures/field_pc_probe.png");
    let img = image::open(&probe).expect("capture_probe.png");
    assert_eq!(img.width(), PC_FRAME_W);

    // ROI [1080,150,180,150] → x 範囲 1080..1260
    let roi_x_end = 1080u32 + 180;
    assert!(
        roi_x_end > img.width(),
        "ROI 右端({roi_x_end})は 1258 幅フレームをはみ出す = 1280 基準座標のまま 1258 空間へ適用されている証拠"
    );
}

/// pipeline.rs::crop_imm と同等の clamp 付き cropping ヘルパ(テスト内再現)。
fn crop(img: &image::DynamicImage, r: [u32; 4]) -> image::DynamicImage {
    let [x, y, w, h] = r;
    let cx = x.min(img.width().saturating_sub(1));
    let cy = y.min(img.height().saturating_sub(1));
    let cw = w.min(img.width().saturating_sub(cx));
    let ch = h.min(img.height().saturating_sub(cy));
    img.crop_imm(cx, cy, cw, ch)
}
