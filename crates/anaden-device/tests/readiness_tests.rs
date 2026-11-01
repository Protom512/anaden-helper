#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use anaden_device::{
    FrameDigest, ReadinessSample, ReadinessVerdict, classify_readiness, frame_digest,
};
use image::{DynamicImage, RgbaImage};

fn samples_uniform(digest_value: u64, socket_count: u32, count: usize) -> Vec<ReadinessSample> {
    vec![
        ReadinessSample {
            digest: FrameDigest(digest_value),
            socket_count,
        };
        count
    ]
}

fn samples_frozen_offline(digest_values: &[u64]) -> Vec<ReadinessSample> {
    digest_values
        .iter()
        .map(|&d| ReadinessSample {
            digest: FrameDigest(d),
            socket_count: 0,
        })
        .collect()
}

#[test]
fn frozen_frames_with_zero_sockets_is_splash_hang() {
    let samples = samples_uniform(0x2fad_4aae_dead_beef, 0, 5);
    assert_eq!(classify_readiness(&samples), ReadinessVerdict::SplashHang);
}

#[test]
fn frozen_frames_with_sockets_in_any_sample_is_active() {
    let mut samples = samples_uniform(42, 0, 4);
    samples[2].socket_count = 3;
    assert_eq!(classify_readiness(&samples), ReadinessVerdict::Active);
}

#[test]
fn digest_change_on_second_sample_is_active() {
    let samples = samples_frozen_offline(&[100, 200, 200]);
    assert_eq!(classify_readiness(&samples), ReadinessVerdict::Active);
}

#[test]
fn digest_change_on_last_sample_is_active() {
    let samples = samples_frozen_offline(&[100, 100, 100, 200]);
    assert_eq!(classify_readiness(&samples), ReadinessVerdict::Active);
}

#[test]
fn two_frozen_offline_samples_is_splash_hang() {
    let samples = samples_frozen_offline(&[7, 7]);
    assert_eq!(classify_readiness(&samples), ReadinessVerdict::SplashHang);
}

#[test]
fn empty_samples_fail_closed_to_splash_hang() {
    assert_eq!(classify_readiness(&[]), ReadinessVerdict::SplashHang);
}

#[test]
fn single_frozen_offline_sample_fail_closed_to_splash_hang() {
    let samples = samples_frozen_offline(&[9]);
    assert_eq!(classify_readiness(&samples), ReadinessVerdict::SplashHang);
}

#[test]
fn single_sample_with_socket_is_active() {
    let samples = vec![ReadinessSample {
        digest: FrameDigest(9),
        socket_count: 1,
    }];
    assert_eq!(classify_readiness(&samples), ReadinessVerdict::Active);
}

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
    let a = solid_image(4, 1, [7, 7, 7, 255]);
    let b = solid_image(2, 2, [7, 7, 7, 255]);
    assert_ne!(frame_digest(&a), frame_digest(&b));
}

#[test]
fn frame_digest_end_to_end_with_classify() {
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
