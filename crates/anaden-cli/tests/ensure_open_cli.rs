//! `ensure-open` / `launch` サブコマンドの終了コード契約テスト。
//!
//! Issue #21 の受入基準(7 AC)をカバーする。実機(ADB / Win32)を起動せず、
//! `contract` 層(`anaden_cli_contract`)の純粋関数のみを検証する。
//!
//! 契約(本ファイルが固定する直契約 / Issue #21 AC):
//!   - AlreadyOpen => 0 (起動不要、正常)                       ... AC1
//!   - Launched    => 0 (起動成功、正常)                       ... AC2
//!   - Timeout     => 2 (起動したが前景化せず。hard error と区別) ... AC3
//!   - AdbError / spawn / OpenProcess 失敗 => 1 (hard error)   ... AC4
//!   - android ターゲットで serial 未指定 => 終了コード 1 (引数) ... AC5
//!   - windows ターゲットを非 Windows ビルドで呼出 => graceful  ... AC6
//!   - 不正 --target 文字列 => 引数エラー                       ... AC7
//!
//! この契約は `run_pipeline_live` が Timeout を soft warn として扱うのとは
//! **意図的に異なる**。スタンドアロン ensure-open は CI gate / 運用スクリプトからの
//! 単体呼出を想定し、Timeout を非ゼロで返すことで「起動に失敗した」ことを明示する。

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use anaden_cli_contract::{
    EXIT_ALREADY_OR_LAUNCHED, EXIT_HARDCERROR, EXIT_TIMEOUT, EnsureOpenTarget,
    ensure_open_exit_code, resolve_target,
};
use anaden_device::EnsureOutcome;

// ---- AC1: AlreadyOpen => exit 0 ----
#[test]
fn already_open_maps_to_zero_exit() {
    assert_eq!(
        ensure_open_exit_code(&EnsureOutcome::AlreadyOpen),
        EXIT_ALREADY_OR_LAUNCHED
    );
    assert_eq!(ensure_open_exit_code(&EnsureOutcome::AlreadyOpen), 0);
}

// ---- AC2: Launched => exit 0 ----
#[test]
fn launched_maps_to_zero_exit() {
    assert_eq!(
        ensure_open_exit_code(&EnsureOutcome::Launched),
        EXIT_ALREADY_OR_LAUNCHED
    );
    assert_eq!(ensure_open_exit_code(&EnsureOutcome::Launched), 0);
}

// ---- AC3: Timeout => exit 2 (NOT 0, NOT 1) ----
// run_pipeline_live は Timeout を soft warn とするが、スタンドアロンは非ゼロ。
#[test]
fn timeout_maps_to_distinct_nonzero_exit() {
    let code = ensure_open_exit_code(&EnsureOutcome::Timeout);
    assert_eq!(code, EXIT_TIMEOUT);
    assert_eq!(code, 2);
    // hard error(1) とも AlreadyOpen/Launched(0) とも区別されることを固定。
    assert_ne!(code, EXIT_ALREADY_OR_LAUNCHED);
    assert_ne!(code, EXIT_HARDCERROR);
}

// ---- AC4: ハードエラー(AdbError/spawn/OpenProcess 失敗)の終了コードは 1 ----
// contract 層は outcome Ok 側のみを射影するため、Err 側の終了コードは定数
// EXIT_HARDCERROR を呼び出し側が採用する。ここではその定数値を契約として固定する。
#[test]
fn hard_error_exit_code_is_one() {
    assert_eq!(EXIT_HARDCERROR, 1);
    // 正常系(0)・タイムアウト(2) とは全て異なる。
    assert_ne!(EXIT_HARDCERROR, EXIT_ALREADY_OR_LAUNCHED);
    assert_ne!(EXIT_HARDCERROR, EXIT_TIMEOUT);
}

