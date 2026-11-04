use std::time::Duration;

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputAction {
    Tap(ScreenPoint),
    Swipe {
        from: ScreenPoint,
        to: ScreenPoint,
        duration_ms: u64,
    },
    LongPress(ScreenPoint, u64),
    Wait(Duration),
}

impl InputAction {
    pub fn tap(x: u32, y: u32) -> Self {
        Self::Tap(ScreenPoint::new(x, y))
    }

    pub fn swipe(x1: u32, y1: u32, x2: u32, y2: u32, duration_ms: u64) -> Self {
        Self::Swipe {
            from: ScreenPoint::new(x1, y1),
            to: ScreenPoint::new(x2, y2),
            duration_ms,
        }
    }

    pub fn wait_ms(ms: u64) -> Self {
        Self::Wait(Duration::from_millis(ms))
    }

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
