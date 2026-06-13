//! 画面上の領域を表す型。

use serde::{Deserialize, Serialize};

/// 画面上の矩形領域。左上端を基準に幅と高さで指定する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScreenRegion {
    /// 左上端の X 座標（ピクセル）
    pub x: u32,
    /// 左上端の Y 座標（ピクセル）
    pub y: u32,
    /// 領域の幅（ピクセル）
    pub width: u32,
    /// 領域の高さ（ピクセル）
    pub height: u32,
}

impl ScreenRegion {
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// 領域の中心座標を返す。
    pub fn center(&self) -> (u32, u32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }

    /// 領域の右端の X 座標。
    pub fn right(&self) -> u32 {
        self.x + self.width
    }

    /// 領域の下端の Y 座標。
    pub fn bottom(&self) -> u32 {
        self.y + self.height
    }

    /// 指定座標がこの領域に含まれるか。
    pub fn contains(&self, x: u32, y: u32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_calculation() {
        let region = ScreenRegion::new(100, 200, 80, 60);
        assert_eq!(region.center(), (140, 230));
    }

    #[test]
    fn contains_point() {
        let region = ScreenRegion::new(10, 20, 100, 50);
        assert!(region.contains(50, 40));
        assert!(!region.contains(5, 40));
        assert!(!region.contains(50, 80));
    }
}
