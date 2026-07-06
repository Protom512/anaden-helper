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

/// `run` サブコマンドの CI 成功終了コード。
///
/// ゴール到達 / Stop アクション / 終端タスク到達 など「宣言的終端状態への正常収束」は
/// すべて CI gate 上 success(0) とみなす。非ゴールモードの `MaxIterations` 到達も
/// 今日の挙動(exit 0)を保持する(後述)。
pub const EXIT_RUN_SUCCESS: i32 = 0;

/// `run` サブコマンドのハードエラー終了コード(capture/execute IO 失敗)。
///
/// `EXIT_HARDCERROR`(1) と同一値。`standalone_exit_code` の hard-error 契約と一貫。
pub const EXIT_RUN_HARDCERROR: i32 = EXIT_HARDCERROR;

/// `run` サブコマンドの soft-failure(タイムアウト系)終了コード。
///
/// ゴールモードで `GoalTimeout` / `MaxIterations` に到達した際の「成果物は出たが
/// 宣言的ゴール未到達」を表現する非ゼロ値。`EXIT_TIMEOUT`(2) と同一値で、
/// `ensure-open` の soft-failure 契約と並行する。
pub const EXIT_RUN_TIMEOUT: i32 = EXIT_TIMEOUT;

/// [`LoopStopReason`](anaden_engine::LoopStopReason) を `run` サブコマンドの
/// 終了コードへ射影する純粋関数(Issue #37 AC: exit code + 成果レポート)。
///
/// # 契約(真実の源)
///
/// | `LoopStopReason`        | 終了コード | 由来 |
/// |-------------------------|-----------|------|
/// | `Stop`                  | 0 (success) | `Action::Stop` 到達 = 宣言的終端 |
/// | `TerminalTask`          | 0 (success) | next 無し終端タスク到達 = 正常収束 |
/// | `GoalReached`           | 0 (success) | 宣言的ゴール到達 = CI success(Issue #37) |
/// | `MaxIterations`         | 0 (success) | **非ゴールモードは今日の exit 0 を保持** |
/// | `GoalTimeout`           | 2 (timeout) | ゴール未到達だが進捗あり = soft failure(Issue #37) |
/// | `CaptureError`          | 1 (hard)    | capture IO 失敗 = ハードエラー |
/// | `ExecuteError`          | 1 (hard)    | execute IO 失敗 = ハードエラー |
///
/// # 設計意図(pre-T5 決定ゲート: org-feedback estimate approval)
///
/// **非ゴールパスは今日の exit 0 を保存**しゼロ以外終了コードはゴール活性時のみ表面化
/// する方針を取る。これにより既存 CI の `run` 呼出意味論を断片化させない。
/// 現時点では `GoalReached` / `GoalTimeout` バリアントは未追加(T3 待ち)のため、
/// 本関数は既存5バリアントのみを扱い、`MaxIterations` は success(0) へ射影する。
/// T3 でバリアントが追加された際は本関数の match 式を拡張し、`MaxIterations` の
/// 意味論を「ゴール活性時のみ timeout(2)」へ切り替えるか別途検証コンテキストを
/// 導入すること(後続タスク)。
///
/// # 純粋性
///
/// 本関数はデバイス I/O を持たず決定論的。`tests/` からデバイスなしで検証可能。
/// 借用(`&LoopStopReason`)で受け取るため呼出側は同一の reason をラベル化等へ再利用できる。
pub fn run_exit_code(reason: &anaden_engine::LoopStopReason) -> i32 {
    match reason {
        // 宣言的終端への正常収束 = CI success。今日の run 挙動(exit 0)を保持。
        anaden_engine::LoopStopReason::Stop => EXIT_RUN_SUCCESS,
        anaden_engine::LoopStopReason::TerminalTask => EXIT_RUN_SUCCESS,
        // 非ゴールモードの最大イテレーション到達は今日の exit 0 を保存。
        // (ゴール活性時の timeout 扱いは T3 でバリアント追加後に再検討)
        anaden_engine::LoopStopReason::MaxIterations => EXIT_RUN_SUCCESS,
        // 宣言的ゴール到達 = CI success(Issue #37 T3/T4)。
        anaden_engine::LoopStopReason::GoalReached => EXIT_RUN_SUCCESS,
        // ゴール活性時のタイムアウト(成果物出たがゴール未到達) = soft failure(2)。
        anaden_engine::LoopStopReason::GoalTimeout => EXIT_RUN_TIMEOUT,
        // IO 系ハードエラー = exit 1。
        anaden_engine::LoopStopReason::CaptureError => EXIT_RUN_HARDCERROR,
        anaden_engine::LoopStopReason::ExecuteError => EXIT_RUN_HARDCERROR,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    // ---- run_exit_code (Issue #37 exit-code contract, device-free) ----
    // 契約: Stop/TerminalTask => 0(success), MaxIterations => 0(今日の挙動保持),
    //       CaptureError/ExecuteError => 1(hard error)。
    // ゴール活性時の GoalReached(0) / GoalTimeout(2) は T3 でバリアント追加後に拡張。

    #[test]
    fn run_exit_code_stop_is_zero() {
        // Action::Stop 到達は宣言的終端 = CI success(UC: ユーザー明示停止)。
        assert_eq!(run_exit_code(&anaden_engine::LoopStopReason::Stop), 0);
    }

    #[test]
    fn run_exit_code_terminal_task_is_zero() {
        // next 無し終端タスク到達 = 正常収束 = CI success(UC: パイプライン末端)。
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::TerminalTask),
            0
        );
    }

    #[test]
    fn run_exit_code_max_iterations_preserves_today_zero() {
        // pre-T5 決定ゲート: 非ゴールモードの MaxIterations は今日の exit 0 を保持。
        // 既存 CI の `run` 呼出意味論を断片化させない。
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::MaxIterations),
            0
        );
    }

    #[test]
    fn run_exit_code_capture_error_is_hard_error() {
        // capture IO 失敗はハードエラー。standalone の EXIT_HARDCERROR(1) と並行。
        let code = run_exit_code(&anaden_engine::LoopStopReason::CaptureError);
        assert_eq!(code, EXIT_RUN_HARDCERROR);
        assert_eq!(code, 1, "CaptureError must map to hard-error exit 1");
        assert_ne!(code, 0, "CaptureError must be non-zero");
    }

    #[test]
    fn run_exit_code_execute_error_is_hard_error() {
        // execute IO 失敗はハードエラー。recovery_hook 失敗等もここへ集約。
        let code = run_exit_code(&anaden_engine::LoopStopReason::ExecuteError);
        assert_eq!(code, EXIT_RUN_HARDCERROR);
        assert_eq!(code, 1, "ExecuteError must map to hard-error exit 1");
        assert_ne!(code, 0, "ExecuteError must be non-zero");
    }

    #[test]
    fn run_exit_code_hard_errors_distinct_from_timeout_precedent() {
        // EXIT_RUN_TIMEOUT(2) と EXIT_RUN_HARDCERROR(1) は区別される。
        // ゴール未到達 soft-failure(2) と IO ハードエラー(1) の分離が run 契約の核。
        assert_ne!(EXIT_RUN_HARDCERROR, EXIT_RUN_TIMEOUT);
        assert_eq!(EXIT_RUN_TIMEOUT, 2);
    }

    #[test]
    fn run_exit_code_success_variants_share_zero() {
        // Stop と TerminalTask は CI gate 上同一个(success=0)。分岐複雑化を避ける。
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::Stop),
            run_exit_code(&anaden_engine::LoopStopReason::TerminalTask)
        );
        // MaxIterations も非ゴールモードでは success に揃う(今日の挙動)。
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::MaxIterations),
            run_exit_code(&anaden_engine::LoopStopReason::Stop)
        );
    }

    #[test]
    fn run_exit_code_goal_reached_is_zero() {
        // 宣言的ゴール到達 = CI success(UC: goal-driven loop がゴール条件を満たした)。
        // GoalReached は EXIT_RUN_SUCCESS(0) へ射影される。ゴール到達と Stop/TerminalTask
        // は CI gate 上同一个(success=0)。
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalReached),
            EXIT_RUN_SUCCESS
        );
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalReached),
            0,
            "GoalReached must map to success exit 0"
        );
        // Stop/TerminalTask と同一个(success=0)であることも担保。
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalReached),
            run_exit_code(&anaden_engine::LoopStopReason::Stop)
        );
    }

    #[test]
    fn run_exit_code_goal_timeout_is_two() {
        // ゴール活性時のタイムアウト(成果物は出たが宣言的ゴール未到達) = soft failure(2)。
        // GoalTimeout は EXIT_RUN_TIMEOUT(2) へ射影される。EXIT_TIMEOUT と同一値。
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalTimeout),
            EXIT_RUN_TIMEOUT
        );
        assert_eq!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalTimeout),
            2,
            "GoalTimeout must map to timeout exit 2"
        );
        // hard-error(1) とは区別される(soft failure vs hard error)。
        assert_ne!(
            run_exit_code(&anaden_engine::LoopStopReason::GoalTimeout),
            EXIT_RUN_HARDCERROR,
            "GoalTimeout must not collapse to hard-error(1)"
        );
    }

    #[test]
    fn run_exit_code_covers_all_variants() {
        // LoopStopReason へ新バリアントが追加された際、このテストが未対応を検出する。
        // GoalReached / GoalTimeout は Issue #37 で追加済み(T3)。
        let variants = [
            anaden_engine::LoopStopReason::Stop,
            anaden_engine::LoopStopReason::TerminalTask,
            anaden_engine::LoopStopReason::MaxIterations,
            anaden_engine::LoopStopReason::GoalReached,
            anaden_engine::LoopStopReason::GoalTimeout,
            anaden_engine::LoopStopReason::CaptureError,
            anaden_engine::LoopStopReason::ExecuteError,
        ];
        for v in &variants {
            // 各バリアントが success(0) / timeout(2) / hard-error(1) のいずれかへ解決されること。
            let code = run_exit_code(v);
            assert!(
                code == EXIT_RUN_SUCCESS || code == EXIT_RUN_TIMEOUT || code == EXIT_RUN_HARDCERROR,
                "variant {:?} produced unexpected exit code {}",
                v,
                code
            );
        }
    }
}
