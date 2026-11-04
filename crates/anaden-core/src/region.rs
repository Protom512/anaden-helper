use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScreenRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
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

    pub fn center(&self) -> (u32, u32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }

    pub fn right(&self) -> u32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> u32 {
        self.y + self.height
    }

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
