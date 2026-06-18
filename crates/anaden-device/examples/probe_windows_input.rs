//! Win32 SendInput / PostMessage 入力注入 検証プローブ (windows-input-wfsdrv-sendinput)。
//!
//! # 目的
//! Win32 `SendInput(INPUT_MOUSE)` で AnotherEden.exe のウィンドウ上の座標をクリックし、
//! ゲームが反応するか(= アンチチート wfsdrv に弾かれないか)を判定する。
//!
//! 前提の「SendInput 自体が本環境で機能するか」をメモ帳で先に確認(フェーズ1)した上で、
//! AnotherEden.exe を前景化して座標クリックを送信(フェーズ2)する 2 段階構成。
//!
//! 最終的に `pipeline_driver.rs` の `Input` trait(execute(&self, action: &InputAction))
//! を、InputAction::{Tap, Swipe, LongPress} を SendInput/PostMessage に翻訳する
//! Windows impl(Win32InputExecutor)で満たす足場を見据える。
//!
//! # API
//! - 主軸: Win32 `SendInput(INPUT_MOUSE)`。物理マウスと同等に扱われ、ゲストプロセスからは
//!   ユーザクリックと区別できない。wfsdrv(2021製)が DLL インジェクション型でなければ通す
//!   可能性が最も高い。
//! - fallback: `PostMessageW(WM_LBUTTONDOWN/UP)`。ウィンドウメッセージキューへの合成メッセージ。
//!   GetMessage 経由だと synthetic フラグが立ち一部ゲームは弾く → fallback 扱い。
//!   cocos2d-x は client 座標として lParam を解釈するため、ClientToScreen でクライアント座標を
//!   画面絶対座標へ直して SendInput、PostMessage はクライアント座標のまま lParam を組み立てる
//!   (実装注意点)。
//!
//! # 実行
//! フェーズ1(メモ帳で SendInput 前提確認。--self-test):
//!   cargo run --release -p anaden-device --example probe_windows_input -- --self-test
//!
//! フェーズ2(AnotherEden.exe 実ゲーム。プロセス名で PID 自動解決):
//!   cargo run --release -p anaden-device --example probe_windows_input -- \
//!     --process AnotherEden.exe --x 960 --y 540 --hold-ms 60 --settle-ms 1500
//!
//! フェーズ2 fallback(PostMessage 背面送信):
//!   cargo run --release -p anaden-device --example probe_windows_input -- \
//!     --process AnotherEden.exe --x 960 --y 540 --method postmessage
//!
//! 引数未指定時デフォルト:
//!   --process AnotherEden.exe --x 960 --y 540 --hold-ms 60 --settle-ms 1500 --method sendinput
//!
//! # 成功基準(自動判定)
//! 1. フェーズ1: メモ帳特定 + 前景化 OK + SendInput 戻り値==2 → "SELF_TEST PASSED"
//! 2. フェーズ2: PID 解決 + GetWindowRect + 前景化 + SendInput==2 → "SEND_INJECTION_OK"
//! 3. fallback: PostMessageW DOWN/UP 両 Ok → "POSTMESSAGE_DELIVERED"
//!
//! # 目視が必要
//! SendInput 戻り値==2 でも wfsdrv が到達前に握り潰せば「送信成功・反応なし」。
//! この切り分けは画面遷移の目視のみ。プローブは settle-ms 待った後 "CHECK VISUALLY" で止まる。

// Linux でワークスペースビルドが壊れないよう Windows 専用化。
#![cfg(windows)]
// API 検証プローブのため、未使用 import/dead_code を許容。
#![allow(dead_code)]

use std::mem;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetSystemMetrics, GetWindowRect, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, PostMessageW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_RESTORE, SetCursorPos, SetForegroundWindow,
    SetProcessDPIAware, ShowWindow, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WNDENUMPROC,
};
use windows::core::BOOL;

// wParam 区分(PostMessage 用)。
const MK_LBUTTON: usize = 0x0001;

// メソッド区分。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Method {
    SendInput,
    PostMessage,
}

// ---- プロセス名 → PID 解決 (CreateToolhelp32Snapshot + W 版で UTF-16) ----

fn entry_name(entry: &PROCESSENTRY32W) -> String {
    let mut end = entry.szExeFile.len();
    for (i, &c) in entry.szExeFile.iter().enumerate() {
        if c == 0 {
            end = i;
            break;
        }
    }
    String::from_utf16_lossy(&entry.szExeFile[..end])
}

