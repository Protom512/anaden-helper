//! Windows 版ゲーム起動・生存監視の実証プローブ (probe_windows_launch)。
//!
//! 目的:
//!   Launcher.exe を起動し、子プロセス AnotherEden.exe が 0xC0000005 で即死せずに
//!   安定起動・N秒生存することを、ADB の `am start` を使わずに Win32 プロセス API 単独で
//!   判定する。これにより app_control.rs の AppController(ADB の am start + dumpsys 前景判定)
//!   を Windows 実装で置き換え可能か、つまり pipeline_driver.rs の Capture/Input trait
//!   (行60-71) に Windows バックエンドを被せる前段として「プロセス起動・生存監視」層が
//!   確証を持てることを実証する。KB5094126(WoW64 クラッシュ) は解決済み前提。
//!
//! 使う Win32 API (外部プロセス起動なし):
//!   [1] プロセス起動: std::process::Command (内部で CreateProcessW)。
//!       creation_flags に CREATE_NEW_PROCESS_GROUP(0x00000200) を付与。Launcher は即座に
//!       子 AnotherEden.exe を起動して Exit 0 で終わる設計のため、spawn() でハンドルだけ取り
//!       待機せず、子の出現を別ループで監視する。
//!   [2] プロセス列挙・生存監視: CreateToolhelp32Snapshot(TH32CS_SNAPSHOTPROCESS) +
//!       Process32FirstW / Process32NextW。PROCESSENTRY32W.szExeFile で子を探索。
//!       th32ProcessID / th32ParentProcessID が取れるので「Launcher の子である」ことまで検証可能。
//!   [3] exit code 取得: OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION) + GetExitCodeProcess。
//!       STILL_ACTIVE(0x103) なら生存、それ以外(0xC0000005 等 NT STATUS) を16進で表示。
//!
//! キャプチャ方式(BitBlt/PrintWindow/WGC)は本プローブのスコープ外(probe_windows_capture に譲る)。
//! Input も SendInput/SendMessage 系は probe_windows_input に譲る。
//! 本プローブは「プロセス起動 → 子出現 → 生存 → exit code」の4ステップに専念する。
//!
//! 実行(Windows 上):
//!   cargo build --release -p anaden-device --example probe_windows_launch
//!   cargo run --release -p anaden-device --example probe_windows_launch -- --wait 30
//!
//! 引数:
//!   --wait <秒>      AnotherEden.exe の起動〜生存監視の期間(既定 30)。Launcher spawn から
//!                    合計この秒数以内に子が出現し、かつ deadline まで STILL_ACTIVE が続けば成功
//!                    (発見フェーズ + 生存フェーズで合計 wait 秒を超えない)。
//!   --child <名前>   監視対象の子プロセス名(既定 AnotherEden.exe)。
//!   --launcher <p>   Launcher.exe パス。
//!   --workdir <p>    WorkingDirectory(Game フォルダ)。
//!
//! exit code(自動判定):
//!    0 = PASS. 全条件クリア。
//!   10 = Launcher.exe の spawn 失敗(パス違い/権限)。
//!   20 = 子プロセスが --wait 期間内に出現しなかった。
//!   30 = 子は出現したが --wait 以内に 0xC0000005 等で即死した(OpenProcess 不可による
//!        プロセス消失も即死とみなす)。
//!   40 = CreateToolhelp32Snapshot/OpenProcess の Win32 呼び出し自体がエラー。
//!   50 = 検証前に AnotherEden.exe が既に起動中(既存プロセス)。全て終了してから再実行。

#![cfg(windows)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

// ---- デフォルト定数(上部にまとめて引数で上書き可能) ----

/// Launcher.exe の既定パス。アナザーエデン PC 版インストール先。
const DEFAULT_LAUNCHER: &str =
    r"C:\Program Files\Wright Flyer Studios\ANOTHER EDEN\Launcher\Launcher.exe";
/// WorkingDirectory 既定値。AnotherEden.exe は相対パスでアセット/ICD を探すため必須。
const DEFAULT_WORKDIR: &str = r"C:\Program Files\Wright Flyer Studios\ANOTHER EDEN\Game";
/// 監視対象の子プロセス名(大文字小文字無視で比較)。
const DEFAULT_CHILD: &str = "AnotherEden.exe";
/// --wait 既定値(秒)。
const DEFAULT_WAIT_SECS: u64 = 30;
/// 子プロセス出現・生存監視のポーリング間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// 自動判定の最終結果を stdout へ出力する際のキーワード。
const PASS_TAG: &str = "[PASS]";
const FAIL_TAG: &str = "[FAIL]";
const MANUAL_TAG: &str = "[MANUAL]";

