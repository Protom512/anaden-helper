//! 実演オーサリング (Issue #190 Shard 2/3) の座標変換純関数群。
//!
//! egui 非依存 (egui::Pos2/Rect を持ち込まずプレーンな数値で表現) で、
//! 以下の 4 座標空間の変換を単体テスト可能にする ([`crate::canvas::show`] の
//! インライン変換を抽出・一般化したもの):
//!
//! | 空間 | 表現 | 由来 |
//! |------|------|------|
//! | ウィジェット | `(f32, f32)` | ライブビュー描画矩形内の位置 (egui 画面座標) |
//! | フレームピクセル | `(u32, u32)` | 表示フレーム (= 正規化後スクリーンショット) のピクセル |
//! | 正規化 | `(f32, f32)` | 幅 [`BASE_WIDTH`] (1280) 基準 — TaskDef ROI の定義空間 |
//! | クライアント | `(i32, i32)` | SendInput 注入用・対象ウィンドウ左上原点の物理 px |
//!
//! 正規化は [`anaden_vision::ScreenScaler`] と同じ「幅基準の単一ファクタ」(縦横同一倍率) で
//! あるため、逆変換も単一ファクタで厳密に戻る。PC版キャプチャはクライアント
//! 領域をそのまま撮る (GetClientRect 寸法 = 生フレーム寸法) ため、正規化空間 ↔
//! クライアント空間の往復で元の実ピクセル座標が復元される。

use anaden_vision::BASE_WIDTH;

/// ウィジェット上の描画矩形 (egui Rect と同じ情報を持つプレーン表現)。
///
/// `left`/`top` は egui 画面座標・`width`/`height` はポイント寸法。
/// キャンバス側で `egui::Rect` から本型へ変換して純関数へ渡す。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewRect {
    /// 矩形左端 (egui 画面座標)。
    pub left: f32,
    /// 矩形上端。
    pub top: f32,
    /// 幅 (ポイント)。
    pub width: f32,
    /// 高さ (ポイント)。
    pub height: f32,
}

impl ViewRect {
    /// 点が矩形内にあるか (egui `Rect::contains` と同じく境界を含む)。
    #[must_use]
    pub fn contains(&self, p: (f32, f32)) -> bool {
        p.0 >= self.left
            && p.0 <= self.left + self.width
            && p.1 >= self.top
            && p.1 <= self.top + self.height
    }
}

/// ウィジェット座標 → フレームピクセル座標。
///
/// 矩形外・退化寸法 (フレームまたは矩形の幅/高さ 0) は `None`。矩形内の点は
/// フレームの最終有効ピクセル (`w-1` / `h-1`) へクランプされる
/// ([`crate::canvas::show`] の `to_img` と同一契約)。
#[must_use]
pub fn widget_to_frame(p: (f32, f32), rect: ViewRect, frame: (u32, u32)) -> Option<(u32, u32)> {
    if frame.0 == 0 || frame.1 == 0 || rect.width <= 0.0 || rect.height <= 0.0 {
        return None;
    }
    if !rect.contains(p) {
        return None;
    }
    let fx = ((p.0 - rect.left) / rect.width).clamp(0.0, 1.0) * frame.0 as f32;
    let fy = ((p.1 - rect.top) / rect.height).clamp(0.0, 1.0) * frame.1 as f32;
    Some((
        fx.min(frame.0 as f32 - 1.0) as u32,
        fy.min(frame.1 as f32 - 1.0) as u32,
    ))
}

/// フレームピクセル座標 → ウィジェット座標 (オーバーレイ描画用の逆変換)。
///
/// 退化寸法時は矩形左上を返す (描画側で無害)。[`crate::canvas::show`] の
/// `img_to_screen` と同一式。
#[must_use]
pub fn frame_to_widget(p: (u32, u32), rect: ViewRect, frame: (u32, u32)) -> (f32, f32) {
    if frame.0 == 0 || frame.1 == 0 {
        return (rect.left, rect.top);
    }
    (
        rect.left + (p.0 as f32 / frame.0 as f32) * rect.width,
        rect.top + (p.1 as f32 / frame.1 as f32) * rect.height,
    )
}

/// フレームピクセル座標 → 正規化空間 (幅 [`BASE_WIDTH`] 基準・f32)。
///
/// フレーム幅が既に [`BASE_WIDTH`] (正規化済みスクリーンショット) なら恒等。
/// フレーム寸法 0 は `None` (ゼロ除算回避)。
#[must_use]
pub fn frame_to_normalized(p: (u32, u32), frame: (u32, u32)) -> Option<(f32, f32)> {
    if frame.0 == 0 || frame.1 == 0 {
        return None;
    }
    let s = BASE_WIDTH as f32 / frame.0 as f32;
    Some((p.0 as f32 * s, p.1 as f32 * s))
}