fn find_pid_by_name(name_lower: &str) -> Option<u32> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry: PROCESSENTRY32W = mem::zeroed();
        entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut entry).is_err() {
            let _ = windows::Win32::Foundation::CloseHandle(snap);
            return None;
        }
        loop {
            if entry_name(&entry).to_ascii_lowercase() == name_lower {
                let pid = entry.th32ProcessID;
                let _ = windows::Win32::Foundation::CloseHandle(snap);
                return Some(pid);
            }
            if Process32NextW(snap, &mut entry).is_err() {
                break;
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
        None
    }
}

// ---- PID → 可視トップレベルウィンドウ HWND 解決 (EnumWindows) ----

// EnumWindows のコールバックは static mut で PID/結果を受け渡す(単発実行なので競合なし)。
// HWND.0 は *mut c_void(Windows 0.62)。
static mut G_MATCH_PID: u32 = 0;
static mut G_BEST: Option<*mut core::ffi::c_void> = None;

extern "system" fn enum_proc(hwnd: HWND, _lparam: LPARAM) -> BOOL {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        let mut wpid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut wpid as *mut u32));
        if wpid == G_MATCH_PID {
            G_BEST = Some(hwnd.0);
            return BOOL(0); // 最初の可視ウィンドウで十分 → 列挙停止
        }
        BOOL(1)
    }
}

fn find_main_window(pid: u32) -> Option<HWND> {
    unsafe {
        G_MATCH_PID = pid;
        G_BEST = None;
        let proc = WNDENUMPROC::Some(enum_proc);
        let _ = EnumWindows(proc, LPARAM(0));
        G_BEST.map(HWND)
    }
}

#[inline]
fn hwnd_eq(a: HWND, b: HWND) -> bool {
    a.0 == b.0
}

#[inline]
fn hwnd_is_null(h: HWND) -> bool {
    h.0.is_null()
}

// ---- DPI アウェア化 ----

/// main 先頭で呼び、SM_CXSCREEN/SM_CYSCREEN および仮想画面メトリクスが
/// 物理(DPI 正規化済み)ピクセルを返すようにする。
/// Per-Monitor V2 不可(古い OS)の場合はシステム DPI アウェアへフォールバック。
fn enable_dpi_awareness() {
    unsafe {
        // SetProcessDpiAwarenessContext は Result<()> を返す。
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            // フォールバック: システム DPI アウェア。
            let _ = SetProcessDPIAware();
        }
    }
}

// ---- 画面解像度 ----

/// 仮想デスクトップ全体(全モニタ結合)の幅・高さ・左上オフセットを返す。
/// MOUSEEVENTF_VIRTUALDESK と一貫させるため SM_CXSCREEN(プライマリのみ)ではなく
/// 仮想デスクトップ基準で正規化する。セカンダリモニタにゲームがある場合のずれを防ぐ。
fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let ox = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let oy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        (vw, vh, ox, oy)
    }
}

/// 画面絶対座標(px, 仮想デスクトップ原点基準) → 仮想デスクトップ絶対座標(0..65535)へ
/// 変換(SendInput 用)。MSDN 公式: (px * 65535) / (extent - 1)。
fn to_absolute(px: i32, py: i32, vw: i32, vh: i32) -> (i32, i32) {
    let vw1 = (vw as i64 - 1).max(1);
    let vh1 = (vh as i64 - 1).max(1);
    let dx = ((px as i64 * 65535) / vw1) as i32;
    let dy = ((py as i64 * 65535) / vh1) as i32;
    (dx, dy)
}

/// クライアント座標(client_x,client_y) → 画面全体座標(物理px)。SendInput 用。
/// ClientToScreen は BOOL(成功失敗)を返す。失敗時はウィンドウ rect 左上で補完。
fn client_to_screen_abs(hwnd: HWND, client_x: i32, client_y: i32) -> (i32, i32) {
    unsafe {
        let mut pt = POINT {
            x: client_x,
            y: client_y,
        };
        if ClientToScreen(hwnd, &mut pt as *mut POINT).as_bool() {
            (pt.x, pt.y)
        } else {
            // fallback: GetWindowRect 左上をクライアント原点とみなす。
            let mut r: RECT = mem::zeroed();
            let _ = GetWindowRect(hwnd, &mut r as *mut RECT);
            (r.left + client_x, r.top + client_y)
        }
    }
}

// ---- 前景化(AttachThreadInput 併用で確実化) ----

