use std::path::Path;

use image::DynamicImage;

use anaden_core::InputAction;

use crate::authoring_coords;
use crate::authoring_session::{AuthoringSession, GestureOutcome};
use crate::canvas;
use crate::scenario_save::{ScenarioSaveError, ScenarioSaveOutcome};

const DEFAULT_AUTHORING_NAME: &str = "MyFirstAuthoredRun";

pub trait AuthoringInputInjector {
    fn click(&mut self, pos: (i32, i32)) -> Result<(), String>;
}

#[derive(Debug, Default)]
pub struct RecordingInjector {
    pub clicks: Vec<(i32, i32)>,
    pub fail_next: bool,
}

impl RecordingInjector {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl AuthoringInputInjector for RecordingInjector {
    fn click(&mut self, pos: (i32, i32)) -> Result<(), String> {
        if self.fail_next {
            self.fail_next = false;
            return Err("recording injector: 注入失敗 (テスト)".to_string());
        }
        self.clicks.push(pos);
        Ok(())
    }
}

pub struct UnavailableAuthoringInjector {
    reason: String,
}

impl UnavailableAuthoringInjector {
    #[must_use]
    pub fn new(reason: &str) -> Self {
        Self {
            reason: reason.to_string(),
        }
    }
}

impl AuthoringInputInjector for UnavailableAuthoringInjector {
    fn click(&mut self, _pos: (i32, i32)) -> Result<(), String> {
        Err(self.reason.clone())
    }
}

#[cfg(windows)]
pub struct Win32AuthoringInjector {
    executor: anaden_device::Win32InputExecutor,
}

#[cfg(windows)]
impl Win32AuthoringInjector {
    #[must_use]
    pub fn new(process: &str) -> Self {
        Self {
            executor: anaden_device::Win32InputExecutor::new_without_dpi(process),
        }
    }
}

#[cfg(windows)]
impl AuthoringInputInjector for Win32AuthoringInjector {
    fn click(&mut self, pos: (i32, i32)) -> Result<(), String> {
        let action = InputAction::tap(pos.0.max(0) as u32, pos.1.max(0) as u32);
        self.executor
            .execute_blocking(&action)
            .map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelGesture {
    Discarded,
    Handled {
        outcome: GestureOutcome,
        record_error: Option<String>,
        injected: bool,
        inject_error: Option<String>,
    },
}

#[derive(Debug)]
pub struct AuthoringPanel {
    inject_enabled: bool,
    pub(crate) name: String,
    session: Option<AuthoringSession>,
    region_drag: canvas::RoiEdit,
    frame_dims: Option<(u32, u32)>,
    client_dims: Option<(u32, u32)>,
    pub(crate) status: String,
}

impl Default for AuthoringPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthoringPanel {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inject_enabled: false,
            name: DEFAULT_AUTHORING_NAME.to_string(),
            session: None,
            region_drag: canvas::RoiEdit::default(),
            frame_dims: None,
            client_dims: None,
            status: "「記録開始」後にライブビューをクリック (タップ) / ドラッグ (認識領域)"
                .to_string(),
        }
    }

    #[must_use]
    pub fn inject_enabled(&self) -> bool {
        self.inject_enabled
    }

    pub fn set_inject_enabled(&mut self, enabled: bool) {
        self.inject_enabled = enabled;
        self.status = if enabled {
            "オーサリングモード有効: クリックが実機 (PC版ウィンドウ) へ注入されます".to_string()
        } else {
            "入力注入を無効にしました (記録のみ行います)".to_string()
        };
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = name.to_string();
    }

    #[must_use]
    pub fn is_started(&self) -> bool {
        self.session.is_some()
    }

    #[must_use]
    pub fn session(&self) -> Option<&AuthoringSession> {
        self.session.as_ref()
    }

    pub fn region_drag(&mut self) -> &mut canvas::RoiEdit {
        &mut self.region_drag
    }

    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn start_session(&mut self) {
        let trimmed = self.name.trim().to_string();
        let name = if trimmed.is_empty() {
            DEFAULT_AUTHORING_NAME.to_string()
        } else {
            trimmed
        };
        self.name = name.clone();
        self.session = Some(AuthoringSession::new(&name));
        self.status = format!(
            "記録開始: {name} — クリックでタップ、ドラッグで認識領域 (両方で 1 ステップ確定)"
        );
    }

