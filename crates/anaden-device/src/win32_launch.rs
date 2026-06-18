//! Windows 版ゲーム(アナザーエデン PC 版)の起動・生存監視。
//!
//! 概要:
//!   `Launcher.exe` を起動し、子プロセス `AnotherEden.exe` が 0xC0000005 等で即死せずに
//!   安定起動・指定期間生存することを、ADB(`am start` / `dumpsys`)を使わずに
//!   Win32 プロセス API 単独で判定する。`app_control.rs` の `AppController`(ADB 依存) を
//!   Windows 実装で置き換えるためのプロセス起動・生存監視層。
//!
//!   動作検証済みプローブ `examples/probe_windows_launch.rs` のロジックをそのまま構造体化した
//!   もので、プローブの exit code 判定を `Result<EnsureOutcome, AdbError>` へ読み替える。
//!
//! 使う Win32 API:
//!   [1] プロセス起動: `std::process::Command`(内部で `CreateProcessW`)。
//!       `creation_flags` に `CREATE_NEW_PROCESS_GROUP`(0x00000200) を付与。Launcher は即座に
//!       子 `AnotherEden.exe` を起動して Exit 0 で終わる設計のため、`spawn()` で PID だけ取り
//!       待機せず、子の出現を別ループで監視する。
//!       **stdio は必ず `Stdio::null()`**: パイプ継承すると Launcher の stdout バッファが満杯に
//!       なり、親プロセスが read しない限り Launcher がブロックしてプローブ本体がフリーズする
//!       (プローブで実証済みのバグ)。
//!   [2] プロセス列挙・生存監視: `CreateToolhelp32Snapshot(TH32CS_SNAPSHOTPROCESS)` +
//!       `Process32FirstW` / `Process32NextW`。`PROCESSENTRY32W.szExeFile` で子を探索。
//!   [3] exit code 取得: `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `GetExitCodeProcess`。
//!       `STILL_ACTIVE`(0x103) なら生存。
//!
//! Linux では本モジュール全体がコンパイル対象外(`#[cfg(windows)]`)。
//! `lib.rs` の `#[cfg(windows)] mod win32_launch;` 宣言で取り込むことを前提とする。

#![cfg(windows)]
// 本モジュールは後続フェーズで lib.rs の `#[cfg(windows)] mod win32_launch;` +
// `#[cfg(windows)] pub use win32_launch::{Win32Launch, DEFAULT_*};` で取り込まれるまで
// 外部参照が無い。またプロセス列挙ヘルパ(find_pid/snapshot_processes 等)は capture/input と
// 共通化のため別途 `win32_proc.rs` へ抽出予定。それまでの過渡的な dead_code 警告を許容する。
#![allow(dead_code)]

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tracing::{info, warn};

use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
use windows::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::app_control::EnsureOutcome;
use crate::client::AdbError;

// ---- デフォルト定数 ----

/// `Launcher.exe` の既定パス。アナザーエデン PC 版インストール先。
pub const DEFAULT_LAUNCHER: &str =
    r"C:\Program Files\Wright Flyer Studios\ANOTHER EDEN\Launcher\Launcher.exe";

/// WorkingDirectory 既定値。`AnotherEden.exe` は相対パスでアセット/ICD を探すため必須。
pub const DEFAULT_WORKDIR: &str = r"C:\Program Files\Wright Flyer Studios\ANOTHER EDEN\Game";

/// 監視対象の子プロセス名(大文字小文字無視で比較)。
pub const DEFAULT_CHILD: &str = "AnotherEden.exe";

/// `ensure_open` の既定待機秒数。
pub const DEFAULT_WAIT: Duration = Duration::from_secs(30);

/// 子プロセス出現・生存監視のポーリング間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// `STILL_ACTIVE` 定数の u32 表現。`GetExitCodeProcess` は戻り値を u32 で返す。
/// `STILL_ACTIVE` は `Win32::Foundation` の NTSTATUS newtype(0x103)。
const STILL_ACTIVE_U32: u32 = STILL_ACTIVE.0 as u32;