fn main() {
    // 引数(軽量パーサ。不明フラグは無視)。
    let args: Vec<String> = std::env::args().collect();
    let mut wait_secs: u64 = DEFAULT_WAIT_SECS;
    let mut child_name: String = DEFAULT_CHILD.to_string();
    let mut launcher: String = DEFAULT_LAUNCHER.to_string();
    let mut workdir: String = DEFAULT_WORKDIR.to_string();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--wait" => {
                if let Some(v) = args.get(i + 1)
                    && let Ok(n) = v.parse()
                {
                    wait_secs = n;
                }
                i += 2;
            }
            "--child" => {
                if let Some(v) = args.get(i + 1) {
                    child_name = v.clone();
                }
                i += 2;
            }
            "--launcher" => {
                if let Some(v) = args.get(i + 1) {
                    launcher = v.clone();
                }
                i += 2;
            }
            "--workdir" => {
                if let Some(v) = args.get(i + 1) {
                    workdir = v.clone();
                }
                i += 2;
            }
            other => {
                eprintln!("warn: 不明な引数を無視: {other}");
                i += 1;
            }
        }
    }

    let wait = Duration::from_secs(wait_secs);
    println!("== probe_windows_launch ==");
    println!("launcher : {launcher}");
    println!("workdir  : {workdir}");
    println!("child    : {child_name}");
    println!("wait     : {}s", wait_secs);

    let code = run(&launcher, &workdir, &child_name, wait);
    std::process::exit(code);
}

