#![cfg(windows)]
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

const MK_LBUTTON: usize = 0x0001;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Method {
    SendInput,
    PostMessage,
}

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
            return BOOL(0);
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

fn enable_dpi_awareness() {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            let _ = SetProcessDPIAware();
        }
    }
}

fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let ox = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let oy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        (vw, vh, ox, oy)
    }
}

fn to_absolute(px: i32, py: i32, vw: i32, vh: i32) -> (i32, i32) {
    let vw1 = (vw as i64 - 1).max(1);
    let vh1 = (vh as i64 - 1).max(1);
    let dx = ((px as i64 * 65535) / vw1) as i32;
    let dy = ((py as i64 * 65535) / vh1) as i32;
    (dx, dy)
}

fn client_to_screen_abs(hwnd: HWND, client_x: i32, client_y: i32) -> (i32, i32) {
    unsafe {
        let mut pt = POINT {
            x: client_x,
            y: client_y,
        };
        if ClientToScreen(hwnd, &mut pt as *mut POINT).as_bool() {
            (pt.x, pt.y)
        } else {
            let mut r: RECT = mem::zeroed();
            let _ = GetWindowRect(hwnd, &mut r as *mut RECT);
            (r.left + client_x, r.top + client_y)
        }
    }
}

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

unsafe fn send_mouse(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS) -> u32 {
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

fn sendinput_click(screen_x: i32, screen_y: i32, hold_ms: u64) -> u32 {
    let (vw, vh, ox, oy) = virtual_screen();
    let (ax, ay) = to_absolute(screen_x - ox, screen_y - oy, vw, vh);
    let base = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE;
    let down = unsafe { send_mouse(ax, ay, base | MOUSEEVENTF_LEFTDOWN) };
    std::thread::sleep(Duration::from_millis(hold_ms));
    let up = unsafe { send_mouse(ax, ay, base | MOUSEEVENTF_LEFTUP) };
    down + up
}

fn make_lparam(client_x: i32, client_y: i32) -> LPARAM {
    let v = (((client_y as u32) << 16) | ((client_x as u32) & 0xFFFF)) as isize;
    LPARAM(v)
}

fn postmessage_click(hwnd: HWND, client_x: i32, client_y: i32, hold_ms: u64) -> (u32, u32) {
    let lp = make_lparam(client_x, client_y);
    let (sx, sy) = client_to_screen_abs(hwnd, client_x, client_y);
    unsafe {
        let _ = SetCursorPos(sx, sy);
        let _ = PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(0), lp);
        let down = PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(MK_LBUTTON), lp);
        std::thread::sleep(Duration::from_millis(hold_ms));
        let up = PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(MK_LBUTTON), lp);
        (down.is_ok() as u32, up.is_ok() as u32)
    }
}

struct Args {
    self_test: bool,
    process: String,
    x: i32,
    y: i32,
    hold_ms: u64,
    settle_ms: u64,
    method: Method,
}

fn next_value(argv: &[String], i: &mut usize) -> Option<String> {
    argv.get(*i + 1).cloned().inspect(|_| *i += 1)
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
        match k {
            "--self-test" => a.self_test = true,
            "--process" => {
                if let Some(v) = next_value(&argv, &mut i) {
                    a.process = v;
                }
            }
            "--x" => {
                a.x = next_value(&argv, &mut i)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(a.x)
            }
            "--y" => {
                a.y = next_value(&argv, &mut i)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(a.y)
            }
            "--hold-ms" => {
                a.hold_ms = next_value(&argv, &mut i)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(a.hold_ms)
            }
            "--settle-ms" => {
                a.settle_ms = next_value(&argv, &mut i)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(a.settle_ms)
            }
            "--method" => {
                if let Some(v) = next_value(&argv, &mut i) {
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
            std::thread::sleep(Duration::from_millis(a.settle_ms));
            println!(
                ">>> CHECK VISUALLY: クリック座標に反応があったかゲーム画面を確認 (client=({},{}) screen_abs=({},{})",
                a.x, a.y, screen_x, screen_y
            );
            0
        }
        Method::PostMessage => {
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
    enable_dpi_awareness();
    let a = parse_args();
    let code = if a.self_test {
        phase1_self_test()
    } else {
        phase2(&a)
    };
    std::process::exit(code);
}
