//! `ensure-open` / `launch` サブコマンドの終了コード契約テスト。
//!
//! Issue #21 の受入基準のうち終了コード契約をカバーする。実機を起動せず、
//! `contract` 層(`anaden_cli_contract`)の純粋関数のみを検証する。
//!
//! 契約(本ファイルが固定する直契約 / Issue #21 AC):
//!   - AlreadyOpen => 0 (起動不要、正常)                       ... AC1
//!   - Launched    => 0 (起動成功、正常)                       ... AC2
//!   - Timeout     => 2 (起動したが前景化せず。hard error と区別) ... AC3
//!   - spawn / OpenProcess 失敗 => 1 (hard error)              ... AC4
//!
//! (Issue #188: `--target android` / serial 必須の AC5〜AC7 相当は
//!  Android 削除に伴いテストごと削除 — ensure-open / launch は Win32 固定)
//!
//! この契約は `run_pipeline_live` が Timeout を soft warn として扱うのとは
//! **意図的に異なる**。スタンドアロン ensure-open は CI gate / 運用スクリプトからの
//! 単体呼出を想定し、Timeout を非ゼロで返すことで「起動に失敗した」ことを明示する。

#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]
#![allow(clippy::expect_used)]

use anaden_cli_contract::{
    EXIT_ALREADY_OR_LAUNCHED, EXIT_HARDCERROR, EXIT_TIMEOUT, ensure_open_exit_code,
    standalone_exit_code,
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

// ---- AC4: ハードエラー(spawn/OpenProcess 失敗)の終了コードは 1 ----
// contract 層は outcome Ok 側のみを射影するため、Err 側の終了コードは定数
// EXIT_HARDCERROR を呼び出し側が採用する。ここではその定数値を契約として固定する。
#[test]
fn hard_error_exit_code_is_one() {
    assert_eq!(EXIT_HARDCERROR, 1);
    // 正常系(0)・タイムアウト(2) とは全て異なる。
    assert_ne!(EXIT_HARDCERROR, EXIT_ALREADY_OR_LAUNCHED);
    assert_ne!(EXIT_HARDCERROR, EXIT_TIMEOUT);
}

// ---- AC4 真経路: hard error(spawn/OpenProcess 失敗) ⇒ exit 1 (真経路) ----
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