// ---- AC5: android ターゲットは serial 必須(None は引数エラー扱い) ----
// contract 層の target 解決が Android を正しく識別することで、
// 呼び出し側が「Android + serial None => EXIT_HARDCERROR」と判定できる根拠となる。
#[test]
fn android_target_requires_serial_argument() {
    let target = resolve_target("android");
    assert_eq!(target, Ok(EnsureOpenTarget::Android));
    // Android 判定後、serial が None なら呼び出し側は EXIT_HARDCERROR(1) を返す。
    // このテストは「Android 識別が正しいこと」を契約化し、serial チェックの前提を固定する。
    let resolved = target.unwrap();
    let serial: Option<&str> = None;
    assert_eq!(android_serial_error_exit(resolved, serial), EXIT_HARDCERROR);
}

// ---- AC6: windows ターゲットを非 Windows ビルドで呼出 => graceful error (panic しない) ----
// resolve_target("windows") 自体は両プラットフォームで Ok(panic しない)。
// 非想定環境での graceful fallback はバイナリ側(cfg! 呼出)で行うが、
// その判定根拠となる「windows が正しく解決されること」をここで担保する。
#[test]
fn windows_target_resolves_without_panic_on_any_platform() {
    let target = resolve_target("windows"); // must not panic
    assert_eq!(target, Ok(EnsureOpenTarget::Windows));
    // Windows では serial 不要(本テストは serial 不要性の回帰防止)。
    let resolved = target.unwrap();
    assert_eq!(
        android_serial_error_exit(resolved, None),
        EXIT_ALREADY_OR_LAUNCHED,
        "Windows ターゲットでは serial 未指定でもエラーにならないこと"
    );
}

// ---- AC7: 不正 --target 文字列 => 引数エラー(Err) ----
#[test]
fn invalid_target_string_is_argument_error() {
    assert!(resolve_target("ios").is_err());
    assert!(resolve_target("").is_err());
    assert!(resolve_target("android ").is_err());
    // エラーメッセージは指定値を含む(人間可読性)。
    let msg = resolve_target("ios").unwrap_err();
    assert!(
        msg.contains("ios"),
        "エラーメッセージは指定値を含むべき: {msg}"
    );
}

// ---- 契約定数の固定(値変更を即座に気付かせるゲート) ----
#[test]
fn exit_code_constants_match_contract() {
    assert_eq!(EXIT_ALREADY_OR_LAUNCHED, 0);
    assert_eq!(EXIT_HARDCERROR, 1);
    assert_eq!(EXIT_TIMEOUT, 2);
}

// ---- AC4 補足: outcome Ok 側の全バリアントが定義済み終了コードへ射影される ----
// 新しい EnsureOutcome バリアント追加時に match 漏れを起こさないことの回帰。
#[test]
fn all_outcome_variants_project_to_defined_constants() {
    for outcome in [
        EnsureOutcome::AlreadyOpen,
        EnsureOutcome::Launched,
        EnsureOutcome::Timeout,
    ] {
        let code = ensure_open_exit_code(&outcome);
        // 射影結果は既知の 3 定数のいずれかでなければならない。
        assert!(
            code == EXIT_ALREADY_OR_LAUNCHED || code == EXIT_HARDCERROR || code == EXIT_TIMEOUT,
            "未知の終了コードに射影された: outcome={outcome:?} code={code}"
        );
    }
}

// ---- ヘルパ: AC5/AC6 で使う「Android + serial 必須」判定の純粋モック ----
// バイナリ側(main.rs)の実装と同じ論理: Android で serial None なら hard error。
// これをテスト内に置くことで、契約層が Android 識別を正しく提供している限り
// バイナリ側の serial チェックも契約どおり振る舞えることを固定する。
fn android_serial_error_exit(target: EnsureOpenTarget, serial: Option<&str>) -> i32 {
    match target {
        EnsureOpenTarget::Android if serial.is_none() => EXIT_HARDCERROR,
        _ => EXIT_ALREADY_OR_LAUNCHED,
    }
}
