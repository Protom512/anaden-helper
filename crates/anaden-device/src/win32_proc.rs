#![cfg(windows)]

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetParent, GetWindowThreadProcessId, IsWindowVisible,
};
use windows::core::BOOL;

pub struct ProcEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
}

pub fn snapshot_processes() -> windows::core::Result<Vec<ProcEntry>> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        let result = enum_inner(snapshot);
        let _ = CloseHandle(snapshot);
        result
    }
}

pub fn find_pid_by_name(name: &str) -> Option<u32> {
    let needle = name.to_ascii_lowercase();
    let entries = snapshot_processes().ok()?;
    entries
        .into_iter()
        .find(|e| e.name.to_ascii_lowercase() == needle)
        .map(|e| e.pid)
}

unsafe fn enum_inner(snapshot: HANDLE) -> windows::core::Result<Vec<ProcEntry>> {
    unsafe {
        let mut entries: Vec<ProcEntry> = Vec::new();
        let mut pe: PROCESSENTRY32W = std::mem::zeroed();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut pe).is_err() {
            return Err(windows::core::Error::from_thread());
        }

        loop {
            let name = entry_name(&pe.szExeFile);
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

fn entry_name(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

struct HwndSearchState {
    target_pid: u32,
    found: Option<HWND>,
}

pub(crate) fn find_visible_top_level_window(pid: u32) -> Option<HWND> {
    let mut state = HwndSearchState {
        target_pid: pid,
        found: None,
    };
    let lparam = LPARAM(&mut state as *mut HwndSearchState as isize);
    unsafe {
        let _ = EnumWindows(Some(enum_proc), lparam);
    }
    state.found
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        let state = &mut *(lparam.0 as *mut HwndSearchState);
        if state.found.is_some() {
            return BOOL(0);
        }
        let visible = IsWindowVisible(hwnd).as_bool();
        let owned = is_owned_window(hwnd);
        let mut wpid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut wpid as *mut u32));
        if is_candidate_window(visible, owned, wpid, state.target_pid) {
            state.found = Some(hwnd);
            return BOOL(0);
        }
        BOOL(1)
    }
}

fn is_owned_window(hwnd: HWND) -> bool {
    match unsafe { GetParent(hwnd) } {
        Ok(parent) => !parent.is_invalid(),
        Err(_) => false,
    }
}

fn is_candidate_window(visible: bool, owned: bool, pid: u32, target_pid: u32) -> bool {
    visible && !owned && pid == target_pid
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn unowned_visible_window_of_target_pid_is_the_candidate() {
        assert!(is_candidate_window(true, false, 42, 42));
    }

    #[test]
    fn owned_window_is_never_a_candidate_even_for_target_pid() {
        assert!(!is_candidate_window(true, true, 42, 42));
    }

    #[test]
    fn invisible_window_is_never_a_candidate() {
        assert!(!is_candidate_window(false, false, 42, 42));
    }

    #[test]
    fn pid_mismatch_is_never_a_candidate() {
        assert!(!is_candidate_window(true, false, 7, 42));
    }
}
