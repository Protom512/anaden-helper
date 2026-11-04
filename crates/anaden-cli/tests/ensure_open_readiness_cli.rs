//! `ensure-open` / `launch` の NotReady (splash-hang) 終了コード契約テスト (Issue #275)。
//!
//! `ensure_open_cli.rs` (Issue #21 AC1-AC4) の拡張契約として、readiness probe が
//! splash-hang を検出した場合の NotReady => exit 3 を固定する。実機を起動せず、
//! `contract` 層 (`anaden_cli_contract`) の純粋関数と device の判定コア純関数で検証する。
//!
//! 契約(本ファイルが固定する直契約 / Issue #275 AC2):
//!   - NotReady => 3 (CI gate 失敗。fail-closed)
//!   - 3 は 0 (成功) / 1 (hard error) / 2 (Timeout) のいずれとも異なる
//!   - 判定コア: 「フレーム不変 + TCP ソケット 0」のサンプル列は SplashHang と判定される
//!     (実測シグネチャ 2026-11-04 session-log boot1/boot2)

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use anaden_cli_contract::{
    EXIT_ALREADY_OR_LAUNCHED, EXIT_HARDCERROR, EXIT_NOT_READY, EXIT_TIMEOUT, ensure_open_exit_code,
    ensure_outcome_label, standalone_exit_code,
};
use anaden_device::{EnsureOutcome, ReadinessSample, ReadinessVerdict, classify_readiness};

// ---- NotReady => exit 3 (fail-closed) ----
#[test]
fn not_ready_maps_to_distinct_nonzero_exit() {
    let code = ensure_open_exit_code(&EnsureOutcome::NotReady);
    assert_eq!(code, EXIT_NOT_READY);
    assert_eq!(code, 3);
    // 既存契約 (0/1/2) と全て区別されることを固定。
    assert_ne!(code, EXIT_ALREADY_OR_LAUNCHED);
    assert_ne!(code, EXIT_HARDCERROR);
    assert_ne!(code, EXIT_TIMEOUT);
}

#[test]
fn not_ready_maps_to_three_via_standalone() {
    // standalone 経路 (ensure-open/launch) の真経路: Ok(NotReady) は exit 3。
    // (E は Result の Ok 側から推論できないため明示型指定 — ensure_open_cli.rs と同じ手法)
    let not_ready: Result<&EnsureOutcome, &str> = Ok(&EnsureOutcome::NotReady);
    assert_eq!(standalone_exit_code(not_ready), EXIT_NOT_READY);
    // Ok 側の他バリアント (Timeout=2) とも区別される。
    let timeout: Result<&EnsureOutcome, &str> = Ok(&EnsureOutcome::Timeout);
    assert_ne!(
        standalone_exit_code(not_ready),
        standalone_exit_code(timeout)
    );
}

#[test]
fn exit_not_ready_constant_is_three() {
    // 契約定数の固定(値変更を即座に気付かせるゲート)。
    assert_eq!(EXIT_NOT_READY, 3);
}

// ---- ラベル契約: NotReady は splash-hang を人間可読に伝える ----
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

// ---- 判定コアとの結合: 実測 splash-hang シグネチャ => SplashHang => (CLI で) exit 3 ----
#[test]
fn field_signature_splash_hang_classifies_to_not_ready_exit_path() {
    // 2026-11-04 live セッション (Issue #275 出所) の splash-hang 実測:
    // フレーム MD5 不変 (複数キャプチャで同一) + TCP ソケット 0。
    let samples = vec![
        ReadinessSample {
            digest: anaden_device::FrameDigest(0x2fad_4aae_dead_beef),
            socket_count: 0,
        };
        3
    ];
    let verdict = classify_readiness(&samples);
    assert_eq!(verdict, ReadinessVerdict::SplashHang);
    // SplashHang を受けた CLI は outcome を NotReady へ差し替え、exit 3 で
    // fail-closed する (run_ensure_open_or_launch + exit_standalone 経路)。
    // ここではその終点の契約 (NotReady => 3) を固定する。
    assert_eq!(
        ensure_open_exit_code(&EnsureOutcome::NotReady),
        EXIT_NOT_READY
    );
}

#[test]
fn healthy_boot_signature_does_not_reach_not_ready() {
    // 正常遷移 (フレーム変化) は Active => outcome は Launched/AlreadyOpen のまま
    // (exit 0)。NotReady は splash-hang 時のみ。
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
