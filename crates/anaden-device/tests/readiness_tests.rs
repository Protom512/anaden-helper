//! readiness 判定コア (`classify_readiness` / `frame_digest`) の振る舞い検証 (Issue #275)。
//!
//! 実ゲーム・実ウィンドウは使わない。`classify_readiness` は注入サンプル列の
//! 純関数、`frame_digest` はメモリ上で構築した `DynamicImage` の入力で検証する
//! (Issue #275 AC4: 注入キャプチャ/deterministic fake で splash-hang・正常遷移の両ケース)。
//!
//! 判定契約 (本ファイルが固定する直契約 / Issue #275 AC1):
//!   - 全サンプル digest 同一 + 全サンプル socket 0 => SplashHang (splash-hang 実測シグネチャ)
//!   - digest 変化あり                     => Active (レンダ進行)
//!   - digest 不変でも socket > 0          => Active (ネットワーク待ちの可能性)
//!   - サンプル 2 未満 (活性の証拠なし)    => SplashHang (fail-closed)

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use anaden_device::{
    FrameDigest, ReadinessSample, ReadinessVerdict, classify_readiness, frame_digest,
};
use image::{DynamicImage, RgbaImage};

/// 同一 digest・同一 socket_count のサンプル列を組むヘルパ。
fn samples_uniform(digest_value: u64, socket_count: u32, count: usize) -> Vec<ReadinessSample> {
    vec![
        ReadinessSample {
            digest: FrameDigest(digest_value),
            socket_count,
        };
        count
    ]
}

/// digest 値の列からサンプル列を組むヘルパ (socket_count は全て 0)。
fn samples_frozen_offline(digest_values: &[u64]) -> Vec<ReadinessSample> {
    digest_values
        .iter()
        .map(|&d| ReadinessSample {
            digest: FrameDigest(d),
            socket_count: 0,
        })
        .collect()
}

// ---- splash-hang 実測シグネチャの再現 (session-log 2026-11-04 boot1/boot2) ----
// 5 captures すべて同一 MD5 + ソケット 0 => SplashHang。
#[test]
fn frozen_frames_with_zero_sockets_is_splash_hang() {
    let samples = samples_uniform(0x2fad_4aae_dead_beef, 0, 5);
    assert_eq!(classify_readiness(&samples), ReadinessVerdict::SplashHang);
}

#[test]
fn frozen_frames_with_sockets_in_any_sample_is_active() {
    // frame 不変でも 1 サンプルでもソケットがあれば hang と断定しない
    // (ダウンロード・ログイン中の静的画面の可能性)。
    let mut samples = samples_uniform(42, 0, 4);
    samples[2].socket_count = 3;
    assert_eq!(classify_readiness(&samples), ReadinessVerdict::Active);
}

#[test]
fn digest_change_on_second_sample_is_active() {
    // 正常遷移: 最初のフレームから描画が変わる = レンダ進行。
    let samples = samples_frozen_offline(&[100, 200, 200]);
    assert_eq!(classify_readiness(&samples), ReadinessVerdict::Active);
}

#[test]
fn digest_change_on_last_sample_is_active() {
    // 変化の検出が最後のサンプルでも Active (「不変」は全サンプル条件)。
    let samples = samples_frozen_offline(&[100, 100, 100, 200]);
    assert_eq!(classify_readiness(&samples), ReadinessVerdict::Active);
}

#[test]
fn two_frozen_offline_samples_is_splash_hang() {
    // 最小サンプル数 (2) でも不変 + 0 ソケットなら hang。
    let samples = samples_frozen_offline(&[7, 7]);
    assert_eq!(classify_readiness(&samples), ReadinessVerdict::SplashHang);
}

// ---- 縮退入力の fail-closed 契約 ----

#[test]
fn empty_samples_fail_closed_to_splash_hang() {
    // サンプル無し = 活性の証拠無し。vacuous な Active を返さない。
    assert_eq!(classify_readiness(&[]), ReadinessVerdict::SplashHang);
}

#[test]
fn single_frozen_offline_sample_fail_closed_to_splash_hang() {
    let samples = samples_frozen_offline(&[9]);
    assert_eq!(classify_readiness(&samples), ReadinessVerdict::SplashHang);
}

#[test]
fn single_sample_with_socket_is_active() {
    // 1 サンプルでもソケット観測は活性の証拠。
    let samples = vec![ReadinessSample {
        digest: FrameDigest(9),
        socket_count: 1,
    }];
    assert_eq!(classify_readiness(&samples), ReadinessVerdict::Active);
}

// ---- frame_digest: 変化検出の入力側契約 ----

fn solid_image(w: u32, h: u32, rgba: [u8; 4]) -> DynamicImage {
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(w, h, image::Rgba(rgba)))
}

#[test]
fn frame_digest_is_stable_for_identical_images() {
    let a = solid_image(4, 4, [10, 20, 30, 255]);
    let b = solid_image(4, 4, [10, 20, 30, 255]);
    assert_eq!(frame_digest(&a), frame_digest(&b));
}

#[test]
fn frame_digest_differs_when_pixel_changes() {
    let a = solid_image(4, 4, [10, 20, 30, 255]);
    let b = solid_image(4, 4, [10, 20, 31, 255]);
    assert_ne!(frame_digest(&a), frame_digest(&b));
}

#[test]
fn frame_digest_differs_when_dimensions_change() {
    // バイト長が同一 (4x1 と 2x2 は同ピクセル数) でも寸法混入で分離される。
    let a = solid_image(4, 1, [7, 7, 7, 255]);
    let b = solid_image(2, 2, [7, 7, 7, 255]);
    assert_ne!(frame_digest(&a), frame_digest(&b));
}

#[test]
fn frame_digest_end_to_end_with_classify() {
    // 結合: 同一画像のダイジェスト列は SplashHang、変化する列は Active。
    let frozen = vec![
        ReadinessSample {
            digest: frame_digest(&solid_image(2, 2, [1, 2, 3, 255])),
            socket_count: 0,
        };
        3
    ];
    assert_eq!(classify_readiness(&frozen), ReadinessVerdict::SplashHang);

    let changing = vec![
        ReadinessSample {
            digest: frame_digest(&solid_image(2, 2, [1, 2, 3, 255])),
            socket_count: 0,
        },
        ReadinessSample {
            digest: frame_digest(&solid_image(2, 2, [9, 9, 9, 255])),
            socket_count: 0,
        },
    ];
    assert_eq!(classify_readiness(&changing), ReadinessVerdict::Active);
}
