#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use std::path::PathBuf;

use anaden_vision::ScreenScaler;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

const PC_FRAME_W: u32 = 1258;
const PC_FRAME_H: u32 = 708;

#[test]
fn pc_capture_probe_has_measured_1258x708_dimensions() {
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
    let probe = repo_root().join("templates/captures/field_pc_probe.png");
    let img = image::open(&probe).expect("capture_probe.png");
    assert_eq!(img.width(), PC_FRAME_W);

    let roi_x_end = 1080u32 + 180;
    assert!(
        roi_x_end > img.width(),
        "ROI 右端({roi_x_end})は 1258 幅フレームをはみ出す = 1280 基準座標のまま 1258 空間へ適用されている証拠"
    );
}