/// Windows 版ゲームの起動・生存監視を行う。
///
/// `launcher` / `workdir` / `child` はそれぞれ `Launcher.exe` パス・作業ディレクトリ・
/// 監視対象の子プロセス名(例: `AnotherEden.exe`)を保持する。本構造体は状態を持たないため
/// `Clone` 可能。
#[derive(Clone)]
pub struct Win32Launch {
    launcher: String,
    workdir: String,
    child: String,
}

impl Win32Launch {
    /// 明示的なパス指定で構築する。
    pub fn new(launcher: &str, workdir: &str, child: &str) -> Self {
        Self {
            launcher: launcher.to_string(),
            workdir: workdir.to_string(),
            child: child.to_string(),
        }
    }

    /// 既定のパス定数(`DEFAULT_LAUNCHER` / `DEFAULT_WORKDIR` / `DEFAULT_CHILD`)で構築する。
    pub fn default_paths() -> Self {
        Self::new(DEFAULT_LAUNCHER, DEFAULT_WORKDIR, DEFAULT_CHILD)
    }

    /// `Launcher.exe` を起動し、子プロセスの出現と生存を `wait` の期間監視する。
    ///
    /// 既に子プロセスが存在する場合は `EnsureOutcome::AlreadyOpen`(起動スキップ)。
    /// 起動して `wait` 以内に子が出現・生存すれば `EnsureOutcome::Launched`。
    /// 子が出現しない、または即死した場合は `EnsureOutcome::Timeout`。
    /// スポーン失敗や Win32 API エラーは `AdbError::CommandFailed` で包んで伝播する。
    ///
    /// ブロッキングするプロセス列挙・`std::thread::sleep` ポーリングは `spawn_blocking` へ逃し、
    /// 呼び出し側の async ランタイムを止めない。
    pub async fn ensure_open(&self, wait: Duration) -> Result<EnsureOutcome, AdbError> {
        let launcher = self.launcher.clone();
        let workdir = self.workdir.clone();
        let child = self.child.clone();
        tokio::task::spawn_blocking(move || run_blocking(&launcher, &workdir, &child, wait))
            .await
            .map_err(|e| AdbError::CommandFailed {
                message: format!("ensure_open の blocking タスクがパニック/中止: {e}"),
            })?
    }

    /// 子プロセスが現在生存しているか(プロセス列挙に存在するか)。
    ///
    /// リカバリフック等から呼ぶ。内部でプロセススナップショットを取得するため
    /// `spawn_blocking` へ逃す。スナップショット取得自体の失敗は偽(非生存)扱いとし、
    /// エラーは伝播しない(`bool` 戻り値)。
    pub async fn is_alive(&self) -> bool {
        let child = self.child.clone();
        tokio::task::spawn_blocking(move || child_exists(&child))
            .await
            .unwrap_or(false)
    }

    /// 起動部分のみを行う。`ensure_open` の起動ステップ相当。
    ///
    /// リカバリフックから「強制再起動」のために呼ぶ。既存プロセスの有無は確認せず、
    /// 無条件で `Launcher.exe` を spawn する。spawn 失敗は `AdbError::CommandFailed`。
    /// spawn は即座に帰る(Launcher は自身で子を起動して Exit 0 する設計)。
    pub async fn launch_app(&self) -> Result<(), AdbError> {
        let launcher = self.launcher.clone();
        let workdir = self.workdir.clone();
        tokio::task::spawn_blocking(move || spawn_launcher(&launcher, &workdir).map(|_| ()))
            .await
            .map_err(|e| AdbError::CommandFailed {
                message: format!("launch_app の blocking タスクがパニック/中止: {e}"),
            })?
            .map_err(|e| AdbError::CommandFailed { message: e })?;
        Ok(())
    }
}

