//! ヒューマンライクな入力ゆらぎ — タップ座標ジッタと interval ランダム化 (Issue #253)。
//!
//! 現行 [`crate::PipelineDriver`] は固定 interval・ROI 中心の固定タップで、操作パターンに
//! 分散がない。デイリー自動化を毎日 0/6/12/18 時に動かす運用では一定さが目立つため、
//! タップ座標に ±`tap_radius_px` px (normalize 後 1280 空間) の一様ジッタ、
//! ループ間隔に ±`interval_ratio` (±10-20%) のジッタを加える。
//!
//! # 再現性 (evidence 規約との整合)
//!
//! 乱数は外部 crate に依存しない決定論的 SplitMix64。シードは [`Jitter::seed`] で
//! 取得でき、[`crate::ProgressReport::jitter_seed`] 経由で report.json に記録される。
//! 同一シード + 同一タスク列 → 同一ゆらぎ列を完全再現できる。
//!
//! # 適用空間
//!
//! タップ座標のジッタは **normalize 後 1280 空間** で適用する (デバイス解像度非依存。
//! 1280 空間の ±3px は実機 1484px 幅で約 ±3.5px に相当)。rescale (実機座標への逆変換)
//! の前段で挿入することで、どの解像度でも同じ「人間の指ぶれ」の大きさになる。

use std::time::Duration;

use crate::pipeline_runner::InputCommand;

/// ジッタの強度パラメータ。
///
/// 既定値は Issue #253 仕様案の中央値 (タップ ±3px / interval ±15%)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JitterConfig {
    /// タップ座標ジッタの半径 (px, normalize 後 1280 空間)。[-r, +r] の一様分布。
    /// 0 で座標ジッタ無効。
    pub tap_radius_px: i32,
    /// interval ジッタの比率 (0.15 = ±15%)。factor = 1 ± ratio の一様分布。
    /// 0.0 で interval ジッタ無効。
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

/// シード指定可能な決定論的ジッタ発生器 (SplitMix64)。
///
/// [`PipelineDriver`](crate::PipelineDriver) の `with_jitter` に渡すと、
/// 発火タップ座標とループ interval にゆらぎを加える。
/// 内部状態は SplitMix64 の 64bit 1 個のみで、シードから完全決定論的に推移する。
/// (設定に f64 を含むため [`Eq`] は導出しない。)
#[derive(Debug, Clone, PartialEq)]
pub struct Jitter {
    cfg: JitterConfig,
    seed: u64,
    state: u64,
}

