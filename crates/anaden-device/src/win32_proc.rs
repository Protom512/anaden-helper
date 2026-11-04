#![cfg(windows)]

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};

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
