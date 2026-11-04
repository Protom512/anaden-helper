use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub pipeline_dir: PathBuf,
    pub start_task: String,
    pub algorithm: Option<String>,
    pub interval_secs: u64,
    pub max_iters: u64,
    pub width: Option<u32>,
    pub ensure_open: bool,
    pub ensure_open_wait_secs: u64,
    pub recover_launch: bool,
    pub recover_nomatch_threshold: u32,
    pub verify_after_fire: bool,
    pub goal: Option<anaden_core::Goal>,
}

pub fn resolve_algorithm(value: &str) -> Result<anaden_vision::Algorithm, String> {
    match value {
        "sse" => Ok(anaden_vision::Algorithm::Sse),
        "ccoeff" => Ok(anaden_vision::Algorithm::Ccoeff),
        other => Err(format!(
            "algorithm は `sse` または `ccoeff` です(指定値: {other})"
        )),
    }
}

impl RunOptions {
    pub fn validate(&self) -> Result<(), anyhow::Error> {
        if self.start_task.trim().is_empty() {
            anyhow::bail!("start_task が空です(開始タスク名を指定してください)");
        }
        if self.interval_secs == 0 {
            anyhow::bail!("interval_secs は 1 以上を指定してください(指定値: 0)");
        }
        if self.max_iters == 0 {
            anyhow::bail!("max_iters は 1 以上を指定してください(指定値: 0)");
        }
        if !self.pipeline_dir.is_dir() {
            anyhow::bail!(
                "pipeline_dir がディレクトリではありません: {}",
                self.pipeline_dir.display()
            );
        }
        if let Some(a) = &self.algorithm {
            resolve_algorithm(a).map_err(|e| anyhow::anyhow!(e))?;
        }
        crate::validate_goal(&self.goal)?;
        if !self.start_task_exists_in_pipeline()? {
            anyhow::bail!(
                "start_task `{}` が pipeline ディレクトリ {} に定義されていません",
                self.start_task,
                self.pipeline_dir.display()
            );
        }
        Ok(())
    }

    fn start_task_exists_in_pipeline(&self) -> Result<bool, anyhow::Error> {
        let tasks = anaden_vision::load_pipeline(&self.pipeline_dir)
            .map_err(|e| anyhow::anyhow!("パイプライン読込失敗: {e}"))?;
        Ok(tasks.iter().any(|t| t.name == self.start_task))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Idle,
    Running,
    Finished,
}

#[derive(Debug)]
pub enum StartError {
    AlreadyRunning,
    InvalidOptions(anyhow::Error),
}

impl StartError {
    pub fn is_already_running(&self) -> bool {
        matches!(self, StartError::AlreadyRunning)
    }
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::AlreadyRunning => {
                write!(f, "pipeline は既に実行中です(二重起動は防止されています)")
            }
            StartError::InvalidOptions(e) => write!(f, "実行オプションが不正です: {e}"),
        }
    }
}

impl std::error::Error for StartError {}

pub type RunFn = fn(
    RunOptions,
    CancellationToken,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<anaden_engine::LoopOutcome, anyhow::Error>> + Send>,
>;

#[derive(Default)]
struct Slot {
    cancel: Option<CancellationToken>,
    running: bool,
    finished: bool,
}

#[derive(Default)]
pub struct PipelineController {
    slot: Arc<Mutex<Slot>>,
}

impl PipelineController {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn state_async(&self) -> RunState {
        let slot = self.slot.lock().await;
        slot_state(&slot)
    }

    pub fn state(&self) -> RunState {
        match self.slot.try_lock() {
            Ok(slot) => slot_state(&slot),
            Err(_) => RunState::Running,
        }
    }

    #[allow(clippy::result_large_err)]
    pub fn try_start(
        &self,
        opts: RunOptions,
        run: RunFn,
    ) -> Result<
        tokio::task::JoinHandle<Result<anaden_engine::LoopOutcome, anyhow::Error>>,
        StartError,
    > {
        if let Err(e) = opts.validate() {
            return Err(StartError::InvalidOptions(e));
        }
        {
            let mut slot = self
                .slot
                .try_lock()
                .map_err(|_| StartError::AlreadyRunning)?;
            if slot.running {
                return Err(StartError::AlreadyRunning);
            }
            let cancel = CancellationToken::new();
            slot.cancel = Some(cancel.clone());
            slot.running = true;
            slot.finished = false;
        }
        let slot_arc = Arc::clone(&self.slot);
        let cancel = {
            match self.slot.try_lock() {
                Ok(s) => s.cancel.clone(),
                Err(_) => None,
            }
        };
        let cancel = match cancel {
            Some(c) => c,
            None => {
                return Err(StartError::AlreadyRunning);
            }
        };
        let jh = tokio::spawn(async move {
            let result = run(opts, cancel).await;
            if let Ok(mut slot) = slot_arc.try_lock() {
                slot.running = false;
                slot.finished = true;
                slot.cancel = None;
            }
            result
        });
        Ok(jh)
    }

    pub async fn cancel(&self) {
        let cancel = {
            let slot = self.slot.lock().await;
            slot.cancel.clone()
        };
        if let Some(c) = cancel {
            c.cancel();
        }
    }
}

fn slot_state(slot: &Slot) -> RunState {
    if slot.running {
        RunState::Running
    } else if slot.finished {
        RunState::Finished
    } else {
        RunState::Idle
    }
}

pub fn is_pipeline_dir(dir: &Path) -> bool {
    dir.is_dir()
        && std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .any(|e| e.path().extension().is_some_and(|x| x == "toml"))
            })
            .unwrap_or(false)
}