/// 正規化空間 (幅 [`BASE_WIDTH`] 基準) → フレームピクセル座標 (round + 端クランプ)。
///
/// フレーム寸法 0 は `None`。
#[must_use]
pub fn normalized_to_frame(p: (f32, f32), frame: (u32, u32)) -> Option<(u32, u32)> {
    from_base(p, frame)
}

/// 正規化空間 (幅 [`BASE_WIDTH`] 基準) → クライアント座標 (SendInput 用・i32)。
///
/// `client` は対象ウィンドウのクライアント領域寸法 (物理 px)。PC版キャプチャは
/// クライアント領域をそのまま撮るため生フレーム寸法と一致し、正規化前の実
/// ピクセル座標が復元される。round + 端クランプ (発火座標がクライアント外に
/// 出ないようにする)。クライアント寸法 0 は `None`。
#[must_use]
pub fn normalized_to_client(p: (f32, f32), client: (u32, u32)) -> Option<(i32, i32)> {
    from_base(p, client).map(|(x, y)| (x as i32, y as i32))
}

/// 正規化空間 → 幅 `dst.0` の対象空間 (round + 端クランプ・共通コア)。
fn from_base(p: (f32, f32), dst: (u32, u32)) -> Option<(u32, u32)> {
    if dst.0 == 0 || dst.1 == 0 {
        return None;
    }
    let s = dst.0 as f32 / BASE_WIDTH as f32;
    let x = (p.0 * s).round().clamp(0.0, (dst.0 - 1) as f32) as u32;
    let y = (p.1 * s).round().clamp(0.0, (dst.1 - 1) as f32) as u32;
    Some((x, y))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    // ---- ウィジェット ↔ フレームピクセル ----

    /// 恒等: 描画矩形 == フレーム寸法なら座標も一致する。
    #[test]
    fn widget_to_frame_identity_when_rect_matches_frame() {
        let rect = ViewRect {
            left: 0.0,
            top: 0.0,
            width: 1280.0,
            height: 720.0,
        };
        assert_eq!(widget_to_frame((0.0, 0.0), rect, (1280, 720)), Some((0, 0)));
        assert_eq!(
            widget_to_frame((640.0, 360.0), rect, (1280, 720)),
            Some((640, 360))
        );
    }

    /// スケール: 2 倍のキャンバスでは座標が半分になる。
    #[test]
    fn widget_to_frame_scales_on_larger_canvas() {
        let rect = ViewRect {
            left: 0.0,
            top: 0.0,
            width: 2560.0,
            height: 1440.0,
        };
        assert_eq!(
            widget_to_frame((1280.0, 720.0), rect, (1280, 720)),
            Some((640, 360))
        );
    }

    /// 端点: 矩形右下端はフレーム最終有効ピクセル (w-1, h-1) へクランプされる。
    #[test]
    fn widget_to_frame_endpoints_map_to_valid_pixels() {
        let rect = ViewRect {
            left: 0.0,
            top: 0.0,
            width: 640.0,
            height: 360.0,
        };
        // 右下端 (境界含む) → (1279, 719)。フレーム幅そのもの (1280) にはならない。
        assert_eq!(
            widget_to_frame((640.0, 360.0), rect, (1280, 720)),
            Some((1279, 719))
        );
        assert_eq!(widget_to_frame((0.0, 0.0), rect, (1280, 720)), Some((0, 0)));
    }

    /// 矩形外の点は `None`。
    #[test]
    fn widget_to_frame_outside_rect_is_none() {
        let rect = ViewRect {
            left: 10.0,
            top: 20.0,
            width: 600.0,
            height: 800.0,
        };
        assert_eq!(widget_to_frame((9.9, 400.0), rect, (300, 400)), None);
        assert_eq!(widget_to_frame((610.1, 400.0), rect, (300, 400)), None);
        assert_eq!(widget_to_frame((400.0, 19.9), rect, (300, 400)), None);
        assert_eq!(widget_to_frame((400.0, 820.1), rect, (300, 400)), None);
    }

    /// オフセット付き・非正方形キャンバスでも軸ごとに正しくスケールする。
    #[test]
    fn widget_to_frame_handles_offset_and_portrait_canvas() {
        let rect = ViewRect {
            left: 10.0,
            top: 20.0,
            width: 600.0,
            height: 800.0,
        };
        // 矩形中心 (10+300, 20+400) → フレーム (150, 200)。
        assert_eq!(
            widget_to_frame((310.0, 420.0), rect, (300, 400)),
            Some((150, 200))
        );
        // 4 分割点: 縦横独立にスケールすることを確認。
        assert_eq!(
            widget_to_frame((160.0, 220.0), rect, (300, 400)),
            Some((75, 100))
        );
    }

    /// 往復: ウィジェット → フレーム → ウィジェットは元の点 (半ピクセル精度内) に戻る。
    #[test]
    fn widget_frame_roundtrip_stays_within_half_pixel() {
        let rect = ViewRect {
            left: 5.0,
            top: 7.0,
            width: 777.0,
            height: 444.0,
        };
        let frame = (1280, 720);
        for w in [5.0, 100.5, 394.25, 680.9, 781.9] {
            for h in [7.0, 50.5, 228.75, 400.1, 450.9] {
                let Some(f) = widget_to_frame((w, h), rect, frame) else {
                    panic!("inside point ({w},{h}) must map");
                };
                let back = frame_to_widget(f, rect, frame);
                assert!(
                    (back.0 - w).abs() <= rect.width / frame.0 as f32 + 0.5,
                    "x roundtrip drifted: {w} -> {f:?} -> {back:?}"
                );
                assert!(
                    (back.1 - h).abs() <= rect.height / frame.1 as f32 + 0.5,
                    "y roundtrip drifted: {h} -> {f:?} -> {back:?}"
                );
            }
        }
    }

    /// 退化寸法 (フレーム 0 / 矩形 0) は `None`。
    #[test]
    fn widget_to_frame_degenerate_dims_are_none() {
        let rect = ViewRect {
            left: 0.0,
            top: 0.0,
            width: 100.0,
            height: 100.0,
        };
        assert_eq!(widget_to_frame((50.0, 50.0), rect, (0, 100)), None);
        assert_eq!(widget_to_frame((50.0, 50.0), rect, (100, 0)), None);
        let zero = ViewRect {
            left: 0.0,
            top: 0.0,
            width: 0.0,
            height: 0.0,
        };
        assert_eq!(widget_to_frame((0.0, 0.0), zero, (100, 100)), None);
    }

    // ---- フレームピクセル ↔ 正規化空間 ----

    /// 正規化済みフレーム (幅 1280) では正規化空間へ恒等。
    #[test]
    fn frame_to_normalized_identity_at_base_width() {
        assert_eq!(
            frame_to_normalized((640, 360), (1280, 720)),
            Some((640.0, 360.0))
        );
    }

    /// 2 倍幅フレーム (2560) では座標が半分の正規化値になる。
    #[test]
    fn frame_to_normalized_scales_by_width_ratio() {
        assert_eq!(
            frame_to_normalized((1280, 720), (2560, 1440)),
            Some((640.0, 360.0))
        );
        // PC 版実測 1258x708 → 1280 基準 (ScreenScaler と同一の単一ファクタ:
        // 縦横とも 1280/1258 倍 — 720/708 倍ではないため y=354 は 360.19… へ)。
        let (x, y) = frame_to_normalized((629, 354), (1258, 708)).unwrap();
        let s = BASE_WIDTH as f32 / 1258.0;
        assert!((x - 640.0).abs() < 0.01, "x: {x}");
        assert!((y - 354.0 * s).abs() < 0.01, "y: {y}");
    }

    /// 往復: フレーム → 正規化 → フレームで元のピクセルに戻る (one-to-one)。
    #[test]
    fn frame_normalized_frame_roundtrip_restores_pixel() {
        for frame in [(1280u32, 720u32), (2560, 1440), (1258, 708), (1280, 576)] {
            for (x, y) in [(0u32, 0u32), (1, 1), (100, 60), (629, 354)] {
                if x >= frame.0 || y >= frame.1 {
                    continue;
                }
                let n = frame_to_normalized((x, y), frame).unwrap();
                assert_eq!(
                    normalized_to_frame(n, frame),
                    Some((x, y)),
                    "roundtrip failed for ({x},{y}) in {frame:?}"
                );
            }
        }
    }

    // ---- 正規化空間 → クライアント (SendInput 用) ----

    /// PC 版実測寸法 (1258x708) で正規化前の実ピクセル座標が復元される。
    #[test]
    fn normalized_to_client_restores_raw_client_pixels() {
        // 生 (629,354) → 正規化 (≈640,≈360) → クライアント (629,354)。
        let n = frame_to_normalized((629, 354), (1258, 708)).unwrap();
        assert_eq!(normalized_to_client(n, (1258, 708)), Some((629, 354)));
    }

    /// 端クランプ: 正規化空間の外側 (過大/負) はクライアント有効範囲に収まる。
    #[test]
    fn normalized_to_client_clamps_to_client_bounds() {
        assert_eq!(
            normalized_to_client((5000.0, 5000.0), (1258, 708)),
            Some((1257, 707))
        );
        assert_eq!(
            normalized_to_client((-10.0, -10.0), (1258, 708)),
            Some((0, 0))
        );
    }

    /// クライアント寸法 0 は `None`。
    #[test]
    fn normalized_to_client_zero_client_is_none() {
        assert_eq!(normalized_to_client((640.0, 360.0), (0, 708)), None);
        assert_eq!(normalized_to_client((640.0, 360.0), (1258, 0)), None);
    }
}