impl Jitter {
    /// 既定強度 ([`JitterConfig::default`]) でシードを固定して作る。
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self::with_config(seed, JitterConfig::default())
    }

    /// 強度を明示して作る。
    #[must_use]
    pub fn with_config(seed: u64, cfg: JitterConfig) -> Self {
        // 状態初期値が 0 でも SplitMix64 は問題なく推移する (golden gamma 加算方式)。
        Self {
            cfg,
            seed,
            state: seed,
        }
    }

    /// このインスタンスのシード (report.json 記録用)。
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// SplitMix64: 決定論的 64bit 乱数を 1 つ進める。
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// [-radius, +radius] の一様整数。
    fn uniform_offset(&mut self, radius: i32) -> i32 {
        if radius <= 0 {
            return 0;
        }
        let span = 2 * radius as u64 + 1;
        (self.next_u64() % span) as i32 - radius
    }

    /// [0, 1) の一様 f64。
    fn uniform_unit(&mut self) -> f64 {
        // 上位 53bit を使用 (f64 仮数部_full)。
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// タップ座標 1 点にジッタを適用する。負座標へは飽和 (0 未満にしない)。
    #[must_use]
    pub fn jitter_point(&mut self, x: u32, y: u32) -> (u32, u32) {
        let r = self.cfg.tap_radius_px;
        let jx = (x as i64 + i64::from(self.uniform_offset(r))).max(0);
        let jy = (y as i64 + i64::from(self.uniform_offset(r))).max(0);
        (jx as u32, jy as u32)
    }

    /// 発火コマンド (normalize 後 1280 空間) にジッタを適用する。
    /// Tap は座標、Swipe は始点・終点それぞれ独立に揺らす。
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

    /// ループ間隔にジッタを適用する。
    ///
    /// `base` を中心に ±`interval_ratio` の一様分布で揺らす。
    /// 計算はミリ秒精度。結果は最低 1ms にクランプ (0ms interval がジッタで
    /// さらに 0 になることはあっても、負や ゼロ未満にはならない)。
    /// `interval_ratio == 0.0` のときは `base` をそのまま返す。
    #[must_use]
    pub fn jitter_interval(&mut self, base: Duration) -> Duration {
        if self.cfg.interval_ratio == 0.0 {
            return base;
        }
        let u = self.uniform_unit(); // [0, 1)
        let factor = 1.0 + self.cfg.interval_ratio * (2.0 * u - 1.0); // [1-r, 1+r)
        let ms = base.as_millis() as f64 * factor;
        Duration::from_millis(ms.max(1.0) as u64)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    /// 同一シード → 同一系列 (完全決定論)。
    #[test]
    fn same_seed_produces_identical_sequence() {
        let mut a = Jitter::new(42);
        let mut b = Jitter::new(42);
        for _ in 0..100 {
            assert_eq!(a.jitter_point(640, 360), b.jitter_point(640, 360));
        }
    }

    /// 異シード → (ほぼ確実に) 異系列。100 点連続一致は非現実的なので弁別できる。
    #[test]
    fn different_seed_produces_different_sequence() {
        let mut a = Jitter::new(1);
        let mut b = Jitter::new(2);
        let same = (0..100).all(|_| a.jitter_point(640, 360) == b.jitter_point(640, 360));
        assert!(!same, "different seeds must not produce identical sequence");
    }

    /// ジッタ後の座標は常に半径内かつ 0 以上。
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
        // 境界: 原点でも負にならない (負オフセットは 0 に飽和、正オフセットはそのまま)。
        for _ in 0..200 {
            let (x, y) = j.jitter_point(0, 0);
            assert!(x <= 3 && y <= 3, "origin jitter must stay in [0, radius]");
        }
    }

    /// radius 0 は座標を変えない (無効化スイッチ)。
    #[test]
    fn zero_radius_is_identity() {
        let mut j = Jitter::with_config(9, JitterConfig {
            tap_radius_px: 0,
            interval_ratio: 0.15,
        });
        assert_eq!(j.jitter_point(100, 200), (100, 200));
    }

    /// interval ジッタは比率内に収まる。長い系列で上下限を少なくとも一度は踏む
    /// (一様性の緩い健全性チェック) ことも確認。
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

    /// ratio 0 は interval を変えない (無効化スイッチ)。
    #[test]
    fn zero_ratio_interval_is_identity() {
        let mut j = Jitter::with_config(9, JitterConfig {
            tap_radius_px: 3,
            interval_ratio: 0.0,
        });
        assert_eq!(j.jitter_interval(Duration::from_millis(777)), Duration::from_millis(777));
    }

    /// 1ms 未満の base でも最低 1ms にクランプされる (ゼロ割・負防止)。
    #[test]
    fn tiny_interval_clamps_to_at_least_one_ms() {
        let mut j = Jitter::new(55);
        for _ in 0..100 {
            assert!(!j.jitter_interval(Duration::ZERO).is_zero());
        }
    }

    /// jitter_command は Tap/Swipe 両バリアントを処理する。
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

    /// seed() は構築シードを返す (report.json 記録との一致)。
    #[test]
    fn seed_accessor_returns_construction_seed() {
        assert_eq!(Jitter::new(987_654_321).seed(), 987_654_321);
    }
}
