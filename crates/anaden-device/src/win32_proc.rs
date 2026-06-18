#![cfg(windows)]
//! Windows プロセス列挙の共通ヘルパ(Win32 バックエンド横断)。
//!
//! `CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS)` + `Process32First/NextW` を用いた
//! プロセス列挙を一本化し、capture / input / launch の各モジュールから参照する。
//!
//! 提供する API:
//! - [`snapshot_processes`]: 全プロセスを列挙し [`ProcEntry`] のリストを返す。
//! - [`find_pid_by_name`]: プロセス名(exe 名、大文字小文字区別なし)から PID を解決する。
//!
//! これらは元々 `win32_capture` / `win32_input` / `win32_launch` に重複定義されていた
//! 同等の処理を統合したもの(issue#2)。ロジックは input 版(`entry_name` ヘルパ経由の
//! 小文字比較)を基準とし、launch 版(`snapshot_processes` の Result 返却)を統合している。

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};

/// 1プロセス分の列挙結果。
///
/// `parent_pid` は子プロセス監視(launch)でのみ必要だが、列挙コストはほぼゼロのため
/// 常に保持する。capture/input は `pid` のみ、launch は `pid`/`parent_pid`/`name` を使う。
pub struct ProcEntry {
    /// プセス ID。
    pub pid: u32,
    /// 親プロセス ID(`PROCESSENTRY32W.th32ParentProcessID`)。
    pub parent_pid: u32,
    /// exe 名(`szExeFile`、拡張子含む)。
    pub name: String,
}

/// プロセススナップショットを取得して全プロセスを列挙する。
///
/// `CreateToolhelp32Snapshot` の成否を `windows::core::Result` で伝播する。
/// ハンドルはエラー時も含めて必ず `CloseHandle` してから返す。
pub fn snapshot_processes() -> windows::core::Result<Vec<ProcEntry>> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        // Handle はエラー時も CloseHandle してから伝播。
        let result = enum_inner(snapshot);
        let _ = CloseHandle(snapshot);
        result
    }
}

/// プロセス名(exe 名、大文字小文字区別なし、`.exe` 含む)から PID を解決する。
///
/// [`snapshot_processes`] の失敗時は `None`(列挙自体ができなければ PID も分からない)。
/// 最初に一致した PID を返す(同名プロセス複数起動時の挙動は capture/input と同一)。
///
/// 元々 `win32_capture::find_pid_by_name` / `win32_input::find_pid_by_name` の
/// 2実装が存在したが同等ロジックのため本関数へ統合(issue#2)。
pub fn find_pid_by_name(name: &str) -> Option<u32> {
    let needle = name.to_ascii_lowercase();
    let entries = snapshot_processes().ok()?;
    entries
        .into_iter()
        .find(|e| e.name.to_ascii_lowercase() == needle)
        .map(|e| e.pid)
}

/// `snapshot_processes` の列挙本体。snapshot ハンドルの `CloseHandle` は呼び出し元で行う。
///
/// Rust 2024 edition: `unsafe fn` 内でも明示 `unsafe` block が必要。
unsafe fn enum_inner(snapshot: HANDLE) -> windows::core::Result<Vec<ProcEntry>> {
    unsafe {
        let mut entries: Vec<ProcEntry> = Vec::new();
        let mut pe: PROCESSENTRY32W = std::mem::zeroed();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut pe).is_err() {
            // windows-rs 0.62 では Error::from_win32() が無いためスレッドの最終エラーから生成。
            return Err(windows::core::Error::from_thread());
        }

        loop {
            let name = entry_name(&pe.szExeFile);
            // windows-rs 0.62 の PROCESSENTRY32W はフラットなフィールド構造(共用体なし)。
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

/// `PROCESSENTRY32W.szExeFile`(ヌル終端 UTF-16 固定配列)から `String` を取り出す。
///
/// capture/input/launch 各モジュールで個別に持っていた同等のヘルパを統合。
fn entry_name(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}
