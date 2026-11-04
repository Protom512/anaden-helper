#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use anaden_cli_contract::{
    EXIT_ALREADY_OR_LAUNCHED, EXIT_HARDCERROR, EXIT_TIMEOUT, ensure_open_exit_code,
    standalone_exit_code,
};
use anaden_device::EnsureOutcome;

#[test]
fn already_open_maps_to_zero_exit() {
    assert_eq!(
        ensure_open_exit_code(&EnsureOutcome::AlreadyOpen),
        EXIT_ALREADY_OR_LAUNCHED
    );
    assert_eq!(ensure_open_exit_code(&EnsureOutcome::AlreadyOpen), 0);
}

#[test]
fn launched_maps_to_zero_exit() {
    assert_eq!(
        ensure_open_exit_code(&EnsureOutcome::Launched),
        EXIT_ALREADY_OR_LAUNCHED
    );
    assert_eq!(ensure_open_exit_code(&EnsureOutcome::Launched), 0);
}

#[test]
fn timeout_maps_to_distinct_nonzero_exit() {
    let code = ensure_open_exit_code(&EnsureOutcome::Timeout);
    assert_eq!(code, EXIT_TIMEOUT);
    assert_eq!(code, 2);
    assert_ne!(code, EXIT_ALREADY_OR_LAUNCHED);
    assert_ne!(code, EXIT_HARDCERROR);
}

#[test]
fn hard_error_exit_code_is_one() {
    assert_eq!(EXIT_HARDCERROR, 1);
    assert_ne!(EXIT_HARDCERROR, EXIT_ALREADY_OR_LAUNCHED);
    assert_ne!(EXIT_HARDCERROR, EXIT_TIMEOUT);
}

#[test]
fn hard_error_maps_to_exit_one_via_standalone() {
    let r: Result<&EnsureOutcome, &str> = Err("spawn failed");
    assert_eq!(standalone_exit_code(r), EXIT_HARDCERROR);
    assert_eq!(standalone_exit_code(r), 1);
    assert_eq!(
        standalone_exit_code::<()>(Ok(&EnsureOutcome::Timeout)),
        EXIT_TIMEOUT
    );
}

#[test]
fn exit_code_constants_match_contract() {
    assert_eq!(EXIT_ALREADY_OR_LAUNCHED, 0);
    assert_eq!(EXIT_HARDCERROR, 1);
    assert_eq!(EXIT_TIMEOUT, 2);
}

#[test]
fn all_outcome_variants_project_to_defined_constants() {
    for outcome in [
        EnsureOutcome::AlreadyOpen,
        EnsureOutcome::Launched,
        EnsureOutcome::Timeout,
    ] {
        let code = ensure_open_exit_code(&outcome);
        assert!(
            code == EXIT_ALREADY_OR_LAUNCHED || code == EXIT_HARDCERROR || code == EXIT_TIMEOUT,
            "未知の終了コードに射影された: outcome={outcome:?} code={code}"
        );
    }
}