fn bring_to_foreground(hwnd: HWND) -> bool {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let cur_thread = GetCurrentThreadId();
        let fg = GetForegroundWindow();
        let fg_thread = if hwnd_is_null(fg) {
            0
        } else {
            GetWindowThreadProcessId(fg, None)
        };
        // 別スレッドがフォアを握る場合は入力キューを接続して SetForegroundWindow を通す。
        let mut attached = false;
        if fg_thread != 0 && fg_thread != cur_thread {
            attached = AttachThreadInput(cur_thread, fg_thread, true).as_bool();
        }
        let ok = SetForegroundWindow(hwnd);
        if attached {
            let _ = AttachThreadInput(cur_thread, fg_thread, false);
        }
        ok.as_bool()
    }
}

// ---- SendInput でクリック(DOWN + hold + UP) ----

/// 指定 dwFlags でマウスイベント1つを SendInput。戻り値=挿入されたイベント数。
unsafe fn send_mouse(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS) -> u32 {
    // Rust 2024: unsafe fn 内でも unsafe ブロックが必要。
    unsafe {
        let mut inp: INPUT = mem::zeroed();
        inp.r#type = INPUT_MOUSE;
        inp.Anonymous.mi = MOUSEINPUT {
            dx,
            dy,
            mouseData: 0,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };
        SendInput(&[inp], mem::size_of::<INPUT>() as i32)
    }
}

/// 画面絶対座標(px, 画面左上原点=仮想デスクトップ原点) で DOWN→hold→UP を送信。
/// DOWN/UP 両方の挿入成功(==2)なら 2 を返す。MOUSEEVENTF_VIRTUALDESK を使うため、
/// 仮想デスクトップ全体で正規化する(セカンダリモニタ基準のずれを回避)。
fn sendinput_click(screen_x: i32, screen_y: i32, hold_ms: u64) -> u32 {
    let (vw, vh, ox, oy) = virtual_screen();
    // 仮想デスクトップ原点(ox,oy)からの相対ピクセルへ変換して正規化。
    let (ax, ay) = to_absolute(screen_x - ox, screen_y - oy, vw, vh);
    let base = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE;
    // DOWN と UP は別 INPUT(独立イベント)。
    let down = unsafe { send_mouse(ax, ay, base | MOUSEEVENTF_LEFTDOWN) };
    std::thread::sleep(Duration::from_millis(hold_ms));
    let up = unsafe { send_mouse(ax, ay, base | MOUSEEVENTF_LEFTUP) };
    down + up // DOWN/UP 各1の挿入成功で計2。片方失敗(0)なら 0 or 1。
}

// ---- PostMessage で背面クリック(DOWN + hold + UP)。lParam はクライアント座標(MAKELPARAM) ----

fn make_lparam(client_x: i32, client_y: i32) -> LPARAM {
    // LPARAM = MAKELPARAM(y, x) = (y << 16) | (x & 0xFFFF)。下位=x, 高位=y。
    let v = (((client_y as u32) << 16) | ((client_x as u32) & 0xFFFF)) as isize;
    LPARAM(v)
}

/// (down_ok, up_ok)。各 Ok で 1、Err で 0。
/// WM_LBUTTONDOWN の前に WM_MOUSEMOVE でホバー状態を確立し、かつ SetCursorPos で
/// 実カーソルも該当 client 座標(画面絶対)へ移動する。一部ゲームが GetCursorPos で
/// 座標を再取得する不整合を防ぐ。
fn postmessage_click(hwnd: HWND, client_x: i32, client_y: i32, hold_ms: u64) -> (u32, u32) {
    let lp = make_lparam(client_x, client_y);
    // 実カーソルを client 座標(画面絶対)へ移動 → GetCursorPos で再取得するゲームとの整合。
    let (sx, sy) = client_to_screen_abs(hwnd, client_x, client_y);
    unsafe {
        let _ = SetCursorPos(sx, sy);
        // ホバー状態を作るため WM_MOUSEMOVE も送る(一部ゲームはこれを見てから DOWN を受ける)。
        let _ = PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(0), lp);
        let down = PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(MK_LBUTTON), lp);
        std::thread::sleep(Duration::from_millis(hold_ms));
        let up = PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(MK_LBUTTON), lp);
        (down.is_ok() as u32, up.is_ok() as u32)
    }
}

// ---- 引数解析 ----

