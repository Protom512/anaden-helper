//! readiness probe の Win32 I/O 層 (Issue #275)。
//!
//! `readiness` (判定コア・純関数) への入力を、有界 (bounded) のサンプリングループで
//! 収集する。1 サンプル = キャプチャ 1 枚のフレームダイジェスト + 対象プロセス群の
//! TCP ソケット数。
//!
//! 使う Win32 API:
//! - フレーム取得: [`Win32Capture::capture_blocking`](crate::Win32Capture) (PrintWindow)。
//! - ソケット数: `GetExtendedTcpTable`(AF_INET + AF_INET6, `TCP_TABLE_OWNER_PID_ALL`)。
//!   手動 probe の `netstat -ano | grep <pid>` 相当を API 呼び出しで行う
//!   (外部プロセス起動・テキストパース無し)。
//!
//! ループは「render 変化を観測した時点で打ち切り」を行う。変化 = Active 確定であり、
//! 以降のサンプリング (interval sleep を含む) は時間の無駄のためである。逆に
//! フレームが不変の場合は設定サンプル数まで取り切ってから判定コアへ渡す
//! (「一定時間不変」の時間跨度が必要なため)。

#![cfg(windows)]

use std::mem::size_of;
use std::time::Duration;

use tracing::{info, warn};
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};

use crate::error::DeviceError;
use crate::readiness::{ReadinessSample, frame_digest};
use crate::win32_capture::Win32Capture;
use crate::win32_proc::snapshot_processes;

/// readiness probe の既定サンプル数。
///
/// 手動 probe の実績 (session-log 2026-11-04): attempt 2 は 3 captures (~12s) で
/// hang 確定、attempt 1 は 5 captures (~2.5 min)。4 x 5s (~15s) を既定として
/// 正常起動の誤検出 (遷移間の静的ホールド) と検出所要時間のバランスを取る。
pub const DEFAULT_READINESS_SAMPLES: usize = 4;

/// readiness probe の既定サンプリング間隔。
pub const DEFAULT_READINESS_INTERVAL: Duration = Duration::from_secs(5);

/// readiness probe の設定。
#[derive(Debug, Clone)]
pub struct ReadinessProbeConfig {
    /// サンプル数 (>= 2 を想定。判定コアは 2 未満を fail-closed 扱いする)。
    pub samples: usize,
    /// サンプル間隔。
    pub interval: Duration,
}

impl Default for ReadinessProbeConfig {
    fn default() -> Self {
        Self {
            samples: DEFAULT_READINESS_SAMPLES,
            interval: DEFAULT_READINESS_INTERVAL,
        }
    }
}

/// 対象プロセスの readiness サンプルを有界サンプリングで収集する。
///
/// ブロッキング I/O (PrintWindow キャプチャ・テーブル取得・sleep) を含むため
/// `spawn_blocking` へ逃し、呼び出し側の async ランタイムを止めない
/// (`Win32Launch::ensure_open` と同じ構造)。
///
/// 失敗 (プロセス消失・キャプチャ失敗・テーブル取得失敗) は [`Err`] で
/// fail-closed に伝播する。「判定できない」を偽の Active に黙すことはしない。
pub async fn probe_readiness(
    process: &str,
    config: ReadinessProbeConfig,
) -> Result<Vec<ReadinessSample>, DeviceError> {
    let process = process.to_string();
    tokio::task::spawn_blocking(move || probe_readiness_blocking(&process, &config))
        .await
        .map_err(|e| DeviceError::CommandFailed {
            message: format!("readiness probe の blocking タスクがパニック/中止: {e}"),
        })?
}

/// `probe_readiness` の同期本体。
fn probe_readiness_blocking(
    process: &str,
    config: &ReadinessProbeConfig,
) -> Result<Vec<ReadinessSample>, DeviceError> {
    let pids = resolve_pids(process)?;
    info!(
        "readiness probe: {process} pids={pids:?} samples={} interval={:?}",
        config.samples, config.interval
    );
    let capture = Win32Capture::new(process);
    let mut samples: Vec<ReadinessSample> = Vec::new();
    for i in 0..config.samples {
        let img = capture.capture_blocking()?;
        let digest = frame_digest(&img);
        let socket_count = count_tcp_sockets(&pids)?;
        info!(
            "readiness probe: sample {}/{} digest={:016x} sockets={socket_count}",
            i + 1,
            config.samples,
            digest.0
        );
        samples.push(ReadinessSample {
            digest,
            socket_count,
        });
        // 初回サンプルと異なるダイジェスト = レンダ変化 = Active 確定。
        // 以降のサンプリングを打ち切る (Issue #275 AC1 の bounded wait)。
        if let Some(first) = samples.first()
            && first.digest != digest
        {
            info!("readiness probe: フレーム変化を観測 — サンプリング打ち切り");
            return Ok(samples);
        }
        if i + 1 < config.samples {
            std::thread::sleep(config.interval);
        }
    }
    warn!(
        "readiness probe: {config:?} サンプル中にフレーム変化無し (digest 不変) — 判定は classify_readiness へ"
    );
    Ok(samples)
}

