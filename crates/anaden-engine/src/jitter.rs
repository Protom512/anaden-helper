use std::time::Duration;

use crate::pipeline_runner::InputCommand;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JitterConfig {
    pub tap_radius_px: i32,
    pub interval_ratio: f64,
}

impl Default for JitterConfig {
    fn default() -> Self {
        Self {
            tap_radius_px: 3,
            interval_ratio: 0.15,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Jitter {
    cfg: JitterConfig,
    seed: u64,
    state: u64,
}

impl Jitter {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self::with_config(seed, JitterConfig::default())
    }

    #[must_use]
    pub fn with_config(seed: u64, cfg: JitterConfig) -> Self {
        Self {
            cfg,
            seed,
            state: seed,
        }
    }

    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn uniform_offset(&mut self, radius: i32) -> i32 {
        if radius <= 0 {
            return 0;
        }
        let span = 2 * radius as u64 + 1;
        (self.next_u64() % span) as i32 - radius
    }

    fn uniform_unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    #[must_use]
    pub fn jitter_point(&mut self, x: u32, y: u32) -> (u32, u32) {
        let r = self.cfg.tap_radius_px;
        let jx = (x as i64 + i64::from(self.uniform_offset(r))).max(0);
        let jy = (y as i64 + i64::from(self.uniform_offset(r))).max(0);
        (jx as u32, jy as u32)
    }

    #[must_use]
    pub fn jitter_command(&mut self, cmd: InputCommand) -> InputCommand {
        match cmd {
            InputCommand::Tap { x, y } => {
                let (jx, jy) = self.jitter_point(x, y);
                InputCommand::Tap { x: jx, y: jy }
            }
            InputCommand::Swipe { from, to } => {
                let (fx, fy) = self.jitter_point(from.0, from.1);
                let (tx, ty) = self.jitter_point(to.0, to.1);
                InputCommand::Swipe {
                    from: (fx, fy),
                    to: (tx, ty),
                }
            }
        }
    }

    #[must_use]
    pub fn jitter_interval(&mut self, base: Duration) -> Duration {
        if self.cfg.interval_ratio == 0.0 {
            return base;
        }
        let u = self.uniform_unit();
        let factor = 1.0 + self.cfg.interval_ratio * (2.0 * u - 1.0);
        let ms = base.as_millis() as f64 * factor;
        Duration::from_millis(ms.max(1.0) as u64)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_produces_identical_sequence() {
        let mut a = Jitter::new(42);
        let mut b = Jitter::new(42);
        for _ in 0..100 {
            assert_eq!(a.jitter_point(640, 360), b.jitter_point(640, 360));
        }
    }

    #[test]
    fn different_seed_produces_different_sequence() {
        let mut a = Jitter::new(1);
        let mut b = Jitter::new(2);
        let same = (0..100).all(|_| a.jitter_point(640, 360) == b.jitter_point(640, 360));
        assert!(!same, "different seeds must not produce identical sequence");
    }

    #[test]
    fn jittered_points_stay_in_radius_and_nonnegative() {
        let mut j = Jitter::new(7);
        let (cx, cy) = (640u32, 360u32);
        for _ in 0..500 {
            let (x, y) = j.jitter_point(cx, cy);
            let dx = (x as i64 - i64::from(cx)).abs();
            let dy = (y as i64 - i64::from(cy)).abs();
            assert!(dx <= 3, "x drift {dx} exceeds radius");
            assert!(dy <= 3, "y drift {dy} exceeds radius");
        }
        for _ in 0..200 {
            let (x, y) = j.jitter_point(0, 0);
            assert!(x <= 3 && y <= 3, "origin jitter must stay in [0, radius]");
        }
    }

    #[test]
    fn zero_radius_is_identity() {
        let mut j = Jitter::with_config(
            9,
            JitterConfig {
                tap_radius_px: 0,
                interval_ratio: 0.15,
            },
        );
        assert_eq!(j.jitter_point(100, 200), (100, 200));
    }

    #[test]
    fn interval_jitter_stays_within_ratio() {
        let mut j = Jitter::new(123);
        let base = Duration::from_millis(1000);
        let mut saw_low = false;
        let mut saw_high = false;
        for _ in 0..500 {
            let d = j.jitter_interval(base);
            let ms = d.as_millis() as f64;
            assert!(
                (850.0..=1150.0).contains(&ms),
                "jittered interval {ms}ms out of [850, 1150]"
            );
            saw_low |= ms < 1000.0;
            saw_high |= ms >= 1000.0;
        }
        assert!(
            saw_low && saw_high,
            "uniform jitter must produce both shorter and longer intervals"
        );
    }

    #[test]
    fn zero_ratio_interval_is_identity() {
        let mut j = Jitter::with_config(
            9,
            JitterConfig {
                tap_radius_px: 3,
                interval_ratio: 0.0,
            },
        );
        assert_eq!(
            j.jitter_interval(Duration::from_millis(777)),
            Duration::from_millis(777)
        );
    }

    #[test]
    fn tiny_interval_clamps_to_at_least_one_ms() {
        let mut j = Jitter::new(55);
        for _ in 0..100 {
            assert!(!j.jitter_interval(Duration::ZERO).is_zero());
        }
    }

    #[test]
    fn jitter_command_covers_tap_and_swipe() {
        let mut j = Jitter::new(3);
        if let InputCommand::Tap { x, y } = j.jitter_command(InputCommand::Tap { x: 500, y: 400 }) {
            assert!((497..=503).contains(&x) && (397..=403).contains(&y));
        } else {
            panic!("must remain a Tap");
        }
        if let InputCommand::Swipe { from, to } = j.jitter_command(InputCommand::Swipe {
            from: (10, 10),
            to: (600, 300),
        }) {
            assert!((7..=13).contains(&from.0) && (7..=13).contains(&from.1));
            assert!((597..=603).contains(&to.0) && (297..=303).contains(&to.1));
        } else {
            panic!("must remain a Swipe");
        }
    }

    #[test]
    fn seed_accessor_returns_construction_seed() {
        assert_eq!(Jitter::new(987_654_321).seed(), 987_654_321);
    }
}