struct Args {
    self_test: bool,
    process: String,
    x: i32,
    y: i32,
    hold_ms: u64,
    settle_ms: u64,
    method: Method,
}

fn parse_args() -> Args {
    let argv: Vec<String> = std::env::args().collect();
    let mut a = Args {
        self_test: false,
        process: "AnotherEden.exe".to_string(),
        x: 960,
        y: 540,
        hold_ms: 60,
        settle_ms: 1500,
        method: Method::SendInput,
    };
    let mut i = 1;
    while i < argv.len() {
        let k = argv[i].as_str();
        let mut next = || -> Option<String> {
            i += 1;
            argv.get(i).cloned()
        };
        match k {
            "--self-test" => a.self_test = true,
            "--process" => {
                if let Some(v) = next() {
                    a.process = v;
                }
            }
            "--x" => {
                if let Some(v) = next() {
                    if let Ok(n) = v.parse() {
                        a.x = n;
                    }
                }
            }
            "--y" => {
                if let Some(v) = next() {
                    if let Ok(n) = v.parse() {
                        a.y = n;
                    }
                }
            }
            "--hold-ms" => {
                if let Some(v) = next() {
                    if let Ok(n) = v.parse() {
                        a.hold_ms = n;
                    }
                }
            }
            "--settle-ms" => {
                if let Some(v) = next() {
                    if let Ok(n) = v.parse() {
                        a.settle_ms = n;
                    }
                }
            }
            "--method" => {
                if let Some(v) = next() {
                    a.method = match v.as_str() {
                        "postmessage" => Method::PostMessage,
                        _ => Method::SendInput,
                    };
                }
            }
            _ => {}
        }
        i += 1;
    }
    a
}

// ---- フェーズ1: notepad で SendInput 前提確認 ----

fn phase1_self_test() -> i32 {
    println!(">>> PHASE1 SELF_TEST: notepad で SendInput 前提確認");
    let pid = match find_pid_by_name("notepad.exe") {
        Some(p) => p,
        None => {
            println!(
                ">>> [FAIL] notepad.exe が起動していません。メモ帳を開いてから --self-test を再実行してください。"
            );
            println!(">>> SELF_TEST FAILED: window=notfound");
            return 1;
        }
    };
    println!("[INFO] notepad.exe pid={pid}");
    let hwnd = match find_main_window(pid) {
        Some(h) => h,
        None => {
            println!(">>> [FAIL] notepad の可視ウィンドウが見つかりません");
            println!(">>> SELF_TEST FAILED: no visible window");
            return 1;
        }
    };
    println!("[INFO] notepad HWND={:?}", hwnd.0);
    let _ = bring_to_foreground(hwnd);
    std::thread::sleep(Duration::from_millis(150));
    let fg = unsafe { GetForegroundWindow() };
    let fg_ok = hwnd_eq(fg, hwnd);
    println!(
        "[{}] foreground==notepad (fg={:?})",
        if fg_ok { "PASS" } else { "FAIL" },
        fg.0
    );
    // notepad クライアント中央へ適当にクリック(反応は問わない)。
    let (cx, cy) = client_to_screen_abs(hwnd, 100, 100);
    let sent = sendinput_click(cx, cy, 30);
    let sent_ok = sent == 2;
    println!(
        "[{}] SendInput 挿入数==2 (got={})",
        if sent_ok { "PASS" } else { "FAIL" },
        sent
    );
    if sent_ok && fg_ok {
        println!(">>> PHASE1 SELF_TEST PASSED (window=notepad, foreground=OK, sent={sent})");
        0
    } else {
        println!(">>> SELF_TEST FAILED: sent={sent}<2 (または foreground 失敗)");
        1
    }
}

// ---- フェーズ2: 対象プロセスのウィンドウをクリック ----