/// プロセス名に一致する全 PID を解決する。
///
/// 複数インスタンス (ランチャー重複等) に備え、ソケット計上は一致 PID 全てを
/// 対象とする。1 つも見つからない場合は [`Err`] (起動保証直後に消失した等)。
fn resolve_pids(process: &str) -> Result<Vec<u32>, DeviceError> {
    let entries = snapshot_processes().map_err(|e| DeviceError::CommandFailed {
        message: format!("readiness probe: プロセス列挙失敗: {e}"),
    })?;
    let needle = process.to_ascii_lowercase();
    let pids: Vec<u32> = entries
        .into_iter()
        .filter(|e| e.name.to_ascii_lowercase() == needle)
        .map(|e| e.pid)
        .collect();
    if pids.is_empty() {
        return Err(DeviceError::CommandFailed {
            message: format!("readiness probe: {process} が見つからない (起動直後に消失した?)"),
        });
    }
    Ok(pids)
}

/// 対象 PID 群が保持する TCP ソケット数 (v4 + v6、LISTEN 含む全状態)。
///
/// 手動 probe の `netstat -ano` 行数カウントに相当する。
pub fn count_tcp_sockets(pids: &[u32]) -> Result<u32, DeviceError> {
    let v4 = count_tcp_table(pids, AF_INET.0 as u32, size_of::<MIB_TCPROW_OWNER_PID>())?;
    let v6 = count_tcp_table(pids, AF_INET6.0 as u32, size_of::<MIB_TCP6ROW_OWNER_PID>())?;
    Ok(v4 + v6)
}

/// `GetExtendedTcpTable` (TCP_TABLE_OWNER_PID_ALL) で指定アドレスファミリの
/// テーブルを取得し、`pids` が所有する行数を数える。
///
/// 行レイアウトはいずれも `dwOwningPid` が最終フィールドであるため、行サイズのみで
/// パースする (transmute 不要・境界アクセスは `get` 経由で panic 無し)。
fn count_tcp_table(pids: &[u32], af: u32, row_size: usize) -> Result<u32, DeviceError> {
    let mut size: u32 = 0;
    // 1 回目はサイズ問い合わせ (バッファ不足エラーは期待された戻り値のため無視)。
    unsafe {
        GetExtendedTcpTable(None, &mut size, false, af, TCP_TABLE_OWNER_PID_ALL, 0);
    }
    if size == 0 {
        // テーブル空 (ソケット 0 本)。問い合わせ自体は成功相当。
        return Ok(0);
    }
    let mut buf = vec![0u8; size as usize];
    let rc = unsafe {
        GetExtendedTcpTable(
            Some(buf.as_mut_ptr().cast()),
            &mut size,
            false,
            af,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        )
    };
    if rc != 0 {
        return Err(DeviceError::CommandFailed {
            message: format!("readiness probe: GetExtendedTcpTable 失敗 (rc={rc}, af={af})"),
        });
    }
    // 先頭 u32 = 行数 (dwNumEntries)。その直後に行が続く。chunks_exact は完全な行
    // のみを列挙するため、行数ヘッダとバッファ長の不整合でも panic しない。
    let rows = buf.get(4..).unwrap_or(&[]);
    let mut count = 0u32;
    for row in rows.chunks_exact(row_size) {
        // dwOwningPid は v4/v6 いずれの行でも最終 u32。
        if let Some(pid) = read_u32(row, row_size - 4)
            && pids.contains(&pid)
        {
            count += 1;
        }
    }
    Ok(count)
}

/// バッファ先頭からの little-endian (native) u32 読み取り。範囲外は `None`。
fn read_u32(buf: &[u8], offset: usize) -> Option<u32> {
    let bytes = buf.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}
