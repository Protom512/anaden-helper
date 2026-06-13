//! デバイスへの入力操作を表す型。
//!
//! 入力は副作用だが、ここでは「副作用の指示」を値として表現する。
//! 実際の実行は `anaden-device` の `InputExecutor` が行う。

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 画面上の座標（ピクセル単位）。
/// 左上を (0, 0) とする。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScreenPoint {
    pub x: u32,
    pub y: u32,
}

impl ScreenPoint {
    pub fn new(x: u32, y: u32) -> Self {
        Self { x, y }
    }
}

/// デバイスへの入力操作。
///
/// 設計意図: 操作の「意図」を値として表現し、実行は `InputExecutor` に委ねる。
/// これにより、Strategy はデバイスの詳細を知らずに操作を組み立てられる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputAction {
    /// 指定座標をタップする
    Tap(ScreenPoint),
    /// 座標 `from` から座標 `to` へスワイプする（所要時間ミリ秒）
    Swipe {
        from: ScreenPoint,
        to: ScreenPoint,
        duration_ms: u64,
    },
    /// 指定座標を長押しする（所要時間ミリ秒）
    LongPress(ScreenPoint, u64),
    /// 何もせず待機する
    Wait(Duration),
}

impl InputAction {
    /// 便利コンストラクタ: タップ操作を生成する。
    pub fn tap(x: u32, y: u32) -> Self {
        Self::Tap(ScreenPoint::new(x, y))
    }

    /// 便利コンストラクタ: スワイプ操作を生成する。
    pub fn swipe(x1: u32, y1: u32, x2: u32, y2: u32, duration_ms: u64) -> Self {
        Self::Swipe {
            from: ScreenPoint::new(x1, y1),
            to: ScreenPoint::new(x2, y2),
            duration_ms,
        }
    }

    /// 便利コンストラクタ: ミリ秒単位の待機を生成する。
    pub fn wait_ms(ms: u64) -> Self {
        Self::Wait(Duration::from_millis(ms))
    }

    /// 便利コンストラクタ: 秒単位の待機を生成する。
    pub fn wait_secs(secs: u64) -> Self {
        Self::Wait(Duration::from_secs(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_convenience_constructor() {
        let action = InputAction::tap(100, 200);
        assert_eq!(action, InputAction::Tap(ScreenPoint::new(100, 200)));
    }

    #[test]
    fn swipe_convenience_constructor() {
        let action = InputAction::swipe(0, 500, 0, 100, 300);
        assert_eq!(
            action,
            InputAction::Swipe {
                from: ScreenPoint::new(0, 500),
                to: ScreenPoint::new(0, 100),
                duration_ms: 300,
            }
        );
    }

    #[test]
    fn wait_constructors() {
        let wait = InputAction::wait_ms(500);
        assert_eq!(wait, InputAction::Wait(Duration::from_millis(500)));

        let wait = InputAction::wait_secs(2);
        assert_eq!(wait, InputAction::Wait(Duration::from_secs(2)));
    }
}
