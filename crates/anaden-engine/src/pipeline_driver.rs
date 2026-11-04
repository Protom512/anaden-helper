use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use image::DynamicImage;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use anaden_core::InputAction;
use anaden_core::ScreenRegion;
use anaden_device::DeviceError;
use anaden_vision::{BASE_WIDTH, CropInfo, ScreenScaler, TaskDef};

use crate::jitter::Jitter;
use crate::pipeline_runner::{InputCommand, PipelineState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryContext {
    pub attempt: u32,
    pub streak: u32,
}

pub type RecoveryHook = Box<
    dyn FnMut(RecoveryContext) -> Pin<Box<dyn Future<Output = Result<(), DeviceError>> + Send>>
        + Send,
>;

pub trait RecoveryClock: Send + Sync {
    fn elapsed(&mut self) -> Duration;
}

pub struct SystemRecoveryClock {
    started: Instant,
}

impl SystemRecoveryClock {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl Default for SystemRecoveryClock {
    fn default() -> Self {
        Self::new()
    }
}

impl RecoveryClock for SystemRecoveryClock {
    fn elapsed(&mut self) -> Duration {
        self.started.elapsed()
    }
}

struct RecoveryGuard {
    threshold: u32,
    grace: Duration,
    streak: u32,
    recovered_at: Option<Duration>,
    max_attempts: u32,
    attempts: u32,
    total_attempts: u32,
    max_consecutive: u32,
}

impl RecoveryGuard {
    fn new(threshold: u32, grace: Duration, max_attempts: u32) -> Self {
        Self {
            threshold,
            grace,
            streak: 0,
            recovered_at: None,
            max_attempts,
            attempts: 0,
            total_attempts: 0,
            max_consecutive: 0,
        }
    }

    fn streak(&self) -> u32 {
        self.streak
    }

    fn in_grace(&self, now: Duration) -> bool {
        match self.recovered_at {
            Some(t) => now.saturating_sub(t) < self.grace,
            None => false,
        }
    }

    fn record_miss(&mut self, now: Duration) -> bool {
        if self.in_grace(now) {
            return false;
        }
        self.streak = self.streak.saturating_add(1);
        self.threshold > 0 && self.streak >= self.threshold
    }

    fn record_match(&mut self) {
        self.streak = 0;
        self.recovered_at = None;
        self.attempts = 0;
    }

    fn recovered(&mut self, now: Duration) {
        self.streak = 0;
        self.recovered_at = Some(now);
    }

    fn consume_attempt(&mut self) -> Option<u32> {
        if self.attempts >= self.max_attempts {
            return None;
        }
        self.attempts = self.attempts.saturating_add(1);
        self.total_attempts = self.total_attempts.saturating_add(1);
        self.max_consecutive = self.max_consecutive.max(self.attempts);
        Some(self.attempts)
    }

    fn telemetry(&self) -> RecoveryTelemetry {
        RecoveryTelemetry {
            recovery_attempts: self.total_attempts,
            max_consecutive_recovery: self.max_consecutive,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RecoveryTelemetry {
    recovery_attempts: u32,
    max_consecutive_recovery: u32,
}

struct OutcomeCtx {
    per_task_matches: Vec<TaskMatchCount>,
    started: Instant,
    recovery: RecoveryTelemetry,
}

impl OutcomeCtx {
    fn new(per_task_matches: Vec<TaskMatchCount>, started: Instant, guard: &RecoveryGuard) -> Self {
        Self {
            per_task_matches,
            started,
            recovery: guard.telemetry(),
        }
    }
}

enum RecoveryStep {
    Recovered,
    Exhausted,
    HookFailed(DeviceError),
}

struct LoopState {
    iterations: u64,
    fired: Vec<InputCommand>,
    per_task_matches: Vec<TaskMatchCount>,
    started: Instant,
    guard: RecoveryGuard,
    recovery_enabled: bool,
}

impl LoopState {
    fn new(
        recover_nomatch_threshold: u32,
        grace: Duration,
        max_recovery_attempts: u32,
        recover_present: bool,
    ) -> Self {
        Self {
            iterations: 0,
            fired: Vec::new(),
            per_task_matches: Vec::new(),
            started: Instant::now(),
            guard: RecoveryGuard::new(recover_nomatch_threshold, grace, max_recovery_attempts),
            recovery_enabled: recover_nomatch_threshold > 0 && recover_present,
        }
    }

    fn ctx(&mut self) -> OutcomeCtx {
        OutcomeCtx::new(
            std::mem::take(&mut self.per_task_matches),
            self.started,
            &self.guard,
        )
    }

    fn record_match_tick(&mut self, current_before: &str) {
        self.guard.record_match();
        bump_task_match(current_before, &mut self.per_task_matches);
    }

    fn goal_eval_inputs(&self) -> GoalEvalInputs<'_> {
        GoalEvalInputs {
            iterations: self.iterations,
            fired_commands: &self.fired,
            per_task_matches: &self.per_task_matches,
            started: self.started,
            recovery: self.guard.telemetry(),
        }
    }
}

enum StepFlow {
    Continue,
    Stop(Box<LoopOutcome>),
}

impl StepFlow {
    fn stop(outcome: LoopOutcome) -> Self {
        Self::Stop(Box::new(outcome))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchArm {
    Fired,
    NoFire,
}

struct MatchStepInput<'a> {
    current_before: &'a str,
    next_current: Option<String>,
    just_fired: Option<InputCommand>,
    match_info: Option<(String, f32, ScreenRegion)>,
}

pub trait GoalClock: Send {
    fn elapsed_secs(&mut self) -> u64;
}

pub struct SystemClock {
    started: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl GoalClock for SystemClock {
    fn elapsed_secs(&mut self) -> u64 {
        self.started.elapsed().as_secs()
    }
}

struct GoalEvalInputs<'a> {
    iterations: u64,
    fired_commands: &'a [InputCommand],
    per_task_matches: &'a [TaskMatchCount],
    started: Instant,
    recovery: RecoveryTelemetry,
}

struct GoalLoopCtx<'a> {
    goal: &'a anaden_core::Goal,
    clock: &'a mut dyn GoalClock,
    goal_ctx: anaden_core::GoalStatusContext,
}

enum GoalMode<'a> {
    Inactive,
    Active(GoalLoopCtx<'a>),
}

pub fn rescale_command(cmd: InputCommand, device_width: u32, crop_info: &CropInfo) -> InputCommand {
    let to_real = |x_1280: u32, y_1280: u32| -> (u32, u32) {
        let s = (crop_info.width as f64) / (BASE_WIDTH as f64);
        let x_real = (x_1280 as f64 * s).round() as u64 + crop_info.offset_x as u64;
        let y_real = (y_1280 as f64 * s).round() as u64 + crop_info.offset_y as u64;
        (
            x_real.min(device_width as u64) as u32,
            y_real.min(device_width as u64) as u32,
        )
    };
    match cmd {
        InputCommand::Tap { x, y } => {
            let (rx, ry) = to_real(x, y);
            InputCommand::Tap { x: rx, y: ry }
        }
        InputCommand::Swipe { from, to } => {
            let (fx, fy) = to_real(from.0, from.1);
            let (tx, ty) = to_real(to.0, to.1);
            InputCommand::Swipe {
                from: (fx, fy),
                to: (tx, ty),
            }
        }
    }
}

#[async_trait]
pub trait Capture: Send + Sync {
    async fn capture(&self) -> Result<DynamicImage, DeviceError>;
}

#[async_trait]
pub trait Input: Send + Sync {
    async fn execute(&self, action: &InputAction) -> Result<(), DeviceError>;
}

#[cfg(windows)]
#[async_trait]
impl Capture for anaden_device::Win32Capture {
    async fn capture(&self) -> Result<DynamicImage, DeviceError> {
        anaden_device::Win32Capture::capture(self).await
    }
}

#[cfg(windows)]
#[async_trait]
impl Input for anaden_device::Win32InputExecutor {
    async fn execute(&self, action: &InputAction) -> Result<(), DeviceError> {
        anaden_device::Win32InputExecutor::execute(self, action).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepErrorKind {
    Capture,
    Execute,
    VerifyCapture,
    Other,
}

pub fn is_recoverable_io_error(kind: StepErrorKind) -> bool {
    matches!(kind, StepErrorKind::Capture | StepErrorKind::VerifyCapture)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    Fired {
        next_current: Option<String>,
        fired: Option<InputCommand>,
    },
    NoFire {
        next_current: Option<String>,
    },
    NoMatch,
    Error {
        kind: StepErrorKind,
        message: String,
    },
    FiredUnverified {
        next_current: Option<String>,
        fired: Option<InputCommand>,
    },
    WatchFired {
        task: String,
        fired: Option<InputCommand>,
    },
}

pub struct PipelineDriver<C: Capture, I: Input> {
    capture: C,
    input: I,
    scaler: ScreenScaler,
    state: PipelineState,
    tasks: Vec<TaskDef>,
    device_width: u32,
    swipe_duration_ms: u64,
    verify_after_fire: bool,
    last_match: Option<(String, f32, ScreenRegion)>,
    last_crop_info: CropInfo,
    cancel: Option<CancellationToken>,
    snapshot_counter: u64,
    diag_counter: u64,
    recovery_grace: Duration,
    recovery_clock: Box<dyn RecoveryClock>,
    max_recovery_attempts: u32,
    jitter: Option<Jitter>,
    verify_stats: VerifyStats,
    watch: Vec<String>,
}

impl<C: Capture, I: Input> PipelineDriver<C, I> {
    pub fn new(
        capture: C,
        input: I,
        state: PipelineState,
        tasks: Vec<TaskDef>,
        device_width: u32,
        swipe_duration_ms: u64,
    ) -> Self {
        Self {
            capture,
            input,
            scaler: ScreenScaler::new(),
            state,
            tasks,
            device_width,
            swipe_duration_ms,
            verify_after_fire: false,
            last_match: None,
            last_crop_info: CropInfo::default(),
            cancel: None,
            snapshot_counter: 0,
            diag_counter: 0,
            recovery_grace: Duration::ZERO,
            recovery_clock: Box::new(SystemRecoveryClock::new()),
            max_recovery_attempts: u32::MAX,
            jitter: None,
            verify_stats: VerifyStats::default(),
            watch: Vec::new(),
        }
    }

    pub fn with_verify(mut self, enabled: bool) -> Self {
        self.verify_after_fire = enabled;
        self
    }

    pub fn with_cancel(mut self, token: CancellationToken) -> Self {
        self.cancel = Some(token);
        self
    }

    pub fn with_recovery_grace(mut self, grace: Duration) -> Self {
        self.recovery_grace = grace;
        self
    }

    pub fn with_recovery_clock(mut self, clock: Box<dyn RecoveryClock>) -> Self {
        self.recovery_clock = clock;
        self
    }

    pub fn with_max_recovery_attempts(mut self, max_attempts: u32) -> Self {
        self.max_recovery_attempts = max_attempts;
        self
    }

    pub fn with_jitter(mut self, jitter: Option<Jitter>) -> Self {
        self.jitter = jitter;
        self
    }

    pub fn with_watch(mut self, watch: Vec<String>) -> Self {
        self.watch = watch;
        self
    }

    pub fn scaler(&self) -> &ScreenScaler {
        &self.scaler
    }

    pub fn current(&self) -> &str {
        self.state.current()
    }

    pub async fn run_once(&mut self) -> StepOutcome {
        let t_cycle = std::time::Instant::now();
        let t_cap = std::time::Instant::now();
        let screen = match self.capture.capture().await {
            Ok(img) => img,
            Err(e) => {
                warn!("pipeline capture error: {e}");
                return StepOutcome::Error {
                    kind: StepErrorKind::Capture,
                    message: format!("capture: {e}"),
                };
            }
        };
        let capture_ms = t_cap.elapsed().as_secs_f64() * 1000.0;
        let raw_w = screen.width();
        let raw_h = screen.height();
        let (normalized, crop_info) = self.scaler.normalize_capture(&screen);
        self.last_crop_info = crop_info;
        let crop_w = crop_info.width;
        let crop_h = crop_info.height;
        if (crop_w, crop_h) != (raw_w, raw_h) {
            debug!(
                "letterbox crop: raw={raw_w}x{raw_h} -> content={crop_w}x{crop_h} \
                 (offset={},{}, black bars removed)",
                crop_info.offset_x, crop_info.offset_y
            );
        }
        let norm_w = normalized.width();
        let norm_h = normalized.height();
        debug!(
            "frame raw={raw_w}x{raw_h} normalized={norm_w}x{norm_h} (device_width={})",
            self.device_width
        );
        if !self.watch.is_empty()
            && let Some(outcome) = self.check_watch(&normalized, norm_w, norm_h).await
        {
            return outcome;
        }
        let t_rec = std::time::Instant::now();
        let matched_task_name = self.state.current().to_string();
        let tick = match self.state.tick(&normalized, &self.tasks) {
            Ok(Some(r)) => r,
            Ok(None) => {
                debug!(
                    "cycle latency: capture={capture_ms:.2}ms recognize={:.2}ms e2e={:.2}ms (NoMatch) raw={raw_w}x{raw_h} norm={norm_w}x{norm_h}",
                    t_rec.elapsed().as_secs_f64() * 1000.0,
                    t_cycle.elapsed().as_secs_f64() * 1000.0
                );
                self.save_snapshot(&matched_task_name, &normalized);
                self.save_diagnose_report(&matched_task_name, &normalized);
                return StepOutcome::NoMatch;
            }
            Err(e) => {
                warn!("template detect error (task '{matched_task_name}'): {e}");
                return StepOutcome::Error {
                    kind: StepErrorKind::Other,
                    message: format!("template: {e}"),
                };
            }
        };
        let recognize_ms = t_rec.elapsed().as_secs_f64() * 1000.0;
        if let (Some(region), Some(confidence)) = (tick.matched_region, tick.matched_confidence) {
            self.last_match = Some((matched_task_name, confidence, region));
        }
        if let Some(cmd) = tick.command {
            let cmd = match self.jitter.as_mut() {
                Some(j) => j.jitter_command(cmd),
                None => cmd,
            };
            let device_cmd = rescale_command(cmd, self.device_width, &self.last_crop_info);
            if let Err(e) = self.execute_command(&device_cmd).await {
                warn!("pipeline execute error: {e}");
                return StepOutcome::Error {
                    kind: StepErrorKind::Execute,
                    message: format!("execute: {e}"),
                };
            }
            debug!("fired: {:?}", device_cmd);
            debug!(
                "cycle latency: capture={capture_ms:.2}ms recognize={recognize_ms:.2}ms e2e={:.2}ms (Fired)",
                t_cycle.elapsed().as_secs_f64() * 1000.0
            );
            StepOutcome::Fired {
                next_current: tick.next_current,
                fired: Some(device_cmd),
            }
        } else {
            debug!(
                "cycle latency: capture={capture_ms:.2}ms recognize={recognize_ms:.2}ms e2e={:.2}ms (NoFire)",
                t_cycle.elapsed().as_secs_f64() * 1000.0
            );
            StepOutcome::NoFire {
                next_current: tick.next_current,
            }
        }
    }

    async fn check_watch(
        &mut self,
        normalized: &DynamicImage,
        norm_w: u32,
        norm_h: u32,
    ) -> Option<StepOutcome> {
        for name in self.watch.clone() {
            let step = match anaden_vision::run_step(&self.tasks, normalized, &name) {
                Ok(Some(s)) => s,
                Ok(None) => continue,
                Err(e) => {
                    warn!("watch task '{name}' detect failed: {e}");
                    return Some(StepOutcome::Error {
                        kind: StepErrorKind::Other,
                        message: format!("watch template: {e}"),
                    });
                }
            };
            let command = crate::pipeline_runner::action_to_command(
                &step.action,
                Some(step.matched_region),
                norm_w,
                norm_h,
            );
            let fired = match command {
                Some(cmd) => {
                    let cmd = match self.jitter.as_mut() {
                        Some(j) => j.jitter_command(cmd),
                        None => cmd,
                    };
                    let device_cmd = rescale_command(cmd, self.device_width, &self.last_crop_info);
                    match self.execute_command(&device_cmd).await {
                        Ok(()) => {
                            debug!("watch fired: {name} -> {device_cmd:?}");
                            Some(device_cmd)
                        }
                        Err(e) => {
                            warn!("watch execute error ({name}): {e}");
                            return Some(StepOutcome::Error {
                                kind: StepErrorKind::Execute,
                                message: format!("execute: {e}"),
                            });
                        }
                    }
                }
                None => None,
            };
            return Some(StepOutcome::WatchFired { task: name, fired });
        }
        None
    }

    pub async fn run_once_verified(&mut self) -> StepOutcome {
        let pre_task = self.state.current().to_string();
        let fired = self.run_once().await;
        match fired {
            StepOutcome::Fired {
                next_current,
                fired: just_fired,
            } => {
                self.verify_action_effect(&pre_task, just_fired, next_current)
                    .await
            }
            other => other,
        }
    }

    async fn verify_action_effect(
        &mut self,
        task_name: &str,
        just_fired: Option<InputCommand>,
        next_current: Option<String>,
    ) -> StepOutcome {
        self.verify_stats.checks += 1;
        let screen = match self.capture.capture().await {
            Ok(img) => img,
            Err(e) => {
                warn!("verify capture error: {e}");
                return StepOutcome::Error {
                    kind: StepErrorKind::VerifyCapture,
                    message: format!("verify_capture: {e}"),
                };
            }
        };
        let (normalized, _crop_info) = self.scaler.normalize_capture(&screen);
        let still_present = match anaden_vision::run_step(&self.tasks, &normalized, task_name) {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(e) => {
                warn!("verify detect error (task '{task_name}'): {e}");
                false
            }
        };
        if still_present {
            self.verify_stats.failed += 1;
            self.save_snapshot(task_name, &normalized);
            self.state.set_current(task_name.to_string());
            StepOutcome::FiredUnverified {
                next_current,
                fired: just_fired,
            }
        } else {
            self.verify_stats.passed += 1;
            StepOutcome::Fired {
                next_current,
                fired: just_fired,
            }
        }
    }

    async fn execute_command(&self, cmd: &InputCommand) -> Result<(), DeviceError> {
        let action = match *cmd {
            InputCommand::Tap { x, y } => InputAction::tap(x, y),
            InputCommand::Swipe { from, to } => {
                InputAction::swipe(from.0, from.1, to.0, to.1, self.swipe_duration_ms)
            }
        };
        self.input.execute(&action).await
    }

    fn is_cancelled(&self) -> bool {
        match &self.cancel {
            Some(token) => token.is_cancelled(),
            None => false,
        }
    }

    async fn sleep_or_cancelled(&self, interval: Duration) {
        match &self.cancel {
            Some(token) => {
                tokio::select! {
                    _ = token.cancelled() => debug!("cancel observed during interval sleep"),
                    _ = tokio::time::sleep(interval) => {}
                }
            }
            None => {
                tokio::time::sleep(interval).await;
            }
        }
    }

    fn snapshot_dir() -> Option<PathBuf> {
        std::env::var_os("ANADEN_SNAPSHOT_DIR").map(PathBuf::from)
    }

    fn save_snapshot(&mut self, task: &str, frame: &DynamicImage) {
        let Some(dir) = Self::snapshot_dir() else {
            return;
        };
        self.snapshot_counter = self.snapshot_counter.saturating_add(1);
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = dir.join(format!(
            "{task}_{counter}_{stamp}.png",
            counter = self.snapshot_counter
        ));
        let _ = std::fs::create_dir_all(&dir);
        let file = match std::fs::File::create(&path) {
            Ok(f) => f,
            Err(e) => {
                warn!("snapshot create failed ({}): {e}", path.display());
                return;
            }
        };
        let mut buffered = std::io::BufWriter::new(file);
        match frame.write_to(&mut buffered, image::ImageFormat::Png) {
            Ok(()) => debug!("snapshot saved: {}", path.display()),
            Err(e) => warn!("snapshot write failed ({}): {e}", path.display()),
        }
    }

    fn save_diagnose_report(&mut self, task: &str, frame: &DynamicImage) {
        let Some(dir) = crate::diagnostics::diag_report_dir() else {
            return;
        };
        self.diag_counter = self.diag_counter.saturating_add(1);
        let dump_frame = std::env::var_os("ANADEN_SNAPSHOT_DIR").is_none();
        crate::diagnostics::save_diagnose_report(
            &dir,
            task,
            self.diag_counter,
            frame,
            &self.tasks,
            dump_frame,
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopStopReason {
    Stop,
    TerminalTask,
    MaxIterations,
    CaptureError,
    ExecuteError,
    GoalReached,
    GoalTimeout,
    Interrupted,
    RecoveryExhausted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TaskMatchCount {
    pub task: String,
    pub matches: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProgressReport {
    pub iterations: u64,
    pub fired_count: u64,
    pub per_task_matches: Vec<TaskMatchCount>,
    pub elapsed_ms: u64,
    pub terminal_task: Option<String>,
    pub reached_goal: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<LoopStopReason>,
    #[serde(default)]
    pub recovery_attempts: u32,
    #[serde(default)]
    pub max_consecutive_recovery: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jitter_seed: Option<u64>,
    #[serde(default)]
    pub verify: VerifyReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VerifyReport {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub checks: u64,
    #[serde(default)]
    pub passed: u64,
    #[serde(default)]
    pub failed: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct VerifyStats {
    checks: u64,
    passed: u64,
    failed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopOutcome {
    pub iterations: u64,
    pub fired_commands: Vec<InputCommand>,
    pub terminal: String,
    pub reason: LoopStopReason,
    #[serde(default)]
    pub progress_report: ProgressReport,
}

pub fn bump_task_match(current: &str, list: &mut Vec<TaskMatchCount>) {
    if let Some(entry) = list.iter_mut().find(|e| e.task == current) {
        entry.matches = entry.matches.saturating_add(1);
    } else {
        list.push(TaskMatchCount {
            task: current.to_string(),
            matches: 1,
        });
    }
}

pub fn format_progress_report(outcome: &LoopOutcome) -> String {
    let pr = &outcome.progress_report;
    let iterations = pr.iterations.max(outcome.iterations);
    let fired_count = pr.fired_count.max(outcome.fired_commands.len() as u64);
    let terminal_task: &str = pr
        .terminal_task
        .as_deref()
        .or(Some(outcome.terminal.as_str()))
        .unwrap_or("(unknown)");
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!(
        "progress: iterations: {iterations}, fired: {fired_count}, elapsed: {}ms",
        pr.elapsed_ms
    ));
    lines.push(format!("terminal: {terminal_task}"));
    if let Some(goal) = &pr.reached_goal {
        lines.push(format!("reached_goal: {goal}"));
    }
    if pr.per_task_matches.is_empty() {
        lines.push("per_task_matches: (none)".to_string());
    } else {
        let summary: Vec<String> = pr
            .per_task_matches
            .iter()
            .map(|m| format!("{}={}", m.task, m.matches))
            .collect();
        lines.push(format!("per_task_matches: {}", summary.join(", ")));
    }
    lines.join("\n")
}

#[must_use]
pub fn stop_reason_label(reason: LoopStopReason) -> &'static str {
    match reason {
        LoopStopReason::Stop => "Stop アクション到達(正常)",
        LoopStopReason::TerminalTask => "終端タスク到達(正常)",
        LoopStopReason::MaxIterations => "最大サイクル到達",
        LoopStopReason::GoalReached => "宣言的ゴール到達(正常)",
        LoopStopReason::GoalTimeout => "ゴール未到達タイムアウト(soft failure)",
        LoopStopReason::Interrupted => "Ctrl+C/SIGINT で中断",
        LoopStopReason::CaptureError => "キャプチャエラーで停止",
        LoopStopReason::ExecuteError => "発火エラーで停止",
        LoopStopReason::RecoveryExhausted => {
            "リカバリ試行上限到達で停止 (環境故障で進行不能・02-recovery-budget)"
        }
    }
}

fn goal_descriptor(goal: &anaden_core::Goal) -> String {
    stop_condition_descriptor(&goal.stop)
}

fn stop_condition_descriptor(stop: &anaden_core::StopCondition) -> String {
    match stop {
        anaden_core::StopCondition::LoopCount { target } => {
            format!("loop_count={target}")
        }
        anaden_core::StopCondition::TemplateMatch { task, confidence } => {
            format!("template_match conf>={confidence} (task={task})")
        }
        anaden_core::StopCondition::Timeout { secs } => format!("timeout={secs}"),
        anaden_core::StopCondition::All { conditions } => {
            let inner = conditions
                .iter()
                .map(stop_condition_descriptor)
                .collect::<Vec<_>>()
                .join(", ");
            format!("all[{}]({inner})", conditions.len())
        }
        anaden_core::StopCondition::Any { conditions } => {
            let inner = conditions
                .iter()
                .map(stop_condition_descriptor)
                .collect::<Vec<_>>()
                .join(", ");
            format!("any[{}]({inner})", conditions.len())
        }
    }
}

impl<C: Capture, I: Input> PipelineDriver<C, I> {
    pub async fn run_loop(&mut self, interval: Duration, max_iterations: u64) -> LoopOutcome {
        self.run_loop_with_recovery(interval, max_iterations, 0, None)
            .await
    }

    async fn run_recovery_step(
        &mut self,
        cause: &str,
        guard: &mut RecoveryGuard,
        recover: &mut Option<RecoveryHook>,
    ) -> RecoveryStep {
        let Some(attempt) = guard.consume_attempt() else {
            let t = guard.telemetry();
            warn!(
                "{cause}; recovery budget exhausted (attempts={}, max consecutive={}) \
                 -> stopping with RecoveryExhausted",
                t.recovery_attempts, t.max_consecutive_recovery
            );
            return RecoveryStep::Exhausted;
        };
        let streak = guard.streak();
        info!("{cause}; invoking recovery hook (attempt {attempt}, streak {streak})");
        let Some(hook) = recover.as_mut() else {
            return RecoveryStep::Exhausted;
        };
        match hook(RecoveryContext { attempt, streak }).await {
            Ok(()) => {
                let recovered_at = self.recovery_clock.elapsed();
                info!(
                    "recovery hook succeeded; resetting streak + boot grace {:?} (Issue #210)",
                    self.recovery_grace
                );
                guard.recovered(recovered_at);
                RecoveryStep::Recovered
            }
            Err(e) => RecoveryStep::HookFailed(e),
        }
    }

    async fn run_recovery_or_stop(
        &mut self,
        cause: &str,
        state: &mut LoopState,
        recover: &mut Option<RecoveryHook>,
    ) -> Option<LoopOutcome> {
        match self
            .run_recovery_step(cause, &mut state.guard, recover)
            .await
        {
            RecoveryStep::Recovered => None,
            RecoveryStep::Exhausted => Some(self.build_outcome(
                state.iterations - 1,
                std::mem::take(&mut state.fired),
                LoopStopReason::RecoveryExhausted,
                "recovery_exhausted",
                state.ctx(),
            )),
            RecoveryStep::HookFailed(e) => {
                warn!("recovery hook failed: {e}");
                Some(self.build_outcome(
                    state.iterations - 1,
                    std::mem::take(&mut state.fired),
                    LoopStopReason::ExecuteError,
                    "recovery_failed",
                    state.ctx(),
                ))
            }
        }
    }

    fn eval_then_terminal(
        &mut self,
        state: &mut LoopState,
        goal_mode: &mut GoalMode<'_>,
        match_info: Option<(String, f32, ScreenRegion)>,
        next_current: &Option<String>,
        arm: MatchArm,
    ) -> StepFlow {
        if let Some(stop) = self.eval_goal_stop(goal_mode, match_info, state.goal_eval_inputs()) {
            return StepFlow::stop(stop);
        }
        let Some(_) = next_current else {
            let (reason, text) = match arm {
                MatchArm::Fired => (LoopStopReason::TerminalTask, self.current().to_string()),
                MatchArm::NoFire => (LoopStopReason::Stop, "stop".to_string()),
            };
            return StepFlow::stop(self.build_outcome(
                state.iterations,
                std::mem::take(&mut state.fired),
                reason,
                &text,
                state.ctx(),
            ));
        };
        StepFlow::Continue
    }

    fn step_matched(
        &mut self,
        state: &mut LoopState,
        goal_mode: &mut GoalMode<'_>,
        input: MatchStepInput<'_>,
        arm: MatchArm,
    ) -> StepFlow {
        state.record_match_tick(input.current_before);
        if let Some(c) = input.just_fired {
            state.fired.push(c);
        }
        if matches!(arm, MatchArm::Fired)
            && let Some(name) = &input.next_current
        {
            debug!("fired, advancing to {}", name);
        }
        let flow =
            self.eval_then_terminal(state, goal_mode, input.match_info, &input.next_current, arm);
        if matches!(flow, StepFlow::Continue)
            && matches!(arm, MatchArm::NoFire)
            && let Some(name) = &input.next_current
        {
            debug!("no-fire, transitioning to {}", name);
        }
        flow
    }

    async fn step_no_match(
        &mut self,
        state: &mut LoopState,
        goal_mode: &mut GoalMode<'_>,
        recover: &mut Option<RecoveryHook>,
        miss_cause: &'static str,
        match_info: Option<(String, f32, ScreenRegion)>,
    ) -> StepFlow {
        let now = self.recovery_clock.elapsed();
        let due = state.recovery_enabled && state.guard.record_miss(now);
        if let Some(stop) = self.eval_goal_stop(goal_mode, match_info, state.goal_eval_inputs()) {
            return StepFlow::stop(stop);
        }
        if due && let Some(stop) = self.run_recovery_or_stop(miss_cause, state, recover).await {
            return StepFlow::stop(stop);
        }
        StepFlow::Continue
    }

    async fn step_fired_unverified(
        &mut self,
        state: &mut LoopState,
        goal_mode: &mut GoalMode<'_>,
        recover: &mut Option<RecoveryHook>,
        current_before: &str,
        just_fired: Option<InputCommand>,
        match_info: Option<(String, f32, ScreenRegion)>,
    ) -> StepFlow {
        bump_task_match(current_before, &mut state.per_task_matches);
        if let Some(c) = just_fired {
            state.fired.push(c);
        }
        self.step_no_match(
            state,
            goal_mode,
            recover,
            "FiredUnverified streak >= threshold",
            match_info,
        )
        .await
    }

    async fn step_error(
        &mut self,
        state: &mut LoopState,
        recover: &mut Option<RecoveryHook>,
        kind: StepErrorKind,
        message: String,
    ) -> StepFlow {
        if !is_recoverable_io_error(kind) {
            return StepFlow::stop(self.build_outcome(
                state.iterations - 1,
                std::mem::take(&mut state.fired),
                LoopStopReason::ExecuteError,
                "io_error",
                state.ctx(),
            ));
        }
        let now = self.recovery_clock.elapsed();
        if state.guard.in_grace(now) {
            warn!("pipeline capture error during boot grace (game restarting): {message}");
            StepFlow::Continue
        } else if state.recovery_enabled {
            info!("capture error outside grace (game presumed dead): {message}");
            match self
                .run_recovery_or_stop("capture error outside grace", state, recover)
                .await
            {
                Some(stop) => StepFlow::stop(stop),
                None => StepFlow::Continue,
            }
        } else {
            StepFlow::stop(self.build_outcome(
                state.iterations - 1,
                std::mem::take(&mut state.fired),
                LoopStopReason::CaptureError,
                "io_error",
                state.ctx(),
            ))
        }
    }

    fn step_watch_fired(
        &mut self,
        state: &mut LoopState,
        task: &str,
        fired: Option<InputCommand>,
    ) -> StepFlow {
        state.record_match_tick(task);
        if let Some(cmd) = fired {
            state.fired.push(cmd);
        }
        StepFlow::Continue
    }

    pub async fn run_loop_with_recovery(
        &mut self,
        interval: Duration,
        max_iterations: u64,
        recover_nomatch_threshold: u32,
        mut recover: Option<RecoveryHook>,
    ) -> LoopOutcome {
        self.run_loop_core(
            interval,
            max_iterations,
            recover_nomatch_threshold,
            &mut recover,
            &mut GoalMode::Inactive,
        )
        .await
    }

    async fn run_loop_core(
        &mut self,
        interval: Duration,
        max_iterations: u64,
        recover_nomatch_threshold: u32,
        recover: &mut Option<RecoveryHook>,
        goal_mode: &mut GoalMode<'_>,
    ) -> LoopOutcome {
        let mut state = LoopState::new(
            recover_nomatch_threshold,
            self.recovery_grace,
            self.max_recovery_attempts,
            recover.is_some(),
        );
        self.verify_stats = VerifyStats::default();
        loop {
            if self.is_cancelled() {
                return self.build_outcome(
                    state.iterations,
                    std::mem::take(&mut state.fired),
                    LoopStopReason::Interrupted,
                    "interrupted",
                    state.ctx(),
                );
            }
            state.iterations += 1;
            if state.iterations > max_iterations {
                return self.build_outcome(
                    state.iterations - 1,
                    std::mem::take(&mut state.fired),
                    LoopStopReason::MaxIterations,
                    "max_iterations",
                    state.ctx(),
                );
            }
            let current_before = self.current().to_string();
            let step = if self.verify_after_fire {
                self.run_once_verified().await
            } else {
                self.run_once().await
            };
            let match_info = match (&*goal_mode, &step) {
                (GoalMode::Active(_), StepOutcome::Fired { .. } | StepOutcome::NoFire { .. }) => {
                    self.last_match.take()
                }
                _ => None,
            };
            let flow = match step {
                StepOutcome::Fired {
                    next_current,
                    fired: just_fired,
                } => self.step_matched(
                    &mut state,
                    goal_mode,
                    MatchStepInput {
                        current_before: &current_before,
                        next_current,
                        just_fired,
                        match_info,
                    },
                    MatchArm::Fired,
                ),
                StepOutcome::NoFire { next_current } => self.step_matched(
                    &mut state,
                    goal_mode,
                    MatchStepInput {
                        current_before: &current_before,
                        next_current,
                        just_fired: None,
                        match_info,
                    },
                    MatchArm::NoFire,
                ),
                StepOutcome::NoMatch => {
                    self.step_no_match(
                        &mut state,
                        goal_mode,
                        recover,
                        "NoMatch streak >= threshold",
                        match_info,
                    )
                    .await
                }
                StepOutcome::FiredUnverified {
                    next_current: _,
                    fired: just_fired,
                } => {
                    self.step_fired_unverified(
                        &mut state,
                        goal_mode,
                        recover,
                        &current_before,
                        just_fired,
                        match_info,
                    )
                    .await
                }
                StepOutcome::Error { kind, message } => {
                    self.step_error(&mut state, recover, kind, message).await
                }
                StepOutcome::WatchFired { task, fired } => {
                    self.step_watch_fired(&mut state, &task, fired)
                }
            };
            match flow {
                StepFlow::Continue => {}
                StepFlow::Stop(outcome) => return *outcome,
            }
            let wait = match self.jitter.as_mut() {
                Some(j) => j.jitter_interval(interval),
                None => interval,
            };
            self.sleep_or_cancelled(wait).await;
        }
    }

    pub async fn run_loop_with_goal(
        &mut self,
        interval: Duration,
        max_iterations: u64,
        recover_nomatch_threshold: u32,
        mut recover: Option<RecoveryHook>,
        goal: Option<&anaden_core::Goal>,
        mut clock: impl GoalClock,
    ) -> LoopOutcome {
        let Some(goal_ref) = goal else {
            return self
                .run_loop_with_recovery(
                    interval,
                    max_iterations,
                    recover_nomatch_threshold,
                    recover,
                )
                .await;
        };
        let mut goal_mode = GoalMode::Active(GoalLoopCtx {
            goal: goal_ref,
            clock: &mut clock,
            goal_ctx: anaden_core::GoalStatusContext::new(),
        });
        self.run_loop_core(
            interval,
            max_iterations,
            recover_nomatch_threshold,
            &mut recover,
            &mut goal_mode,
        )
        .await
    }

    fn evaluate_goal_reason(
        &self,
        goal: &anaden_core::Goal,
        ctx: &anaden_core::GoalStatusContext,
        elapsed_secs: u64,
    ) -> Option<(LoopStopReason, &'static str)> {
        match anaden_core::evaluate(goal, ctx, elapsed_secs) {
            anaden_core::GoalStatus::NotYet => None,
            anaden_core::GoalStatus::Reached(_) => {
                Some((LoopStopReason::GoalReached, "goal_reached"))
            }
            anaden_core::GoalStatus::Failed(_) => {
                Some((LoopStopReason::GoalTimeout, "goal_timeout"))
            }
        }
    }

    fn eval_goal_stop(
        &self,
        mode: &mut GoalMode<'_>,
        match_info: Option<(String, f32, ScreenRegion)>,
        inputs: GoalEvalInputs<'_>,
    ) -> Option<LoopOutcome> {
        let GoalMode::Active(ctx) = mode else {
            return None;
        };
        ctx.goal_ctx.tick(match_info);
        let elapsed = ctx.clock.elapsed_secs();
        self.evaluate_goal(ctx.goal, &ctx.goal_ctx, elapsed, inputs)
    }

    fn evaluate_goal(
        &self,
        goal: &anaden_core::Goal,
        ctx: &anaden_core::GoalStatusContext,
        elapsed_secs: u64,
        inputs: GoalEvalInputs,
    ) -> Option<LoopOutcome> {
        let (reason, terminal) = self.evaluate_goal_reason(goal, ctx, elapsed_secs)?;
        let mut outcome = self.build_outcome(
            inputs.iterations,
            inputs.fired_commands.to_vec(),
            reason,
            terminal,
            OutcomeCtx {
                per_task_matches: inputs.per_task_matches.to_vec(),
                started: inputs.started,
                recovery: inputs.recovery,
            },
        );
        outcome.progress_report.reached_goal = Some(goal_descriptor(goal));
        Some(outcome)
    }

    fn build_outcome(
        &self,
        iterations: u64,
        fired_commands: Vec<InputCommand>,
        reason: LoopStopReason,
        terminal: &str,
        ctx: OutcomeCtx,
    ) -> LoopOutcome {
        let terminal_task = if terminal.is_empty() {
            None
        } else {
            Some(terminal.to_string())
        };
        let progress_report = ProgressReport {
            iterations,
            fired_count: fired_commands.len() as u64,
            per_task_matches: ctx.per_task_matches,
            elapsed_ms: ctx.started.elapsed().as_millis() as u64,
            terminal_task: terminal_task.clone(),
            reached_goal: None,
            stop_reason: Some(reason.clone()),
            recovery_attempts: ctx.recovery.recovery_attempts,
            max_consecutive_recovery: ctx.recovery.max_consecutive_recovery,
            jitter_seed: self.jitter.as_ref().map(Jitter::seed),
            verify: VerifyReport {
                enabled: self.verify_after_fire,
                checks: self.verify_stats.checks,
                passed: self.verify_stats.passed,
                failed: self.verify_stats.failed,
            },
        };
        LoopOutcome {
            iterations,
            fired_commands,
            terminal: terminal.to_string(),
            reason,
            progress_report,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use anaden_core::ScreenRegion;
    use anaden_vision::Action;
    use image::{DynamicImage, GrayImage, Luma};
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    // ---- pipeline_runner.rs のテストヘルパ相当（複製） ----

    fn luma_dyn(img: GrayImage) -> DynamicImage {
        DynamicImage::ImageLuma8(img)
    }

    fn write_template_persisted(needle: &GrayImage) -> PathBuf {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = tmp.path().join("needle.png");
        needle.save(&p).expect("save png");
        let _persisted = tmp.keep();
        p
    }

    // ---- 新スケールモデル対応の matched-frame ヘルパ ----
    //
    // run_once は capture → crop_to_content → normalize(1258→1280x720) → detect を経る。
    // detect は needle を正規化後寸法(1280x720)へ動的スケールする。旧 embed() 方式（needle を
    // screenshot に直接埋め込み）は、normalize 拡大と detect needle スケールが異なる resize 経路
    // となりズレて NoMatch になる。よって needle と screenshot を **同一の raw-1258x708 特徴画像
    // から生成** し、両者が run_once 通過後に一致するようにする:
    //   - raw_feature(): 1258x708 の広域 gradient 特徴画像
    //   - needle = raw_feature の NEEDLE_ROI(12x12) を crop（=raw-1258 空間のテンプレ）
    //   - screenshot = raw_feature 全体
    //   - TaskDef.roi = MATCH_ROI（needle を含む小窓）→ detect が crop して走査を局所化。
    //
    // 局所化の理由: run_loop 系テストは max_iterations まで複数サイクル回る。1258x708 の全面
    // ccoeff 走査（O(N·M)）は1サイクルでも重く、複数サイクルで時間・メモリ爆発する。MATCH_ROI で
    // needle 周辺の小窓のみ走査し、run_loop 制御ロジックの検証を現実的時間に保つ。
    //
    // 整合性: detect は TaskDef.roi を roi_to_normalized(roi, norm_w=1280, norm_h=720) でスケール
    // して crop する。needle も同比(1258→1280)でスケール。screenshot の needle 領域も normalize
    // (1258→1280) で拡大。三者とも「raw ROI を 1258→1280 比で Triangle 拡大」なので一致する。

    /// テンプレ needle の raw-1258 空間 ROI（左上原点・12x12）。
    const NEEDLE_ROI: [u32; 4] = [150, 75, 12, 12];
    /// マッチ走査を局所化する TaskDef.roi（needle を含む小窓）。
    const MATCH_ROI: [u32; 4] = [140, 65, 48, 36];

    /// raw-1258x708 特徴画像（周期200の広域 gradient）。needle と screenshot の共通原料。
    fn raw_feature() -> GrayImage {
        let mut img = GrayImage::new(FULL_W, FULL_H);
        for y in 0..FULL_H {
            for x in 0..FULL_W {
                let v = (((x * 2) + y) % 200) as u8;
                img.put_pixel(x, y, Luma([v]));
            }
        }
        img
    }

    /// raw_feature の NEEDLE_ROI 領域を crop してテンプレ PNG として保存し、絶対パスを返す。
    fn crop_needle_template() -> PathBuf {
        let raw = raw_feature();
        let [nx, ny, nw, nh] = NEEDLE_ROI;
        let mut needle = GrayImage::new(nw, nh);
        for dy in 0..nh {
            for dx in 0..nw {
                needle.put_pixel(dx, dy, *raw.get_pixel(nx + dx, ny + dy));
            }
        }
        write_template_persisted(&needle)
    }

    /// matched-frame 用の (screenshot, needle_template_path) を生成。
    /// screenshot は raw-1258x708 特徴画像全体、needle は NEEDLE_ROI 領域の crop。
    fn matched_frame() -> (DynamicImage, PathBuf) {
        let raw = raw_feature();
        let tpl = crop_needle_template();
        (luma_dyn(raw), tpl)
    }

    // テスト画面寸法。PC版実測 raw-1258x708 空間（=PC_CLIENT_*_MEASURED）。
    // raw_feature をこの寸法で作ることで、normalize(1258→1280x720) と detect の needle/ROI
    // スケール(1258→1280) が同比となり、needle-screenshot が一致してマッチする。
    const FULL_W: u32 = 1258;
    const FULL_H: u32 = 708;

    fn click_rect_task(name: &str, action: Action, next: Option<Vec<&str>>) -> TaskDef {
        TaskDef {
            name: name.into(),
            state: name.into(),
            algorithm: anaden_vision::Algorithm::Ccoeff,
            template: crop_needle_template(),
            roi: Some(MATCH_ROI),
            threshold: 0.9,
            base: None,
            action: Some(action),
            next: next.map(|v| v.into_iter().map(String::from).collect()),
        }
    }

    // ---- (1) rescale 純関数（crop info ベース） ----
    //
    // rescale_command は normalize後1280空間 → 実機生画像（黒帯込み）への逆写像。
    // CropInfo でコンテンツ領域（黒帯除去後描画領域）の元画像空間における位置・寸法を受け取り:
    //   x_real = x_1280 * crop_w / 1280 + offset_x
    //   y_real = y_1280 * crop_h / 720  + offset_y
    // 黒帯なし（offset=0, size=device）のとき従来の ScreenScaler::from_base と同等。

    #[test]
    fn rescale_tap_no_bars_equivalent_to_from_base_2400() {
        // 黒帯なし: device_width=2400, content=2400x1080(フル)。CropInfo::full(2400,1080)。
        // scale_factor = 2400/1280 = 1.875。640*1.875=1200, 360*1.875=675。
        let info = CropInfo::full(2400, 1080);
        let out = rescale_command(InputCommand::Tap { x: 640, y: 360 }, 2400, &info);
        assert_eq!(out, InputCommand::Tap { x: 1200, y: 675 });
    }

    #[test]
    fn rescale_swipe_no_bars_equivalent_to_from_base_2400() {
        let info = CropInfo::full(2400, 1080);
        let cmd = InputCommand::Swipe {
            from: (640, 360),
            to: (0, 0),
        };
        let out = rescale_command(cmd, 2400, &info);
        assert_eq!(
            out,
            InputCommand::Swipe {
                from: (1200, 675),
                to: (0, 0),
            }
        );
    }

    #[test]
    fn rescale_identity_when_device_width_equals_base_no_bars() {
        // device_width=1280, content=1280x720(フル)。scale=1.0 → 恒等。
        let info = CropInfo::full(1280, 720);
        let out = rescale_command(InputCommand::Tap { x: 640, y: 360 }, 1280, &info);
        assert_eq!(out, InputCommand::Tap { x: 640, y: 360 });
    }

    #[test]
    fn rescale_downscale_small_device_width_no_bars() {
        // device_width=640, content=640x360(フル)。scale=0.5。640*0.5=320, 360*0.5=180。
        let info = CropInfo::full(640, 360);
        let out = rescale_command(InputCommand::Tap { x: 640, y: 360 }, 640, &info);
        assert_eq!(out, InputCommand::Tap { x: 320, y: 180 });
    }

    #[test]
    fn rescale_tap_with_letterbox_offset_shifts_coordinates() {
        // 黒帯入り（M1 検証）: 実機生画像 1918x1048, コンテンツ領域は offset=(120,80) size=(1678,888)。
        // normalize は幅基準（アスペクト比保存）なので、コンテンツ内では両軸とも crop_w/1280 比。
        // s = 1678/1280 = 1.3109375。
        // 1280空間 (640,360) → (640*s, 360*s) = (839.0, 471.94→472) → オフセット加算 (959, 552)。
        // 従来の from_base(1918) なら (640*1918/1280, 360*1918/1280)=(959,539) となり
        // Y がズレていた（黒帯を無視して全幅で均一スケールするため）。新実装で Y がコンテンツ
        // 領域基準（オフセット反映）で正しく 552 になることを検証。
        let info = CropInfo {
            offset_x: 120,
            offset_y: 80,
            width: 1678,
            height: 888,
        };
        let out = rescale_command(InputCommand::Tap { x: 640, y: 360 }, 1918, &info);
        assert_eq!(out, InputCommand::Tap { x: 959, y: 552 });
    }

    #[test]
    fn rescale_swipe_with_letterbox_offset() {
        let info = CropInfo {
            offset_x: 120,
            offset_y: 80,
            width: 1678,
            height: 888,
        };
        // from=(640,360)→(959,552), to=(0,0)→(120,80)（コンテンツ左上=オフセットそのもの）。
        let cmd = InputCommand::Swipe {
            from: (640, 360),
            to: (0, 0),
        };
        let out = rescale_command(cmd, 1918, &info);
        assert_eq!(
            out,
            InputCommand::Swipe {
                from: (959, 552),
                to: (120, 80),
            }
        );
    }

    // ---- (2) fake Capture/Input ----

    struct FakeCapture {
        frames: Arc<Mutex<VecDeque<DynamicImage>>>,
        fail: bool,
    }

    #[async_trait]
    impl Capture for FakeCapture {
        async fn capture(&self) -> Result<DynamicImage, DeviceError> {
            if self.fail {
                return Err(DeviceError::CommandFailed {
                    message: "fake capture failure".into(),
                });
            }
            self.frames
                .lock()
                .expect("frames lock")
                .pop_front()
                .ok_or_else(|| DeviceError::CommandFailed {
                    message: "no more frames".into(),
                })
        }
    }

    /// 発火したアクションを [`InputCommand`] へ戻して記録する fake input。
    struct FakeInput {
        fired: Arc<Mutex<Vec<InputCommand>>>,
        fail: bool,
    }

    #[async_trait]
    impl Input for FakeInput {
        async fn execute(&self, action: &InputAction) -> Result<(), DeviceError> {
            if self.fail {
                return Err(DeviceError::CommandFailed {
                    message: "fake execute failure".into(),
                });
            }
            let cmd = match action {
                InputAction::Tap(p) => InputCommand::Tap { x: p.x, y: p.y },
                InputAction::Swipe {
                    from,
                    to,
                    duration_ms: _,
                } => InputCommand::Swipe {
                    from: (from.x, from.y),
                    to: (to.x, to.y),
                },
                other => panic!("unexpected action in fake: {:?}", other),
            };
            self.fired.lock().expect("fired lock").push(cmd);
            Ok(())
        }
    }

    fn frames_of(images: Vec<DynamicImage>) -> Arc<Mutex<VecDeque<DynamicImage>>> {
        Arc::new(Mutex::new(images.into_iter().collect()))
    }

    fn new_fired() -> Arc<Mutex<Vec<InputCommand>>> {
        Arc::new(Mutex::new(Vec::new()))
    }

    // ---- (3) run_once 検証 ----

    #[tokio::test]
    async fn run_once_fires_rescaled_tap_on_click_rect() {
        // ClickRect roi [520,320,240,80] は raw-1258空間。matched_frame() は raw-1258x708（黒帯なし）。
        // 新スケールモデルでの発火座標導出:
        //  1. roi を screenshot と同じ空間へ: roi_to_normalized([520,320,240,80],1280,720)
        //     = [529,325,244,81]。中心 (651,365)。
        //  2. rescale(crop_info=full(1258,708), device_width=2400): s=1258/1280=0.9828。
        //     (651*0.9828, 365*0.9828) = (640, 359)。
        // テストフレーム(raw-1258) と device_width=2400 は不整合だが、CropInfo が full(1258,708)
        // （フレーム実寸法）なので結果は 1258 周辺になる。raw-1258 実値での厳密検証は
        // click_rect_fires_at_expected_device_coords_on_raw_1258_roi テスト（C2）で行う。
        let (screen, _tpl) = matched_frame();
        let frames = frames_of(vec![screen]);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400, // device_width
            300,
        );

        let out = driver.run_once().await;
        match out {
            StepOutcome::Fired {
                next_current,
                fired: just_fired,
            } => {
                assert_eq!(next_current.as_deref(), Some("LoadGame"));
                // 新スケールモデル: raw-1258 roi → 1280空間 → rescale(crop_info=full(1258,708))。
                assert_eq!(just_fired, Some(InputCommand::Tap { x: 640, y: 359 }));
            }
            other => panic!("expected Fired, got {other:?}"),
        }
        // fake input 側にも同じ座標が記録されている
        assert_eq!(
            fired.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: 640, y: 359 }]
        );
        assert_eq!(driver.current(), "LoadGame");
    }

    /// F34 (05-template-cache): テンプレート読込エラー (欠落) は NoMatch に潰されず
    /// `StepOutcome::Error { kind: Other, message: "template: ..." }` として区別可能である
    /// こと。従来は run_step が None へ潰すため NoMatch (リトライ候補) と区別不能
    /// (fail-invisible) で、テンプレ欠落が永久 NoMatch ループとしてしか観測できなかった。
    #[tokio::test]
    async fn run_once_missing_template_returns_template_error_not_nomatch() {
        let (screen, _tpl) = matched_frame();
        let frames = frames_of(vec![screen]);
        let fired = new_fired();

        let task = TaskDef {
            template: PathBuf::from("/nonexistent/does_not_exist_template.png"),
            ..click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )
        };

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400, // device_width
            300,
        );

        let out = driver.run_once().await;
        match out {
            StepOutcome::Error { kind, message } => {
                assert_eq!(
                    kind,
                    StepErrorKind::Other,
                    "template load error must classify as Other (non-recoverable)"
                );
                assert!(
                    message.starts_with("template:"),
                    "message must use the 'template: ...' convention, got {message:?}"
                );
            }
            other => {
                panic!("expected Error, got {other:?} (NoMatch would be the old squashed behavior)")
            }
        }
        assert!(
            fired.lock().expect("fired lock").is_empty(),
            "no input must fire on template error"
        );
    }

    #[tokio::test]
    async fn run_once_stop_yields_nofire_none() {
        let (screen, _tpl) = matched_frame();
        let frames = frames_of(vec![screen]);
        let fired = new_fired();

        let task = click_rect_task("Title", Action::Stop, Some(vec!["Ignored"]));

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        );

        let out = driver.run_once().await;
        assert_eq!(out, StepOutcome::NoFire { next_current: None });
        assert!(
            fired.lock().expect("fired lock").is_empty(),
            "Stop must not fire any command"
        );
        assert_eq!(driver.current(), "Title");
    }

    #[tokio::test]
    async fn run_once_no_match_keeps_current() {
        // needle 無し画像 → tick None → NoMatch。current 不変。
        let screen = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let frames = frames_of(vec![screen]);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        );

        let out = driver.run_once().await;
        assert_eq!(out, StepOutcome::NoMatch);
        assert!(fired.lock().expect("fired lock").is_empty());
        assert_eq!(driver.current(), "Title");
    }

    #[tokio::test]
    async fn run_once_capture_error_returns_error() {
        let frames = frames_of(vec![]);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: true,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![],
            2400,
            300,
        );

        match driver.run_once().await {
            StepOutcome::Error { kind, message } => {
                assert_eq!(kind, StepErrorKind::Capture);
                assert!(message.starts_with("capture"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
        assert!(fired.lock().expect("fired lock").is_empty());
    }

    // ---- (3b) Issue #212: 本番前処理 (normalize_capture) の engine 経路 pin ----
    //
    // run_once は ScreenScaler::normalize_capture (黒帯クロップ [キャンバス アスペクト
    // 保護付き] → 1280 正規化) を使う。anaden-tool run-pipeline も同一関数を呼ぶため
    // 両経路の前処理は常に一致する (Issue #212 受け入れ基準 1)。ここでは実 asset
    // (実キャプチャ fixture + 実 login pipeline TOML) で engine 経路の end-to-end を
    // pin する。

    /// anaden-vision の tests/fixtures へのパス (crates/anaden-engine から見て)。
    fn vision_fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("anaden-vision")
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    /// 実 login pipeline (templates/pipelines/login) から LoginTapTitlePc を読む。
    fn real_login_tap_title_task() -> TaskDef {
        let login_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("templates")
            .join("pipelines")
            .join("login");
        let tasks = anaden_vision::load_pipeline(&login_dir)
            .unwrap_or_else(|e| panic!("load login pipeline {}: {e}", login_dir.display()));
        tasks
            .into_iter()
            .find(|t| t.name == "LoginTapTitlePc")
            .expect("LoginTapTitlePc must exist")
    }

    /// 健全な実タイトルキャプチャ (#208 fixture・左 8px 黒帯) が engine live 経路
    /// (capture → normalize_capture → roi 付き tick → rescale) で発火することを保証。
    ///
    /// 期待値の導出 (Issue #212 実測・決定論的):
    /// - crop (10,0) 1942x1098 (16:9 比 +0.52% → アスペクト保護は受理) → 1280x724。
    /// - ClickRect roi [900,466,30,30] (raw-1258) → roi_to_normalized(.,1280,724)
    ///   = [916,477,31,31] → 中心 (931,492)。
    /// - rescale (crop 1942 幅・offset_x=10): x = 931*1942/1280 + 10 = 1423,
    ///   y = 492*1942/1280 + 0 = 746 (両軸幅比・round)。
    #[tokio::test]
    async fn run_once_fires_on_real_title_fixture_through_guarded_preprocessing() {
        let fixture = vision_fixture("title_live_1952x1098.png");
        let screen =
            image::open(&fixture).unwrap_or_else(|e| panic!("open {}: {e}", fixture.display()));
        let frames = frames_of(vec![screen]);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("LoginTapTitlePc"),
            vec![real_login_tap_title_task()],
            1952, // device_width = 生キャプチャ幅
            300,
        );

        let out = driver.run_once().await;
        match out {
            StepOutcome::Fired {
                next_current,
                fired: just_fired,
            } => {
                assert_eq!(next_current.as_deref(), Some("LoginWaitFieldPc"));
                // 実機生座標へ rescale 済みの Tap (導出は上記コメント)。
                assert_eq!(just_fired, Some(InputCommand::Tap { x: 1423, y: 746 }));
            }
            other => panic!("expected Fired on real title fixture, got {other:?}"),
        }
        assert_eq!(driver.last_crop_info.offset_x, 10);
        assert_eq!(
            (driver.last_crop_info.width, driver.last_crop_info.height),
            (1942, 1098),
            "engine must consume the guarded crop (accepted: within 16:9 tolerance)"
        );
        assert_eq!(
            fired.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: 1423, y: 746 }]
        );
    }

    /// 過渡ブートフレーム (Issue #212 fixture・キャンバス下半分が未描画純黒) では
    /// アスペクト保護がクロップを棄却し、座標系 (全面 = キャンバス) が保たれること。
    /// 旧実装なら last_crop_info が 1952x577 (幾何破壊) になり、この pin が RED になる。
    #[tokio::test]
    async fn run_once_on_degenerate_frame_keeps_canvas_geometry() {
        let fixture = vision_fixture("title_boot_degenerate_1952x1098.png");
        let screen =
            image::open(&fixture).unwrap_or_else(|e| panic!("open {}: {e}", fixture.display()));
        let frames = frames_of(vec![screen]);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("LoginTapTitlePc"),
            vec![real_login_tap_title_task()],
            1952,
            300,
        );

        let out = driver.run_once().await;
        assert_eq!(
            out,
            StepOutcome::NoMatch,
            "needle 不在の過渡フレームは正しく NoMatch"
        );
        assert_eq!(
            driver.last_crop_info,
            CropInfo::full(1952, 1098),
            "aspect guard must reject the in-canvas-black crop: engine keeps the full canvas \
             geometry (旧実装は 1952x577 に崩れ roi の Y マッピングが破壊された)"
        );
        assert!(fired.lock().expect("fired lock").is_empty());
    }

    // ---- (4) run_loop 検証 ----

    fn needle_screen(seed: u32) -> DynamicImage {
        // matched-frame（raw-1258 特徴画像）。seed は歴史的互換用で未使用（以前も未使用）。
        let _ = seed;
        let (screen, _tpl) = matched_frame();
        screen
    }

    #[tokio::test]
    async fn run_loop_reaches_terminal_task() {
        // Title(ClickRect) → LoadGame(ClickRect) → Terminal(ClickRect, next=None)
        // 3 サイクル全てマッチ発火し、最後に next_current=None で TerminalTask 停止。
        let frames = frames_of(vec![needle_screen(0), needle_screen(1), needle_screen(2)]);
        let fired = new_fired();

        // next は next[0] のみ使われる。current 遷移を模擬するため 3 タスク定義。
        let tasks = vec![
            click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            ),
            click_rect_task(
                "LoadGame",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["Terminal"]),
            ),
            click_rect_task(
                "Terminal",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                None, // 終端
            ),
        ];

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            tasks,
            2400,
            300,
        );

        let outcome = driver.run_loop(Duration::ZERO, 10).await;
        assert_eq!(outcome.reason, LoopStopReason::TerminalTask);
        assert_eq!(outcome.fired_commands.len(), 3);
        // 全発火座標は新スケールモデルで rescale 済み (640,359)
        for c in &outcome.fired_commands {
            assert_eq!(*c, InputCommand::Tap { x: 640, y: 359 });
        }
        assert_eq!(driver.current(), "Terminal");
    }

    #[tokio::test]
    async fn run_loop_stop_action() {
        // Title(Stop) → NoFire(None) → Stop 停止。
        let frames = frames_of(vec![needle_screen(0)]);
        let fired = new_fired();

        let tasks = vec![click_rect_task(
            "Title",
            Action::Stop,
            Some(vec!["Ignored"]),
        )];

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            tasks,
            2400,
            300,
        );

        let outcome = driver.run_loop(Duration::ZERO, 10).await;
        assert_eq!(outcome.reason, LoopStopReason::Stop);
        assert!(outcome.fired_commands.is_empty());
    }

    #[tokio::test]
    async fn run_loop_no_match_hits_max_iterations() {
        // 全フレーム NoMatch(needle 無) → max_iterations 到達。
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        // frames が枯渇すると capture エラーになるため、十分な枚数を用意。
        let many = (0..20).map(|_| blank.clone()).collect();
        let frames = frames_of(many);
        let fired = new_fired();

        let tasks = vec![click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        )];

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            tasks,
            2400,
            300,
        );

        let outcome = driver.run_loop(Duration::ZERO, 5).await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert_eq!(outcome.iterations, 5);
        assert!(outcome.fired_commands.is_empty());
        assert_eq!(driver.current(), "Title");
    }

    #[tokio::test]
    async fn run_loop_capture_error_stops_immediately() {
        let frames = frames_of(vec![]);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: true,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![],
            2400,
            300,
        );

        let outcome = driver.run_loop(Duration::ZERO, 10).await;
        assert_eq!(outcome.reason, LoopStopReason::CaptureError);
        assert!(outcome.fired_commands.is_empty());
    }

    // ---- (5) run_loop_with_recovery 検証 ----

    #[tokio::test]
    async fn recovery_hook_fires_after_nomatch_threshold() {
        // 全フレーム NoMatch(blank)。threshold=3 → 3 回目で hook 呼び出し成功 → streak リセット。
        // その後 NoMatch 再蓄積するが max_iters=10 で MaxIterations 停止。
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let many = (0..30).map(|_| blank.clone()).collect();
        let frames = frames_of(many);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )],
            2400,
            300,
        );

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let hook: RecoveryHook = Box::new(move |_streak| {
            let c = calls_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 10, 3, Some(hook))
            .await;
        // threshold=3, max_iters=10 → NoMatch が 3,6,9 回目で hook 計 3 回起動。
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert!(outcome.fired_commands.is_empty());
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "recovery hook must fire at least once"
        );
    }

    #[tokio::test]
    async fn recovery_disabled_when_threshold_zero() {
        // threshold=0 → hook 呼び出されず。
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let many = (0..20).map(|_| blank.clone()).collect();
        let frames = frames_of(many);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )],
            2400,
            300,
        );

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let hook: RecoveryHook = Box::new(move |_| {
            let c = calls_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 5, 0, Some(hook))
            .await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "hook must not fire when threshold=0"
        );
    }

    #[tokio::test]
    async fn recovery_hook_error_stops_loop() {
        // hook が Err を返す → ExecuteError で即停止。
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let many = (0..20).map(|_| blank.clone()).collect();
        let frames = frames_of(many);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )],
            2400,
            300,
        );

        let hook: RecoveryHook = Box::new(|_| {
            Box::pin(async {
                Err(DeviceError::CommandFailed {
                    message: "launch failed".into(),
                })
            })
        });

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 100, 2, Some(hook))
            .await;
        assert_eq!(outcome.reason, LoopStopReason::ExecuteError);
        assert_eq!(outcome.terminal, "recovery_failed");
    }

    // ---- (5b) Issue #210: リカバリ後の起動猶予 (boot-wait) ----
    //
    // 再起動ストーム (recovery は spawn-only 3ms で起動完了を待たない vs ゲームの
    // タイトル到達は分単位 → 起動途中のゲームを殺し続ける) を構造的に防ぐ猶予の検証。
    // 時間は SteppingRecoveryClock (呼出毎に一定量進む fake) で決定論的に注入する
    // (GoalClock/FakeClock と同じ方針 — tokio::time::pause 非導入)。

    /// 呼出毎に `step` ずつ時間が進むテスト用 [`RecoveryClock`]。
    struct SteppingRecoveryClock {
        next: Duration,
        step: Duration,
    }

    impl SteppingRecoveryClock {
        fn new(step: Duration) -> Self {
            Self {
                next: Duration::ZERO,
                step,
            }
        }
    }

    impl RecoveryClock for SteppingRecoveryClock {
        fn elapsed(&mut self) -> Duration {
            let now = self.next;
            self.next += self.step;
            now
        }
    }

    /// 猶予 + fake クロックを備えた NoMatch driver を構築するヘルパ。
    fn build_grace_driver(
        frames: Arc<Mutex<VecDeque<DynamicImage>>>,
        grace: Duration,
        clock: Box<dyn RecoveryClock>,
    ) -> PipelineDriver<FakeCapture, FakeInput> {
        PipelineDriver::new(
            FakeCapture {
                frames,
                fail: false,
            },
            FakeInput {
                fired: new_fired(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )],
            2400,
            300,
        )
        .with_recovery_grace(grace)
        .with_recovery_clock(clock)
    }

    /// 呼出回数を数える recovery hook。
    fn counting_hook() -> (Arc<AtomicU32>, RecoveryHook) {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let hook: RecoveryHook = Box::new(move |_streak| {
            let c = calls_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });
        (calls, hook)
    }

    /// `RecoveryGuard` 純粋状態機械の遷移 (猶予中加算なし・猶予明け再開・match 解除)。
    /// 予算は u32::MAX (上限なし) — 本テストは猶予/streak 契約のみ (Issue #210 の
    /// 既存契約を 1 ビットも変えない)。予算の挙動は
    /// `recovery_guard_attempt_budget_and_healthy_reset` (02-recovery-budget) が担う。
    #[test]
    fn recovery_guard_grace_and_streak_transitions() {
        let mut g = RecoveryGuard::new(3, Duration::from_secs(90), u32::MAX);
        // 閾値未満では発火しない (streak は加算)。
        assert!(!g.record_miss(Duration::from_secs(0)));
        assert!(!g.record_miss(Duration::from_secs(1)));
        assert_eq!(g.streak(), 2);
        // 閾値到達で発火指示。
        assert!(g.record_miss(Duration::from_secs(2)));
        assert_eq!(g.streak(), 3);
        // リカバリ成功: streak リセット + 猶予タイマー開始 (t=3 から 90s)。
        g.recovered(Duration::from_secs(3));
        assert_eq!(g.streak(), 0);
        assert!(g.in_grace(Duration::from_secs(92)), "89s 経過では猶予中");
        // 猶予中は streak 加算なし・発火なし。
        assert!(!g.record_miss(Duration::from_secs(92)));
        assert_eq!(g.streak(), 0, "grace 中は streak 加算なし");
        // 猶予明け (90s 経過) は streak 再開。
        assert!(!g.record_miss(Duration::from_secs(93)));
        assert_eq!(g.streak(), 1);
        // マッチは streak リセット + 猶予解除 (即カウント再開)。
        g.record_match();
        assert_eq!(g.streak(), 0);
        assert!(!g.record_miss(Duration::from_secs(94)));
        assert_eq!(g.streak(), 1, "match 後は猶予解除で即カウント");
        // threshold=0 (リカバリ無効) は常に非発火。
        let mut z = RecoveryGuard::new(0, Duration::ZERO, u32::MAX);
        assert!(!z.record_miss(Duration::ZERO));
        assert!(!z.record_miss(Duration::ZERO));
    }

    /// 猶予期間中は streak が増えない → 再起動ストームが構造的に起きない。
    /// grace=90s を clock step=5s で消費: 3 回目で hook#1 → 以降 10 サイクル全て
    /// 猶予中 → hook は 1 回のみ (従来挙動なら 3,6,9 回目の 3 回)。
    #[tokio::test]
    async fn recovery_boot_grace_suppresses_restorm_during_grace() {
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let many: Vec<DynamicImage> = (0..30).map(|_| blank.clone()).collect();
        let mut driver = build_grace_driver(
            frames_of(many),
            Duration::from_secs(90),
            Box::new(SteppingRecoveryClock::new(Duration::from_secs(5))),
        );
        let (calls, hook) = counting_hook();

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 10, 3, Some(hook))
            .await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "猶予 (90s) 中は再発火しない — ストーム防止"
        );
    }

    /// 猶予明け後は streak が 0 から再開し、閾値再到達で次のリカバリが発火する。
    /// grace=90s・step=30s: c3 で hook#1 (recovered t=90, 猶予は t=180 まで) →
    /// c6-c8 (t=180,210,240) で streak 1..3 → hook#2。10 サイクルで計 2 回。
    #[tokio::test]
    async fn recovery_boot_grace_expires_then_streak_resumes() {
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let many: Vec<DynamicImage> = (0..30).map(|_| blank.clone()).collect();
        let mut driver = build_grace_driver(
            frames_of(many),
            Duration::from_secs(90),
            Box::new(SteppingRecoveryClock::new(Duration::from_secs(30))),
        );
        let (calls, hook) = counting_hook();

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 10, 3, Some(hook))
            .await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert_eq!(outcome.iterations, 10);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "猶予明け後に streak 再開 → 2 回目のリカバリが発火する"
        );
    }

    /// 猶予中でもマッチすれば即座に通常動作へ復帰し、猶予は解除される。
    /// c3 で hook#1 (猶予 t=180 まで) → c4 の matched フレームで発火 (猶予中の即復帰) +
    /// 猶予解除 → c5-c7 の NoMatch 3 回 (t=120,150,180) で streak 1..3 → hook#2。
    /// 猶予が残っていれば c5/c6 (t<180) は加算されず hook#2 は出ない — この対照が本テスト。
    /// max_iters=7 で hook#2 の直後に打ち切り (以降の猶予サイクルは
    /// recovery_boot_grace_expires_then_streak_resumes が担う)。
    #[tokio::test]
    async fn recovery_grace_clears_on_match_and_resumes_immediately() {
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let (matched, _tpl) = matched_frame();
        // c1-c3: blank (hook#1) / c4: matched (Fired・猶予解除) / c5-c7: blank (hook#2)。
        let mut seq: Vec<DynamicImage> = vec![blank.clone(), blank.clone(), blank.clone(), matched];
        for _ in 0..10 {
            seq.push(blank.clone());
        }
        let mut driver = build_grace_driver(
            frames_of(seq),
            Duration::from_secs(90),
            Box::new(SteppingRecoveryClock::new(Duration::from_secs(30))),
        );
        let (calls, hook) = counting_hook();

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 7, 3, Some(hook))
            .await;
        assert_eq!(
            outcome.fired_commands.len(),
            1,
            "猶予中のマッチで即座に発火 (capture は継続している)"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "match で猶予解除 → 以降の NoMatch は猶予なしで即カウント"
        );
    }

    /// 既定 (猶予 ZERO) は従来挙動: 閾値到達ごとに毎回 hook が発火する。
    /// builder を呼ばない driver で threshold=3, max=10 → 3,6,9 回目の 3 回。
    #[tokio::test]
    async fn recovery_default_zero_grace_fires_every_threshold_multiple() {
        let (mut driver, _fired) = build_nomatch_driver();
        let (calls, hook) = counting_hook();

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 10, 3, Some(hook))
            .await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "猶予なし既定では 3,6,9 回目の 3 回発火 (従来挙動・後方互換)"
        );
    }

    /// ゴール評価付きループ (`run_loop_with_goal`) でも起動猶予が効く。
    /// goal=LoopCount(未到達) + grace=90s + step=5s → hook は 1 回のみ。
    #[tokio::test]
    async fn recovery_boot_grace_applies_on_goal_path() {
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let many: Vec<DynamicImage> = (0..30).map(|_| blank.clone()).collect();
        let mut driver = build_grace_driver(
            frames_of(many),
            Duration::from_secs(90),
            Box::new(SteppingRecoveryClock::new(Duration::from_secs(5))),
        );
        let (calls, hook) = counting_hook();

        let goal = anaden_core::Goal {
            name: "g210".into(),
            stop: anaden_core::StopCondition::LoopCount { target: 100 },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 10, 3, Some(hook), Some(&goal), clock)
            .await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "goal path でも猶予が streak 蓄積を抑制する"
        );
    }

    /// 起動猶予中の capture エラー (再起動直後はゲームウィンドウ未存在 →
    /// GetClientRect 失敗等) は即停止せずループを継続する (Issue #210 実機検証で発見)。
    /// c3 で hook#1 → grace 開始 → c4 以降フレーム枯渇で capture エラーになるが許容され、
    /// MaxIterations まで到達 (CaptureError 停止しない)。猶予なしの既定では
    /// `run_loop_capture_error_stops_immediately` が従来どおり即停止を担保。
    #[tokio::test]
    async fn capture_error_during_boot_grace_is_tolerated() {
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        // 3 枚のみ: c1-c3 で消費し c4 から "no more frames" = capture エラー。
        let frames = frames_of(vec![blank.clone(), blank.clone(), blank]);
        let mut driver = build_grace_driver(
            frames,
            Duration::from_secs(90),
            Box::new(SteppingRecoveryClock::new(Duration::from_secs(5))),
        );
        let (calls, hook) = counting_hook();

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 8, 3, Some(hook))
            .await;
        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "猶予中の capture エラーは致命扱いにしない (ゲーム再起動の一過状態)"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    // ---- (5c) StepOutcome エラーの enum 構造化: 2 ループ間リカバリ契約の統一 ----
    //
    // F1 (verify_capture リカバリ漏れ): `run_once_verified` の事後 capture エラーは
    // message が "verify_capture: ..." で始まるため `starts_with("capture")` が false に
    // なり、recovery 有効でも ExecuteError 即停止していた。capture と同じ「画面取得系
    // IO エラー = ゲーム死/起動中の可能性が高い」に分類し、リカバリ契約へ乗せる。
    //
    // F2 (--goal 弱体化): `run_loop_with_goal` の Error ブランチは grace 中の warn 継続
    // しか持たず、grace 外 + recovery_enabled でも hook を起動せず CaptureError 即停止
    // していた (`run_loop_with_recovery` とコメント上は「同一契約」を宣言しつつ挙動が
    // 乖離)。両ループで同一の分類・リカバリ契約を保証する。

    /// verify_capture エラー (1 枚目 capture 成功 → fire → 事後 capture が IO Err) が
    /// recovery_enabled なら recovery hook 起動後にループを継続することを検証 (ループ単位)。
    /// 既存 `verify_capture_error_returns_error` は run_once 単位の挙動 pin なので、
    /// ここでループへの伝播を pin する。
    #[tokio::test]
    async fn test_verify_capture_error_enters_recovery_path() {
        let (matched, _tpl) = matched_frame();
        // 1 枚のみ: c1 の run_once は成功(fire)するが verify capture が "no more frames"。
        // c2 以降は run_once の capture 自体が失敗する (hook 後の grace 中は warn 継続)。
        let mut driver = build_grace_driver(
            frames_of(vec![matched]),
            Duration::from_secs(90),
            Box::new(SteppingRecoveryClock::new(Duration::from_secs(5))),
        )
        .with_verify(true);
        let (calls, hook) = counting_hook();

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 8, 2, Some(hook))
            .await;
        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "verify_capture エラーは recovery 経路へ分類される (ExecuteError 即停止しない)"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "verify_capture エラーが recovery hook を起動する (以降の capture エラーは grace 中)"
        );
    }

    /// kind 分類の表駆動単体テスト (F3: 文字列 prefix 一致の解消)。
    /// 画面取得系 (Capture / VerifyCapture) はリカバリ対象、入力発火系 (Execute) は
    /// fail-closed。テンプレ読込系 (Other) は設定破損 = 恒常故障のため fail-closed
    /// (05-template-cache / F34)。この契約が両ループの Error 分岐を決定する。
    #[test]
    fn test_step_error_kind_classification() {
        let cases = [
            (StepErrorKind::Capture, true),
            (StepErrorKind::VerifyCapture, true),
            (StepErrorKind::Execute, false),
            (StepErrorKind::Other, false),
        ];
        for (kind, expected) in cases {
            assert_eq!(is_recoverable_io_error(kind), expected, "kind={kind:?}");
        }
    }

    /// grace 外 capture エラー + recovery_enabled で `run_loop_with_goal` が
    /// CaptureError 即停止ではなく recovery hook 起動後に再開することを検証
    /// (`run_loop_with_recovery` の L1157-1198 契約と同一期待)。
    #[tokio::test]
    async fn test_goal_loop_capture_error_matches_recovery_contract() {
        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames_of(vec![]),
                fail: true,
            },
            FakeInput {
                fired: new_fired(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![],
            2400,
            300,
        )
        .with_recovery_grace(Duration::from_secs(90))
        .with_recovery_clock(Box::new(SteppingRecoveryClock::new(Duration::from_secs(5))));
        let (calls, hook) = counting_hook();
        let goal = anaden_core::Goal {
            name: "g-capture-recovery".into(),
            stop: anaden_core::StopCondition::LoopCount { target: 100 },
        };

        let outcome = driver
            .run_loop_with_goal(
                Duration::ZERO,
                6,
                1,
                Some(hook),
                Some(&goal),
                FakeClock::starting_at(0),
            )
            .await;
        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "goal loop の grace 外 capture エラーは recovery hook 経路へ分類される (CaptureError 即停止しない)"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "grace 外 capture エラーが recovery hook を起動する (hook 後は grace 中 warn 継続)"
        );
    }

    // ---- (5d) 02-recovery-budget: リカバリ試行の circuit breaker ----
    //
    // F4 (リカバリ試行に上限なし): 従来は NoMatch が続く限り recovery hook が
    // max_iterations 到達まで発火し続け、永久 NoMatch 状態でゲーム再起動を
    // 無限に繰り返した。またフックの試行番号は CLI 側 AtomicU32 の単調カウンタで
    // 健全サイクル後にリセットされず、即 kill+restart の強いエスカレーションに
    // なっていた。ここでは (1) 試行上限で停止する契約、(2) 健全サイクルでの
    // 試行番号リセット、(3) ProgressReport の復帰テレメトリを pin する。

    /// hook へ渡された試行番号 (RecoveryContext.attempt) を記録する recovery hook。
    fn attempt_recording_hook() -> (Arc<Mutex<Vec<u32>>>, RecoveryHook) {
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let sink = attempts.clone();
        let hook: RecoveryHook = Box::new(move |ctx: RecoveryContext| {
            let a = sink.clone();
            Box::pin(async move {
                a.lock().expect("attempts lock").push(ctx.attempt);
                Ok(())
            })
        });
        (attempts, hook)
    }

    /// RED (1): 恒久 NoMatch 状態で recovery hook は `max_recovery_attempts` 回
    /// (テストでは 3) で頭打ちになり、run は [`LoopStopReason::RecoveryExhausted`]
    /// で終了する。従来は threshold=3 / max_iters=100 で 33 回発火し続けて
    /// MaxIterations 停止だった (上限なし = RED)。
    #[tokio::test]
    async fn test_recovery_attempts_capped() {
        let (driver, _fired) = build_nomatch_driver();
        let mut driver = driver.with_max_recovery_attempts(3);
        let (attempts, hook) = attempt_recording_hook();

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 100, 3, Some(hook))
            .await;

        assert_eq!(
            outcome.reason,
            LoopStopReason::RecoveryExhausted,
            "上限到達後は MaxIterations ではなく RecoveryExhausted で停止する"
        );
        assert_eq!(outcome.terminal, "recovery_exhausted");
        assert_eq!(
            attempts.lock().expect("attempts lock").as_slice(),
            &[1, 2, 3],
            "hook はエスカレーション 1→2→3 の 3 回で頭打ち。4 回目は起動しない"
        );
        // 復帰テレメトリ: 総試行回数と連続最大値が ProgressReport から読める。
        assert_eq!(outcome.progress_report.recovery_attempts, 3);
        assert_eq!(outcome.progress_report.max_consecutive_recovery, 3);
    }

    /// RED (2): recovery 1 回 → マッチ回復 (Fired) → 再 NoMatch のシーケンスで、
    /// 2 回目のリカバリは attempt=1 (launch_app 段階) からやり直しになる。
    /// 従来の単調カウンタでは 2 回目が即 kill+restart (attempt=2) になる想定と対照。
    #[tokio::test]
    async fn test_recovery_counter_resets_on_healthy_cycle() {
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let (matched, _tpl) = matched_frame();
        // c1-c3: NoMatch → hook attempt=1 / c4: matched (Fired = 健全サイクル) で
        // 試行カウンタリセット / c5-c7: NoMatch → hook attempt=1 (2 ではない)。
        // max_iters=7 で c7 の hook#2 直後に打ち切り (以降のエピソードを切り離す)。
        let seq: Vec<DynamicImage> = vec![
            blank.clone(),
            blank.clone(),
            blank.clone(),
            matched,
            blank.clone(),
            blank.clone(),
            blank,
        ];
        let frames = frames_of(seq);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )],
            2400,
            300,
        );

        let (attempts, hook) = attempt_recording_hook();
        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 7, 3, Some(hook))
            .await;

        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert_eq!(
            attempts.lock().expect("attempts lock").as_slice(),
            &[1, 1],
            "健全サイクル (Fired) を挟んだ 2 回目のリカバリは attempt=1 \
             (launch_app 段階) からやり直しになる"
        );
        // テレメトリ: 総試行 2 回・両エピソードとも連続 1 回で復帰。
        assert_eq!(outcome.progress_report.recovery_attempts, 2);
        assert_eq!(outcome.progress_report.max_consecutive_recovery, 1);
        assert!(
            !outcome.fired_commands.is_empty(),
            "c4 の matched フレームで 1 回発火している"
        );
    }

    /// RED (3): ProgressReport の復帰テレメトリ (recovery_attempts /
    /// max_consecutive_recovery) が JSON/TOML 経路で直列化され、round-trip する。
    /// また新フィールド欠落の古い成果物は `#[serde(default)]` で 0 補完される
    /// (CI/studio の旧デシリアライズ互換)。
    #[test]
    fn progress_report_serializes_recovery_telemetry() {
        let pr = ProgressReport {
            jitter_seed: None,
            verify: Default::default(),
            iterations: 9,
            fired_count: 1,
            per_task_matches: vec![],
            elapsed_ms: 10,
            terminal_task: None,
            reached_goal: None,
            stop_reason: Some(LoopStopReason::MaxIterations),
            recovery_attempts: 4,
            max_consecutive_recovery: 2,
        };
        // JSON 経路
        let j = serde_json::to_string(&pr).expect("serialize json");
        assert!(j.contains("\"recovery_attempts\":4"), "got: {j}");
        assert!(j.contains("\"max_consecutive_recovery\":2"), "got: {j}");
        let back: ProgressReport = serde_json::from_str(&j).expect("deserialize json");
        assert_eq!(back.recovery_attempts, 4);
        assert_eq!(back.max_consecutive_recovery, 2);
        // TOML 経路 (対称性担保)
        let t = toml::to_string(&pr).expect("serialize toml");
        assert!(t.contains("recovery_attempts = 4"), "got: {t}");
        assert!(t.contains("max_consecutive_recovery = 2"), "got: {t}");
        // 古い成果物 (新フィールド無し JSON) からのデシリアライズは 0 補完される。
        let legacy_json =
            "{\"iterations\":1,\"fired_count\":0,\"per_task_matches\":[],\"elapsed_ms\":5}";
        let legacy: ProgressReport = serde_json::from_str(legacy_json).expect("legacy deserialize");
        assert_eq!(legacy.recovery_attempts, 0, "got: {legacy_json}");
        assert_eq!(legacy.max_consecutive_recovery, 0, "got: {legacy_json}");
    }

    /// RED (03-report-channel): `ProgressReport::stop_reason` が JSON 経路で
    /// round-trip し、新フィールド欠落の旧成果物は None 補完で読める
    /// (report 単体を消費する CLI/studio のデシリアライズ互換)。
    #[test]
    fn progress_report_serializes_stop_reason() {
        let pr = ProgressReport {
            jitter_seed: None,
            verify: Default::default(),
            iterations: 7,
            fired_count: 3,
            per_task_matches: vec![],
            elapsed_ms: 1200,
            terminal_task: Some("max_iterations".to_string()),
            reached_goal: None,
            stop_reason: Some(LoopStopReason::MaxIterations),
            recovery_attempts: 0,
            max_consecutive_recovery: 0,
        };
        let j = serde_json::to_string(&pr).expect("serialize json");
        // enum は #[serde(rename_all = "snake_case")] で機械可読表現になる。
        assert!(j.contains("\"stop_reason\":\"max_iterations\""), "got: {j}");
        let back: ProgressReport = serde_json::from_str(&j).expect("deserialize json");
        assert_eq!(back.stop_reason, Some(LoopStopReason::MaxIterations));
        // 旧成果物 (本フィールド無し JSON) は None 補完される。
        let legacy_json =
            "{\"iterations\":1,\"fired_count\":0,\"per_task_matches\":[],\"elapsed_ms\":5}";
        let legacy: ProgressReport = serde_json::from_str(legacy_json).expect("legacy deserialize");
        assert_eq!(legacy.stop_reason, None, "got: {legacy_json}");
    }

    /// RED (03-report-channel): run loop の停止 outcome が ProgressReport に
    /// 停止理由を埋め込む (report 単体で「なぜ止まったか」が読める)。
    /// capture 常時失敗 + recovery 無し → CaptureError で即停止する経路で検証。
    #[tokio::test]
    async fn loop_outcome_embeds_stop_reason_in_report() {
        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames_of(vec![]),
                fail: true,
            },
            FakeInput {
                fired: new_fired(),
                fail: false,
            },
            PipelineState::new("t"),
            vec![],
            1280,
            300,
        );
        let out = driver
            .run_loop_with_recovery(Duration::ZERO, 3, 1, None)
            .await;
        assert_eq!(out.reason, LoopStopReason::CaptureError);
        assert_eq!(
            out.progress_report.stop_reason,
            Some(LoopStopReason::CaptureError)
        );
    }

    /// RED (03-report-channel): `stop_reason_label` は全バリアントに空でない
    /// ラベルを返す。CLI の「停止理由:」行と studio の report 表示の単一情報源。
    #[test]
    fn stop_reason_label_covers_all_variants() {
        let all = [
            LoopStopReason::Stop,
            LoopStopReason::TerminalTask,
            LoopStopReason::MaxIterations,
            LoopStopReason::CaptureError,
            LoopStopReason::ExecuteError,
            LoopStopReason::GoalReached,
            LoopStopReason::GoalTimeout,
            LoopStopReason::Interrupted,
            LoopStopReason::RecoveryExhausted,
        ];
        for reason in all {
            assert!(
                !stop_reason_label(reason.clone()).is_empty(),
                "{reason:?} にラベル未定義"
            );
        }
        // studio の stdout 行解析契約 (log_status) が依存する既知値のピン。
        assert_eq!(
            stop_reason_label(LoopStopReason::MaxIterations),
            "最大サイクル到達"
        );
        assert_eq!(
            stop_reason_label(LoopStopReason::GoalReached),
            "宣言的ゴール到達(正常)"
        );
    }

    /// `RecoveryGuard` の試行予算 (circuit breaker) 純粋遷移:
    /// - `consume_attempt` は予算内で 1 起点の試行番号を返す。
    /// - hook 成功 (`recovered`) だけでは試行カウンタをリセットしない
    ///   (ゲーム未復帰ならエスカレーション継続)。
    /// - 予算到達後は [`None`] (呼出側は RecoveryExhausted で停止)。
    /// - 健全サイクル (`record_match`) でカウンタは 0 へ戻る。
    #[test]
    fn recovery_guard_attempt_budget_and_healthy_reset() {
        let mut g = RecoveryGuard::new(1, Duration::ZERO, 2);
        assert!(g.record_miss(Duration::ZERO), "threshold=1 で即 due");
        assert_eq!(g.consume_attempt(), Some(1));
        g.recovered(Duration::ZERO);
        // hook 成功だけではリセットされない (連続故障のエスカレーション)。
        assert!(g.record_miss(Duration::ZERO));
        assert_eq!(g.consume_attempt(), Some(2), "連続 2 回目");
        g.recovered(Duration::ZERO);
        assert!(g.record_miss(Duration::ZERO));
        assert_eq!(g.consume_attempt(), None, "予算 2 到達で打ち切り");
        // 健全サイクルでカウンタは 0 へ戻る (次の故障は新規エピソード)。
        g.record_match();
        assert!(g.record_miss(Duration::ZERO));
        assert_eq!(
            g.consume_attempt(),
            Some(1),
            "健全後に attempt は 1 からやり直し"
        );
    }

    // ---- (6) run_once_verified: アクション後検証 ----

    #[tokio::test]
    async fn verify_success_when_template_disappears() {
        // 発火前フレーム: needle 埋込(マッチ→ClickRect 発火)。
        // 発火後フレーム: 背景のみ(needle 消失) → 検証成功 → Fired。
        let (matched, _tpl) = matched_frame();
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        // run_once が1枚目消費、verify_action_effect が2枚目消費。
        let frames = frames_of(vec![matched, blank]);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        );

        let out = driver.run_once_verified().await;
        match out {
            StepOutcome::Fired {
                next_current,
                fired: just_fired,
            } => {
                assert_eq!(next_current.as_deref(), Some("LoadGame"));
                assert_eq!(just_fired, Some(InputCommand::Tap { x: 640, y: 359 }));
            }
            other => panic!("expected Fired (verified), got {other:?}"),
        }
        // current は next へ進む(検証成功なので)。
        assert_eq!(driver.current(), "LoadGame");
        assert_eq!(
            fired.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: 640, y: 359 }]
        );
    }

    #[tokio::test]
    async fn verify_fails_when_template_persists() {
        // 発火前フレームも発火後フレームも needle 埋込 → 発火後もテンプレ残存 →
        // FiredUnverified。close_btn 誤キャプチャ等の「偽成功」を防ぐ経路。
        let (matched, _tpl) = matched_frame();
        let frames = frames_of(vec![matched.clone(), matched]);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        );

        let out = driver.run_once_verified().await;
        match out {
            StepOutcome::FiredUnverified {
                fired: just_fired,
                next_current,
            } => {
                // 発火自体は起きた(記録用)。
                assert_eq!(just_fired, Some(InputCommand::Tap { x: 640, y: 359 }));
                // next_current は tick 結果を伝搬(ログ用)。
                assert_eq!(next_current.as_deref(), Some("LoadGame"));
            }
            other => panic!("expected FiredUnverified, got {other:?}"),
        }
        // current は発火前タスクへ巻き戻される(対象残存なので next へ進まない)。
        assert_eq!(
            driver.current(),
            "Title",
            "current must be rolled back to pre-task on verify failure"
        );
        // 発火は起きたので fake input に記録される。
        assert_eq!(
            fired.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: 640, y: 359 }]
        );
    }

    #[tokio::test]
    async fn verify_skipped_on_nofire_and_nomatch() {
        // Stop(NoFire) は発火しないので検証スキップ → run_once と同じ NoFire(None)。
        let (screen, _tpl) = matched_frame();
        let frames = frames_of(vec![screen]);
        let fired = new_fired();

        let task = click_rect_task("Title", Action::Stop, Some(vec!["Ignored"]));

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        );

        let out = driver.run_once_verified().await;
        assert_eq!(out, StepOutcome::NoFire { next_current: None });
        assert!(fired.lock().expect("fired lock").is_empty());
    }

    #[tokio::test]
    async fn verify_capture_error_returns_error() {
        // 発火は成功(1枚目 matched)、事後 capture でフレーム枯渇 → Error(verify_capture)。
        let (matched, _tpl) = matched_frame();
        // 2枚目無し → verify_action_effect の capture が "no more frames" エラー。
        let frames = frames_of(vec![matched]);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        );

        match driver.run_once_verified().await {
            StepOutcome::Error { kind, message } => {
                assert_eq!(kind, StepErrorKind::VerifyCapture);
                assert!(message.starts_with("verify_capture"), "got: {message}");
            }
            other => panic!("expected Error(verify_capture), got {other:?}"),
        }
        // 発火は起きたので fake input に記録される。
        assert_eq!(
            fired.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: 640, y: 359 }]
        );
    }

    #[tokio::test]
    async fn verify_loop_treats_fired_unverified_as_nomatch_streak() {
        // with_verify(true) で run_loop_with_recovery が run_once_verified を使い、
        // FiredUnverified が NoMatch streak に加算されること。
        // 全フレーム matched(テンプレ残存) → 毎サイクル FiredUnverified → streak 蓄積 →
        // threshold=2 で recovery hook 発火。current は Title に巻き戻り続ける。
        let (matched, _tpl) = matched_frame();
        // run_once(1) + verify(1) で1サイクル2枚消費。十分な枚数。
        let many: Vec<DynamicImage> = (0..40).map(|_| matched.clone()).collect();
        let frames = frames_of(many);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        )
        .with_verify(true);

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let hook: RecoveryHook = Box::new(move |_| {
            let c = calls_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });

        // threshold=2 で2サイクル目に hook 発火。current は Title に巻き戻り続けるため
        // 終端には到達せず、最終的に MaxIterations で停止。
        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 20, 2, Some(hook))
            .await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert_eq!(
            driver.current(),
            "Title",
            "current rolled back each FiredUnverified cycle"
        );
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "recovery hook must fire on FiredUnverified streak"
        );
        // 発火は起きているので記録に残る(verify 失敗でも fired は蓄積)。
        assert!(
            !outcome.fired_commands.is_empty(),
            "fired commands recorded even on verify failure"
        );
    }

    #[tokio::test]
    async fn verify_disabled_by_default_runs_run_once() {
        // with_verify を呼ばない(デフォルト) → run_once と同等 → FiredUnverified は出ない。
        // 1枚目 matched で発火、2枚目も matched だが検証しないので普通の Fired。
        // run_loop は next へ進み、LoadGame タスクが無いので NoMatch ループ → MaxIterations。
        let (matched, _tpl) = matched_frame();
        let many: Vec<DynamicImage> = (0..20).map(|_| matched.clone()).collect();
        let frames = frames_of(many);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        ); // with_verify 省略 = デフォルト OFF

        let outcome = driver.run_loop(Duration::ZERO, 20).await;
        // デフォルト動作: Title で発火 → next=LoadGame へ進む(検証無し)。
        assert_eq!(driver.current(), "LoadGame");
        assert!(!outcome.fired_commands.is_empty());
    }

    #[tokio::test]
    async fn nomatch_streak_resets_on_fired() {
        // threshold=3 未到達で hook 不発。MaxIterations で停止。
        let (matched, _tpl) = matched_frame();
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));

        // 順序: blank, blank, matched, blank, blank, blank, ...(MaxIterations まで)
        let mut seq: Vec<DynamicImage> = vec![blank.clone(), blank.clone(), matched];
        for _ in 0..20 {
            seq.push(blank.clone());
        }
        let frames = frames_of(seq);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )],
            2400,
            300,
        );

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let hook: RecoveryHook = Box::new(move |_| {
            let c = calls_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });

        // max_iters を十分に大きく: 最終的に NoMatch が 3 に到達して hook 1回発火後にリセット。
        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 12, 3, Some(hook))
            .await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        // 最低1回の Fired がある(matched フレーム分)。
        assert!(
            !outcome.fired_commands.is_empty(),
            "expected at least one fire from matched frame"
        );
    }

    // ---- (7) ProgressReport / format_progress_report ----

    /// `ProgressReport::default` が空フィールドを返す(serde default 互換)。
    #[test]
    fn progress_report_default_is_empty() {
        let pr = ProgressReport::default();
        assert_eq!(pr.iterations, 0);
        assert_eq!(pr.fired_count, 0);
        assert!(pr.per_task_matches.is_empty());
        assert_eq!(pr.elapsed_ms, 0);
        assert_eq!(pr.terminal_task, None);
        assert_eq!(pr.reached_goal, None);
    }

    /// `LoopOutcome` が `progress_report` フィールドを持ち、`build_outcome` が基本統計を埋める。
    /// 端末タスク名・iterations・fired_count が伝わること。
    #[tokio::test]
    async fn run_loop_populates_progress_report_basic_fields() {
        // Title → LoadGame → Terminal の3タスク。全発火 → TerminalTask 停止。
        let frames = frames_of(vec![needle_screen(0), needle_screen(1), needle_screen(2)]);
        let fired = new_fired();

        let tasks = vec![
            click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            ),
            click_rect_task(
                "LoadGame",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["Terminal"]),
            ),
            click_rect_task(
                "Terminal",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                None,
            ),
        ];

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            tasks,
            2400,
            300,
        );

        let outcome = driver.run_loop(Duration::ZERO, 10).await;
        // 基本統計: iterations=3, fired=3, terminal_task=Terminal
        assert_eq!(outcome.progress_report.iterations, 3);
        assert_eq!(outcome.progress_report.fired_count, 3);
        assert_eq!(
            outcome.progress_report.terminal_task.as_deref(),
            Some("Terminal")
        );
        // 到達ゴールは非ゴールモードなので None のまま(後続タスクが入れる)
        assert_eq!(outcome.progress_report.reached_goal, None);
    }

    /// per_task_matches: 発火したタスク毎のマッチ回数が記録される。
    /// Title(1) → LoadGame(1) → Terminal(1) の各1回。
    #[tokio::test]
    async fn run_loop_tracks_per_task_match_counts() {
        let frames = frames_of(vec![needle_screen(0), needle_screen(1), needle_screen(2)]);
        let fired = new_fired();

        let tasks = vec![
            click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            ),
            click_rect_task(
                "LoadGame",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["Terminal"]),
            ),
            click_rect_task(
                "Terminal",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                None,
            ),
        ];

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            tasks,
            2400,
            300,
        );

        let outcome = driver.run_loop(Duration::ZERO, 10).await;
        let matches = &outcome.progress_report.per_task_matches;
        // 3タスクすべて1回ずつマッチ発火している
        assert_eq!(matches.len(), 3);
        let by_name: std::collections::HashMap<&str, u64> = matches
            .iter()
            .map(|m| (m.task.as_str(), m.matches))
            .collect();
        assert_eq!(by_name.get("Title"), Some(&1));
        assert_eq!(by_name.get("LoadGame"), Some(&1));
        assert_eq!(by_name.get("Terminal"), Some(&1));
    }

    /// `format_progress_report` が人間可読文字列を返す純関数。
    /// 文字列内に iterations/fired/terminal の数値が現れること。
    #[test]
    fn format_progress_report_is_human_readable() {
        let outcome = LoopOutcome {
            iterations: 7,
            fired_commands: vec![
                InputCommand::Tap { x: 1, y: 2 },
                InputCommand::Tap { x: 3, y: 4 },
            ],
            terminal: "Terminal".to_string(),
            reason: LoopStopReason::TerminalTask,
            progress_report: ProgressReport {
                jitter_seed: None,
                verify: Default::default(),
                iterations: 7,
                fired_count: 2,
                per_task_matches: vec![
                    TaskMatchCount {
                        task: "Title".to_string(),
                        matches: 1,
                    },
                    TaskMatchCount {
                        task: "LoadGame".to_string(),
                        matches: 1,
                    },
                ],
                elapsed_ms: 1234,
                terminal_task: Some("Terminal".to_string()),
                reached_goal: Some("loop_count=7".to_string()),
                stop_reason: None,
                recovery_attempts: 0,
                max_consecutive_recovery: 0,
            },
        };
        let s = format_progress_report(&outcome);
        assert!(s.contains("iterations: 7"), "got: {s}");
        assert!(s.contains("fired: 2"), "got: {s}");
        assert!(s.contains("elapsed"), "got: {s}");
        assert!(s.contains("1234"), "got: {s}");
        assert!(s.contains("Terminal"), "got: {s}");
        assert!(s.contains("Title"), "got: {s}");
        assert!(s.contains("loop_count=7"), "got: {s}");
    }

    /// `format_progress_report` が空の ProgressReport でも panic しない。
    #[test]
    fn format_progress_report_handles_default() {
        let outcome = LoopOutcome {
            iterations: 0,
            fired_commands: vec![],
            terminal: "io_error".to_string(),
            reason: LoopStopReason::CaptureError,
            progress_report: ProgressReport::default(),
        };
        let s = format_progress_report(&outcome);
        // 空でも基本ラベルは含む
        assert!(s.contains("iterations: 0"), "got: {s}");
        assert!(s.contains("fired: 0"), "got: {s}");
    }

    /// `LoopOutcome` が Serialize 可能で、progress_report フィールドが文字列に現れる。
    /// toml シリアライザ(anaden-engine 既存依存)で CI/studio 消費可能性を検証する。
    #[test]
    fn loop_outcome_is_serializable() {
        let outcome = LoopOutcome {
            iterations: 3,
            fired_commands: vec![InputCommand::Tap { x: 10, y: 20 }],
            terminal: "Terminal".to_string(),
            reason: LoopStopReason::TerminalTask,
            progress_report: ProgressReport {
                jitter_seed: None,
                verify: Default::default(),
                iterations: 3,
                fired_count: 1,
                per_task_matches: vec![TaskMatchCount {
                    task: "Title".to_string(),
                    matches: 1,
                }],
                elapsed_ms: 500,
                terminal_task: Some("Terminal".to_string()),
                reached_goal: None,
                stop_reason: None,
                recovery_attempts: 0,
                max_consecutive_recovery: 0,
            },
        };
        let s = toml::to_string(&outcome).expect("serialize");
        assert!(s.contains("[progress_report]"), "got: {s}");
        assert!(s.contains("iterations = 3"), "got: {s}");
        assert!(s.contains("fired_count = 1"), "got: {s}");
        assert!(s.contains("per_task_matches"), "got: {s}");
    }

    /// `ProgressReport` 単体が Serialize 可能。
    #[test]
    fn progress_report_is_serializable() {
        let pr = ProgressReport {
            jitter_seed: None,
            verify: Default::default(),
            iterations: 10,
            fired_count: 5,
            per_task_matches: vec![TaskMatchCount {
                task: "A".to_string(),
                matches: 5,
            }],
            elapsed_ms: 999,
            terminal_task: Some("A".to_string()),
            reached_goal: Some("template_match conf>=0.85".to_string()),
            stop_reason: None,
            recovery_attempts: 0,
            max_consecutive_recovery: 0,
        };
        let s = toml::to_string(&pr).expect("serialize");
        assert!(s.contains("fired_count = 5"), "got: {s}");
        assert!(s.contains("template_match"), "got: {s}");
    }

    /// `LoopOutcome` が JSON 経路(`serde_json`)でも CI/studio 消費可能な形で
    /// シリアライズされることを検証する。TOML 経路は `loop_outcome_is_serializable`
    /// が担保済み。JSON 経路は `reason = "goal_reached"` の snake_case 表現と
    /// `progress_report.reached_goal` 記述子を両方含むことを assert する。
    /// Issue #37 T5: JSON 成果レポート検証。
    #[test]
    fn loop_outcome_is_json_serializable() {
        let outcome = LoopOutcome {
            iterations: 5,
            fired_commands: vec![InputCommand::Tap { x: 10, y: 20 }],
            terminal: "GoalTerminal".to_string(),
            reason: LoopStopReason::GoalReached,
            progress_report: ProgressReport {
                jitter_seed: None,
                verify: Default::default(),
                iterations: 5,
                fired_count: 1,
                per_task_matches: vec![TaskMatchCount {
                    task: "Clear".to_string(),
                    matches: 1,
                }],
                elapsed_ms: 4321,
                terminal_task: Some("GoalTerminal".to_string()),
                reached_goal: Some("loop_count=5".to_string()),
                stop_reason: None,
                recovery_attempts: 0,
                max_consecutive_recovery: 0,
            },
        };
        let s = serde_json::to_string(&outcome).expect("serialize json");
        // reason が snake_case で JSON に現れる(後方互換性)
        assert!(s.contains("\"reason\":\"goal_reached\""), "got: {s}");
        // ネストした progress_report テーブルが JSON に現れる
        assert!(s.contains("\"progress_report\":{"), "got: {s}");
        // reached_goal 記述子が JSON に現れる(ゴール到達時の成果物)
        assert!(s.contains("\"reached_goal\":\"loop_count=5\""), "got: {s}");
        // 逆シリアライズ(round-trip)も成立すること
        let back: LoopOutcome = serde_json::from_str(&s).expect("deserialize json");
        assert_eq!(back.iterations, 5);
        assert_eq!(back.reason, LoopStopReason::GoalReached);
        assert_eq!(
            back.progress_report.reached_goal.as_deref(),
            Some("loop_count=5")
        );
    }

    /// `ProgressReport` 単体が JSON シリアライズ可能で、`reached_goal` 記述子を含む。
    /// TOML と JSON 双方で reached_goal 記述子が現れることを assert する(AC: 双方経路)。
    #[test]
    fn progress_report_is_json_serializable_with_reached_goal() {
        let pr = ProgressReport {
            jitter_seed: None,
            verify: Default::default(),
            iterations: 8,
            fired_count: 3,
            per_task_matches: vec![TaskMatchCount {
                task: "Boss".to_string(),
                matches: 2,
            }],
            elapsed_ms: 1500,
            terminal_task: Some("Boss".to_string()),
            reached_goal: Some("template_match conf>=0.85".to_string()),
            stop_reason: None,
            recovery_attempts: 0,
            max_consecutive_recovery: 0,
        };
        // JSON 経路
        let j = serde_json::to_string(&pr).expect("serialize json");
        assert!(j.contains("\"fired_count\":3"), "got: {j}");
        assert!(
            j.contains("\"reached_goal\":\"template_match conf>=0.85\""),
            "got: {j}"
        );
        // TOML 経路(対称性担保)
        let t = toml::to_string(&pr).expect("serialize toml");
        assert!(
            t.contains("reached_goal = \"template_match conf>=0.85\""),
            "got: {t}"
        );
    }

    /// デシリアライズ時、`progress_report` 欠落 TOML が `#[serde(default)]` で補完される。
    /// これは既存の古いシリアライズ成果物との後方互換性(CI/studio)を保証する。
    #[test]
    fn loop_outcome_progress_report_is_serde_default() {
        // progress_report テーブルを意図的に省いた TOML。
        // toml は enum の untagged 表現を持たないため、理由フィールドは文字列経由ではなく
        // 直接構築可能な形で検証する: progress_report 欠落時に default が補完されること。
        #[derive(serde::Deserialize)]
        struct Minimal {
            #[allow(dead_code)]
            iterations: u64,
        }
        let toml_str = "iterations = 1\n";
        let m: Minimal = toml::from_str(toml_str).expect("deserialize minimal");
        assert_eq!(m.iterations, 1);

        // LoopOutcome 自体は progress_report が #[serde(default)] なので、
        // フィールド無しでデシリアライズすると default ProgressReport になる。
        let outcome: LoopOutcome = LoopOutcome {
            iterations: 1,
            fired_commands: vec![],
            terminal: "x".to_string(),
            reason: LoopStopReason::Stop,
            progress_report: ProgressReport::default(),
        };
        // フィールド access で default 等価であることを再確認。
        assert_eq!(outcome.progress_report, ProgressReport::default());
    }

    /// `LoopStopReason::GoalReached` が `goal_reached` にシリアライズされる(後方互換)。
    /// Issue #37 T3: ゴール駆動モード完了状態の機械可読 exit-code mapping 用。
    #[test]
    fn loop_stop_reason_goal_reached_serializes_snake_case() {
        let outcome = LoopOutcome {
            iterations: 5,
            fired_commands: vec![],
            terminal: "GoalTerminal".to_string(),
            reason: LoopStopReason::GoalReached,
            progress_report: ProgressReport::default(),
        };
        let s = toml::to_string(&outcome).expect("serialize");
        assert!(
            s.contains("reason = \"goal_reached\""),
            "expected snake_case goal_reached, got: {s}"
        );
    }

    /// `LoopStopReason::GoalTimeout` が `goal_timeout` にシリアライズされる(後方互換)。
    #[test]
    fn loop_stop_reason_goal_timeout_serializes_snake_case() {
        let outcome = LoopOutcome {
            iterations: 99,
            fired_commands: vec![],
            terminal: "GoalTimeout".to_string(),
            reason: LoopStopReason::GoalTimeout,
            progress_report: ProgressReport::default(),
        };
        let s = toml::to_string(&outcome).expect("serialize");
        assert!(
            s.contains("reason = \"goal_timeout\""),
            "expected snake_case goal_timeout, got: {s}"
        );
    }

    /// `GoalReached` / `GoalTimeout` が TOML 経由で round-trip できる(Deserialize 互換)。
    /// 既存バリアントと同一の unit-variant 構造であることを保証する。
    #[test]
    fn loop_stop_reason_goal_variants_round_trip() {
        for (original, expected_str) in [
            (LoopStopReason::GoalReached, "goal_reached"),
            (LoopStopReason::GoalTimeout, "goal_timeout"),
        ] {
            let outcome = LoopOutcome {
                iterations: 1,
                fired_commands: vec![],
                terminal: "t".to_string(),
                reason: original.clone(),
                progress_report: ProgressReport::default(),
            };
            let s = toml::to_string(&outcome).expect("serialize");
            let back: LoopOutcome = toml::from_str(&s).expect("deserialize");
            assert_eq!(
                back.reason, original,
                "round-trip failed for {expected_str}"
            );
        }
    }

    /// `GoalReached`/`GoalTimeout` はユニットバリアント(データフィールド無し)で、
    /// 既存バリアントと同じく `PartialEq`/`Eq`/`Clone` で比較可能であることを検証。
    #[test]
    fn loop_stop_reason_goal_variants_are_unit_and_comparable() {
        let reached = LoopStopReason::GoalReached;
        let timeout = LoopStopReason::GoalTimeout;
        assert_eq!(reached.clone(), reached, "GoalReached clone/equality");
        assert_eq!(timeout.clone(), timeout, "GoalTimeout clone/equality");
        assert_ne!(reached, timeout, "GoalReached != GoalTimeout");
        assert_ne!(
            reached,
            LoopStopReason::ExecuteError,
            "distinct from ExecuteError"
        );
        assert_ne!(
            timeout,
            LoopStopReason::MaxIterations,
            "distinct from MaxIterations"
        );
    }

    // ---- (8) T5: goal=None 後方互換性検証 ----
    //
    // Issue #37 T5-no-goal-backcompat: ゴール駆動モード(T4)が `Option<Goal>` を
    // `run_loop_with_recovery` へ追加しても、`goal=None`(ゴール非宣言)時は既存の
    // max_iterations bounded 挙動が **byte-identical** で保持されなければならない。
    //
    // 本セクションは以下を検証する:
    // (a) 既存テスト `run_loop_no_match_hits_max_iterations` /
    //     `recovery_hook_fires_after_nomatch_threshold` と同一の結果(reason/iterations/
    //     fired_commands/terminal/current)が得られること。
    // (b) goal=None で GoalReached/GoalTimeout が **発火しない** こと。
    //     (停止理由は常に MaxIterations)
    // (c) 「無限ループ相当」特性: max_iters を大きくすると iterations が比例的に増え、
    //     一定の max_iters で打切られるまで延々継続できること。
    // (d) recovery hook の発火回数・streak リセット挙動が goal=None で崩れないこと。

    /// NoMatch フレームを無限に返す driver を構築するヘルパ(goal=None backcompat 共通)。
    /// 全フレーム背景一色なので tick は常に NoMatch → current 不変でループが回る。
    fn build_nomatch_driver() -> (
        PipelineDriver<FakeCapture, FakeInput>,
        Arc<Mutex<Vec<InputCommand>>>,
    ) {
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let many: Vec<DynamicImage> = (0..200).map(|_| blank.clone()).collect();
        let frames = frames_of(many);
        let fired = new_fired();
        let driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )],
            2400,
            300,
        );
        (driver, fired)
    }

    /// goal=None(現行シグネチャ = ゴール非宣言)で NoMatch が持続すると
    /// **MaxIterations** で停止し、iterations が max_iterations に等しいこと。
    /// これは既存 `run_loop_no_match_hits_max_iterations` と同一の契約。
    #[tokio::test]
    async fn backcompat_goal_none_no_match_hits_max_iterations_byte_identical() {
        let (mut driver, fired) = build_nomatch_driver();

        let outcome = driver.run_loop(Duration::ZERO, 5).await;
        // 停止理由は MaxIterations のみ(GoalReached/GoalTimeout は出ない)。
        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "goal=None must stop with MaxIterations, got {:?}",
            outcome.reason
        );
        // iterations は max_iterations に厳密に等しい(byte-identical)。
        assert_eq!(outcome.iterations, 5);
        // 発火無し。
        assert!(
            outcome.fired_commands.is_empty(),
            "no fires on pure NoMatch"
        );
        // current 不変。
        assert_eq!(driver.current(), "Title");
        assert!(fired.lock().expect("fired lock").is_empty());
    }

    /// goal=None + recovery hook(threshold 到達で Ok)でも停止理由は MaxIterations。
    /// これは既存 `recovery_hook_fires_after_nomatch_threshold` と同一の契約。
    #[tokio::test]
    async fn backcompat_goal_none_recovery_hook_fires_under_max_iterations() {
        let (mut driver, _fired) = build_nomatch_driver();

        let calls = Arc::new(AtomicU32::new(0));
        let calls_clone = calls.clone();
        let hook: RecoveryHook = Box::new(move |_streak| {
            let c = calls_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 10, 3, Some(hook))
            .await;
        // goal=None なので MaxIterations(Goal 系停止理由は出ない)。
        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "goal=None with recovery must stop with MaxIterations"
        );
        // threshold=3, max_iters=10 → hook は 3,6,9 回目の最低 3 回発火する想定だが、
        // 最低 1 回の発火だけを厳密契約として検証する(timing に依存しない)。
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "recovery hook must fire at least once under goal=None"
        );
        assert!(outcome.fired_commands.is_empty());
    }

    /// goal=None で recovery hook が Err を返すと **ExecuteError** で即停止
    /// (MaxIterations 以前)。ゴール非宣言時もリカバリ失敗契約は不変。
    #[tokio::test]
    async fn backcompat_goal_none_recovery_error_stops_immediately() {
        let (mut driver, _fired) = build_nomatch_driver();

        let hook: RecoveryHook = Box::new(|_| {
            Box::pin(async {
                Err(DeviceError::CommandFailed {
                    message: "launch failed".into(),
                })
            })
        });

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 100, 2, Some(hook))
            .await;
        assert_eq!(outcome.reason, LoopStopReason::ExecuteError);
        assert_eq!(outcome.terminal, "recovery_failed");
        // max_iters=100 だが即停止 → GoalTimeout ではなく ExecuteError が優先。
        assert!(
            outcome.iterations < 100,
            "must short-circuit before max_iterations"
        );
    }

    /// 「無限ループ相当」特性: goal=None で max_iters を大きくすると、
    /// その max_iters に到達するまでループが延々継続する(打切りによる停止のみ)。
    /// max_iters=5 と max_iters=50 を比較し、iterations が max に比例することを検証。
    #[tokio::test]
    async fn backcompat_goal_none_indefinite_continuation_scales_with_max_iters() {
        // max_iters=5
        let (mut driver_a, _fired_a) = build_nomatch_driver();
        let out_small = driver_a.run_loop(Duration::ZERO, 5).await;
        assert_eq!(out_small.reason, LoopStopReason::MaxIterations);
        assert_eq!(out_small.iterations, 5);

        // max_iters=50 — 10倍。同一条件下で iterations が 10倍に伸びる(=延々継続)。
        let (mut driver_b, _fired_b) = build_nomatch_driver();
        let out_large = driver_b.run_loop(Duration::ZERO, 50).await;
        assert_eq!(out_large.reason, LoopStopReason::MaxIterations);
        assert_eq!(out_large.iterations, 50);

        // 比例関係: 50 == 5 * 10。「max_iters を大きくすると延々継続」の機械的証明。
        assert_eq!(
            out_large.iterations,
            out_small.iterations * 10,
            "iterations must scale linearly with max_iters under goal=None"
        );
        // 両者ともゴール系停止理由ではない。
        assert_ne!(out_large.reason, LoopStopReason::GoalReached);
        assert_ne!(out_large.reason, LoopStopReason::GoalTimeout);
    }

    /// goal=None(既定)時、`progress_report.reached_goal` は常に [`None`]。
    /// T4 が Goal 到達時に reached_goal を埋めるように変更しても、
    /// goal=None では None のままであることを検証(後方互換の境界)。
    #[tokio::test]
    async fn backcompat_goal_none_progress_report_reached_goal_is_none() {
        let (mut driver, _fired) = build_nomatch_driver();
        let outcome = driver.run_loop(Duration::ZERO, 5).await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert!(
            outcome.progress_report.reached_goal.is_none(),
            "reached_goal must be None when goal is not declared"
        );
    }

    /// goal=None の境界値: max_iterations=1 でも MaxIterations で即停止
    /// (1 サイクル後に打切)。ゴール評価が介入して早期終端しないこと。
    #[tokio::test]
    async fn backcompat_goal_none_single_iteration_still_max_iterations() {
        let (mut driver, _fired) = build_nomatch_driver();
        let outcome = driver.run_loop(Duration::ZERO, 1).await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert_eq!(outcome.iterations, 1);
    }

    // ===== UC-3 backward-compat AC: run_loop_with_goal(goal=None) byte-identical to
    //       run_loop_with_recovery =====
    //
    // Issue #37/#41 close 認可条件 Task #4:
    // `run_loop_with_goal` が `goal=None` で渡されたとき `run_loop_with_recovery` へ
    // **verbatim(バイト同一)** で委譲することを、Outcome の全観測可能フィールドを
    // 機械的に比較して検証する。Goal 駆動コードパスが導入されても、ゴール非宣言時の
    // 既存挙動(MaxIterations/Recovery で停止、reached_goal=None)が **1 ビットも変わらない**
    // ことの回帰防止。
    //
    // 既存 backcompat_* テスト群(2650-2783)は run_loop / run_loop_with_recovery を直接呼び、
    // goal=None の *意味論的* 契約(MaxIterations 等)を検証する。本テストはそれらと相補的に、
    // **委譲経路自体(run_loop_with_goal の None 早期 return)** を通した場合の出力が、
    // 直接 run_loop_with_recovery を呼んだ場合と完全一致することを個別に固定する。

    /// `run_loop_with_goal(goal=None)` と `run_loop_with_recovery` の出力が
    /// **完全に同一**であることを、純粋 NoMatch + recovery 無しの最小ケースで検証する。
    /// 両者の `LoopOutcome` を構造的に比較(reason / iterations / terminal / fired_commands /
    /// progress_report)し、GoalReached/GoalTimeout が発火しないことを確認。
    /// また reached_goal が None のままであることも併せて検証(UC-3 AC の核心不変条件)。
    #[tokio::test]
    async fn backcompat_goal_none_run_loop_with_goal_is_byte_identical_to_recovery() {
        // 直接 run_loop_with_recovery を呼んだ基準結果(baseline)。
        let (mut driver_baseline, _fired_baseline) = build_nomatch_driver();
        let baseline = driver_baseline
            .run_loop_with_recovery(Duration::ZERO, 7, 0, None)
            .await;

        // 同一 fixture で run_loop_with_goal(goal=None) を通した結果。
        let (mut driver_via_goal, _fired_via_goal) = build_nomatch_driver();
        let clock = FakeClock::starting_at(0);
        let via_goal = driver_via_goal
            .run_loop_with_goal(Duration::ZERO, 7, 0, None, None, clock)
            .await;

        // === 停止理由: MaxIterations(Goal 系停止理由は出ない)===
        assert_eq!(
            baseline.reason,
            LoopStopReason::MaxIterations,
            "baseline must stop with MaxIterations"
        );
        assert_eq!(
            via_goal.reason,
            LoopStopReason::MaxIterations,
            "run_loop_with_goal(goal=None) must stop with MaxIterations, got {:?}",
            via_goal.reason
        );
        assert_eq!(
            baseline.reason, via_goal.reason,
            "reason diverges between direct and delegated paths"
        );

        // === 全観測可能フィールドのバイト同一性 ===
        assert_eq!(
            baseline.iterations, via_goal.iterations,
            "iterations diverges (delegation must be byte-identical)"
        );
        assert_eq!(
            baseline.terminal, via_goal.terminal,
            "terminal diverges (delegation must be byte-identical)"
        );
        assert_eq!(
            baseline.fired_commands, via_goal.fired_commands,
            "fired_commands diverges (delegation must be byte-identical)"
        );
        assert_eq!(
            baseline.progress_report.iterations, via_goal.progress_report.iterations,
            "progress_report.iterations diverges"
        );
        assert_eq!(
            baseline.progress_report.fired_count, via_goal.progress_report.fired_count,
            "progress_report.fired_count diverges"
        );
        assert_eq!(
            baseline.progress_report.per_task_matches, via_goal.progress_report.per_task_matches,
            "progress_report.per_task_matches diverges"
        );
        assert_eq!(
            baseline.progress_report.terminal_task, via_goal.progress_report.terminal_task,
            "progress_report.terminal_task diverges"
        );

        // === UC-3 AC 核心不変条件: reached_goal は None のまま ===
        assert!(
            baseline.progress_report.reached_goal.is_none(),
            "baseline reached_goal must be None (no goal declared)"
        );
        assert!(
            via_goal.progress_report.reached_goal.is_none(),
            "run_loop_with_goal(goal=None) must leave reached_goal as None — \
             goal-evaluation code must not populate it on the undeclared path"
        );

        // === Goal 系停止理由が一切発火しないことの明示的表明 ===
        assert_ne!(
            via_goal.reason,
            LoopStopReason::GoalReached,
            "goal=None path must NEVER emit GoalReached"
        );
        assert_ne!(
            via_goal.reason,
            LoopStopReason::GoalTimeout,
            "goal=None path must NEVER emit GoalTimeout"
        );
    }

    /// recovery hook 付きでも `run_loop_with_goal(goal=None)` が
    /// `run_loop_with_recovery` と同じ停止契約(MaxIterations 優先、Goal 系不出現)を
    /// 保つことを検証する。hook は Ok を返し続けるため MaxIterations まで到達する。
    /// (recovery Err の ExecuteError 契約は既存 2710 テストが直接経路で担保済み。)
    #[tokio::test]
    async fn backcompat_goal_none_with_recovery_hook_preserves_max_iterations_contract() {
        fn make_hook() -> (Arc<AtomicU32>, RecoveryHook) {
            let calls = Arc::new(AtomicU32::new(0));
            let calls_clone = calls.clone();
            let hook: RecoveryHook = Box::new(move |_streak| {
                let c = calls_clone.clone();
                Box::pin(async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            });
            (calls, hook)
        }

        // run_loop_with_goal(goal=None) + recovery hook 経路。
        let (mut driver, _fired) = build_nomatch_driver();
        let (calls, hook) = make_hook();
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 12, 3, Some(hook), None, clock)
            .await;

        // 停止理由は MaxIterations のみ(Goal 系停止理由は発火しない)。
        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "goal=None + recovery(Ok) must stop with MaxIterations, got {:?}",
            outcome.reason
        );
        assert_ne!(
            outcome.reason,
            LoopStopReason::GoalReached,
            "GoalReached must never fire when goal=None even with recovery"
        );
        assert_ne!(
            outcome.reason,
            LoopStopReason::GoalTimeout,
            "GoalTimeout must never fire when goal=None even with recovery"
        );
        // recovery hook は最低 1 回発火(threshold=3, max=12 → 3,6,9,12 のいずれか)。
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "recovery hook must fire at least once under goal=None delegation path"
        );
        // reached_goal は None(UC-3 AC 不変条件、recovery 付きでも崩れない)。
        assert!(
            outcome.progress_report.reached_goal.is_none(),
            "reached_goal must stay None even with recovery hook under goal=None"
        );
    }

    // ===== T-fix-1: evaluate_goal unit contract =====

    /// LoopCount ゴールが既に target に到達しているとき `evaluate_goal` は
    /// `GoalReached` の `LoopOutcome` を返し、`reached_goal` に記述子を埋める。
    /// まだ到達していなければ [`None`] を返す(継続)。
    #[tokio::test]
    async fn evaluate_goal_loop_count_reached_and_not_yet() {
        let (driver, _fired) = build_nomatch_driver();
        let started = Instant::now();
        let goal_reached = anaden_core::Goal {
            name: "g1".into(),
            stop: anaden_core::StopCondition::LoopCount { target: 5 },
        };
        let goal_pending = anaden_core::Goal {
            name: "g2".into(),
            stop: anaden_core::StopCondition::LoopCount { target: 100 },
        };

        // 到達済み: ctx.iterations >= target
        let mut ctx_hit = anaden_core::GoalStatusContext::new();
        for _ in 0..5 {
            ctx_hit.tick(None);
        }
        let outcome = driver
            .evaluate_goal(
                &goal_reached,
                &ctx_hit,
                0,
                GoalEvalInputs {
                    iterations: 5,
                    fired_commands: &[],
                    per_task_matches: &[],
                    started,
                    recovery: RecoveryTelemetry::default(),
                },
            )
            .expect("expected Some(LoopOutcome) on GoalReached");
        assert_eq!(outcome.reason, LoopStopReason::GoalReached);
        assert_eq!(outcome.terminal, "goal_reached");
        assert_eq!(
            outcome.progress_report.reached_goal.as_deref(),
            Some("loop_count=5"),
            "descriptor must reflect LoopCount target"
        );
        assert_eq!(outcome.progress_report.iterations, 5);

        // 未到達: None (ループ継続)
        let mut ctx_low = anaden_core::GoalStatusContext::new();
        for _ in 0..3 {
            ctx_low.tick(None);
        }
        assert!(
            driver
                .evaluate_goal(
                    &goal_pending,
                    &ctx_low,
                    0,
                    GoalEvalInputs {
                        iterations: 3,
                        fired_commands: &[],
                        per_task_matches: &[],
                        started,
                        recovery: RecoveryTelemetry::default(),
                    },
                )
                .is_none(),
            "must return None when target not reached"
        );
    }

    /// Timeout ゴールが elapsed_secs >= secs のとき `GoalTimeout` で停止し、
    /// `reached_goal` に timeout 記述子を埋める。UC-3 契約。
    #[tokio::test]
    async fn evaluate_goal_timeout_yields_goal_timeout_descriptor() {
        let (driver, _fired) = build_nomatch_driver();
        let started = Instant::now();
        let goal = anaden_core::Goal {
            name: "g3".into(),
            stop: anaden_core::StopCondition::Timeout { secs: 10 },
        };
        let ctx = anaden_core::GoalStatusContext::new();
        let outcome = driver
            .evaluate_goal(
                &goal,
                &ctx,
                10,
                GoalEvalInputs {
                    iterations: 0,
                    fired_commands: &[],
                    per_task_matches: &[],
                    started,
                    recovery: RecoveryTelemetry::default(),
                },
            )
            .expect("expected Some on timeout reached");
        assert_eq!(outcome.reason, LoopStopReason::GoalTimeout);
        assert_eq!(outcome.terminal, "goal_timeout");
        assert_eq!(
            outcome.progress_report.reached_goal.as_deref(),
            Some("timeout=10"),
            "descriptor must reflect Timeout secs"
        );

        // elapsed < secs → None
        assert!(
            driver
                .evaluate_goal(
                    &goal,
                    &ctx,
                    9,
                    GoalEvalInputs {
                        iterations: 0,
                        fired_commands: &[],
                        per_task_matches: &[],
                        started,
                        recovery: RecoveryTelemetry::default(),
                    },
                )
                .is_none(),
            "must return None before timeout"
        );
    }

    /// TemplateMatch ゴール: ctx.last_match が閾値以上で task 一致のとき GoalReached。
    /// UC-2 契約(T-fix-1 では evaluate_goal 単体の経路のみ検証。match_info 伝播は
    /// T-wire-match-info が担う)。
    #[tokio::test]
    async fn evaluate_goal_template_match_reached() {
        let (driver, _fired) = build_nomatch_driver();
        let started = Instant::now();
        let goal = anaden_core::Goal {
            name: "g4".into(),
            stop: anaden_core::StopCondition::TemplateMatch {
                task: "Boss".into(),
                confidence: 0.85,
            },
        };
        let mut ctx = anaden_core::GoalStatusContext::new();
        ctx.tick(Some((
            "Boss".to_string(),
            0.9_f32,
            ScreenRegion::new(0, 0, 10, 10),
        )));
        let outcome = driver
            .evaluate_goal(
                &goal,
                &ctx,
                0,
                GoalEvalInputs {
                    iterations: 1,
                    fired_commands: &[],
                    per_task_matches: &[],
                    started,
                    recovery: RecoveryTelemetry::default(),
                },
            )
            .expect("expected Some on TemplateMatch reached");
        assert_eq!(outcome.reason, LoopStopReason::GoalReached);
        assert_eq!(
            outcome.progress_report.reached_goal.as_deref(),
            Some("template_match conf>=0.85 (task=Boss)"),
            "descriptor must reflect TemplateMatch task/confidence"
        );
    }

    /// 記述子生成器が全 StopCondition バリアントをカバーし、
    /// 既存 fixtures(`loop_count=7`, `template_match conf>=0.85`)の形式と一致する。
    /// Issue #50: All/Any 合成バリアントもカバー（子記述子の集約）。
    #[test]
    fn goal_descriptor_covers_all_variants() {
        let lc = anaden_core::Goal {
            name: "n".into(),
            stop: anaden_core::StopCondition::LoopCount { target: 7 },
        };
        assert_eq!(goal_descriptor(&lc), "loop_count=7");

        let tm = anaden_core::Goal {
            name: "n".into(),
            stop: anaden_core::StopCondition::TemplateMatch {
                task: "X".into(),
                confidence: 0.85,
            },
        };
        assert!(goal_descriptor(&tm).starts_with("template_match conf>=0.85"));

        let to = anaden_core::Goal {
            name: "n".into(),
            stop: anaden_core::StopCondition::Timeout { secs: 30 },
        };
        assert_eq!(goal_descriptor(&to), "timeout=30");

        // Issue #50: All 合成（子記述子をカンマ区切りで集約、先頭に all[N] 接頭辞）
        let all = anaden_core::Goal {
            name: "n".into(),
            stop: anaden_core::StopCondition::All {
                conditions: vec![
                    anaden_core::StopCondition::LoopCount { target: 7 },
                    anaden_core::StopCondition::Timeout { secs: 30 },
                ],
            },
        };
        assert_eq!(
            goal_descriptor(&all),
            "all[2](loop_count=7, timeout=30)",
            "All descriptor must aggregate child descriptors with count prefix"
        );

        // Issue #50: Any 合成
        let any = anaden_core::Goal {
            name: "n".into(),
            stop: anaden_core::StopCondition::Any {
                conditions: vec![
                    anaden_core::StopCondition::TemplateMatch {
                        task: "X".into(),
                        confidence: 0.85,
                    },
                    anaden_core::StopCondition::LoopCount { target: 7 },
                ],
            },
        };
        assert_eq!(
            goal_descriptor(&any),
            "any[2](template_match conf>=0.85 (task=X), loop_count=7)",
            "Any descriptor must aggregate child descriptors with count prefix"
        );
    }

    /// Issue #50: All/Any 記述子の子要素数が接頭辞 `all[N]`/`any[N]` に正しく反映される。
    #[test]
    fn goal_descriptor_composition_count_reflects_children() {
        let single = anaden_core::Goal {
            name: "n".into(),
            stop: anaden_core::StopCondition::All {
                conditions: vec![anaden_core::StopCondition::LoopCount { target: 1 }],
            },
        };
        let desc = goal_descriptor(&single);
        assert!(
            desc.starts_with("all[1]("),
            "single-child All must report count=1, got: {desc}"
        );

        let triple = anaden_core::Goal {
            name: "n".into(),
            stop: anaden_core::StopCondition::Any {
                conditions: vec![
                    anaden_core::StopCondition::LoopCount { target: 1 },
                    anaden_core::StopCondition::LoopCount { target: 2 },
                    anaden_core::StopCondition::LoopCount { target: 3 },
                ],
            },
        };
        let desc = goal_descriptor(&triple);
        assert!(
            desc.starts_with("any[3]("),
            "triple-child Any must report count=3, got: {desc}"
        );
    }

    /// Issue #50: ネストした合成（All の子に Any を含む）も記述子が再帰的に展開される。
    /// descriptor が reached_goal 文字列として JSON round-trip する既存契約を維持するため、
    /// 文字列は人間可読かつ括弧対応が取れる必要がある。
    #[test]
    fn goal_descriptor_nested_composition_recurses() {
        let nested = anaden_core::Goal {
            name: "n".into(),
            stop: anaden_core::StopCondition::All {
                conditions: vec![
                    anaden_core::StopCondition::LoopCount { target: 5 },
                    anaden_core::StopCondition::Any {
                        conditions: vec![
                            anaden_core::StopCondition::Timeout { secs: 10 },
                            anaden_core::StopCondition::LoopCount { target: 9 },
                        ],
                    },
                ],
            },
        };
        let desc = goal_descriptor(&nested);
        assert_eq!(
            desc, "all[2](loop_count=5, any[2](timeout=10, loop_count=9))",
            "nested composition must recurse into child descriptors"
        );
    }

    // ===== T-wire-match-info: UC-2 E2E (TemplateMatch ゴール到達) =====
    //
    // Issue #37 T-wire-match-info: これまで match_info が全 StepOutcome バリアントで
    // None に hardcode されていたため、GoalStatusContext::last_match が populated されず
    // TemplateMatch ゴールが一切 GoalReached に到達しなかった(UC-2 が発火しない)。
    //
    // 修正: Fired/NoFire ブランチで self.last_match.take() を読み出し ctx へ伝播。
    // 本 E2E テストは「マッチングテンプレートが発火したサイクルで TemplateMatch ゴールが
    // GoalReached に到達する」ことを検証する(TDD: 修正前は赤、修正後は緑)。

    /// 決定論的経過秒数を返すテスト用クロック(org-feedback approval condition 参照)。
    /// `elapsed_secs` は tokio::time::pause ではなく呼出毎に決定論的に増やす。
    struct FakeClock {
        next_secs: u64,
    }

    impl FakeClock {
        fn starting_at(secs: u64) -> Self {
            Self { next_secs: secs }
        }
    }

    impl GoalClock for FakeClock {
        fn elapsed_secs(&mut self) -> u64 {
            let now = self.next_secs;
            // 呼出毎に 1 秒進める(UC-3 timeout 跨ぎテストで使用)。
            self.next_secs = self.next_secs.saturating_add(1);
            now
        }
    }

    // ===== UC-1 E2E (LoopCount ゴール到達) =====
    //
    // UC-1 は「宣言した反復数(tick)に到達したら停止する」時間駆動の終端保証。
    // goal.rs の doc に固定されている通り、LoopCount は認識成功率に依存せず、
    // evaluate 呼出回数(tick 数)のみで到達を判定する。よって NoMatch が持続する
    // build_nomatch_driver の fixture でも到達できる(UC-2/UC-3 のようにテンプレート
    // マッチや時間経過に依存しない)。本 E2E テストは UC-2(L3010)/UC-3(L2880 周辺)
    // の既存 E2E と対になる UC-1 の 1 本。

    /// UC-1 正常系: `LoopCount { target: 3 }` を与えると、3 tick(= 3 サイクル)後に
    /// `LoopStopReason::GoalReached` で停止し、`reached_goal == "loop_count=3"`、
    /// `iterations == 3` となること。`build_nomatch_driver` の fixture は NoMatch が
    /// 持続するが、LoopCount は tick 数のみで判定するため NoMatch でも到達する。
    ///
    /// ここでは `FakeClock::starting_at` を使用する(org-feedback approval condition)。
    /// LoopCount は経過秒数に依存しないので、clock の進み方は結果に影響しないが、
    /// `SystemClock` 切替や `tokio::time::pause` の持ち込みは契約違反として禁止する。
    #[tokio::test]
    async fn uc1_loop_count_goal_reaches_goal_reached_after_target_ticks() {
        let (mut driver, _fired) = build_nomatch_driver();

        let goal = anaden_core::Goal {
            name: "farm3".into(),
            stop: anaden_core::StopCondition::LoopCount { target: 3 },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 10, 0, None, Some(&goal), clock)
            .await;

        // UC-1 の核心: target=3 → 3 tick で GoalReached(MaxIterations ではない)。
        assert_eq!(
            outcome.reason,
            LoopStopReason::GoalReached,
            "UC-1 LoopCount target=3 must reach GoalReached after 3 ticks, got {:?}",
            outcome.reason
        );
        assert_eq!(outcome.terminal, "goal_reached");
        // 記述子は goal_descriptor の LoopCount 形式(loop_count=<target>)。
        assert_eq!(
            outcome.progress_report.reached_goal.as_deref(),
            Some("loop_count=3"),
            "reached_goal descriptor must reflect LoopCount target"
        );
        // tick 数 = evaluate 呼出回数 = サイクル数。target 到達と同値。
        assert_eq!(
            outcome.iterations, 3,
            "must reach goal exactly at iteration 3"
        );
    }

    /// UC-1 エッジ: `target` が `max_iterations` を超える場合、ゴール到達前に
    /// `max_iterations` に先到達し `LoopStopReason::MaxIterations` になること。
    /// これは「到達テストが常に GoalReached になる偽陽性」を防ぐ対照であり、
    /// LoopCount の早期停止が max_iterations ガードより優先されないことを検証する。
    #[tokio::test]
    async fn uc1_loop_count_goal_hits_max_iterations_when_target_exceeds_budget() {
        let (mut driver, _fired) = build_nomatch_driver();

        let goal = anaden_core::Goal {
            name: "farm_unbounded".into(),
            // max_iterations=5 より大きい target → 到達不可。
            stop: anaden_core::StopCondition::LoopCount { target: 50 },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 5, 0, None, Some(&goal), clock)
            .await;

        // ゴール未到達 → MaxIterations(GoalReached/GoalTimeout ではない)。
        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "UC-1 LoopCount with target>max_iterations must stop with MaxIterations, got {:?}",
            outcome.reason
        );
        // ゴールに到達していないので記述子は埋まらない。
        assert!(
            outcome.progress_report.reached_goal.is_none(),
            "reached_goal must be None when goal is not reached"
        );
        // iterations は max_iterations に等しい。
        assert_eq!(outcome.iterations, 5);
    }

    /// UC-2: テンプレートマッチが発火した直後のサイクルで TemplateMatch ゴールが
    /// `GoalReached` に到達し、`reached_goal` に記述子が埋まること。
    ///
    /// 手順: needle 埋込画面を与え ClickRect タスクをマッチ→発火させる。
    /// ゴールは同じタスク名 + 閾値 0.85(実測信頼度は閾値を超える)。
    /// run_loop_with_goal は Fired ブランチで last_match.take() により
    /// (task_name, confidence, region) を ctx.last_match へ伝播し、evaluate が
    /// Reached を返す → GoalReached で停止。
    #[tokio::test]
    async fn uc2_template_match_goal_reaches_goal_reached_when_matching_template_fires() {
        let (matched, _tpl) = matched_frame();
        // Title 発火後 next=LoadGame へ遷移するが、初回の Fired でゴール到達するので
        // 1 枚で十分(到達前の巻き戻り等は無い)。
        let frames = frames_of(vec![matched]);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            // next を出す(発火はする)がゴール評価が先に効いて GoalReached で停止。
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        );

        let goal = anaden_core::Goal {
            name: "find_title".into(),
            stop: anaden_core::StopCondition::TemplateMatch {
                task: "Title".into(),
                confidence: 0.85,
            },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 10, 0, None, Some(&goal), clock)
            .await;

        // UC-2 の核心: GoalReached で停止(MaxIterations/TerminalTask ではない)。
        assert_eq!(
            outcome.reason,
            LoopStopReason::GoalReached,
            "UC-2 TemplateMatch goal must reach GoalReached when matching template fires, \
             got {:?}",
            outcome.reason
        );
        assert_eq!(outcome.terminal, "goal_reached");
        // 記述子が task と confidence を反映。
        assert_eq!(
            outcome.progress_report.reached_goal.as_deref(),
            Some("template_match conf>=0.85 (task=Title)"),
            "reached_goal descriptor must reflect the TemplateMatch task/confidence"
        );
        // 1 サイクル目で到達(発火 = ctx.last_match populated → 即 Reached)。
        assert_eq!(
            outcome.iterations, 1,
            "must reach goal on first matching cycle"
        );
        // 発火は起きている。
        assert!(
            !outcome.fired_commands.is_empty(),
            "matching template must have fired a command"
        );
    }

    /// UC-2 回帰保護: last_match の task がゴール task と一致しないと到達しないこと。
    /// これで match_info 伝播が正しくても task 名不一致で NotYet → MaxIterations になる
    /// 対照を確立し、到達テストが「常に到達する偽陽性」にならないようにする。
    #[tokio::test]
    async fn uc2_template_match_goal_does_not_reach_when_task_name_mismatches() {
        let (matched, _tpl) = matched_frame();
        let many: Vec<DynamicImage> = (0..20).map(|_| matched.clone()).collect();
        let frames = frames_of(many);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            // next=None にすると TerminalTask で止まるので、遷移させてループを回す。
            // Title→LoadGame だが LoadGame 定義が無いので以降 NoMatch で MaxIterations。
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        );

        // ゴール task を "OtherTask"(マッチしない)にする → last_match.task == "Title" ≠ "OtherTask"
        // → NotYet → MaxIterations。
        let goal = anaden_core::Goal {
            name: "find_other".into(),
            stop: anaden_core::StopCondition::TemplateMatch {
                task: "OtherTask".into(),
                confidence: 0.85,
            },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 5, 0, None, Some(&goal), clock)
            .await;

        assert_ne!(
            outcome.reason,
            LoopStopReason::GoalReached,
            "must NOT reach GoalReached when task name mismatches last_match"
        );
    }

    // ===== UC-3 E2E (Timeout ゴール到達) =====
    //
    // UC-3 は「宣言した秒数が経過したら停止する」時間駆動の異常終端保証。
    // goal.rs の doc に固定されている通り、Timeout は与えられた elapsed_secs と
    // 宣言値の比較のみで判定する(pure evaluate)。run_loop_with_goal は各 tick 後に
    // GoalClock.elapsed_secs() を呼び、GoalStatus::Failed → LoopStopReason::GoalTimeout
    // で停止する。本 E2E テストは UC-1(L3019)/UC-2(L3096) と対になる UC-3 の 1 本。
    //
    // FakeClock(build_nomatch_driver の fixture と FakeClock::starting_at を流用)が
    // elapsed_secs 呼出毎に +1 するので、NoMatch が持続する build_nomatch_driver でも
    // 宣言秒数に到達したタイミングで GoalTimeout になる。

    /// UC-3 正常系: `Timeout { secs: 3 }` を与えると、FakeClock が呼出毎に +1 進むので
    /// 4 サイクル目(elapsed_secs == 3 >= 3)に `LoopStopReason::GoalTimeout` で停止し、
    /// `terminal == "goal_timeout"`、`reached_goal == "timeout=3"` になること。
    /// また `iterations == 4` は `max_iterations == 10` 未到達であり、ゴールが先に効いて
    /// いる(MaxIterations ではない)ことを検証する。
    ///
    /// ここでは `FakeClock::starting_at(0)` を使用する(org-feedback approval condition)。
    /// FakeClock の進め方(呼出毎 +1)により、開始値 0 → 0,1,2,3 と返し、3 で閾値到達。
    #[tokio::test]
    async fn uc3_timeout_goal_reaches_goal_timeout_after_declared_secs() {
        let (mut driver, _fired) = build_nomatch_driver();

        let goal = anaden_core::Goal {
            name: "hard_stop".into(),
            stop: anaden_core::StopCondition::Timeout { secs: 3 },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 10, 0, None, Some(&goal), clock)
            .await;

        // UC-3 の核心: elapsed_secs が secs(=3)に到達 → GoalTimeout(MaxIterations ではない)。
        // FakeClock starting_at(0) は呼出毎に 0,1,2,3 を返すので、4 tick 目で 3>=3 到達。
        assert_eq!(
            outcome.reason,
            LoopStopReason::GoalTimeout,
            "UC-3 Timeout secs=3 must reach GoalTimeout once FakeClock advances past the \
             limit, got {:?}",
            outcome.reason
        );
        assert_eq!(
            outcome.terminal, "goal_timeout",
            "terminal must be goal_timeout for Timeout failure"
        );
        // 記述子は goal_descriptor の Timeout 形式(timeout=<secs>)。
        assert_eq!(
            outcome.progress_report.reached_goal.as_deref(),
            Some("timeout=3"),
            "reached_goal descriptor must reflect Timeout secs"
        );
        // iterations は max_iterations(=10)未到達 → ゴールが先に効いた。
        assert!(
            outcome.iterations < 10,
            "goal must terminate before max_iterations; got iterations={}",
            outcome.iterations
        );
        // FakeClock の呼出毎 +1 トレース: tick1=0, tick2=1, tick3=2, tick4=3(>=3 → stop)。
        assert_eq!(
            outcome.iterations, 4,
            "must reach timeout exactly on iteration 4 (elapsed 0,1,2,3)"
        );
    }

    // ===== JSON ラウンドトリップ検証(エンジン産出 outcome) =====
    //
    // 既存 `loop_outcome_is_json_serializable`(L2423) は hand-built な LoopOutcome を
    // 使っており、serde の形状(reason snake_case / progress_report ネスト / reached_goal)
    // が JSON 経路で保たれることを検証している。一方で「エンジン経路(run_loop_with_goal)
    // を通じて実際に populate された reached_goal を持つ outcome」が JSON ラウンドトリップ
    // を生き延びるかは別の保証になる: evaluate_goal → goal_descriptor が埋めた文字列が
    // シリアライズ→デシリアライズ後も完全に一致することを、UC-1 と UC-2 の両方の
    // ゴール種別で検証する(UC-3 Timeout は本チケットのスコープ外: #42 の未達 AC は
    // UC-1/UC-2 の JSON 耐性で明示されているため)。
    // Issue #42 / 親 #37 Shard 3: JSON 成果レポート検証。

    /// UC-1(LoopCount) エンジン産出 outcome が JSON ラウンドトリップを生き延びること。
    /// run_loop_with_goal で実際に GoalReached まで駆動し、`reached_goal == "loop_count=3"`
    /// が populate された outcome を to_string → from_str し、progress_report.reached_goal
    /// が完全一致で生き残ることを assert する。
    #[tokio::test]
    async fn uc1_loop_count_outcome_reached_goal_survives_json_roundtrip() {
        let (mut driver, _fired) = build_nomatch_driver();

        let goal = anaden_core::Goal {
            name: "farm3".into(),
            stop: anaden_core::StopCondition::LoopCount { target: 3 },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 10, 0, None, Some(&goal), clock)
            .await;

        // 前提: エンジン経路で実際に到達し、reached_goal が populate されていること。
        assert_eq!(outcome.reason, LoopStopReason::GoalReached);
        let expected_descriptor = outcome
            .progress_report
            .reached_goal
            .as_deref()
            .expect("reached_goal must be populated by engine for UC-1 LoopCount");
        assert_eq!(expected_descriptor, "loop_count=3");

        // JSON ラウンドトリップ: to_string → from_str。
        let s = serde_json::to_string(&outcome).expect("serialize json");
        let back: LoopOutcome = serde_json::from_str(&s).expect("deserialize json");

        // progress_report.reached_goal がラウンドトリップを生き延びる(本テストの核心)。
        assert_eq!(
            back.progress_report.reached_goal.as_deref(),
            Some("loop_count=3"),
            "UC-1 reached_goal descriptor must survive JSON round-trip; json={s}"
        );
        // reason も snake_case 経路で往復できること(対称性担保)。
        assert_eq!(back.reason, LoopStopReason::GoalReached);
        // iterations も保持されること(progress_report と top-level の両方)。
        assert_eq!(back.iterations, 3);
        assert_eq!(back.progress_report.iterations, 3);
    }

    /// UC-2(TemplateMatch) エンジン産出 outcome が JSON ラウンドトリップを生き延びること。
    /// needle 埋込画面で Title タスクをマッチ→発火させ、TemplateMatch ゴール到達で
    /// populate された reached_goal(記述子は task と confidence を含む) が to_string → from_str
    /// 後も完全一致で生き残ることを assert する。UC-1 と記述子フォーマットが異なるため
    /// 両方を網羅する(template_match conf>=<c> (task=<t>) 形式の JSON 耐性)。
    #[tokio::test]
    async fn uc2_template_match_outcome_reached_goal_survives_json_roundtrip() {
        let (matched, _tpl) = matched_frame();
        let frames = frames_of(vec![matched]);
        let fired = new_fired();

        let task = click_rect_task(
            "Title",
            Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            },
            Some(vec!["LoadGame"]),
        );

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task],
            2400,
            300,
        );

        let goal = anaden_core::Goal {
            name: "find_title".into(),
            stop: anaden_core::StopCondition::TemplateMatch {
                task: "Title".into(),
                confidence: 0.85,
            },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 10, 0, None, Some(&goal), clock)
            .await;

        // 前提: エンジン経路で TemplateMatch ゴール到達し、reached_goal が populate されていること。
        assert_eq!(outcome.reason, LoopStopReason::GoalReached);
        let expected_descriptor = outcome
            .progress_report
            .reached_goal
            .as_deref()
            .expect("reached_goal must be populated by engine for UC-2 TemplateMatch");
        assert_eq!(
            expected_descriptor,
            "template_match conf>=0.85 (task=Title)"
        );

        // JSON ラウンドトリップ: to_string → from_str。
        let s = serde_json::to_string(&outcome).expect("serialize json");
        let back: LoopOutcome = serde_json::from_str(&s).expect("deserialize json");

        // progress_report.reached_goal がラウンドトリップを生き延びる(本テストの核心)。
        // 記述子は空白・記号(>=, ()) を含むため、JSON 文字列 escaping も耐性検証に含まれる。
        assert_eq!(
            back.progress_report.reached_goal.as_deref(),
            Some("template_match conf>=0.85 (task=Title)"),
            "UC-2 reached_goal descriptor must survive JSON round-trip; json={s}"
        );
        // reason も snake_case 経路で往復できること(対称性担保)。
        assert_eq!(back.reason, LoopStopReason::GoalReached);
        // 発火履歴も形状を保って往復できること(fired_commands が空でない)。
        assert!(
            !back.fired_commands.is_empty(),
            "fired_commands must survive JSON round-trip non-empty"
        );
    }

    // ===== ガード1: CancellationToken 対応(Ctrl+C / SIGINT) =====
    //
    // PipelineDriver::with_cancel(token) で外部キャンセル口を設定すると、各ループが
    // (a) サイクル冒頭で is_cancelled() を判定して LoopStopReason::Interrupted で停止、
    // (b) 末尾の interval 待ちを tokio::select! で cancel 即時抜けする。
    // with_cancel を呼ばない(デフォルト)ときは cancel=None で従来動作(後方互換)。
    // main.rs の呼出元シグネチャを変えないビルダーパターンで実装されているため、
    // 既存の全 run_loop_* テストは無変更で通過する。

    /// cancel 済み token を with_cancel して run_loop を呼ぶと、最初のサイクル冒頭で
    /// 即座に Interrupted 停止する(1 tick も消費しない)。
    #[tokio::test]
    async fn cancel_pre_cancelled_token_stops_immediately_as_interrupted() {
        let (mut driver, _fired) = build_nomatch_driver();
        let token = CancellationToken::new();
        token.cancel();
        driver = driver.with_cancel(token);

        let outcome = driver.run_loop(Duration::from_secs(60), 100).await;
        assert_eq!(
            outcome.reason,
            LoopStopReason::Interrupted,
            "pre-cancelled token must stop with Interrupted"
        );
        assert_eq!(outcome.terminal, "interrupted");
        // 冒頭判定で抜けるので 0 tick(発火も無し)。
        assert_eq!(outcome.iterations, 0);
        assert!(outcome.fired_commands.is_empty());
    }

    /// sleep 中に別タスクから cancel すると、select! が即座に抜けて次サイクル冒頭で
    /// Interrupted 停止する。interval を長め(100ms)にして sleep 区間内で cancel させる。
    #[tokio::test]
    async fn cancel_during_interval_sleep_stops_as_interrupted() {
        let (mut driver, _fired) = build_nomatch_driver();
        let token = CancellationToken::new();
        let cancel_token = token.clone();
        driver = driver.with_cancel(token);

        // 50ms 後に別タスクで cancel(初回サイクルは sleep 100ms 中に入る)。
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_token.cancel();
        });

        let outcome = driver.run_loop(Duration::from_millis(100), 1000).await;
        assert_eq!(
            outcome.reason,
            LoopStopReason::Interrupted,
            "cancel during sleep must stop with Interrupted, got {:?}",
            outcome.reason
        );
        // 最低1サイクルは回っている(冒頭判定は最初は false のため)。
        assert!(
            outcome.iterations >= 1,
            "must have run at least one cycle before cancel"
        );
    }

    /// with_cancel を呼ばない(デフォルト cancel=None)ときは従来動作。NoMatch 持続で
    /// MaxIterations になり、Interrupted にはならない(後方互換)。
    #[tokio::test]
    async fn cancel_none_default_preserves_max_iterations_behavior() {
        let (mut driver, _fired) = build_nomatch_driver();
        // with_cancel を呼ばない = デフォルト cancel = None。
        let outcome = driver.run_loop(Duration::ZERO, 5).await;
        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "default (no cancel token) must preserve MaxIterations behavior"
        );
        assert_ne!(
            outcome.reason,
            LoopStopReason::Interrupted,
            "Interrupted must not fire without a cancel token"
        );
        assert_eq!(outcome.iterations, 5);
    }

    /// run_loop_with_goal でも with_cancel が効く: cancel 済み token で即時 Interrupted。
    /// (ゴール評価経路でも cancel 判定がループ冒頭に入っていることの回帰保護。)
    #[tokio::test]
    async fn cancel_with_goal_path_stops_as_interrupted() {
        let (mut driver, _fired) = build_nomatch_driver();
        let token = CancellationToken::new();
        token.cancel();
        driver = driver.with_cancel(token);

        let goal = anaden_core::Goal {
            name: "g".into(),
            stop: anaden_core::StopCondition::LoopCount { target: 100 },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 1000, 0, None, Some(&goal), clock)
            .await;
        assert_eq!(
            outcome.reason,
            LoopStopReason::Interrupted,
            "cancel must take precedence on goal path too"
        );
        assert_eq!(outcome.iterations, 0);
    }

    // ===== ガード4: NoMatch/FiredUnverified スナップショット保存 =====
    //
    // ANADEN_SNAPSHOT_DIR 設定時に NoMatch 発生で normalized フレームが PNG 保存され、
    // 未設定時は no-op(ファイルが1つも作られない)ことを検証する。
    // env 操作はプロセス全体へ伝播するため、set 後に必ず unset して他テストへ
    // 影響を与えない(並列実行の nextest でも自己完結)。

    /// NoMatch 発生時、ANADEN_SNAPSHOT_DIR 設定ディレクトリへ PNG が保存される。
    /// run_once を1回呼び(needle 無し画像 → NoMatch)、dir 内に .png が1つ以上できること。
    #[tokio::test]
    async fn snapshot_saved_on_no_match_when_dir_set() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // env 設定。edition 2024 で set_var/remove_var は unsafe。env はプロセス全域へ
        // 伝播するため、他テストとの並列競合を避けるよう本テストは set→検証→unset を
        // 自己完結させる(Drop guard で確実に unset)。
        unsafe {
            std::env::set_var("ANADEN_SNAPSHOT_DIR", tmp.path());
        }
        // RAII: テスト本体が panic しても env を必ず解除する。
        struct EnvUnset;
        impl Drop for EnvUnset {
            fn drop(&mut self) {
                unsafe {
                    std::env::remove_var("ANADEN_SNAPSHOT_DIR");
                }
            }
        }
        let _unset = EnvUnset;

        let screen = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let frames = frames_of(vec![screen]);
        let fired = new_fired();
        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )],
            2400,
            300,
        );
        let out = driver.run_once().await;
        assert_eq!(out, StepOutcome::NoMatch);

        // dir 内に PNG が1つ以上保存されていること。
        let entries: Vec<_> = std::fs::read_dir(tmp.path())
            .expect("read_dir")
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("png"))
                    .unwrap_or(false)
            })
            .collect();
        assert!(
            !entries.is_empty(),
            "ANADEN_SNAPSHOT_DIR set + NoMatch must save at least one PNG"
        );
        // ファイル名にタスク名 "Title" が含まれること(cause 追跡用)。
        let has_task = entries.iter().any(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with("Title_"))
                .unwrap_or(false)
        });
        assert!(has_task, "snapshot filename must start with task name");
    }

    /// ANADEN_SNAPSHOT_DIR 未設定時は NoMatch でも保存されない(no-op)。
    #[tokio::test]
    async fn snapshot_not_saved_when_dir_unset() {
        // 未設定状態を保証(他テストの set 残りを排除)。
        unsafe {
            std::env::remove_var("ANADEN_SNAPSHOT_DIR");
        }

        let screen = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let frames = frames_of(vec![screen]);
        let fired = new_fired();
        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )],
            2400,
            300,
        );
        let out = driver.run_once().await;
        assert_eq!(out, StepOutcome::NoMatch);

        // snapshot_dir() が None → 保存経路に入っていないことの直接検証。
        assert!(
            PipelineDriver::<FakeCapture, FakeInput>::snapshot_dir().is_none(),
            "snapshot_dir must be None when ANADEN_SNAPSHOT_DIR unset"
        );
        // カウンタもインクリメントされていない(save_snapshot 早期 return)。
        assert_eq!(
            driver.snapshot_counter, 0,
            "counter must not increment when dir unset"
        );
        // 診断カウンタも未インクリメント(ANADEN_DIAG_REPORT 未設定のため no-op)。
        assert_eq!(driver.diag_counter, 0);
    }

    // ---- (C2) ClickRect 発火テスト: raw-1258 実値化 + 黒帯オフセット検証 ----
    //
    // 旧 run_once_fires_rescaled_tap_on_click_rect は ClickRect の roi を「1280基準座標」として
    // 扱い、raw-1258 実値（TapToStartPc の [900,466,30,30] 等）で検証していなかった（silent 無効化）。
    // ここでは実 TOML 定義と同じ raw-1258 空間の roi を与え、新スケールモデルで正しい実機座標へ
    // 変換されることを検証する。さらに黒帯入りフレームで ClickSelf 発火座標に黒帯オフセットが
    // 反映されることを検証する（M1）。

    /// `matched_frame()` のコンテンツ（raw-1258x708 特徴画像）の周囲に黒帯を加えた画像を返す。
    /// `bar_x`/`bar_y` は左右/上下の黒帯ピクセル幅。コンテンツは (bar_x, bar_y) に配置。
    fn matched_frame_with_bars(bar_x: u32, bar_y: u32) -> DynamicImage {
        let raw = raw_feature();
        let full_w = FULL_W + 2 * bar_x;
        let full_h = FULL_H + 2 * bar_y;
        let mut img = GrayImage::from_pixel(full_w, full_h, Luma([0u8]));
        for y in 0..FULL_H {
            for x in 0..FULL_W {
                let p = *raw.get_pixel(x, y);
                img.put_pixel(bar_x + x, bar_y + y, p);
            }
        }
        luma_dyn(img)
    }

    #[tokio::test]
    async fn click_rect_fires_at_expected_device_coords_on_raw_1258_roi() {
        // TapToStartPc 相当の ClickRect roi [900,466,30,30]（raw-1258 空間の実 TOML 値）。
        // フレーム raw-1258x708（黒帯なし）、device_width=1258（フレームと一致）。
        //
        // 期待発火座標の導出:
        //  1. roi → screenshot と同じ空間へ: roi_to_normalized([900,466,30,30],1280,720)
        //     = [916,474,31,31]（1258→1280 は拡大、高さ 708→720 も拡大）。
        //     中心 = (916+15, 474+15) = (931, 489)。
        //  2. rescale(crop_info=full(1258,708), device_width=1258): s=1258/1280=0.9828。
        //     (931*0.9828, 489*0.9828) = (915, 481)。
        let (screen, _tpl) = matched_frame();
        let frames = frames_of(vec![screen]);
        let fired = new_fired();

        let task = TaskDef {
            name: "TapToStart".into(),
            state: "TapToStart".into(),
            algorithm: anaden_vision::Algorithm::Ccoeff,
            template: crop_needle_template(),
            roi: Some(MATCH_ROI),
            threshold: 0.9,
            base: None,
            action: Some(Action::ClickRect {
                roi: ScreenRegion::new(900, 466, 30, 30),
            }),
            next: Some(vec!["LoadGamePc".into()]),
        };

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames: frames.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("TapToStart"),
            vec![task],
            1258, // device_width（フレーム raw-1258 と一致）
            300,
        );

        let out = driver.run_once().await;
        match out {
            StepOutcome::Fired {
                fired: just_fired, ..
            } => {
                // raw-1258 実値 roi が正しい実機座標 (915,481) へ変換されること。
                assert_eq!(
                    just_fired,
                    Some(InputCommand::Tap { x: 915, y: 481 }),
                    "raw-1258 roi [900,466,30,30] は (915,481) へ変換されるべき"
                );
            }
            other => panic!("expected Fired, got {other:?}"),
        }
        assert_eq!(
            fired.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: 915, y: 481 }]
        );
    }

    #[tokio::test]
    async fn click_self_fires_with_letterbox_offset_on_bars() {
        // M1 検証: 黒帯入りフレームで ClickSelf 発火座標に黒帯オフセットが反映されること。
        // 黒帯なしフレームでの発火座標と、黒帯入りフレームでの発火座標の差が、
        // 黒帯オフセット（コンテンツ左上の元画像座標）に一致することを検証する。
        //
        // matched_frame() の needle は NEEDLE_ROI=[150,75,12,12]（raw-1258）。黒帯入りでは
        // コンテンツを (bar_x,bar_y) に配置するため、crop でコンテンツだけ抜けば detect 結果
        // （matched_region）は黒帯なしと同空間になる。よって InputCommand の1280空間座標は両者で
        // 同じ。差は rescale 時の crop_info.offset のみから生じる。
        let bar_x = 100u32;
        let bar_y = 50u32;

        // 黒帯なし: matched_frame() は raw-1258x708。crop_info=full(1258,708), offset=(0,0)。
        let (screen_nobar, _tpl) = matched_frame();
        let frames_nobar = frames_of(vec![screen_nobar]);
        let fired_nobar = new_fired();
        let task_nobar = TaskDef {
            name: "Title".into(),
            state: "Title".into(),
            algorithm: anaden_vision::Algorithm::Ccoeff,
            template: crop_needle_template(),
            roi: Some(MATCH_ROI),
            threshold: 0.9,
            base: None,
            action: Some(Action::ClickSelf),
            next: Some(vec!["LoadGame".into()]),
        };
        let mut driver_nobar = PipelineDriver::new(
            FakeCapture {
                frames: frames_nobar.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired_nobar.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task_nobar],
            1458, // device_width（黒帯入り全体幅 1258+2*100 と同じ値を使うが、黒帯なしなので影響しない）
            300,
        );
        let out_nobar = driver_nobar.run_once().await;
        let fired_nobar_cmd = match out_nobar {
            StepOutcome::Fired { fired: c, .. } => c,
            other => panic!("expected Fired (no-bar), got {other:?}"),
        };

        // 黒帯入り: コンテンツ1258x708 を (100,50) に配置 → 全体 1458x808。
        // crop_to_content が黒帯を除去し CropInfo{offset:(100,50), size:(1258,708)} を返す。
        let screen_bar = matched_frame_with_bars(bar_x, bar_y);
        let frames_bar = frames_of(vec![screen_bar]);
        let fired_bar = new_fired();
        let task_bar = TaskDef {
            name: "Title".into(),
            state: "Title".into(),
            algorithm: anaden_vision::Algorithm::Ccoeff,
            template: crop_needle_template(),
            roi: Some(MATCH_ROI),
            threshold: 0.9,
            base: None,
            action: Some(Action::ClickSelf),
            next: Some(vec!["LoadGame".into()]),
        };
        let mut driver_bar = PipelineDriver::new(
            FakeCapture {
                frames: frames_bar.clone(),
                fail: false,
            },
            FakeInput {
                fired: fired_bar.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![task_bar],
            1458, // device_width（黒帯入り全体幅と一致）
            300,
        );
        let out_bar = driver_bar.run_once().await;
        let fired_bar_cmd = match out_bar {
            StepOutcome::Fired { fired: c, .. } => c,
            other => panic!("expected Fired (with-bar), got {other:?}"),
        };

        // 両者とも発火していること。
        let nobar = fired_nobar_cmd.expect("no-bar must fire");
        let bar = fired_bar_cmd.expect("with-bar must fire");
        let (nx, ny) = match nobar {
            InputCommand::Tap { x, y } => (x, y),
            other => panic!("expected Tap (no-bar), got {other:?}"),
        };
        let (bx, by) = match bar {
            InputCommand::Tap { x, y } => (x, y),
            other => panic!("expected Tap (with-bar), got {other:?}"),
        };
        // 黒帯入りフレームではコンテンツが (bar_x,bar_y) に配置されるため、rescale 時の
        // CropInfo.offset が (bar_x,bar_y) となり発火座標へ平行移動分が加算される。
        // matched_region 自体は detect の丸めで両者で数px 異なりうるため、差が「オフセット方向に
        // 有意（bar の半分以上）」であることを検証する（厳密なオフセット一致は純関数
        // rescale_tap_with_letterbox_offset_shifts_coordinates で検証済み）。
        let dx = bx as i64 - nx as i64;
        let dy = by as i64 - ny as i64;
        assert!(
            dx >= (bar_x as i64) / 2,
            "黒帯入りX座標({bx}) - 黒帯なし({nx}) = {dx} は bar_x={bar_x} の半分以上であるべき \
            （黒帯オフセットが rescale に反映された証拠）"
        );
        assert!(
            dy >= (bar_y as i64) / 2,
            "黒帯入りY座標({by}) - 黒帯なし({ny}) = {dy} は bar_y={bar_y} の半分以上であるべき \
            （黒帯オフセットが rescale に反映された証拠）"
        );
        // fake input にも反映されている。
        assert_eq!(
            fired_bar.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: bx, y: by }]
        );
    }

    // ===== Issue #71: NoMatch 診断モード(フレームダンプ + conf 内訳レポート) =====
    //
    // ANADEN_DIAG_REPORT 設定 + NoMatch 発生で diag_<task>_<n>.png と .md が保存され、
    // 未設定時はファイル IO ゼロ(後方互換)。
    // env 操作はプロセス全体へ伝播するため、set→検証→unset を Drop guard で自己完結させる
    // (nextest 並列実行でも他テストへ影響しない)。

    /// NoMatch フレーム用の diagnostic テストドライバを構築する共通ヘルパ。
    /// フレームは灰色一色 = 全テンプレ NoMatch。
    fn diag_no_match_driver(
        frames: Arc<Mutex<VecDeque<DynamicImage>>>,
        tasks: Vec<TaskDef>,
    ) -> PipelineDriver<FakeCapture, FakeInput> {
        let fired = new_fired();
        PipelineDriver::new(
            FakeCapture {
                frames,
                fail: false,
            },
            FakeInput { fired, fail: false },
            PipelineState::new("Title"),
            tasks,
            2400,
            300,
        )
    }

    /// (a) NoMatch フレームでダンプ + レポート両方が指定 dir に生成される。
    ///
    /// ゲーティング設計: `ANADEN_SNAPSHOT_DIR` 指定 dir に save_snapshot の PNG
    /// ダンプと診断 MD レポートの両方が並ぶ(SNAPSHOT_DIR 優先・二重 PNG 保存なし)。
    #[tokio::test]
    async fn diag_report_and_frame_dump_saved_on_no_match_when_env_set() {
        let tmp = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::remove_var("ANADEN_DIAG_REPORT");
            std::env::set_var("ANADEN_SNAPSHOT_DIR", tmp.path());
        }
        struct EnvUnset;
        impl Drop for EnvUnset {
            fn drop(&mut self) {
                unsafe {
                    std::env::remove_var("ANADEN_SNAPSHOT_DIR");
                }
            }
        }
        let _unset = EnvUnset;

        // needle なし灰色フレーム → click_rect_task の needle は埋め込まれていないので NoMatch。
        let screen = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let mut driver = diag_no_match_driver(
            frames_of(vec![screen]),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )],
        );

        let out = driver.run_once().await;
        assert_eq!(out, StepOutcome::NoMatch);

        // PNG ダンプ + MD レポートの両方が生成されていること。
        let png_count = count_entries(tmp.path(), "png");
        let md_count = count_entries(tmp.path(), "md");
        assert!(png_count >= 1, "frame dump (png) must be saved");
        assert!(md_count >= 1, "diagnose report (md) must be saved");

        // レポート本文検証: ヘッダ + タスク行。
        let md_body = read_first_matching(tmp.path(), "md").expect("report file readable");
        assert!(md_body.contains("# NoMatch Diagnose Report"));
        assert!(md_body.contains("Title"), "report must list template row");
    }

    /// (b) レポート内訳が diagnose_all の降順と一致する。
    ///
    /// 2テンプレ(NeedleA 埋込・NeedleB 非埋込)で NoMatch 起こさないとレポートが
    /// 出ないため、カレントタスク「Title」を非埋込にして NoMatch を発生させつつ、
    /// 同フレームに対する `diagnose_all` の期待順序とレポート行順を突き合わせる。
    #[tokio::test]
    async fn diag_report_row_order_matches_diagnose_all_descending() {
        let tmp = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::remove_var("ANADEN_DIAG_REPORT");
            std::env::set_var("ANADEN_SNAPSHOT_DIR", tmp.path());
        }
        struct EnvUnset;
        impl Drop for EnvUnset {
            fn drop(&mut self) {
                unsafe {
                    std::env::remove_var("ANADEN_SNAPSHOT_DIR");
                }
            }
        }
        let _unset = EnvUnset;

        // raw_feature から2種の needle を作る。NeedleB はフレームに埋め込むが
        // threshold を 1.1(到達不能)にして NoMatch を発生させる。
        let raw = raw_feature();
        let tpl_path = crop_needle_template();

        let mk_task = |name: &str, thr: f32| TaskDef {
            name: name.into(),
            state: name.into(),
            algorithm: anaden_vision::Algorithm::Ccoeff,
            template: tpl_path.clone(),
            roi: Some(MATCH_ROI),
            threshold: thr,
            base: None,
            action: Some(Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            }),
            next: Some(vec!["LoadGame".into()]),
        };
        // Title: needle はフレーム内に存在するが threshold=1.1 で到達不能 → NoMatch。
        // Other: 同一 needle・threshold=0.5 ならマッチする(内訳では上位)。
        let tasks = vec![mk_task("Title", 1.1), mk_task("Other", 1.1)];

        let screen = luma_dyn(raw.clone());
        let mut driver = diag_no_match_driver(frames_of(vec![screen.clone()]), tasks.clone());
        let out = driver.run_once().await;
        assert_eq!(out, StepOutcome::NoMatch);

        // 期待値: run_once と同じ前処理(normalize_capture)を通した
        // フレームに対する diagnose_all の順序。
        let (normalized, _) = ScreenScaler::new().normalize_capture(&screen);
        let expected = anaden_vision::diagnose_all(&tasks, &normalized);
        assert_eq!(expected.len(), 2);
        let expected_tasks: Vec<&str> = expected.iter().map(|e| e.task.as_str()).collect();

        let md_body = read_first_matching(tmp.path(), "md").expect("report file readable");
        let pos = |name: &str| {
            md_body
                .find(&format!("| {name} |"))
                .expect("task row present")
        };
        let idx = |name: &str| {
            md_body
                .lines()
                .position(|l| l.contains(&format!("| {name} |")))
                .expect("task row present")
        };
        // レポート行順が diagnose_all の降順と一致。
        assert!(
            idx(expected_tasks[0]) < idx(expected_tasks[1]),
            "report rows must follow diagnose_all descending order: {expected_tasks:?}"
        );
        // 両タスク行が存在すること(内訳に全テンプレが載る)。
        let _ = pos(expected_tasks[0]);
        let _ = pos(expected_tasks[1]);
    }

    /// (c) 空テンプレ/needle 過大でレポートが skipped 行を出し panic しない。
    #[tokio::test]
    async fn diag_report_skips_broken_template_without_panic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        unsafe {
            std::env::remove_var("ANADEN_DIAG_REPORT");
            std::env::set_var("ANADEN_SNAPSHOT_DIR", tmp.path());
        }
        struct EnvUnset;
        impl Drop for EnvUnset {
            fn drop(&mut self) {
                unsafe {
                    std::env::remove_var("ANADEN_SNAPSHOT_DIR");
                }
            }
        }
        let _unset = EnvUnset;

        // 空テンプレート(0バイト = 読込失敗)と needle 過大(フレーム全面サイズ)の2タスク。
        let empty_tpl = tmp.path().join("empty.png");
        std::fs::write(&empty_tpl, b"").expect("write empty template");
        let big_tpl = {
            let p = tmp.path().join("big.png");
            GrayImage::from_pixel(FULL_W + 100, FULL_H + 100, Luma([200u8]))
                .save(&p)
                .expect("save big needle");
            p
        };
        let mk_task = |name: &str, tpl: PathBuf| TaskDef {
            name: name.into(),
            state: name.into(),
            algorithm: anaden_vision::Algorithm::Ccoeff,
            template: tpl,
            roi: None,
            threshold: 0.9,
            base: None,
            action: Some(Action::ClickRect {
                roi: ScreenRegion::new(520, 320, 240, 80),
            }),
            next: Some(vec!["LoadGame".into()]),
        };
        let tasks = vec![mk_task("Broken", empty_tpl), mk_task("Oversized", big_tpl)];

        let screen = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let mut driver = diag_no_match_driver(frames_of(vec![screen]), tasks);

        // panic せず NoMatch を返すこと。
        let out = driver.run_once().await;
        assert_eq!(out, StepOutcome::NoMatch);

        // レポートに skipped 行(両タスク)と理由が出ること。
        let md_body = read_first_matching(tmp.path(), "md").expect("report file readable");
        assert!(md_body.contains("Broken"), "broken template row");
        assert!(md_body.contains("Oversized"), "oversized needle row");
        assert!(
            md_body.contains("template load failed"),
            "skip reason for broken template"
        );
    }

    /// (d) 環境変数未設定時はファイル IO ゼロ(既存後方互換)。
    #[tokio::test]
    async fn diag_report_not_written_when_env_unset() {
        // 未設定状態を保証(他テストの set 残りを排除)。
        unsafe {
            std::env::remove_var("ANADEN_DIAG_REPORT");
            std::env::remove_var("ANADEN_SNAPSHOT_DIR");
        }

        let tmp = tempfile::tempdir().expect("tempdir");
        let screen = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let mut driver = diag_no_match_driver(
            frames_of(vec![screen]),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                Some(vec!["LoadGame"]),
            )],
        );

        let out = driver.run_once().await;
        assert_eq!(out, StepOutcome::NoMatch);

        // 診断ファイルが1つも作られないこと。ANADEN_SNAPSHOT_DIR も同時に未設定保証
        // (snapshot 保存との混入を防ぐ)。
        unsafe {
            std::env::remove_var("ANADEN_SNAPSHOT_DIR");
        }
        let entries: Vec<_> = std::fs::read_dir(tmp.path())
            .expect("read_dir")
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            entries.is_empty(),
            "no file IO when ANADEN_DIAG_REPORT unset: {:?}",
            entries.iter().map(|e| e.file_name()).collect::<Vec<_>>()
        );
        assert_eq!(driver.diag_counter, 0, "counter must not increment");
    }

    /// dir 内の指定拡張子ファイル数を数えるテストヘルパ。
    fn count_entries(dir: &Path, ext: &str) -> usize {
        std::fs::read_dir(dir)
            .expect("read_dir")
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case(ext))
                    .unwrap_or(false)
            })
            .count()
    }

    /// dir 内の指定拡張子ファイルを1つ読んで内容を返すテストヘルパ。
    fn read_first_matching(dir: &Path, ext: &str) -> Option<String> {
        std::fs::read_dir(dir)
            .expect("read_dir")
            .filter_map(|e| e.ok())
            .find(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case(ext))
                    .unwrap_or(false)
            })
            .and_then(|e| std::fs::read_to_string(e.path()).ok())
    }

    // ===== Issue #253: ジッタ (tap / interval) と Issue #260: verify スキーマ =====

    /// 1 fire で終端する driver (next 空 = TerminalTask) を構築。
    /// frames は呼出側で制御 (検証フレームを含めるかで passed/failed が変わる)。
    fn build_single_fire_driver(
        frames: Vec<DynamicImage>,
    ) -> (
        PipelineDriver<FakeCapture, FakeInput>,
        Arc<Mutex<Vec<InputCommand>>>,
    ) {
        let fired = new_fired();
        let driver = PipelineDriver::new(
            FakeCapture {
                frames: frames_of(frames),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(140, 65, 48, 36),
                },
                Some(vec![]), // next 空 = 終端 → 1 fire で TerminalTask 停止
            )],
            FULL_W,
            300,
        );
        (driver, fired)
    }

    /// ジッタ有効時、発火タップ座標は非ジッタ時の中心から radius 内 (デバイス空間で
    /// ±3px ≒ normalize 空間 ±3px × 1258/1280) に収まり、かつ少なくとも一度は動く。
    /// シードは ProgressReport.jitter_seed に記録される。
    #[tokio::test]
    async fn jitter_shifts_fired_tap_within_radius_and_records_seed() {
        let raw = luma_dyn(raw_feature());
        // ベースライン (ジッタ無効): 中心固定タップ。
        let (mut plain, plain_fired) = build_single_fire_driver(vec![raw.clone(), raw.clone()]);
        let plain_outcome = plain.run_loop(Duration::ZERO, 5).await;
        let (px, py) = match plain_fired.lock().expect("lock").as_slice() {
            [InputCommand::Tap { x, y }, ..] => (*x, *y),
            other => panic!("expected tap, got {other:?}"),
        };
        assert!(
            plain_outcome.progress_report.jitter_seed.is_none(),
            "jitter disabled run must not record a seed"
        );

        // ジッタ有効 (固定シード): 同一フレーム列で座標が radius 内で動く。
        let seed = 20261102u64;
        let (mut jit, jit_fired) = build_single_fire_driver(vec![raw.clone(), raw]);
        jit = jit.with_jitter(Some(Jitter::new(seed)));
        let outcome = jit.run_loop(Duration::ZERO, 5).await;
        let fired = jit_fired.lock().expect("lock").clone();
        assert_eq!(fired.len(), 1, "single-fire driver must fire exactly once");
        let InputCommand::Tap { x, y } = fired[0] else {
            panic!("expected tap");
        };
        let dx = (x as i64 - i64::from(px)).abs();
        let dy = (y as i64 - i64::from(py)).abs();
        assert!(
            dx <= 3,
            "x jitter {dx} exceeds radius (plain={px}, got={x})"
        );
        assert!(
            dy <= 3,
            "y jitter {dy} exceeds radius (plain={py}, got={y})"
        );
        assert!(
            (dx, dy) != (0, 0),
            "jitter must displace the tap at least once for seed {seed}"
        );
        assert_eq!(
            outcome.progress_report.jitter_seed,
            Some(seed),
            "seed must be recorded in progress_report"
        );
    }

    /// 検証成功 (発火後フレームで対象消失): verify { checks:1, passed:1, failed:0 }。
    #[tokio::test]
    async fn verify_report_counts_pass_when_target_disappears() {
        let raw = luma_dyn(raw_feature());
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let (mut d, _) = build_single_fire_driver(vec![raw, blank]);
        d = d.with_verify(true);
        let outcome = d.run_loop(Duration::ZERO, 5).await;
        let v = &outcome.progress_report.verify;
        assert!(v.enabled, "verify must be reported as enabled");
        assert_eq!(v.checks, 1, "exactly one post-fire check");
        assert_eq!(v.passed, 1);
        assert_eq!(v.failed, 0);
        assert_eq!(
            v.checks,
            v.passed + v.failed,
            "invariant checks==passed+failed"
        );
    }

    /// 検証失敗 (発火後も対象残存 = 誤成功): verify { checks:1, passed:0, failed:1 }。
    /// FiredUnverified はループ継続するため、以降フレーム枯渇で CaptureError 停止も許容。
    #[tokio::test]
    async fn verify_report_counts_fail_when_target_persists() {
        let raw = luma_dyn(raw_feature());
        let (mut d, _) = build_single_fire_driver(vec![raw.clone(), raw]);
        d = d.with_verify(true);
        let outcome = d.run_loop(Duration::ZERO, 5).await;
        let v = &outcome.progress_report.verify;
        assert_eq!(v.checks, 1, "exactly one post-fire check");
        assert_eq!(v.passed, 0);
        assert_eq!(
            v.failed, 1,
            "persistent target must count as verify failure"
        );
    }

    /// 検証無効 run: verify.enabled == false で計数 0 (後方互換)。
    #[tokio::test]
    async fn verify_report_disabled_when_verify_off() {
        let raw = luma_dyn(raw_feature());
        let (mut d, _) = build_single_fire_driver(vec![raw.clone(), raw]);
        let outcome = d.run_loop(Duration::ZERO, 5).await;
        let v = &outcome.progress_report.verify;
        assert!(!v.enabled);
        assert_eq!((v.checks, v.passed, v.failed), (0, 0, 0));
    }

    /// 旧 report.json (jitter_seed / verify フィールド無し) は default 補完で読める
    /// (03-report-channel の後方互換契約 — 旧成果物の再消費を壊さない)。
    #[test]
    fn progress_report_deserializes_legacy_json_without_new_fields() {
        let legacy = serde_json::json!({
            "iterations": 3,
            "fired_count": 1,
            "per_task_matches": [],
            "elapsed_ms": 500,
            "terminal_task": null,
            "reached_goal": null
        });
        let pr: ProgressReport = serde_json::from_value(legacy).expect("legacy report must parse");
        assert_eq!(pr.iterations, 3);
        assert!(pr.jitter_seed.is_none());
        assert_eq!(pr.verify, VerifyReport::default());
    }

    // ===== Issue #252: watch (常時監視 interrupt) タスク機構 =====
    //
    // Title (本流) と Popup (watch) の 2 テンプレを raw_feature 上の別 ROI に置き、
    // フレーム側の該当領域を平塗りで消すことで「どちらだけがマッチする画面か」を
    // 作り分ける (run_loop を回すテストで両者の優先順位と current 非遷移を検証)。

    /// watch 用テンプレの raw-1258 空間 ROI (Title 用 NEEDLE_ROI と離れた位置)。
    const POPUP_NEEDLE_ROI: [u32; 4] = [400, 300, 12, 12];
    /// POPUP_NEEDLE_ROI を含むマッチ走査小窓。
    const POPUP_MATCH_ROI: [u32; 4] = [390, 290, 48, 36];

    /// raw_feature の指定 ROI 領域を平塗り (値 128) で潰したフレームを返す。
    /// 潰された領域のテンプレはマッチしなくなる (= その「画面が写っていない」状態)。
    fn frame_with_region_masked(roi: [u32; 4]) -> DynamicImage {
        let mut raw = raw_feature();
        let [x, y, w, h] = roi;
        for dy in 0..h {
            for dx in 0..w {
                raw.put_pixel(x + dx, y + dy, Luma([128]));
            }
        }
        luma_dyn(raw)
    }

    /// 指定テンプレ/ROI の ClickRect タスクを組み立てる (click_rect_task の汎用版)。
    fn task_with_template(
        name: &str,
        template: PathBuf,
        roi: [u32; 4],
        next: Option<Vec<&str>>,
    ) -> TaskDef {
        TaskDef {
            name: name.into(),
            state: name.into(),
            algorithm: anaden_vision::Algorithm::Ccoeff,
            template,
            roi: Some(roi),
            threshold: 0.9,
            base: None,
            action: Some(Action::ClickRect {
                roi: ScreenRegion::new(roi[0], roi[1], roi[2], roi[3]),
            }),
            next: next.map(|v| v.into_iter().map(String::from).collect()),
        }
    }

    /// Title (本流・終端) + Popup (watch) の 2 タスク driver。
    fn build_watch_driver(
        frames: Vec<DynamicImage>,
    ) -> (
        PipelineDriver<FakeCapture, FakeInput>,
        Arc<Mutex<Vec<InputCommand>>>,
    ) {
        // Popup テンプレ (raw_feature の POPUP_NEEDLE_ROI 領域) を PNG 保存。
        let raw = raw_feature();
        let [px, py, pw, ph] = POPUP_NEEDLE_ROI;
        let mut needle = GrayImage::new(pw, ph);
        for dy in 0..ph {
            for dx in 0..pw {
                needle.put_pixel(dx, dy, *raw.get_pixel(px + dx, py + dy));
            }
        }
        let popup_tpl = write_template_persisted(&needle);

        let fired = new_fired();
        let driver = PipelineDriver::new(
            FakeCapture {
                frames: frames_of(frames),
                fail: false,
            },
            FakeInput {
                fired: fired.clone(),
                fail: false,
            },
            PipelineState::new("Title"),
            vec![
                task_with_template("Title", crop_needle_template(), MATCH_ROI, Some(vec![])),
                task_with_template("Popup", popup_tpl, POPUP_MATCH_ROI, None),
            ],
            FULL_W,
            300,
        );
        (driver, fired)
    }

    /// watch タスクがマッチした tick は WatchFired を返し、current は進まない
    /// (= Title は未評価のまま次サイクルへ残る)。
    #[tokio::test]
    async fn watch_match_fires_and_keeps_current() {
        // Title の走査窓全体を消したフレーム = Popup (watch) だけが写っている画面。
        // (needle 12px 部分だけの消去では不十分 — 走査窓 48x36 内の別位置に
        // 周期パターンが一致し得るため、窓全体を平塗りする。)
        let popup_only = frame_with_region_masked(MATCH_ROI);
        let (mut d, fired) = build_watch_driver(vec![popup_only.clone(), popup_only]);
        d = d.with_watch(vec!["Popup".into()]);

        let outcome = d.run_once().await;
        match &outcome {
            StepOutcome::WatchFired { task, fired: f } => {
                assert_eq!(task, "Popup");
                assert!(f.is_some(), "watch ClickRect must fire a command");
            }
            other => panic!("expected WatchFired, got {other:?}"),
        }
        assert_eq!(
            d.current(),
            "Title",
            "watch handling must not advance current"
        );
        assert_eq!(fired.lock().expect("lock").len(), 1);
    }

    /// watch と current が同じフレームでマッチする場合、watch が優先される
    /// (current の発火・遷移より先にポップアップを閉じる)。
    #[tokio::test]
    async fn watch_takes_priority_over_current_task() {
        let both = luma_dyn(raw_feature());
        let (mut d, fired) = build_watch_driver(vec![both.clone(), both]);
        d = d.with_watch(vec!["Popup".into()]);

        let outcome = d.run_once().await;
        assert!(
            matches!(outcome, StepOutcome::WatchFired { .. }),
            "watch must win over current, got {outcome:?}"
        );
        assert_eq!(d.current(), "Title");
        assert_eq!(fired.lock().expect("lock").len(), 1, "only watch fires");
    }

    /// watch 非マッチのフレームでは通常の current 評価が行われる (透過)。
    #[tokio::test]
    async fn watch_no_match_falls_through_to_current() {
        // Popup の走査窓全体を消したフレーム = Title だけが写っている画面。
        let title_only = frame_with_region_masked(POPUP_MATCH_ROI);
        let (mut d, _) = build_watch_driver(vec![title_only.clone(), title_only]);
        d = d.with_watch(vec!["Popup".into()]);

        let outcome = d.run_once().await;
        match &outcome {
            StepOutcome::Fired { next_current, .. } => {
                assert!(next_current.is_none(), "Title is terminal (empty next)");
            }
            other => panic!("expected Fired (Title terminal), got {other:?}"),
        }
    }

    /// e2e: ポップアップ割込みをはさんでも pipeline は完走する (acceptance。
    /// watch 発火は per_task_matches に計上され、fired_count に含まれる)。
    #[tokio::test]
    async fn run_loop_completes_through_watch_interruption() {
        let popup_only = frame_with_region_masked(MATCH_ROI);
        let title_only = frame_with_region_masked(POPUP_MATCH_ROI);
        let (mut d, fired) = build_watch_driver(vec![popup_only, title_only]);
        d = d.with_watch(vec!["Popup".into()]);

        let outcome = d.run_loop(Duration::ZERO, 10).await;
        assert_eq!(
            outcome.reason,
            LoopStopReason::TerminalTask,
            "pipeline must complete, got {:?}",
            outcome.reason
        );
        assert_eq!(outcome.iterations, 2, "popup tick + title tick");
        assert_eq!(
            outcome.progress_report.fired_count, 2,
            "watch fire + title fire"
        );
        // per_task_matches: Popup (watch) と Title が各 1 回。
        let popup_matches = outcome
            .progress_report
            .per_task_matches
            .iter()
            .find(|t| t.task == "Popup")
            .map(|t| t.matches);
        assert_eq!(popup_matches, Some(1), "watch task must be counted");
        assert_eq!(fired.lock().expect("lock").len(), 2);
    }

    /// watch 未設定 (既定) では watch チェックが一切行われない (後方互換):
    /// Popup だけのフレームは Title 非マッチ = NoMatch になる。
    #[tokio::test]
    async fn no_watch_config_preserves_nomatch_behavior() {
        let popup_only = frame_with_region_masked(MATCH_ROI);
        let (mut d, fired) = build_watch_driver(vec![popup_only.clone(), popup_only]);
        // with_watch を呼ばない = 既定 watch 空。

        let outcome = d.run_once().await;
        assert!(
            matches!(outcome, StepOutcome::NoMatch),
            "without watch, popup-only frame is NoMatch, got {outcome:?}"
        );
        assert_eq!(d.current(), "Title");
        assert!(fired.lock().expect("lock").is_empty());
    }
}