/// プローブ本体。exit code を返す。
fn run(launcher: &str, workdir: &str, child_name: &str, wait: Duration) -> i32 {
    // ---------------------------------------------------------------
    // [ステップ0] 事前ガード: spawn の直前に、監視対象の子プロセス(既定 AnotherEden.exe)
    //   が既に存在していないかスナップショットで確認。既存プロセスが残っていると、
    //   ステップ2 の「子発見」が本来 spawn した子ではなく既存プロセスにヒットしてしまい
    //   検証が無意味になる(偽成功)。存在すれば即座に exit 50 で停止し、検証前に全終了を促す。
    // ---------------------------------------------------------------
    match snapshot_processes() {
        Ok(entries) => {
            let preexisting = entries.iter().find(|e| eq_ignore_case(&e.name, child_name));
            if let Some(e) = preexisting {
                eprintln!(
                    "{FAIL_TAG} 既存プロセス検出: {child_name} が既に起動中 (pid={} parent_pid={})",
                    e.pid, e.parent_pid
                );
                eprintln!("     → exit 50: ゲーム既存プロセス検出。検証前に全て終了すること。");
                return 50;
            }
        }
        Err(e) => {
            // 事前スナップショット失敗は致命的(以降の監視もできない)。exit 40。
            eprintln!("{FAIL_TAG} 事前スナップショット失敗: {e}");
            eprintln!("     → exit 40: windows-rs feature / 昇格を確認。");
            return 40;
        }
    }

    // ---------------------------------------------------------------
    // [ステップ1] Launcher.exe を起動(spawn)。
    //   DETACHED_PROCESS ではなく CREATE_NEW_PROCESS_GROUP を付与。
    //   Launcher は即座に子 AnotherEden.exe を起動して自身は Exit 0 で終わる設計なので、
    //   spawn() でハンドルだけ取り、終了を待機しない。
    // ---------------------------------------------------------------
    let launcher_started = Instant::now();

    let spawn_result = Command::new(launcher)
        .current_dir(workdir)
        // Launcher の GL ログ出力でブロックしないよう stdio を切る(オプション)。
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // CREATE_NEW_PROCESS_GROUP(0x00000200)。コンソール信号がプローブ本体へ
        // 伝播しないようにする最小の分離。
        .creation_flags(CREATE_NEW_PROCESS_GROUP.0)
        .spawn();

    let launcher_child = match spawn_result {
        Ok(c) => {
            let pid = c.id();
            println!("[1] Launcher spawn OK (launcher_pid={pid})");
            // c をドロップすると子が kill される実装環境もあるため、明示的に離す。
            // std::process::Child の drop は kill しない(終了待ちもしない)ので保持不要。
            // ただし PID は記録しておく(親子関係検証用)。
            let _ = c;
            pid
        }
        Err(e) => {
            eprintln!(
                "{FAIL_TAG} Launcher spawn 失敗: {e} (launcher={launcher}, workdir={workdir})"
            );
            eprintln!("     → exit 10: パス/権限を確認してください。");
            return 10;
        }
    };

    // ---------------------------------------------------------------
    // [ステップ2] --wait 期間内に AnotherEden.exe が1回以上出現するかポーリング監視。
    //   ポーリング間隔 500ms。
    //
    //   ※待機時間の整合: doc「--wait 秒ずっと STILL_ACTIVE が続けば成功」に合わせ、
    //     発見フェーズ(discover)と生存確認フェーズ(survive)で「spawn から合計 wait 秒」を
    //     超えないよう、両フェーズ共通の絶対 deadline を launcher_started 起点で設ける。
    //     最悪でも合計 wait 秒(2×wait にならない)。
    // ---------------------------------------------------------------
    let overall_deadline = launcher_started + wait;
    let mut first_seen: Option<(u32, u32)> = None; // (pid, parent_pid)
    let mut snapshot_failed = false;

    while Instant::now() < overall_deadline {
        match snapshot_processes() {
            Ok(entries) => {
                for entry in &entries {
                    if eq_ignore_case(&entry.name, child_name) {
                        first_seen = Some((entry.pid, entry.parent_pid));
                        break;
                    }
                }
                if first_seen.is_some() {
                    break;
                }
            }
            Err(e) => {
                // スナップショット取得自体のエラー。後で exit 40 判定へ。
                eprintln!("warn: CreateToolhelp32Snapshot 失敗: {e}");
                snapshot_failed = true;
                break;
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    let (child_pid, child_parent_pid) = match first_seen {
        Some(v) => {
            let discover_ms = launcher_started.elapsed().as_millis();
            println!(
                "[2] 子プロセス発見: pid={} parent_pid={} (発見まで {}ms)",
                v.0, v.1, discover_ms
            );
            if v.1 == launcher_child {
                println!("    親子関係: Launcher({}) → child (OK)", launcher_child);
            } else {
                println!(
                    "    親子関係メモ: child parent_pid={} != launcher_pid={} (別Launcher世代/既存プロセスの可能性)",
                    v.1, launcher_child
                );
            }
            v
        }
        None => {
            if snapshot_failed {
                eprintln!(
                    "{FAIL_TAG} CreateToolhelp32Snapshot 呼び出し失敗により子を監視できなかった"
                );
                eprintln!("     → exit 40: windows-rs feature / 昇格を確認。");
                return 40;
            }
            eprintln!(
                "{FAIL_TAG} 子プロセス {child_name} が --wait({:.0}s) 期間内に出現しなかった",
                wait.as_secs_f64()
            );
            eprintln!(
                "     → exit 20: Launcher が子を起動していない/即座に失敗。KB5094126 回帰疑い。"
            );
            return 20;
        }
    };

    // ---------------------------------------------------------------
    // [ステップ3] 子発見後、GetExitCodeProcess == STILL_ACTIVE が継続するか監視。
    //   即死(0xC0000005 等)を検出する。生存監視も [ステップ2] と同じ絶対 deadline
    //   (overall_deadline = launcher_started + wait) を共有するため、発見+生存の合計は
    //   「spawn から wait 秒」に収まる(doc「--wait 秒ずっと STILL_ACTIVE なら成功」と整合)。
    // ---------------------------------------------------------------
    let mut last_exit_code: u32 = STILL_ACTIVE_0;
    let mut alive = true;
    // exit code の問い合わせに一度でも成功したか。初回から OpenProcess 不可(権限等)の場合は
    // 真の API エラー(exit 40)とするため、成功履歴で (a)即死/(b)APIエラー を区別する。
    let mut ever_queried_ok = false;

    while Instant::now() < overall_deadline {
        match query_exit_code(child_pid) {
            Ok(code) => {
                ever_queried_ok = true;
                last_exit_code = code;
                if code == STILL_ACTIVE_0 {
                    // 生存継続。次ポーリングへ。
                } else {
                    alive = false;
                    break;
                }
            }
            Err(e) => {
                // OpenProcess が失敗した2ケースを区別:
                //   (a) 直前まで子を観測できていた(一度でも Ok 返却有)のに突然 OpenProcess 不可に
                //       なった → OS がプロセスを破棄した = 即死(exit 30)。APIエラー扱いしない。
                //   (b) 権限不足等で最初から一度も OpenProcess 不可 → 真の API エラー(exit 40)。
                if ever_queried_ok {
                    eprintln!(
                        "warn: OpenProcess 失敗: {e} → 直前まで観測可能だった子が消失 = 即死と判定"
                    );
                    alive = false;
                    break;
                } else {
                    eprintln!("{FAIL_TAG} OpenProcess 呼び出し失敗(初回から不可): {e}");
                    eprintln!("     → exit 40: 権限/feature を確認。");
                    return 40;
                }
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    // ---------------------------------------------------------------
    // [ステップ4] 判定とサマリ出力。
    // ---------------------------------------------------------------
    let survived_secs: f64 = if alive {
        wait.as_secs_f64()
    } else {
        // 即死した時点の経過時間。
        launcher_started.elapsed().as_secs_f64()
    };

    if alive {
        // PASS: --wait 全期間 STILL_ACTIVE が続いた。
        println!(
            "{PASS_TAG} LAUNCH_OK child_pid={child_pid} survived={}s (exit_code=STILL_ACTIVE 0x103)",
            wait.as_secs()
        );
        print_summary(
            child_pid,
            child_parent_pid,
            launcher_child,
            0x103,
            survived_secs,
            true,
        );
        println!(
            "{MANUAL_TAG} ゲーム画面が実際に描画されタイトル/プレイ可能状態に到達したかを目視してください(プロセスが生きていても黒画面やロード停止の可能性あり)。これは probe_windows_capture に委ねる。"
        );
        return 0;
    }

    // FAIL exit 30: 即死。
    eprintln!(
        "{FAIL_TAG} 子プロセス {child_name}(pid={child_pid}) が即死: exit_code=0x{:08X}",
        last_exit_code
    );
    eprintln!(
        "     → exit 30: 0xC0000005 等の NT STATUS。launcher-opengl-fix.md の WoW64 クラッシュ回帰疑い。"
    );
    print_summary(
        child_pid,
        child_parent_pid,
        launcher_child,
        last_exit_code,
        survived_secs,
        false,
    );
    30
}

/// STILL_ACTIVE 定数の u32 表現。GetExitCodeProcess は戻り値を u32 で返す。
/// STILL_ACTIVE は Win32::Foundation の NTSTATUS newtype(0x103)。
const STILL_ACTIVE_0: u32 = STILL_ACTIVE.0 as u32; // 0x103

/// サマリ行を印字。呼出側スクリプトが文字列/exit code で判定できるようにする。
fn print_summary(
    child_pid: u32,
    child_parent_pid: u32,
    launcher_pid: u32,
    exit_code: u32,
    survived_secs: f64,
    alive: bool,
) {
    let status = if alive { "ALIVE(STILL_ACTIVE)" } else { "DEAD" };
    println!(
        "SUMMARY child_pid={child_pid} child_parent_pid={child_parent_pid} launcher_pid={launcher_pid} exit_code=0x{:08X} survived={:.2}s status={status}",
        exit_code, survived_secs
    );
}

// ===== Win32 ヘルパ =====

/// 1プロセス分の列挙結果。
struct ProcEntry {
    pid: u32,
    parent_pid: u32,
    name: String,
}

/// プロセススナップショットを取得して全プロセスを列挙する。
fn snapshot_processes() -> windows::core::Result<Vec<ProcEntry>> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        // Handle は確実に閉じる。エラー時も CloseHandle してから伝播。
        let result = enum_inner(snapshot);
        let _ = CloseHandle(snapshot);
        result
    }
}

unsafe fn enum_inner(snapshot: HANDLE) -> windows::core::Result<Vec<ProcEntry>> {
    // Rust 2024 edition: unsafe fn 内でも明示 unsafe block が必要。
    unsafe {
        let mut entries: Vec<ProcEntry> = Vec::new();
        let mut pe: PROCESSENTRY32W = std::mem::zeroed();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut pe).is_err() {
            // Process32FirstW の失敗は Win32 エラー。windows-rs 0.62 では
            // Error::from_win32() が無いため、スレッドの最終エラー文字列から生成する。
            return Err(windows::core::Error::from_thread());
        }

        loop {
            let name = wsz_to_string(&pe.szExeFile);
            // windows-rs 0.62 の PROCESSENTRY32W はフラットなフィールド構造
            // (共用体なし)。th32ProcessID / th32ParentProcessID を直接読む。
            let pid = pe.th32ProcessID;
            let parent_pid = pe.th32ParentProcessID;
            entries.push(ProcEntry {
                pid,
                parent_pid,
                name,
            });

            if Process32NextW(snapshot, &mut pe).is_err() {
                break;
            }
        }

        Ok(entries)
    }
}

/// PROCESSENTRY32W から pid / parent_pid を取り出す(将来拡張用に残す)。
#[allow(dead_code)]
#[inline]
fn read_pids(pe: &PROCESSENTRY32W) -> (u32, u32) {
    (pe.th32ProcessID, pe.th32ParentProcessID)
}

/// 指定 PID の exit code を取得する。生存なら STILL_ACTIVE(0x103)。
fn query_exit_code(pid: u32) -> windows::core::Result<u32> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)?;
        let mut code: u32 = 0;
        let r = GetExitCodeProcess(handle, &mut code);
        let _ = CloseHandle(handle);
        r.map(|_| code)
    }
}

/// ワイド文字列(NUW終端含む)を String へ。NUL で打ち切る。
fn wsz_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// 大文字小文字を無視して文字列が等しいか。
fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

// OsStr→ワイド変換ヘルパ(本プローブでは未使用だが、将来の引数拡張/Win32 API 文字列渡しの
// 参照実装として残す)。デッドコード警告抑制。
#[allow(dead_code)]
fn to_wide<S: AsRef<OsStr>>(s: S) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}