fn phase2(a: &Args) -> i32 {
    println!(">>> PHASE2: process={} method={:?}", a.process, a.method);
    let pid = match find_pid_by_name(&a.process.to_ascii_lowercase()) {
        Some(p) => p,
        None => {
            println!(
                ">>> ERROR: process not found ({}). ゲームを起動してから再実行してください。",
                a.process
            );
            return 2;
        }
    };
    println!("[INFO] {} pid={}", a.process, pid);
    let hwnd = match find_main_window(pid) {
        Some(h) => h,
        None => {
            println!(">>> ERROR: no visible window for pid={pid}");
            return 2;
        }
    };
    println!("[INFO] target HWND={:?}", hwnd.0);

    let rect = unsafe {
        let mut r: RECT = mem::zeroed();
        if let Err(e) = GetWindowRect(hwnd, &mut r as *mut RECT) {
            println!(">>> [FAIL] GetWindowRect: {e}");
            return 3;
        }
        r
    };
    let rect_ok = rect.right > rect.left && rect.bottom > rect.top;
    println!(
        "[{}] GetWindowRect: left={} top={} right={} bottom={} ({}x{})",
        if rect_ok { "PASS" } else { "FAIL" },
        rect.left,
        rect.top,
        rect.right,
        rect.bottom,
        rect.right - rect.left,
        rect.bottom - rect.top
    );
    if !rect_ok {
        println!(">>> [FAIL] rect 妥当性違反");
        return 3;
    }

    // 指定座標(x,y)はウィンドウのクライアント左上からの相対ピクセル。
    // pipeline_driver.rs InputAction 座標(画面左上原点の実ピクセル)と対称にするため、
    // クライアント原点の画面座標 + (x,y) で SendInput 用画面絶対座標を作る。
    let (origin_x, origin_y) = client_to_screen_abs(hwnd, 0, 0);
    let screen_x = origin_x + a.x;
    let screen_y = origin_y + a.y;
    println!(
        "[INFO] click target: client({},{}) -> screen_abs({},{}) [client_origin=({},{})]",
        a.x, a.y, screen_x, screen_y, origin_x, origin_y
    );

    match a.method {
        Method::SendInput => {
            let _ = bring_to_foreground(hwnd);
            std::thread::sleep(Duration::from_millis(150));
            let fg = unsafe { GetForegroundWindow() };
            let fg_ok = hwnd_eq(fg, hwnd);
            println!(
                "[{}] foreground==target (fg={:?})",
                if fg_ok { "PASS" } else { "FAIL" },
                fg.0
            );
            if !fg_ok {
                println!(
                    ">>> [FAIL] 前景化失敗。別アプリがフォアを握るか SetForegroundWindow 拒否。"
                );
                return 4;
            }
            let sent = sendinput_click(screen_x, screen_y, a.hold_ms);
            let sent_ok = sent == 2;
            println!(
                "[{}] SendInput 挿入数==2 (got={})",
                if sent_ok { "PASS" } else { "FAIL" },
                sent
            );
            if !sent_ok {
                println!(
                    ">>> [FAIL] SendInput 戻り値<2。UIPI/デスクトップ分離/管理者権限不足でブロックの疑い。"
                );
                return 5;
            }
            println!(
                ">>> PHASE2 SEND_INJECTION_OK (window={:?}, foreground=OK, sent={})",
                hwnd.0, sent
            );
            // 反応は目視。settle-ms 待って止まる。
            std::thread::sleep(Duration::from_millis(a.settle_ms));
            println!(
                ">>> CHECK VISUALLY: クリック座標に反応があったかゲーム画面を確認 (client=({},{}) screen_abs=({},{})",
                a.x, a.y, screen_x, screen_y
            );
            0
        }
        Method::PostMessage => {
            // 背面送信。前景化不要。lParam はクライアント座標(a.x,a.y)。
            let (down_ok, up_ok) = postmessage_click(hwnd, a.x, a.y, a.hold_ms);
            println!(
                "[{}] PostMessage DOWN Ok (got={})",
                if down_ok == 1 { "PASS" } else { "FAIL" },
                down_ok
            );
            println!(
                "[{}] PostMessage UP Ok (got={})",
                if up_ok == 1 { "PASS" } else { "FAIL" },
                up_ok
            );
            if down_ok != 1 || up_ok != 1 {
                println!(">>> [FAIL] PostMessageW が Err を返しました");
                return 6;
            }
            println!(
                ">>> PHASE2 POSTMESSAGE_DELIVERED (window={:?}, lParam=client({},{})",
                hwnd.0, a.x, a.y
            );
            std::thread::sleep(Duration::from_millis(a.settle_ms));
            println!(
                ">>> CHECK VISUALLY: PostMessage 背面クリックに反応があったかゲーム画面を確認 (client=({},{})",
                a.x, a.y
            );
            0
        }
    }
}

fn main() {
    // 先頭で DPI アウェア化: 画面メトリクスを物理ピクセルに揃え、座標計算のずれを防ぐ。
    enable_dpi_awareness();
    let a = parse_args();
    let code = if a.self_test {
        phase1_self_test()
    } else {
        phase2(&a)
    };
    std::process::exit(code);
}