    pub fn push_frame(&mut self, frame: &DynamicImage, client_dims: Option<(u32, u32)>) {
        self.frame_dims = Some((frame.width(), frame.height()));
        self.client_dims = client_dims;
        if let Some(session) = &mut self.session {
            session.push_frame(frame);
        }
    }

    pub fn canvas_tap(
        &mut self,
        pos: (u32, u32),
        injector: &mut dyn AuthoringInputInjector,
    ) -> PanelGesture {
        let Some(session) = self.session.as_mut() else {
            self.status = "セッション未開始: 先に「記録開始」してください".to_string();
            return PanelGesture::Discarded;
        };
        let record = session.record_tap(pos);
        let outcome = match record {
            Ok(gesture) => gesture,
            Err(e) => {
                let out = PanelGesture::Handled {
                    outcome: GestureOutcome::Pending,
                    record_error: Some(e.to_string()),
                    injected: false,
                    inject_error: None,
                };
                self.status = format!("タップ記録失敗: {e}");
                return out;
            }
        };
        let mut injected = false;
        let mut inject_error = None;
        if self.inject_enabled {
            match self.client_pos(pos) {
                Ok(client) => match injector.click(client) {
                    Ok(()) => injected = true,
                    Err(e) => inject_error = Some(e),
                },
                Err(e) => inject_error = Some(e),
            }
        }
        self.status = gesture_status("タップ", &outcome, injected, inject_error.as_deref());
        PanelGesture::Handled {
            outcome,
            record_error: None,
            injected,
            inject_error,
        }
    }

    pub fn canvas_region(&mut self, roi: [u32; 4]) -> PanelGesture {
        let Some(session) = self.session.as_mut() else {
            self.status = "セッション未開始: 先に「記録開始」してください".to_string();
            return PanelGesture::Discarded;
        };
        match session.record_region(roi) {
            Ok(outcome) => {
                self.status = gesture_status("領域", &outcome, false, None);
                PanelGesture::Handled {
                    outcome,
                    record_error: None,
                    injected: false,
                    inject_error: None,
                }
            }
            Err(e) => {
                self.status = format!("領域記録失敗: {e}");
                PanelGesture::Handled {
                    outcome: GestureOutcome::Pending,
                    record_error: Some(e.to_string()),
                    injected: false,
                    inject_error: None,
                }
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        let undone = self.session.as_mut().is_some_and(AuthoringSession::undo);
        if undone {
            self.status = "取り消しました".to_string();
        }
        undone
    }

    pub fn save(
        &mut self,
        pipelines_root: &Path,
    ) -> Option<Result<ScenarioSaveOutcome, ScenarioSaveError>> {
        let Some(session) = self.session.as_mut() else {
            self.status = "セッション未開始: 保存できるシナリオがありません".to_string();
            return None;
        };
        let result = session.save(pipelines_root);
        match &result {
            Ok(outcome) => {
                self.status = format!(
                    "保存しました: {} (警告 {} 件)",
                    outcome.dir.display(),
                    outcome.warnings.len()
                );
            }
            Err(e) => self.status = format!("保存失敗: {e}"),
        }
        Some(result)
    }

    fn client_pos(&self, pos: (u32, u32)) -> Result<(i32, i32), String> {
        let frame = self
            .frame_dims
            .ok_or_else(|| "フレーム未取得のため注入座標を決定できません".to_string())?;
        let client = self
            .client_dims
            .ok_or_else(|| "生キャプチャ寸法が未取得のため注入座標を決定できません".to_string())?;
        let normalized = authoring_coords::frame_to_normalized(pos, frame)
            .ok_or_else(|| "フレーム寸法が不正のため注入座標を決定できません".to_string())?;
        authoring_coords::normalized_to_client(normalized, client)
            .ok_or_else(|| "クライアント寸法が不正のため注入座標を決定できません".to_string())
    }
}

fn gesture_status(
    kind: &str,
    outcome: &GestureOutcome,
    injected: bool,
    inject_error: Option<&str>,
) -> String {
    let mut s = match outcome {
        GestureOutcome::Pending => format!("{kind}を記録しました (もう一方のジェスチャ待ち)"),
        GestureOutcome::Confirmed { warnings } => {
            format!("ステップ確定 ({kind}) — 警告 {} 件", warnings.len())
        }
    };
    if injected {
        s.push_str(" / 実機へ注入しました");
    }
    if let Some(e) = inject_error {
        s.push_str(&format!(" / 注入失敗: {e}"));
    }
    s
}
