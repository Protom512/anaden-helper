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

pub const DEFAULT_READINESS_SAMPLES: usize = 4;

pub const DEFAULT_READINESS_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct ReadinessProbeConfig {
    pub samples: usize,
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

pub fn count_tcp_sockets(pids: &[u32]) -> Result<u32, DeviceError> {
    let v4 = count_tcp_table(pids, AF_INET.0 as u32, size_of::<MIB_TCPROW_OWNER_PID>())?;
    let v6 = count_tcp_table(pids, AF_INET6.0 as u32, size_of::<MIB_TCP6ROW_OWNER_PID>())?;
    Ok(v4 + v6)
}

fn count_tcp_table(pids: &[u32], af: u32, row_size: usize) -> Result<u32, DeviceError> {
    let mut size: u32 = 0;
    unsafe {
        GetExtendedTcpTable(None, &mut size, false, af, TCP_TABLE_OWNER_PID_ALL, 0);
    }
    if size == 0 {
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
    let rows = buf.get(4..).unwrap_or(&[]);
    let mut count = 0u32;
    for row in rows.chunks_exact(row_size) {
        if let Some(pid) = read_u32(row, row_size - 4)
            && pids.contains(&pid)
        {
            count += 1;
        }
    }
    Ok(count)
}

fn read_u32(buf: &[u8], offset: usize) -> Option<u32> {
    let bytes = buf.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}
