//! PC (Windows 16:9) キャプチャの座標系契約テスト (旧 T7 改訂)。
//!
//! 【背景】
//! Issue #188 で Android (20:9) テンプレート資産 (templates/pipelines/{field_loop,
//! nav_to_field,worldmap_loop,_title_load}) を削除し PC 専用化した。本ファイルは
//! PC 専用化後も意味を持つ契約のみを残す:
//! - Win32Capture(PrintWindow) 実測フレーム寸法 1258x708 の固定化 (T1/T2 実測)。
//! - `ScreenScaler::normalize` は常に 1280 幅基準へリサイズする (16:9 を保存し
//!   1258x708 → 1280x720)。PC 版テンプレ/ROI は RAW 1258x708 空間でオーサリング
//!   され、detect が roi_to_normalized/needle_to_normalized で 1280 空間へスケール
//!   する設計と対をなす契約。
//! - 1280 基準の x 座標が 1258 幅 RAW 空間をはみ出す具体例 (座標系混用の検知)。
//!
//! 【旧 T7 の 20:9→16:9 劣化証明について】
//! 20:9 テンプレ (旧 field_loop/hud_tr.png) を 16:9 フレームへ流用すると conf 0.99 →
//! 0.67 程度に劣化する検証は、Issue #188 でテンプレ資産とともに削除した。

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use anaden_vision::ScreenScaler;

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
fn screen_scaler_normalizes_pc_capture_to_1280_base() {
    // normalize は常に1280幅基準へリサイズする(早期 return 廃止)。
    // PC キャプチャ 1258x708 → 1280x720(16:9 を保存)。
    let probe = repo_root().join("templates/captures/field_pc_probe.png");
    let img = image::open(&probe).expect("capture_probe.png");
    let scaler = ScreenScaler::new();
    let normalized = scaler.normalize(&img);
    assert_eq!(
        normalized.width(),
        1280,
        "PC キャプチャは normalize で1280幅へリサイズされる"
    );
    assert_eq!(
        normalized.height(),
        720,
        "高さは 708*(1280/1258)=720.4→720(16:9 を保存)"
    );
}

#[test]
fn x_1080_partially_clips_on_1258_width_pc_frame() {
    // 座標系混用の検知: 1280 基準の ROI x=1080..1260 は 1258 幅 RAW フレームに対し
    // 右端が 2px はみ出す。pipeline.rs::crop_imm は clamp するため例外にはならないが、
    // 「1280 基準座標を 1258 RAW 空間へそのまま適用すると位置がずれる」具体例。
    // (旧 android 版 tap_hud_tr.toml の ROI 由来 — Issue #188 で資産削除後も
    //  座標系契約の回帰検知として固定化する)
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
