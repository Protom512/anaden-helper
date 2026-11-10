#![cfg(windows)]

use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetParent, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
};

use anaden_device::point_on_any_monitor;

fn parse_i32(name: &str, v: Option<&String>) -> Result<i32, String> {
    v.and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("{name} needs i32"))
}

fn main() -> Result<(), String> {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            return Err("dpi awareness setup failed".into());
        }
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut process = "AnotherEden.exe".to_string();
    let mut x: Option<i32> = None;
    let mut y: Option<i32> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--process" => {
                process = args
                    .get(i + 1)
                    .cloned()
                    .ok_or_else(|| "--process needs value".to_string())?;
                i += 2;
            }
            "--x" => {
                x = Some(parse_i32("--x", args.get(i + 1))?);
                i += 2;
            }
            "--y" => {
                y = Some(parse_i32("--y", args.get(i + 1))?);
                i += 2;
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }
    let (nx, ny) = match (x, y) {
        (Some(x), Some(y)) => (x, y),
        _ => {
            return Err(
                "usage: move_window --x <i32> --y <i32> [--process AnotherEden.exe]\n\
                 Moves the top-left of the game window, keeping its current size.\
                 \nMove-only (SWP_NOSIZE): resizing can stall the renderer (DEF-010)."
                    .into(),
            );
        }
    };

    let hwnd =
        find_window(&process).ok_or_else(|| format!("visible window not found: {process}"))?;

    let before = window_rect(hwnd)?;
    println!(
        "before: window {}x{} at ({},{})",
        before.right - before.left,
        before.bottom - before.top,
        before.left,
        before.top
    );

    unsafe {
        SetWindowPos(
            hwnd,
            None,
            nx,
            ny,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOACTIVATE,
        )
        .map_err(|e| format!("SetWindowPos failed: {e}"))?;
    }

    let after = window_rect(hwnd)?;
    println!(
        "after:  window {}x{} at ({},{})",
        after.right - after.left,
        after.bottom - after.top,
        after.left,
        after.top
    );
    let corners = [
        (after.left, after.top),
        (after.right, after.top),
        (after.left, after.bottom),
        (after.right, after.bottom),
    ];
    let off = corners
        .iter()
        .filter(|(cx, cy)| !point_on_any_monitor(*cx, *cy))
        .count();
    println!("corners off-monitor: {off}/4");
    println!(
        "VERDICT={}",
        if off == 0 {
            "FULLY_ON_MONITOR"
        } else {
            "PARTIALLY_OFF_MONITOR (DEF-009 placement)"
        }
    );
    Ok(())
}

fn window_rect(hwnd: HWND) -> Result<RECT, String> {
    let mut rect = RECT::default();
    unsafe {
        GetWindowRect(hwnd, &mut rect).map_err(|e| format!("GetWindowRect failed: {e}"))?;
    }
    Ok(rect)
}

fn find_pid_by_name(name: &str) -> Option<u32> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or_default(),
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_err() {
            return None;
        }
        loop {
            let cur = entry.szExeFile;
            let cut = cur.iter().position(|c| *c == 0).unwrap_or(cur.len());
            if String::from_utf16_lossy(&cur[..cut]) == name {
                return Some(entry.th32ProcessID);
            }
            if Process32NextW(snapshot, &mut entry).is_err() {
                return None;
            }
        }
    }
}

struct HwndSearchState {
    target_pid: u32,
    found: Option<HWND>,
}

fn find_window(process: &str) -> Option<HWND> {
    let mut state = HwndSearchState {
        target_pid: find_pid_by_name(process)?,
        found: None,
    };
    let lparam = LPARAM(&mut state as *mut HwndSearchState as isize);
    unsafe {
        let _ = EnumWindows(Some(enum_proc_pid), lparam);
    }
    state.found
}

unsafe extern "system" fn enum_proc_pid(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    unsafe {
        let state = &mut *(lparam.0 as *mut HwndSearchState);
        if state.found.is_some() {
            return windows::Win32::Foundation::TRUE;
        }
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return windows::Win32::Foundation::TRUE;
        }
        if let Ok(parent) = GetParent(hwnd)
            && !parent.is_invalid()
        {
            return windows::Win32::Foundation::TRUE;
        }
        let mut wpid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut wpid as *mut u32));
        if wpid == state.target_pid {
            state.found = Some(hwnd);
        }
    }
    windows::Win32::Foundation::TRUE
}
