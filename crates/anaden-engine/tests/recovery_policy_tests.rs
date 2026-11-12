use anaden_device::DeviceError;
use anaden_engine::{
    RecoveryCause, RecoveryPlan, StepErrorKind, classify_device_error, is_recoverable_io_error,
    plan_recovery, recovery_cause_for,
};

#[test]
fn placement_fault_maps_to_the_gate_rejected_error_kind() {
    let e = DeviceError::PlacementFault {
        message: "off-monitor".into(),
    };
    assert_eq!(classify_device_error(&e), StepErrorKind::GateRejected);
}

#[test]
fn command_failure_stays_a_generic_execute_error() {
    let e = DeviceError::CommandFailed {
        message: "sendinput failed".into(),
    };
    assert_eq!(classify_device_error(&e), StepErrorKind::Execute);
}

#[test]
fn only_unknown_errors_are_not_recoverable() {
    assert!(is_recoverable_io_error(StepErrorKind::GateRejected));
    assert!(is_recoverable_io_error(StepErrorKind::Execute));
    assert!(is_recoverable_io_error(StepErrorKind::Capture));
    assert!(is_recoverable_io_error(StepErrorKind::VerifyCapture));
    assert!(!is_recoverable_io_error(StepErrorKind::Other));
}

#[test]
fn only_gate_rejection_is_a_placement_fault() {
    assert_eq!(
        recovery_cause_for(StepErrorKind::GateRejected),
        RecoveryCause::PlacementFault
    );
    for kind in [
        StepErrorKind::Execute,
        StepErrorKind::Capture,
        StepErrorKind::VerifyCapture,
        StepErrorKind::Other,
    ] {
        assert_eq!(
            recovery_cause_for(kind),
            RecoveryCause::Transient,
            "{kind:?} must stay transient"
        );
    }
}

#[test]
fn placement_fault_always_clamps_and_never_restarts() {
    for attempt in [1, 2, 3, 99] {
        for alive in [false, true] {
            assert_eq!(
                plan_recovery(RecoveryCause::PlacementFault, attempt, alive),
                RecoveryPlan::ClampWindow,
                "attempt={attempt} alive={alive}: a placement fault must never kill the game"
            );
        }
    }
}

#[test]
fn first_transient_attempt_launches_without_killing() {
    for alive in [false, true] {
        assert_eq!(
            plan_recovery(RecoveryCause::Transient, 1, alive),
            RecoveryPlan::Launch
        );
    }
}

#[test]
fn later_transient_attempt_waits_while_the_process_is_alive() {
    for attempt in [2, 3, 7] {
        assert_eq!(
            plan_recovery(RecoveryCause::Transient, attempt, true),
            RecoveryPlan::WaitForExisting,
            "a live process may be loading; restarting would destroy its state (DEF-011)"
        );
    }
}

#[test]
fn later_transient_attempt_restarts_only_when_the_process_is_gone() {
    for attempt in [2, 3, 7] {
        assert_eq!(
            plan_recovery(RecoveryCause::Transient, attempt, false),
            RecoveryPlan::Restart
        );
    }
}

#[test]
fn only_transient_recovery_arms_a_boot_grace() {
    assert!(RecoveryCause::Transient.arms_boot_grace());
    assert!(
        !RecoveryCause::PlacementFault.arms_boot_grace(),
        "a clamp does not restart the game, so a boot grace would only blind the loop (DEF-012)"
    );
}