/// `ensure_open` の同期本体。プローブ `run()` の判定ロジックを `EnsureOutcome` へ読み替える。
///
/// 戻り値:
/// - `Ok(AlreadyOpen)`: 事前ガードで子プロセスが既に存在した。
/// - `Ok(Launched)`: spawn → 子出現 → 生存確認(STILL_ACTIVE 維持)まで完遂。
/// - `Ok(Timeout)`: 子が `wait` 以内に出現しなかった、または即死した。
/// - `Err`: spawn 失敗、または事前/監視中の Win32 API 呼び出しエラー。
fn run_blocking(
    launcher: &str,
    workdir: &str,
    child: &str,
    wait: Duration,
) -> Result<EnsureOutcome, AdbError> {
    // [ステップ0] 事前ガード: 既に子が存在すれば AlreadyOpen。
    //   既存プロセスが残っていると後続の「子発見」が既存プロセスにヒットして偽成功になる。
    match snapshot_processes() {
        Ok(entries) => {
            if entries.iter().any(|e| e.name.eq_ignore_ascii_case(child)) {
                info!("Win32Launch: {child} は既に起動中 → AlreadyOpen");
                return Ok(EnsureOutcome::AlreadyOpen);
            }
        }
        Err(e) => {
            return Err(AdbError::CommandFailed {
                message: format!("Win32Launch: 事前スナップショット失敗: {e}"),
            });
        }
    }

    // [ステップ1] Launcher spawn。std::thread::sleep を含む待機が続くので blocking 内。
    let launcher_started = Instant::now();
    let launcher_child = match spawn_launcher(launcher, workdir) {
        Ok(pid) => {
            info!("Win32Launch: Launcher spawn OK (launcher_pid={pid})");
            pid
        }
        Err(msg) => {
            return Err(AdbError::CommandFailed { message: msg });
        }
    };

    // 発見フェーズ + 生存確認フェーズで「spawn から合計 wait 秒」を超えない絶対 deadline。
    let overall_deadline = launcher_started + wait;

    // [ステップ2] --wait 期間内に子が出現するかポーリング。
    let mut first_seen: Option<(u32, u32)> = None; // (pid, parent_pid)
    let mut snapshot_failed = false;
    while Instant::now() < overall_deadline {
        match snapshot_processes() {
            Ok(entries) => {
                for entry in &entries {
                    if entry.name.eq_ignore_ascii_case(child) {
                        first_seen = Some((entry.pid, entry.parent_pid));
                        break;
                    }
                }
                if first_seen.is_some() {
                    break;
                }
            }
            Err(e) => {
                warn!("Win32Launch: CreateToolhelp32Snapshot 失敗: {e}");
                snapshot_failed = true;
                break;
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    let (child_pid, child_parent_pid) = match first_seen {
        Some(v) => {
            if v.1 == launcher_child {
                info!(
                    "Win32Launch: 子発見 pid={} parent={} (Launcher({}) → child, OK)",
                    v.0, v.1, launcher_child
                );
            } else {
                warn!(
                    "Win32Launch: 子発見 pid={} parent_pid={} != launcher_pid={} (別世代/既存の可能性)",
                    v.0, v.1, launcher_child
                );
            }
            v
        }
        None => {
            if snapshot_failed {
                return Err(AdbError::CommandFailed {
                    message:
                        "Win32Launch: CreateToolhelp32Snapshot 呼び出し失敗により子を監視できなかった"
                            .to_string(),
                });
            }
            warn!(
                "Win32Launch: 子 {child} が {:.0}s 期間内に出現しなかった → Timeout",
                wait.as_secs_f64()
            );
            return Ok(EnsureOutcome::Timeout);
        }
    };

    // [ステップ3] 子発見後、GetExitCodeProcess == STILL_ACTIVE が継続するか監視。
    let mut last_exit_code: u32 = STILL_ACTIVE_U32;
    let mut alive = true;
    // 初回から OpenProcess 不可(権限等)は真の API エラーと区別するための成功履歴。
    let mut ever_queried_ok = false;
    while Instant::now() < overall_deadline {
        match query_exit_code(child_pid) {
            Ok(code) => {
                ever_queried_ok = true;
                last_exit_code = code;
                if code != STILL_ACTIVE_U32 {
                    alive = false;
                    break;
                }
            }
            Err(e) => {
                // 直前まで観測可能だったのに突然 OpenProcess 不可 → OS がプロセスを破棄 = 即死。
                // 初回から不可 → 真の API エラー。
                if ever_queried_ok {
                    warn!(
                        "Win32Launch: OpenProcess 失敗({e}) → 子消失 = 即死と判定 (pid={child_pid})"
                    );
                    alive = false;
                    break;
                } else {
                    return Err(AdbError::CommandFailed {
                        message: format!(
                            "Win32Launch: OpenProcess 呼び出し失敗(初回から不可): {e}"
                        ),
                    });
                }
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    if alive {
        info!(
            "Win32Launch: LAUNCH_OK child_pid={child_pid} survived={}s (STILL_ACTIVE)",
            wait.as_secs()
        );
        Ok(EnsureOutcome::Launched)
    } else {
        warn!(
            "Win32Launch: 子 pid={child_pid} parent={child_parent_pid} が即死: exit_code=0x{:08X} → Timeout",
            last_exit_code
        );
        Ok(EnsureOutcome::Timeout)
    }
}

/// `Launcher.exe` を起動し、その PID を返す。
///
/// **`Stdio::null()` 必須**: `Stdio::piped()` だとパイプを親が読まない限り Launcher の
/// 書き込みがブロックし、親プロセスがフリーズする(プローブで実証済み)。
/// `CREATE_NEW_PROCESS_GROUP`(0x00000200) でコンソール信号の親への伝播を切断する。
///
/// エラーはメッセージ文字列へ包む(呼び出し側で `AdbError::CommandFailed` 化)。
fn spawn_launcher(launcher: &str, workdir: &str) -> Result<u32, String> {
    let mut cmd = Command::new(launcher);
    cmd.current_dir(workdir)
        // パイプ継承回避: Launcher の stdout/stderr バッファ満杯によるブロックを防ぐ。
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // CREATE_NEW_PROCESS_GROUP: コンソール信号が親へ伝播しないようにする最小の分離。
        .creation_flags(CREATE_NEW_PROCESS_GROUP.0);

    cmd.spawn()
        .map(|child| {
            let pid = child.id();
            // std::process::Child の drop は kill も wait もしないので保持不要。
            // Launcher は即座に子を起動して Exit 0 で終わる設計。PID だけ記録。
            let _ = child;
            pid
        })
        .map_err(|e| format!("Launcher spawn 失敗: {e} (launcher={launcher}, workdir={workdir})"))
}

/// 子プロセスが現在プロセス一覧に存在するか。
fn child_exists(child: &str) -> bool {
    match snapshot_processes() {
        Ok(entries) => entries.iter().any(|e| e.name.eq_ignore_ascii_case(child)),
        Err(_) => false,
    }
}

// ===== Win32 ヘルパ =====

/// プロセススナップショットを取得して全プロセスを列挙する。
///
/// 共通ヘルパ `win32_proc::snapshot_processes` へ委譲(issue#2 で重複解消)。
/// 本モジュールからは `ProcEntry` の `name` / `pid` / `parent_pid` を全て使用する。
fn snapshot_processes() -> windows::core::Result<Vec<super::win32_proc::ProcEntry>> {
    super::win32_proc::snapshot_processes()
}

/// 指定 PID の exit code を取得する。生存なら `STILL_ACTIVE`(0x103)。
fn query_exit_code(pid: u32) -> windows::core::Result<u32> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?;
        let mut code: u32 = 0;
        let r = GetExitCodeProcess(handle, &mut code);
        let _ = CloseHandle(handle);
        r.map(|_| code)
    }
}
