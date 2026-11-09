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
use anaden_vision::{Action, BASE_WIDTH, CropInfo, ScreenScaler, TaskDef};

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
    execute_error_streak: u32,
}

const EXECUTE_ERROR_ABSORB_LIMIT: u32 = 2;

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
            execute_error_streak: 0,
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

    fn note_step_outcome(&mut self, step: &StepOutcome) {
        if !matches!(step, StepOutcome::Error { .. }) {
            self.execute_error_streak = 0;
        }
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
    matches!(
        kind,
        StepErrorKind::Capture | StepErrorKind::VerifyCapture | StepErrorKind::Execute
    )
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
        stop: bool,
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
            let stop = matches!(step.action, Action::Stop);
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
            return Some(StepOutcome::WatchFired {
                task: name,
                fired,
                stop,
            });
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
        if matches!(kind, StepErrorKind::Other) {
            return StepFlow::stop(self.build_outcome(
                state.iterations - 1,
                std::mem::take(&mut state.fired),
                LoopStopReason::ExecuteError,
                "io_error",
                state.ctx(),
            ));
        }
        if matches!(kind, StepErrorKind::Execute) {
            state.execute_error_streak = state.execute_error_streak.saturating_add(1);
            let streak = state.execute_error_streak;
            if streak < EXECUTE_ERROR_ABSORB_LIMIT {
                warn!(
                    "transient execute error (streak {streak}/{EXECUTE_ERROR_ABSORB_LIMIT}): absorbed, skipping cycle: {message}"
                );
                return StepFlow::Continue;
            }
        }
        let now = self.recovery_clock.elapsed();
        if state.guard.in_grace(now) {
            warn!("pipeline io error ({kind:?}) during boot grace (game restarting): {message}");
            StepFlow::Continue
        } else if state.recovery_enabled {
            info!("io error outside grace (environment presumed dead): {message}");
            let cause = if matches!(kind, StepErrorKind::Execute) {
                "execute error outside grace"
            } else {
                "capture error outside grace"
            };
            match self.run_recovery_or_stop(cause, state, recover).await {
                Some(stop) => StepFlow::stop(stop),
                None => StepFlow::Continue,
            }
        } else {
            let reason = if matches!(kind, StepErrorKind::Execute) {
                LoopStopReason::ExecuteError
            } else {
                LoopStopReason::CaptureError
            };
            StepFlow::stop(self.build_outcome(
                state.iterations - 1,
                std::mem::take(&mut state.fired),
                reason,
                "io_error",
                state.ctx(),
            ))
        }
    }

    async fn step_watch_fired(
        &mut self,
        state: &mut LoopState,
        goal_mode: &mut GoalMode<'_>,
        recover: &mut Option<RecoveryHook>,
        task: &str,
        fired: Option<InputCommand>,
        stop: bool,
    ) -> StepFlow {
        if stop {
            return StepFlow::stop(self.build_outcome(
                state.iterations,
                std::mem::take(&mut state.fired),
                LoopStopReason::Stop,
                &format!("watch stop: {task}"),
                state.ctx(),
            ));
        }
        let Some(cmd) = fired else {
            return self
                .step_no_match(
                    state,
                    goal_mode,
                    recover,
                    "watch observe streak >= threshold",
                    None,
                )
                .await;
        };
        state.fired.push(cmd);
        state.record_match_tick(task);
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
            state.note_step_outcome(&step);
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
                StepOutcome::WatchFired { task, fired, stop } => {
                    self.step_watch_fired(&mut state, goal_mode, recover, &task, fired, stop)
                        .await
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

    const NEEDLE_ROI: [u32; 4] = [150, 75, 12, 12];
    const MATCH_ROI: [u32; 4] = [140, 65, 48, 36];

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

    fn matched_frame() -> (DynamicImage, PathBuf) {
        let raw = raw_feature();
        let tpl = crop_needle_template();
        (luma_dyn(raw), tpl)
    }

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

    #[test]
    fn rescale_tap_no_bars_equivalent_to_from_base_2400() {
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
        let info = CropInfo::full(1280, 720);
        let out = rescale_command(InputCommand::Tap { x: 640, y: 360 }, 1280, &info);
        assert_eq!(out, InputCommand::Tap { x: 640, y: 360 });
    }

    #[test]
    fn rescale_downscale_small_device_width_no_bars() {
        let info = CropInfo::full(640, 360);
        let out = rescale_command(InputCommand::Tap { x: 640, y: 360 }, 640, &info);
        assert_eq!(out, InputCommand::Tap { x: 320, y: 180 });
    }

    #[test]
    fn rescale_tap_with_letterbox_offset_shifts_coordinates() {
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

    struct FlakyInput {
        fired: Arc<Mutex<Vec<InputCommand>>>,
        remaining_failures: AtomicU32,
    }

    #[async_trait]
    impl Input for FlakyInput {
        async fn execute(&self, action: &InputAction) -> Result<(), DeviceError> {
            let remaining = self.remaining_failures.load(Ordering::SeqCst);
            if remaining > 0 {
                self.remaining_failures
                    .store(remaining - 1, Ordering::SeqCst);
                return Err(DeviceError::CommandFailed {
                    message: "flaky execute failure".into(),
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

    fn new_fired() -> Arc<Mutex<Vec<InputCommand>>> {
        Arc::new(Mutex::new(Vec::new()))
    }

    #[tokio::test]
    async fn run_once_fires_rescaled_tap_on_click_rect() {
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
            2400,
            300,
        );

        let out = driver.run_once().await;
        match out {
            StepOutcome::Fired {
                next_current,
                fired: just_fired,
            } => {
                assert_eq!(next_current.as_deref(), Some("LoadGame"));
                assert_eq!(just_fired, Some(InputCommand::Tap { x: 640, y: 359 }));
            }
            other => panic!("expected Fired, got {other:?}"),
        }
        assert_eq!(
            fired.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: 640, y: 359 }]
        );
        assert_eq!(driver.current(), "LoadGame");
    }

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
            2400,
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

    fn vision_fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("anaden-vision")
            .join("tests")
            .join("fixtures")
            .join(name)
    }

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
            1952,
            300,
        );

        let out = driver.run_once().await;
        match out {
            StepOutcome::Fired {
                next_current,
                fired: just_fired,
            } => {
                assert_eq!(next_current.as_deref(), Some("LoginWaitFieldPc"));
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

    fn needle_screen(seed: u32) -> DynamicImage {
        let _ = seed;
        let (screen, _tpl) = matched_frame();
        screen
    }

    #[tokio::test]
    async fn run_loop_reaches_terminal_task() {
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
        assert_eq!(outcome.reason, LoopStopReason::TerminalTask);
        assert_eq!(outcome.fired_commands.len(), 3);
        for c in &outcome.fired_commands {
            assert_eq!(*c, InputCommand::Tap { x: 640, y: 359 });
        }
        assert_eq!(driver.current(), "Terminal");
    }

    #[tokio::test]
    async fn run_loop_stop_action() {
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
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
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

    #[tokio::test]
    async fn recovery_hook_fires_after_nomatch_threshold() {
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
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert!(outcome.fired_commands.is_empty());
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "recovery hook must fire at least once"
        );
    }

    #[tokio::test]
    async fn recovery_disabled_when_threshold_zero() {
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

    #[tokio::test]
    async fn transient_execute_error_is_absorbed_once() {
        let (matched, _tpl) = matched_frame();
        let frames = frames_of(vec![matched.clone(), matched]);
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames,
                fail: false,
            },
            FlakyInput {
                fired,
                remaining_failures: AtomicU32::new(1),
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                None,
            )],
            2400,
            300,
        );

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 5, 0, None)
            .await;
        assert_eq!(
            outcome.reason,
            LoopStopReason::TerminalTask,
            "1 回目の execute 失敗は吸収され、次サイクルで terminal に到達する"
        );
        assert_eq!(outcome.iterations, 2);
        assert_eq!(outcome.fired_commands.len(), 1);
    }

    #[tokio::test]
    async fn sustained_execute_errors_reach_recovery_not_immediate_stop() {
        let (matched, _tpl) = matched_frame();
        let frames = frames_of((0..8).map(|_| matched.clone()).collect());
        let fired = new_fired();

        let mut driver = PipelineDriver::new(
            FakeCapture {
                frames,
                fail: false,
            },
            FlakyInput {
                fired,
                remaining_failures: AtomicU32::new(99),
            },
            PipelineState::new("Title"),
            vec![click_rect_task(
                "Title",
                Action::ClickRect {
                    roi: ScreenRegion::new(520, 320, 240, 80),
                },
                None,
            )],
            2400,
            300,
        )
        .with_recovery_grace(Duration::from_secs(90))
        .with_recovery_clock(Box::new(SteppingRecoveryClock::new(Duration::from_secs(5))));
        let (calls, hook) = counting_hook();

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 6, 1, Some(hook))
            .await;

        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "連続 execute 失敗は吸収限界を超えると recovery hook 経路に流れる (即停止しない)"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "吸収限界到達後、最初の recovery 起動で 1 回"
        );
        assert_eq!(outcome.iterations, 6);
    }

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

    #[test]
    fn recovery_guard_grace_and_streak_transitions() {
        let mut g = RecoveryGuard::new(3, Duration::from_secs(90), u32::MAX);
        assert!(!g.record_miss(Duration::from_secs(0)));
        assert!(!g.record_miss(Duration::from_secs(1)));
        assert_eq!(g.streak(), 2);
        assert!(g.record_miss(Duration::from_secs(2)));
        assert_eq!(g.streak(), 3);
        g.recovered(Duration::from_secs(3));
        assert_eq!(g.streak(), 0);
        assert!(g.in_grace(Duration::from_secs(92)), "89s 経過では猶予中");
        assert!(!g.record_miss(Duration::from_secs(92)));
        assert_eq!(g.streak(), 0, "grace 中は streak 加算なし");
        assert!(!g.record_miss(Duration::from_secs(93)));
        assert_eq!(g.streak(), 1);
        g.record_match();
        assert_eq!(g.streak(), 0);
        assert!(!g.record_miss(Duration::from_secs(94)));
        assert_eq!(g.streak(), 1, "match 後は猶予解除で即カウント");
        let mut z = RecoveryGuard::new(0, Duration::ZERO, u32::MAX);
        assert!(!z.record_miss(Duration::ZERO));
        assert!(!z.record_miss(Duration::ZERO));
    }

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

    #[tokio::test]
    async fn recovery_grace_clears_on_match_and_resumes_immediately() {
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let (matched, _tpl) = matched_frame();
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

    #[tokio::test]
    async fn capture_error_during_boot_grace_is_tolerated() {
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
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

    #[tokio::test]
    async fn test_verify_capture_error_enters_recovery_path() {
        let (matched, _tpl) = matched_frame();
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

    #[test]
    fn test_step_error_kind_classification() {
        let cases = [
            (StepErrorKind::Capture, true),
            (StepErrorKind::VerifyCapture, true),
            (StepErrorKind::Execute, true),
            (StepErrorKind::Other, false),
        ];
        for (kind, expected) in cases {
            assert_eq!(is_recoverable_io_error(kind), expected, "kind={kind:?}");
        }
    }

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
        assert_eq!(outcome.progress_report.recovery_attempts, 3);
        assert_eq!(outcome.progress_report.max_consecutive_recovery, 3);
    }

    #[tokio::test]
    async fn test_recovery_counter_resets_on_healthy_cycle() {
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
        let (matched, _tpl) = matched_frame();
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
        assert_eq!(outcome.progress_report.recovery_attempts, 2);
        assert_eq!(outcome.progress_report.max_consecutive_recovery, 1);
        assert!(
            !outcome.fired_commands.is_empty(),
            "c4 の matched フレームで 1 回発火している"
        );
    }

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
        let j = serde_json::to_string(&pr).expect("serialize json");
        assert!(j.contains("\"recovery_attempts\":4"), "got: {j}");
        assert!(j.contains("\"max_consecutive_recovery\":2"), "got: {j}");
        let back: ProgressReport = serde_json::from_str(&j).expect("deserialize json");
        assert_eq!(back.recovery_attempts, 4);
        assert_eq!(back.max_consecutive_recovery, 2);
        let t = toml::to_string(&pr).expect("serialize toml");
        assert!(t.contains("recovery_attempts = 4"), "got: {t}");
        assert!(t.contains("max_consecutive_recovery = 2"), "got: {t}");
        let legacy_json =
            "{\"iterations\":1,\"fired_count\":0,\"per_task_matches\":[],\"elapsed_ms\":5}";
        let legacy: ProgressReport = serde_json::from_str(legacy_json).expect("legacy deserialize");
        assert_eq!(legacy.recovery_attempts, 0, "got: {legacy_json}");
        assert_eq!(legacy.max_consecutive_recovery, 0, "got: {legacy_json}");
    }

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
        assert!(j.contains("\"stop_reason\":\"max_iterations\""), "got: {j}");
        let back: ProgressReport = serde_json::from_str(&j).expect("deserialize json");
        assert_eq!(back.stop_reason, Some(LoopStopReason::MaxIterations));
        let legacy_json =
            "{\"iterations\":1,\"fired_count\":0,\"per_task_matches\":[],\"elapsed_ms\":5}";
        let legacy: ProgressReport = serde_json::from_str(legacy_json).expect("legacy deserialize");
        assert_eq!(legacy.stop_reason, None, "got: {legacy_json}");
    }

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
        assert_eq!(
            stop_reason_label(LoopStopReason::MaxIterations),
            "最大サイクル到達"
        );
        assert_eq!(
            stop_reason_label(LoopStopReason::GoalReached),
            "宣言的ゴール到達(正常)"
        );
    }

    #[test]
    fn recovery_guard_attempt_budget_and_healthy_reset() {
        let mut g = RecoveryGuard::new(1, Duration::ZERO, 2);
        assert!(g.record_miss(Duration::ZERO), "threshold=1 で即 due");
        assert_eq!(g.consume_attempt(), Some(1));
        g.recovered(Duration::ZERO);
        assert!(g.record_miss(Duration::ZERO));
        assert_eq!(g.consume_attempt(), Some(2), "連続 2 回目");
        g.recovered(Duration::ZERO);
        assert!(g.record_miss(Duration::ZERO));
        assert_eq!(g.consume_attempt(), None, "予算 2 到達で打ち切り");
        g.record_match();
        assert!(g.record_miss(Duration::ZERO));
        assert_eq!(
            g.consume_attempt(),
            Some(1),
            "健全後に attempt は 1 からやり直し"
        );
    }

    #[tokio::test]
    async fn verify_success_when_template_disappears() {
        let (matched, _tpl) = matched_frame();
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));
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
        assert_eq!(driver.current(), "LoadGame");
        assert_eq!(
            fired.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: 640, y: 359 }]
        );
    }

    #[tokio::test]
    async fn verify_fails_when_template_persists() {
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
                assert_eq!(just_fired, Some(InputCommand::Tap { x: 640, y: 359 }));
                assert_eq!(next_current.as_deref(), Some("LoadGame"));
            }
            other => panic!("expected FiredUnverified, got {other:?}"),
        }
        assert_eq!(
            driver.current(),
            "Title",
            "current must be rolled back to pre-task on verify failure"
        );
        assert_eq!(
            fired.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: 640, y: 359 }]
        );
    }

    #[tokio::test]
    async fn verify_skipped_on_nofire_and_nomatch() {
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

        match driver.run_once_verified().await {
            StepOutcome::Error { kind, message } => {
                assert_eq!(kind, StepErrorKind::VerifyCapture);
                assert!(message.starts_with("verify_capture"), "got: {message}");
            }
            other => panic!("expected Error(verify_capture), got {other:?}"),
        }
        assert_eq!(
            fired.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: 640, y: 359 }]
        );
    }

    #[tokio::test]
    async fn verify_loop_treats_fired_unverified_as_nomatch_streak() {
        let (matched, _tpl) = matched_frame();
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
        assert!(
            !outcome.fired_commands.is_empty(),
            "fired commands recorded even on verify failure"
        );
    }

    #[tokio::test]
    async fn verify_disabled_by_default_runs_run_once() {
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
        );

        let outcome = driver.run_loop(Duration::ZERO, 20).await;
        assert_eq!(driver.current(), "LoadGame");
        assert!(!outcome.fired_commands.is_empty());
    }

    #[tokio::test]
    async fn nomatch_streak_resets_on_fired() {
        let (matched, _tpl) = matched_frame();
        let blank = luma_dyn(GrayImage::from_pixel(FULL_W, FULL_H, Luma([128u8])));

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

        let outcome = driver
            .run_loop_with_recovery(Duration::ZERO, 12, 3, Some(hook))
            .await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert!(
            !outcome.fired_commands.is_empty(),
            "expected at least one fire from matched frame"
        );
    }

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

    #[tokio::test]
    async fn run_loop_populates_progress_report_basic_fields() {
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
        assert_eq!(outcome.progress_report.iterations, 3);
        assert_eq!(outcome.progress_report.fired_count, 3);
        assert_eq!(
            outcome.progress_report.terminal_task.as_deref(),
            Some("Terminal")
        );
        assert_eq!(outcome.progress_report.reached_goal, None);
    }

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
        assert_eq!(matches.len(), 3);
        let by_name: std::collections::HashMap<&str, u64> = matches
            .iter()
            .map(|m| (m.task.as_str(), m.matches))
            .collect();
        assert_eq!(by_name.get("Title"), Some(&1));
        assert_eq!(by_name.get("LoadGame"), Some(&1));
        assert_eq!(by_name.get("Terminal"), Some(&1));
    }

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
        assert!(s.contains("iterations: 0"), "got: {s}");
        assert!(s.contains("fired: 0"), "got: {s}");
    }

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
        assert!(s.contains("\"reason\":\"goal_reached\""), "got: {s}");
        assert!(s.contains("\"progress_report\":{"), "got: {s}");
        assert!(s.contains("\"reached_goal\":\"loop_count=5\""), "got: {s}");
        let back: LoopOutcome = serde_json::from_str(&s).expect("deserialize json");
        assert_eq!(back.iterations, 5);
        assert_eq!(back.reason, LoopStopReason::GoalReached);
        assert_eq!(
            back.progress_report.reached_goal.as_deref(),
            Some("loop_count=5")
        );
    }

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
        let j = serde_json::to_string(&pr).expect("serialize json");
        assert!(j.contains("\"fired_count\":3"), "got: {j}");
        assert!(
            j.contains("\"reached_goal\":\"template_match conf>=0.85\""),
            "got: {j}"
        );
        let t = toml::to_string(&pr).expect("serialize toml");
        assert!(
            t.contains("reached_goal = \"template_match conf>=0.85\""),
            "got: {t}"
        );
    }

    #[test]
    fn loop_outcome_progress_report_is_serde_default() {
        #[derive(serde::Deserialize)]
        struct Minimal {
            #[allow(dead_code)]
            iterations: u64,
        }
        let toml_str = "iterations = 1\n";
        let m: Minimal = toml::from_str(toml_str).expect("deserialize minimal");
        assert_eq!(m.iterations, 1);

        let outcome: LoopOutcome = LoopOutcome {
            iterations: 1,
            fired_commands: vec![],
            terminal: "x".to_string(),
            reason: LoopStopReason::Stop,
            progress_report: ProgressReport::default(),
        };
        assert_eq!(outcome.progress_report, ProgressReport::default());
    }

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

    #[tokio::test]
    async fn backcompat_goal_none_no_match_hits_max_iterations_byte_identical() {
        let (mut driver, fired) = build_nomatch_driver();

        let outcome = driver.run_loop(Duration::ZERO, 5).await;
        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "goal=None must stop with MaxIterations, got {:?}",
            outcome.reason
        );
        assert_eq!(outcome.iterations, 5);
        assert!(
            outcome.fired_commands.is_empty(),
            "no fires on pure NoMatch"
        );
        assert_eq!(driver.current(), "Title");
        assert!(fired.lock().expect("fired lock").is_empty());
    }

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
        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "goal=None with recovery must stop with MaxIterations"
        );
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "recovery hook must fire at least once under goal=None"
        );
        assert!(outcome.fired_commands.is_empty());
    }

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
        assert!(
            outcome.iterations < 100,
            "must short-circuit before max_iterations"
        );
    }

    #[tokio::test]
    async fn backcompat_goal_none_indefinite_continuation_scales_with_max_iters() {
        let (mut driver_a, _fired_a) = build_nomatch_driver();
        let out_small = driver_a.run_loop(Duration::ZERO, 5).await;
        assert_eq!(out_small.reason, LoopStopReason::MaxIterations);
        assert_eq!(out_small.iterations, 5);

        let (mut driver_b, _fired_b) = build_nomatch_driver();
        let out_large = driver_b.run_loop(Duration::ZERO, 50).await;
        assert_eq!(out_large.reason, LoopStopReason::MaxIterations);
        assert_eq!(out_large.iterations, 50);

        assert_eq!(
            out_large.iterations,
            out_small.iterations * 10,
            "iterations must scale linearly with max_iters under goal=None"
        );
        assert_ne!(out_large.reason, LoopStopReason::GoalReached);
        assert_ne!(out_large.reason, LoopStopReason::GoalTimeout);
    }

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

    #[tokio::test]
    async fn backcompat_goal_none_single_iteration_still_max_iterations() {
        let (mut driver, _fired) = build_nomatch_driver();
        let outcome = driver.run_loop(Duration::ZERO, 1).await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert_eq!(outcome.iterations, 1);
    }

    #[tokio::test]
    async fn backcompat_goal_none_run_loop_with_goal_is_byte_identical_to_recovery() {
        let (mut driver_baseline, _fired_baseline) = build_nomatch_driver();
        let baseline = driver_baseline
            .run_loop_with_recovery(Duration::ZERO, 7, 0, None)
            .await;

        let (mut driver_via_goal, _fired_via_goal) = build_nomatch_driver();
        let clock = FakeClock::starting_at(0);
        let via_goal = driver_via_goal
            .run_loop_with_goal(Duration::ZERO, 7, 0, None, None, clock)
            .await;

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

        assert!(
            baseline.progress_report.reached_goal.is_none(),
            "baseline reached_goal must be None (no goal declared)"
        );
        assert!(
            via_goal.progress_report.reached_goal.is_none(),
            "run_loop_with_goal(goal=None) must leave reached_goal as None — \
             goal-evaluation code must not populate it on the undeclared path"
        );

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

        let (mut driver, _fired) = build_nomatch_driver();
        let (calls, hook) = make_hook();
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 12, 3, Some(hook), None, clock)
            .await;

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
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "recovery hook must fire at least once under goal=None delegation path"
        );
        assert!(
            outcome.progress_report.reached_goal.is_none(),
            "reached_goal must stay None even with recovery hook under goal=None"
        );
    }

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
            self.next_secs = self.next_secs.saturating_add(1);
            now
        }
    }

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

        assert_eq!(
            outcome.reason,
            LoopStopReason::GoalReached,
            "UC-1 LoopCount target=3 must reach GoalReached after 3 ticks, got {:?}",
            outcome.reason
        );
        assert_eq!(outcome.terminal, "goal_reached");
        assert_eq!(
            outcome.progress_report.reached_goal.as_deref(),
            Some("loop_count=3"),
            "reached_goal descriptor must reflect LoopCount target"
        );
        assert_eq!(
            outcome.iterations, 3,
            "must reach goal exactly at iteration 3"
        );
    }

    #[tokio::test]
    async fn uc1_loop_count_goal_hits_max_iterations_when_target_exceeds_budget() {
        let (mut driver, _fired) = build_nomatch_driver();

        let goal = anaden_core::Goal {
            name: "farm_unbounded".into(),
            stop: anaden_core::StopCondition::LoopCount { target: 50 },
        };
        let clock = FakeClock::starting_at(0);
        let outcome = driver
            .run_loop_with_goal(Duration::ZERO, 5, 0, None, Some(&goal), clock)
            .await;

        assert_eq!(
            outcome.reason,
            LoopStopReason::MaxIterations,
            "UC-1 LoopCount with target>max_iterations must stop with MaxIterations, got {:?}",
            outcome.reason
        );
        assert!(
            outcome.progress_report.reached_goal.is_none(),
            "reached_goal must be None when goal is not reached"
        );
        assert_eq!(outcome.iterations, 5);
    }

    #[tokio::test]
    async fn uc2_template_match_goal_reaches_goal_reached_when_matching_template_fires() {
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

        assert_eq!(
            outcome.reason,
            LoopStopReason::GoalReached,
            "UC-2 TemplateMatch goal must reach GoalReached when matching template fires, \
             got {:?}",
            outcome.reason
        );
        assert_eq!(outcome.terminal, "goal_reached");
        assert_eq!(
            outcome.progress_report.reached_goal.as_deref(),
            Some("template_match conf>=0.85 (task=Title)"),
            "reached_goal descriptor must reflect the TemplateMatch task/confidence"
        );
        assert_eq!(
            outcome.iterations, 1,
            "must reach goal on first matching cycle"
        );
        assert!(
            !outcome.fired_commands.is_empty(),
            "matching template must have fired a command"
        );
    }

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
        assert_eq!(
            outcome.progress_report.reached_goal.as_deref(),
            Some("timeout=3"),
            "reached_goal descriptor must reflect Timeout secs"
        );
        assert!(
            outcome.iterations < 10,
            "goal must terminate before max_iterations; got iterations={}",
            outcome.iterations
        );
        assert_eq!(
            outcome.iterations, 4,
            "must reach timeout exactly on iteration 4 (elapsed 0,1,2,3)"
        );
    }

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

        assert_eq!(outcome.reason, LoopStopReason::GoalReached);
        let expected_descriptor = outcome
            .progress_report
            .reached_goal
            .as_deref()
            .expect("reached_goal must be populated by engine for UC-1 LoopCount");
        assert_eq!(expected_descriptor, "loop_count=3");

        let s = serde_json::to_string(&outcome).expect("serialize json");
        let back: LoopOutcome = serde_json::from_str(&s).expect("deserialize json");

        assert_eq!(
            back.progress_report.reached_goal.as_deref(),
            Some("loop_count=3"),
            "UC-1 reached_goal descriptor must survive JSON round-trip; json={s}"
        );
        assert_eq!(back.reason, LoopStopReason::GoalReached);
        assert_eq!(back.iterations, 3);
        assert_eq!(back.progress_report.iterations, 3);
    }

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

        let s = serde_json::to_string(&outcome).expect("serialize json");
        let back: LoopOutcome = serde_json::from_str(&s).expect("deserialize json");

        assert_eq!(
            back.progress_report.reached_goal.as_deref(),
            Some("template_match conf>=0.85 (task=Title)"),
            "UC-2 reached_goal descriptor must survive JSON round-trip; json={s}"
        );
        assert_eq!(back.reason, LoopStopReason::GoalReached);
        assert!(
            !back.fired_commands.is_empty(),
            "fired_commands must survive JSON round-trip non-empty"
        );
    }

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
        assert_eq!(outcome.iterations, 0);
        assert!(outcome.fired_commands.is_empty());
    }

    #[tokio::test]
    async fn cancel_during_interval_sleep_stops_as_interrupted() {
        let (mut driver, _fired) = build_nomatch_driver();
        let token = CancellationToken::new();
        let cancel_token = token.clone();
        driver = driver.with_cancel(token);

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
        assert!(
            outcome.iterations >= 1,
            "must have run at least one cycle before cancel"
        );
    }

    #[tokio::test]
    async fn cancel_none_default_preserves_max_iterations_behavior() {
        let (mut driver, _fired) = build_nomatch_driver();
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

    #[tokio::test]
    async fn snapshot_saved_on_no_match_when_dir_set() {
        let tmp = tempfile::tempdir().expect("tempdir");
        unsafe {
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
        let has_task = entries.iter().any(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with("Title_"))
                .unwrap_or(false)
        });
        assert!(has_task, "snapshot filename must start with task name");
    }

    #[tokio::test]
    async fn snapshot_not_saved_when_dir_unset() {
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

        assert!(
            PipelineDriver::<FakeCapture, FakeInput>::snapshot_dir().is_none(),
            "snapshot_dir must be None when ANADEN_SNAPSHOT_DIR unset"
        );
        assert_eq!(
            driver.snapshot_counter, 0,
            "counter must not increment when dir unset"
        );
        assert_eq!(driver.diag_counter, 0);
    }

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
            1258,
            300,
        );

        let out = driver.run_once().await;
        match out {
            StepOutcome::Fired {
                fired: just_fired, ..
            } => {
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
        let bar_x = 100u32;
        let bar_y = 50u32;

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
            1458,
            300,
        );
        let out_nobar = driver_nobar.run_once().await;
        let fired_nobar_cmd = match out_nobar {
            StepOutcome::Fired { fired: c, .. } => c,
            other => panic!("expected Fired (no-bar), got {other:?}"),
        };

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
            1458,
            300,
        );
        let out_bar = driver_bar.run_once().await;
        let fired_bar_cmd = match out_bar {
            StepOutcome::Fired { fired: c, .. } => c,
            other => panic!("expected Fired (with-bar), got {other:?}"),
        };

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
        assert_eq!(
            fired_bar.lock().expect("fired lock").as_slice(),
            &[InputCommand::Tap { x: bx, y: by }]
        );
    }

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

        let png_count = count_entries(tmp.path(), "png");
        let md_count = count_entries(tmp.path(), "md");
        assert!(png_count >= 1, "frame dump (png) must be saved");
        assert!(md_count >= 1, "diagnose report (md) must be saved");

        let md_body = read_first_matching(tmp.path(), "md").expect("report file readable");
        assert!(md_body.contains("# NoMatch Diagnose Report"));
        assert!(md_body.contains("Title"), "report must list template row");
    }

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
        let tasks = vec![mk_task("Title", 1.1), mk_task("Other", 1.1)];

        let screen = luma_dyn(raw.clone());
        let mut driver = diag_no_match_driver(frames_of(vec![screen.clone()]), tasks.clone());
        let out = driver.run_once().await;
        assert_eq!(out, StepOutcome::NoMatch);

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
        assert!(
            idx(expected_tasks[0]) < idx(expected_tasks[1]),
            "report rows must follow diagnose_all descending order: {expected_tasks:?}"
        );
        let _ = pos(expected_tasks[0]);
        let _ = pos(expected_tasks[1]);
    }

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

        let out = driver.run_once().await;
        assert_eq!(out, StepOutcome::NoMatch);

        let md_body = read_first_matching(tmp.path(), "md").expect("report file readable");
        assert!(md_body.contains("Broken"), "broken template row");
        assert!(md_body.contains("Oversized"), "oversized needle row");
        assert!(
            md_body.contains("template load failed"),
            "skip reason for broken template"
        );
    }

    #[tokio::test]
    async fn diag_report_not_written_when_env_unset() {
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
                Some(vec![]),
            )],
            FULL_W,
            300,
        );
        (driver, fired)
    }

    #[tokio::test]
    async fn jitter_shifts_fired_tap_within_radius_and_records_seed() {
        let raw = luma_dyn(raw_feature());
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

    #[tokio::test]
    async fn verify_report_disabled_when_verify_off() {
        let raw = luma_dyn(raw_feature());
        let (mut d, _) = build_single_fire_driver(vec![raw.clone(), raw]);
        let outcome = d.run_loop(Duration::ZERO, 5).await;
        let v = &outcome.progress_report.verify;
        assert!(!v.enabled);
        assert_eq!((v.checks, v.passed, v.failed), (0, 0, 0));
    }

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

    const POPUP_NEEDLE_ROI: [u32; 4] = [400, 300, 12, 12];
    const POPUP_MATCH_ROI: [u32; 4] = [390, 290, 48, 36];

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

    fn popup_click_rect_action() -> Action {
        Action::ClickRect {
            roi: ScreenRegion::new(
                POPUP_MATCH_ROI[0],
                POPUP_MATCH_ROI[1],
                POPUP_MATCH_ROI[2],
                POPUP_MATCH_ROI[3],
            ),
        }
    }

    fn build_watch_driver(
        frames: Vec<DynamicImage>,
        popup_action: Action,
    ) -> (
        PipelineDriver<FakeCapture, FakeInput>,
        Arc<Mutex<Vec<InputCommand>>>,
    ) {
        let raw = raw_feature();
        let [px, py, pw, ph] = POPUP_NEEDLE_ROI;
        let mut needle = GrayImage::new(pw, ph);
        for dy in 0..ph {
            for dx in 0..pw {
                needle.put_pixel(dx, dy, *raw.get_pixel(px + dx, py + dy));
            }
        }
        let popup_tpl = write_template_persisted(&needle);
        let mut popup_task = task_with_template("Popup", popup_tpl, POPUP_MATCH_ROI, None);
        popup_task.action = Some(popup_action);

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
                popup_task,
            ],
            FULL_W,
            300,
        );
        (driver, fired)
    }

    #[tokio::test]
    async fn watch_match_fires_and_keeps_current() {
        let popup_only = frame_with_region_masked(MATCH_ROI);
        let (mut d, fired) = build_watch_driver(
            vec![popup_only.clone(), popup_only],
            popup_click_rect_action(),
        );
        d = d.with_watch(vec!["Popup".into()]);

        let outcome = d.run_once().await;
        match &outcome {
            StepOutcome::WatchFired { task, fired: f, .. } => {
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

    #[tokio::test]
    async fn watch_takes_priority_over_current_task() {
        let both = luma_dyn(raw_feature());
        let (mut d, fired) =
            build_watch_driver(vec![both.clone(), both], popup_click_rect_action());
        d = d.with_watch(vec!["Popup".into()]);

        let outcome = d.run_once().await;
        assert!(
            matches!(outcome, StepOutcome::WatchFired { .. }),
            "watch must win over current, got {outcome:?}"
        );
        assert_eq!(d.current(), "Title");
        assert_eq!(fired.lock().expect("lock").len(), 1, "only watch fires");
    }

    #[tokio::test]
    async fn watch_no_match_falls_through_to_current() {
        let title_only = frame_with_region_masked(POPUP_MATCH_ROI);
        let (mut d, _) = build_watch_driver(
            vec![title_only.clone(), title_only],
            popup_click_rect_action(),
        );
        d = d.with_watch(vec!["Popup".into()]);

        let outcome = d.run_once().await;
        match &outcome {
            StepOutcome::Fired { next_current, .. } => {
                assert!(next_current.is_none(), "Title is terminal (empty next)");
            }
            other => panic!("expected Fired (Title terminal), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_loop_completes_through_watch_interruption() {
        let popup_only = frame_with_region_masked(MATCH_ROI);
        let title_only = frame_with_region_masked(POPUP_MATCH_ROI);
        let (mut d, fired) =
            build_watch_driver(vec![popup_only, title_only], popup_click_rect_action());
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
        let popup_matches = outcome
            .progress_report
            .per_task_matches
            .iter()
            .find(|t| t.task == "Popup")
            .map(|t| t.matches);
        assert_eq!(popup_matches, Some(1), "watch task must be counted");
        assert_eq!(fired.lock().expect("lock").len(), 2);
    }

    #[tokio::test]
    async fn watch_stop_action_ends_loop_with_stop_reason() {
        let popup = frame_with_region_masked(MATCH_ROI);
        let (mut d, fired) = build_watch_driver(vec![popup.clone(), popup], Action::Stop);
        d = d.with_watch(vec!["Popup".into()]);

        let outcome = d.run_loop_with_recovery(Duration::ZERO, 5, 0, None).await;
        assert_eq!(outcome.reason, LoopStopReason::Stop);
        assert_eq!(outcome.terminal, "watch stop: Popup");
        assert_eq!(outcome.iterations, 1);
        assert_eq!(
            fired.lock().expect("lock").len(),
            0,
            "Stop watch はコマンドを発火しない"
        );
    }

    #[tokio::test]
    async fn watch_observe_streak_reaches_recovery_hook() {
        let popup = frame_with_region_masked(MATCH_ROI);
        let frames: Vec<DynamicImage> = (0..8).map(|_| popup.clone()).collect();
        let (mut d, fired) = build_watch_driver(frames, Action::DoNothing);
        d = d
            .with_recovery_grace(Duration::from_secs(90))
            .with_recovery_clock(Box::new(SteppingRecoveryClock::new(Duration::from_secs(5))));
        d = d.with_watch(vec!["Popup".into()]);
        let (calls, hook) = counting_hook();

        let outcome = d
            .run_loop_with_recovery(Duration::ZERO, 6, 2, Some(hook))
            .await;
        assert_eq!(outcome.reason, LoopStopReason::MaxIterations);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "watch observe 連続で miss streak が閾値に達し recovery hook が起動する"
        );
        assert_eq!(outcome.iterations, 6);
        assert_eq!(
            fired.lock().expect("lock").len(),
            0,
            "observe watch は main tick を抑制し続ける"
        );
    }

    #[tokio::test]
    async fn watch_observe_brief_then_main_resumes_without_recovery() {
        let popup = frame_with_region_masked(MATCH_ROI);
        let cleared = frame_with_region_masked(POPUP_MATCH_ROI);
        let (mut d, _fired) = build_watch_driver(vec![popup, cleared], Action::DoNothing);
        d = d
            .with_recovery_grace(Duration::from_secs(90))
            .with_recovery_clock(Box::new(SteppingRecoveryClock::new(Duration::from_secs(5))));
        d = d.with_watch(vec!["Popup".into()]);
        let (calls, hook) = counting_hook();

        let outcome = d
            .run_loop_with_recovery(Duration::ZERO, 5, 2, Some(hook))
            .await;
        assert_eq!(outcome.reason, LoopStopReason::TerminalTask);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "短い observe は streak を積むが閾値には達しない"
        );
        assert_eq!(outcome.iterations, 2);
    }

    #[tokio::test]
    async fn no_watch_config_preserves_nomatch_behavior() {
        let popup_only = frame_with_region_masked(MATCH_ROI);
        let (mut d, fired) = build_watch_driver(
            vec![popup_only.clone(), popup_only],
            popup_click_rect_action(),
        );

        let outcome = d.run_once().await;
        assert!(
            matches!(outcome, StepOutcome::NoMatch),
            "without watch, popup-only frame is NoMatch, got {outcome:?}"
        );
        assert_eq!(d.current(), "Title");
        assert!(fired.lock().expect("lock").is_empty());
    }
}
