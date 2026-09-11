//! 実演オーサリングモードのパネル状態 + 入力注入ポート (Issue #190 Shard 2/3)。
//!
//! Shard 1 の純状態機械 ([`crate::authoring_session::AuthoringSession`]) の上に
//! 「ジェスチャ → セッションコマンド」の接続と入力注入の抽象化を持つ egui
//! 非依存モジュール。egui 描画 (ライブビュー・ステップリスト・undo/保存ボタン) は
//! [`crate::app_ui_live`] が本状態へ配線する (`scenario_state` + `scenario_ui` と
//! 同一の「純モデル + パネル分離」パターン)。座標変換は
//! [`crate::authoring_coords`] の純関数へ全面委譲。
//!
//! ## 入力注入の安全性
//!
//! 実機 (PC版ウィンドウ) クリックの注入は [`AuthoringPanel::inject_enabled`]
//! トグルが明示的に有効なときのみ行う (既定無効 = 誤クリック防止)。注入は
//! 常に「セッションへのタップ記録が成功した後」に限られ、記録に失敗した
//! 無効クリックは実機へ送出されない。注入失敗はエラーとして status へ
//! 出る (fail-visible・記録は妨げない)。

use std::path::Path;

use image::DynamicImage;

use anaden_core::InputAction;

use crate::authoring_coords;
use crate::authoring_session::{AuthoringSession, GestureOutcome};
use crate::canvas;
use crate::scenario_save::{ScenarioSaveError, ScenarioSaveOutcome};

/// セッション名空欄時のフォールバック (保存先ディレクトリ名として安全な値)。
const DEFAULT_AUTHORING_NAME: &str = "MyFirstAuthoredRun";

/// 実演オーサリングの入力注入ポート。
///
/// クリック座標は対象ウィンドウのクライアント領域左上原点 (物理 px)。本実装は
/// Windows の SendInput ([`Win32AuthoringInjector`] → anaden-device)。
/// テストは recording double ([`RecordingInjector`]) を使う。
pub trait AuthoringInputInjector {
    /// 指定クライアント座標でクリック (DOWN → UP) を注入する。
    ///
    /// # Errors
    /// 注入失敗 (プロセス未検出・前景化失敗・注入不可環境等) をエラー文字列で
    /// 返す (呼出側は status へ表示 — fail-visible)。
    fn click(&mut self, pos: (i32, i32)) -> Result<(), String>;
}

/// テスト用 recording double。注入を記録のみ行い、実際には送出しない。
#[derive(Debug, Default)]
pub struct RecordingInjector {
    /// 注入されたクリック座標 (呼出順)。
    pub clicks: Vec<(i32, i32)>,
    /// 次回の [`AuthoringInputInjector::click`] をエラーにする (fail-visible 経路のテスト)。
    pub fail_next: bool,
}

impl RecordingInjector {
    /// 空の recording double を作成する。
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

/// 注入不可環境 (非 Windows ビルド・Android 取得元) 用のスタブ。
///
/// トグル有効時のクリックは常にエラーを返す (fail-visible)。UI 側でも抑制
/// されるが、経路の安全性のため呼出自体も失敗する。
pub struct UnavailableAuthoringInjector {
    reason: String,
}

impl UnavailableAuthoringInjector {
    /// 失敗理由付きで作成する。
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

/// Windows 実注入実装 (anaden-device `Win32InputExecutor` = SendInput へ委譲)。
///
/// GUI 埋め込みのため DPI アウェア化を行わない `new_without_dpi` で構築する
/// (eframe ホスト起動後のアウェア化は egui 描画を壊す — `Win32Capture` と
/// 同一の措置)。
#[cfg(windows)]
pub struct Win32AuthoringInjector {
    executor: anaden_device::Win32InputExecutor,
}

#[cfg(windows)]
impl Win32AuthoringInjector {
    /// 対象プロセス名 (例: "AnotherEden.exe") で作成する。
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

/// ライブビュー ジェスチャのパネルレベル処理結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelGesture {
    /// セッション未開始のためジェスチャを破棄した (UI は「記録開始」を促す)。
    Discarded,
    /// ジェスチャを処理した (セッション記録 + トグル有効時の注入)。
    Handled {
        /// セッション記録の結果 (Pending / Confirmed)。
        outcome: GestureOutcome,
        /// セッション記録エラー (atomic 破棄済みの表示用文字列)。
        record_error: Option<String>,
        /// 入力注入を実行したか。
        injected: bool,
        /// 入力注入エラー (fail-visible)。
        inject_error: Option<String>,
    },
}

/// 実演オーサリングパネルの状態 (egui 非依存)。
///
/// セッション (Shard 1)・ドラッグ中の領域選択 ([`canvas::RoiEdit`] 流用)・
/// 注入トグル・座標変換に必要なフレーム寸法を保持し、ジェスチャを
/// セッションコマンドへ接続する。
#[derive(Debug)]
pub struct AuthoringPanel {
    /// 入力注入トグル。true のときのみライブビューのクリックで実注入を行う
    /// (誤クリック防止・既定 false)。
    inject_enabled: bool,
    /// シナリオ名入力 (セッション生成に使う)。描画モジュール (app_ui_live) が
    /// テキストボックスへ直接束縛するため pub(crate)。
    pub(crate) name: String,
    /// オーサリングセッション (「記録開始」後に存在)。
    session: Option<AuthoringSession>,
    /// ドラッグ中の認識領域選択 (フレームピクセル座標)。canvas.rs 流用。
    region_drag: canvas::RoiEdit,
    /// 直近で push したフレームの寸法 (正規化空間 = 表示フレーム寸法)。
    frame_dims: Option<(u32, u32)>,
    /// 直近の生キャプチャ寸法 (PC版ではクライアント領域寸法と一致。注入座標変換用)。
    client_dims: Option<(u32, u32)>,
    /// パネル status (ジェスチャ結果・警告・注入エラー・保存結果)。
    pub(crate) status: String,
}

impl Default for AuthoringPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthoringPanel {
    /// 既定パネル (注入無効・シナリオ名 `DEFAULT_AUTHORING_NAME`) を作成する。
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

