//! 接続状態・接続チェック (Issue #175: app_state.rs 分割)。
//!
//! 実機 (adb) / PC版プロセス検出の状態サマリ ([`ConnectionState`] /
//! [`ConnectionStatus`]) とチェック関数 ([`check_android_device`] /
//! [`check_windows_process`]) を定義する。StudioApp 本体は
//! [`crate::app_state_core`]、呼び出し元互換の re-export は
//! [`crate::app_state`] (facade)。

use eframe::egui;

/// 接続状態 (MAA/MDA 参考の状態サマリバッジ・Issue #139 T3)。
///
///豆腐 (グリフ欠落) 排除のため、バッジ表示は Unicode 絵文字ではなく
/// ASCII 括弧ラベル (`[OK]` 等) + 日本語テキストで構成する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// 未チェック (起動直後)。
    Unknown,
    /// 確認中 (プローブ実行中)。
    Checking,
    /// 接続済み (実機検出 / プロセス検出成功)。
    Connected,
    /// 未接続 (検出失敗・理由あり)。
    Disconnected,
}

impl ConnectionState {
    /// 状態サマリバッジの表示文字列 (グリフ確認済み・豆腐なし)。
    #[must_use]
    pub fn badge(self) -> &'static str {
        match self {
            Self::Unknown => "[?] 接続未確認",
            Self::Checking => "[..] 接続確認中",
            Self::Connected => "[OK] 接続済み",
            Self::Disconnected => "[NG] 未接続",
        }
    }

    /// 接続済みかどうか。
    #[must_use]
    pub fn is_connected(self) -> bool {
        matches!(self, Self::Connected)
    }

    /// バッジの表示色 (egui 色)。
    pub(crate) fn badge_color(self) -> egui::Color32 {
        match self {
            Self::Unknown => egui::Color32::from_rgb(150, 150, 150),
            Self::Checking => egui::Color32::from_rgb(230, 160, 30),
            Self::Connected => egui::Color32::from_rgb(60, 180, 75),
            Self::Disconnected => egui::Color32::from_rgb(220, 60, 60),
        }
    }
}

/// 接続チェックの結果 (状態 + エラー理由)。
#[derive(Debug, Clone)]
pub struct ConnectionStatus {
    /// 接続状態。
    pub state: ConnectionState,
    /// チェックの詳細・エラー理由 (エラー理由パネルに表示)。
    pub detail: String,
}

impl Default for ConnectionStatus {
    fn default() -> Self {
        Self {
            state: ConnectionState::Unknown,
            detail: "接続チェック未実行".to_string(),
        }
    }
}

impl ConnectionStatus {
    /// エラー理由パネルの表示行。未接続時は理由を添える。
    #[must_use]
    pub fn reason_line(&self) -> String {
        match self.state {
            ConnectionState::Disconnected => format!("理由: {}", self.detail),
            _ => self.detail.clone(),
        }
    }
}

/// Android 実機 (adb) の接続チェック。
/// `adb -s <serial> get-state` の終了コードと stdout で判定する。
pub fn check_android_device(serial: &str) -> ConnectionStatus {
    if serial.trim().is_empty() {
        return ConnectionStatus {
            state: ConnectionState::Disconnected,
            detail: "adb serial が未入力".to_string(),
        };
    }
    match std::process::Command::new("adb")
        .args(["-s", serial.trim(), "get-state"])
        .output()
    {
        Ok(out) if out.status.success() => {
            let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if state == "device" {
                ConnectionStatus {
                    state: ConnectionState::Connected,
                    detail: format!("adb {serial}: device"),
                }
            } else {
                ConnectionStatus {
                    state: ConnectionState::Disconnected,
                    detail: format!("adb {serial}: 状態が device でない ({state})"),
                }
            }
        }
        Ok(out) => ConnectionStatus {
            state: ConnectionState::Disconnected,
            detail: format!(
                "adb {serial}: get-state 失敗 ({})",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        },
        Err(e) => ConnectionStatus {
            state: ConnectionState::Disconnected,
            detail: format!("adb 起動失敗 (adb への PATH を確認): {e}"),
        },
    }
}

/// PC版 (Windows) プロセス検出チェック。
/// `Win32Capture` の 1 枚キャプチャ成功をプロセス検出成功とみなす。
#[cfg(windows)]
pub fn check_windows_process(exe: &str) -> ConnectionStatus {
    if exe.trim().is_empty() {
        return ConnectionStatus {
            state: ConnectionState::Disconnected,
            detail: "exe 名が未入力".to_string(),
        };
    }
    let probe = anaden_device::Win32Capture::new(exe.trim());
    match probe.capture_blocking() {
        Ok(img) => ConnectionStatus {
            state: ConnectionState::Connected,
            detail: format!("{exe}: プロセス検出済み ({}x{})", img.width(), img.height()),
        },
        Err(e) => ConnectionStatus {
            state: ConnectionState::Disconnected,
            detail: format!("{exe}: プロセス未検出 or キャプチャ失敗 ({e})"),
        },
    }
}

/// PC版チェックの非 Windows フォールバック (GUI 表示整合用)。
#[cfg(not(windows))]
pub fn check_windows_process(_exe: &str) -> ConnectionStatus {
    ConnectionStatus {
        state: ConnectionState::Disconnected,
        detail: "Windows バックエンドはこの OS では利用不可".to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    // ---- Issue #139 T3: 接続状態可視化 ----

    #[test]
    fn connection_state_badges_are_ascii_no_tofu() {
        // バッジ文字列は Unicode 絵文字を含まない (豆腐排除)。
        for (state, expected) in [
            (ConnectionState::Unknown, "[?] 接続未確認"),
            (ConnectionState::Checking, "[..] 接続確認中"),
            (ConnectionState::Connected, "[OK] 接続済み"),
            (ConnectionState::Disconnected, "[NG] 未接続"),
        ] {
            assert_eq!(state.badge(), expected);
            // 絵文字ブロック (U+1F300 以上) を含まないことを機械検証。
            assert!(
                state.badge().chars().all(|c| c < '\u{1F300}'),
                "badge must not contain emoji: {}",
                state.badge()
            );
        }
    }

    #[test]
    fn connection_state_is_connected_only_for_connected() {
        assert!(ConnectionState::Connected.is_connected());
        assert!(!ConnectionState::Unknown.is_connected());
        assert!(!ConnectionState::Checking.is_connected());
        assert!(!ConnectionState::Disconnected.is_connected());
    }

    #[test]
    fn connection_status_default_is_unknown_with_reason() {
        let s = ConnectionStatus::default();
        assert_eq!(s.state, ConnectionState::Unknown);
        assert_eq!(s.reason_line(), "接続チェック未実行");
    }

    #[test]
    fn connection_status_reason_line_prefixes_detail_when_disconnected() {
        let s = ConnectionStatus {
            state: ConnectionState::Disconnected,
            detail: "adb が見つからない".to_string(),
        };
        assert_eq!(s.reason_line(), "理由: adb が見つからない");
        let ok = ConnectionStatus {
            state: ConnectionState::Connected,
            detail: "adb emulator-5554: device".to_string(),
        };
        assert_eq!(ok.reason_line(), "adb emulator-5554: device");
    }

    #[test]
    fn check_android_empty_serial_is_disconnected() {
        let s = check_android_device("");
        assert_eq!(s.state, ConnectionState::Disconnected);
        assert!(s.detail.contains("serial"));
    }

    #[test]
    fn check_windows_empty_exe_is_disconnected() {
        let s = check_windows_process("  ");
        assert_eq!(s.state, ConnectionState::Disconnected);
        assert!(s.detail.contains("exe"));
    }
}
