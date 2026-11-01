#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use anaden_cli_contract::{
    EXIT_ALREADY_OR_LAUNCHED, EXIT_HARDCERROR, EXIT_NOT_READY, EXIT_TIMEOUT, ensure_open_exit_code,
    ensure_outcome_label, standalone_exit_code,
};
use anaden_device::{EnsureOutcome, ReadinessSample, ReadinessVerdict, classify_readiness};

#[test]
fn not_ready_maps_to_distinct_nonzero_exit() {
    let code = ensure_open_exit_code(&EnsureOutcome::NotReady);
    assert_eq!(code, EXIT_NOT_READY);
    assert_eq!(code, 3);
    assert_ne!(code, EXIT_ALREADY_OR_LAUNCHED);
    assert_ne!(code, EXIT_HARDCERROR);
    assert_ne!(code, EXIT_TIMEOUT);
}

#[test]
fn not_ready_maps_to_three_via_standalone() {
    let not_ready: Result<&EnsureOutcome, &str> = Ok(&EnsureOutcome::NotReady);
    assert_eq!(standalone_exit_code(not_ready), EXIT_NOT_READY);
    let timeout: Result<&EnsureOutcome, &str> = Ok(&EnsureOutcome::Timeout);
    assert_ne!(
        standalone_exit_code(not_ready),
        standalone_exit_code(timeout)
    );
}

#[test]
fn exit_not_ready_constant_is_three() {
    assert_eq!(EXIT_NOT_READY, 3);
}

#[test]
fn not_ready_label_mentions_splash_hang() {
    let label = ensure_outcome_label(&EnsureOutcome::NotReady);
    assert!(
        label.contains("splash-hang"),
        "label must name the failure mode: {label}"
    );
    assert!(
        label.contains("not-ready"),
        "label must name not-ready: {label}"
    );
}

#[test]
fn field_signature_splash_hang_classifies_to_not_ready_exit_path() {
    let samples = vec![
        ReadinessSample {
            digest: anaden_device::FrameDigest(0x2fad_4aae_dead_beef),
            socket_count: 0,
        };
        3
    ];
    let verdict = classify_readiness(&samples);
    assert_eq!(verdict, ReadinessVerdict::SplashHang);
    assert_eq!(
        ensure_open_exit_code(&EnsureOutcome::NotReady),
        EXIT_NOT_READY
    );
}

#[test]
fn healthy_boot_signature_does_not_reach_not_ready() {
    let samples = vec![
        ReadinessSample {
            digest: anaden_device::FrameDigest(1),
            socket_count: 0,
        },
        ReadinessSample {
            digest: anaden_device::FrameDigest(2),
            socket_count: 0,
        },
    ];
    assert_eq!(classify_readiness(&samples), ReadinessVerdict::Active);
    assert_eq!(
        ensure_open_exit_code(&EnsureOutcome::Launched),
        EXIT_ALREADY_OR_LAUNCHED
    );
}
