//! `ensure-open` / `launch` サブコマンドの純粋契約層。
//!
//! デバイス層(`anaden_device::EnsureOutcome`)へ依存するが、デバイス I/O は一切行わない。
//! ここにある関数はすべて純粋(副作用なし・決定論的)で、`tests/` からデバイスなしで
//! テスト可能。終了コード契約の数値をここで唯一の源泉(Source of Truth)とする。
//!
//! 設計意図(architecture-coupling-balance):
//!   - `EnsureOutcome` は `anaden_device` の純粋ドメイン enum のまま触らない。
//!   - 終了コードへの射影は CLI 層の関心事なので `anaden_cli_contract`(本モジュール)へ置く。
//!     デバイス層へ終了コード知識を漏れ出させない。

use anaden_device::EnsureOutcome;

/// AlreadyOpen / Launched の両方が返す成功終了コード。
pub const EXIT_ALREADY_OR_LAUNCHED: i32 = 0;

/// ADB / spawn / OpenProcess 等のハードエラー終了コード。
pub const EXIT_HARDCERROR: i32 = 1;

/// 起動は試みたが前景化/生存確認できなかった(Timeout)の終了コード。
/// `run` サブコマンドでは Timeout は soft warn(続行)だが、スタンドアロン
/// `ensure-open` / `launch` では CI gate が失敗と見なせるよう非ゼロとする。
pub const EXIT_TIMEOUT: i32 = 2;

/// `--target` 解決結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsureOpenTarget {
    /// ADB 実機(android)。serial 必須。
    Android,
    /// PC 版(Win32)。serial 不要(未使用)。
    Windows,
}

/// `--target` 文字列を [`EnsureOpenTarget`] へ解決する純粋関数。
///
/// `android` / `windows` 以外は即座にエラーメッセージ(指定値を含む)を返す(panic しない)。
/// メッセージは人間可読で stderr へ出すことを想定。
pub fn resolve_target(value: &str) -> Result<EnsureOpenTarget, String> {
    match value {
        "android" => Ok(EnsureOpenTarget::Android),
        "windows" => Ok(EnsureOpenTarget::Windows),
        other => Err(format!(
            "--target は `android` または `windows` です(指定値: {other})"
        )),
    }
}

/// [`EnsureOutcome`] を終了コードへ射影する純粋関数。
///
/// 契約(Issue #21 AC):
///   - [`EnsureOutcome::AlreadyOpen`] → [`EXIT_ALREADY_OR_LAUNCHED`](0)
///   - [`EnsureOutcome::Launched`]    → [`EXIT_ALREADY_OR_LAUNCHED`](0)
///   - [`EnsureOutcome::Timeout`]     → [`EXIT_TIMEOUT`](2)
///
/// ハードエラー(`AdbError` / spawn / OpenProcess 失敗)の終了コード
/// ([`EXIT_HARDCERROR`](1))は、呼び出し側が `Err` を受け取った時に使う。
/// 本関数は `Ok(outcome)` のみを扱う。借用で受け取るため、呼び出し側は
/// 同一の `outcome` から [`ensure_outcome_label`] も併用できる。
pub fn ensure_open_exit_code(outcome: &EnsureOutcome) -> i32 {
    match outcome {
        EnsureOutcome::AlreadyOpen => EXIT_ALREADY_OR_LAUNCHED,
        EnsureOutcome::Launched => EXIT_ALREADY_OR_LAUNCHED,
        EnsureOutcome::Timeout => EXIT_TIMEOUT,
    }
}

/// スタンドアロン ensure-open/launch の終了コード決定(Ok/Err 双方を覆盖・純粋・決定論的)。
///
/// Issue #21 AC4: Ok 側は [`ensure_open_exit_code`] へ委任、Err 側(ハードエラー:
/// AdbError / spawn / OpenProcess 失敗)は [`EXIT_HARDCERROR`](1)。本関数が AC4 の
/// 「hard error ⇒ exit 1」契約の唯一の真実の源。`std::process::exit` はテスト不能なため、
/// exit の発動は呼出側(main の `exit_standalone`)が行い、本関数は純粋に射影するだけ
/// (テスト可能性と rust-anti-patterns panic 禁止の両立)。ジェネリック `<E>` により
/// anyhow 型へ依存せず device/anyhow フリーを維持。`E: ?Sized` により `&str`(`E = str`)
/// のような unsized なエラー型の借用も受け取れる(`Result<&EnsureOutcome, &str>` でテスト可能)。
pub fn standalone_exit_code<E: ?Sized>(result: Result<&EnsureOutcome, &E>) -> i32 {
    match result {
        Ok(outcome) => ensure_open_exit_code(outcome),
        Err(_) => EXIT_HARDCERROR,
    }
}

/// [`EnsureOutcome`] を人間可読な1行ラベルへ射影する純粋関数。
///
/// `run` パス(android/windows)と standalone サブコマンド(`ensure-open`/`launch`)の
/// 「EnsureOutcome → 人間可読メッセージ」唯一の真実の源。両経路のドリフトを防ぐため、
/// ラベル化は必ずこの関数へ集中させる。
pub fn ensure_outcome_label(outcome: &EnsureOutcome) -> &'static str {
    match outcome {
        EnsureOutcome::AlreadyOpen => "起動不要(既に起動中)",
        EnsureOutcome::Launched => "起動し生存を確認",
        EnsureOutcome::Timeout => "起動タイムアウト",
    }
}