    /// 入力注入トグルの現在値。
    #[must_use]
    pub fn inject_enabled(&self) -> bool {
        self.inject_enabled
    }

    /// 入力注入トグルを設定する (status へも反映)。
    pub fn set_inject_enabled(&mut self, enabled: bool) {
        self.inject_enabled = enabled;
        self.status = if enabled {
            "オーサリングモード有効: クリックが実機 (PC版ウィンドウ) へ注入されます".to_string()
        } else {
            "入力注入を無効にしました (記録のみ行います)".to_string()
        };
    }

    /// シナリオ名入力の現在値。
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// シナリオ名入力を設定する (UI テキストボックス・テストから使用)。
    pub fn set_name(&mut self, name: &str) {
        self.name = name.to_string();
    }

    /// セッションが開始済みか。
    #[must_use]
    pub fn is_started(&self) -> bool {
        self.session.is_some()
    }

    /// セッションへの参照 (描画・テスト用)。
    #[must_use]
    pub fn session(&self) -> Option<&AuthoringSession> {
        self.session.as_ref()
    }

    /// ドラッグ中の領域選択 (フレームピクセル座標)。キャンバスが直接更新する。
    pub fn region_drag(&mut self) -> &mut canvas::RoiEdit {
        &mut self.region_drag
    }

    /// パネル status。
    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }

    /// 「記録開始」: 入力中の名前でセッションを作る (既存ステップは破棄)。
    /// 空欄時は `DEFAULT_AUTHORING_NAME` を採用する。
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

    /// ライブフレームを供給する (表示とセッションへ同一の正規化フレームを渡す)。
    ///
    /// `client_dims` は生キャプチャ寸法 (PC版ではクライアント領域寸法と一致。
    /// 注入座標変換に使う。非ライブ由来は `None`)。
    pub fn push_frame(&mut self, frame: &DynamicImage, client_dims: Option<(u32, u32)>) {
        self.frame_dims = Some((frame.width(), frame.height()));
        self.client_dims = client_dims;
        if let Some(session) = &mut self.session {
            session.push_frame(frame);
        }
    }

    /// ライブビューのクリック (フレームピクセル座標) を処理する。
    ///
    /// セッションへタップを記録し、記録が成功しトグルが有効なら注入も行う。
    /// 注入座標は「フレームピクセル → 正規化 → クライアント」の純関数チェーンで
    /// 決める ([`authoring_coords`])。
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
        // 注入は記録成功時のみ (記録に失敗した無効クリックは実機へ送出しない)。
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

    /// ドラッグで確定した認識領域 (フレームピクセル座標) をセッションへ記録する。
    ///
    /// 領域の注入は行わない (ドラッグは認識範囲の指定であり操作ではない)。
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

    /// 直近の操作を取り消す (未確定ジェスチャ優先・なければ最終ステップ)。
    /// 戻り値は何かを取り消せたか。
    pub fn undo(&mut self) -> bool {
        let undone = self.session.as_mut().is_some_and(AuthoringSession::undo);
        if undone {
            self.status = "取り消しました".to_string();
        }
        undone
    }

    /// セッションを `<pipelines_root>/<名前>/` へ保存する。
    ///
    /// [`AuthoringSession::save`] への委譲。セッション未開始は `None`
    /// (status へ案内)。保存結果は status へも反映する。
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

    /// セッション記録座標 (フレームピクセル = 正規化空間) → 注入用クライアント座標。
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

/// ジェスチャ処理結果 → status 文字列。
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
