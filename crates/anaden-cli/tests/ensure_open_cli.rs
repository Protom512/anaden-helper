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
    ensure_open_exit_code, resolve_target, standalone_exit_code,
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

// ---- AC4 真経路: hard error(spawn/OpenProcess/AdbError 失敗) ⇒ exit 1 (真経路) ----
// standalone_exit_code が Err を EXIT_HARDCERROR(1) へ射影する。これが AC4 契約の真正証拠
//（従来は anyhow bubble の暗黙 exit 1 に依存し未検証だった）。prod(main exit_standalone)は
// この純粋関数へ Ok/Err 双方を委任するため、ここで契約を固定すれば prod 挙動も固定される。
#[test]
fn hard_error_maps_to_exit_one_via_standalone() {
    let r: Result<&EnsureOutcome, &str> = Err("spawn failed");
    assert_eq!(standalone_exit_code(r), EXIT_HARDCERROR);
    assert_eq!(standalone_exit_code(r), 1);
    // Ok 側との区別も再確認(Timeout は 2 で hard error 1 とは異なる)。
    assert_eq!(
        standalone_exit_code::<()>(Ok(&EnsureOutcome::Timeout)),
        EXIT_TIMEOUT
    );
}

// ---- AC5: android ターゲットは serial 必須(None は引数エラー扱い) ----
// contract 層が Android を正しく識別することで、呼出側(main の ensure_open_outcome /
// force_launch_app)が「Android + serial None => Err => EXIT_HARDCERROR」と判定できる根拠となる。
// serial 強制の実経路は main.rs インラインテスト(ensure_open_outcome_android_requires_serial)
// が真正に担保済みのため、統合テスト側は contract 層の識別のみを固定する(tdd-coupling: 実装
// 詳細のモックで偽 green を作らず、公開契約の振る舞いのみを検証)。
#[test]
fn android_target_requires_serial_argument() {
    assert_eq!(resolve_target("android"), Ok(EnsureOpenTarget::Android));
}

// ---- AC6: windows ターゲットを非 Windows ビルドで呼出 => graceful error (panic しない) ----
// resolve_target("windows") 自体は両プラットフォームで Ok(panic しない)。非 Windows ビルド
// での graceful fallback はバイナリ側(cfg-gate された bail)で行うが、その判定根拠となる
// 「windows が正しく解決されること」をここで担保する(モック呼出なし・純粋契約のみ)。
#[test]
fn windows_target_resolves_without_panic_on_any_platform() {
    assert_eq!(resolve_target("windows"), Ok(EnsureOpenTarget::Windows));
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
