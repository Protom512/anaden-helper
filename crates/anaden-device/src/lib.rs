mod ensure;
mod error;
mod readiness;
#[cfg(windows)]
mod win32_capture;
#[cfg(windows)]
mod win32_input;
#[cfg(windows)]
mod win32_launch;
#[cfg(windows)]
mod win32_proc;
#[cfg(windows)]
mod win32_readiness;
#[cfg(windows)]
mod win32_terminate;

pub use ensure::EnsureOutcome;
pub use error::DeviceError;
pub use readiness::{
    FrameDigest, ReadinessSample, ReadinessVerdict, classify_readiness, frame_digest,
};
#[cfg(windows)]
pub use win32_capture::{DEFAULT_PROCESS_NAME, Win32Capture};
#[cfg(windows)]
pub use win32_input::{InputMethod, Win32InputExecutor, point_on_any_monitor};
#[cfg(windows)]
pub use win32_launch::{
    DEFAULT_CHILD, DEFAULT_LAUNCHER, DEFAULT_WAIT, DEFAULT_WORKDIR, Win32Launch,
};
#[cfg(windows)]
pub use win32_readiness::{
    DEFAULT_READINESS_INTERVAL, DEFAULT_READINESS_SAMPLES, ReadinessProbeConfig, count_tcp_sockets,
    probe_readiness,
};
#[cfg(windows)]
pub use win32_terminate::{DEFAULT_CLOSE_WAIT, TerminateStages, terminate_child_staged};
