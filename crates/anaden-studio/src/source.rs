use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use image::{DynamicImage, GrayImage};

#[cfg(windows)]
pub use anaden_device::DEFAULT_PROCESS_NAME;
#[cfg(windows)]
use anaden_device::Win32Capture;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Target {
    #[default]
    Windows,
}

pub struct LiveCapture {
    rx: Receiver<Arc<DynamicImage>>,
    stop: Arc<AtomicBool>,
}

impl LiveCapture {
    pub fn start(interval_ms: u64, exe: &str) -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        Self::start_windows_inner(rx, tx, stop, stop_thread, exe, interval_ms)
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn latest(&self) -> Option<Arc<DynamicImage>> {
        let mut latest = None;
        while let Ok(img) = self.rx.try_recv() {
            latest = Some(img);
        }
        latest
    }

    #[cfg(windows)]
    fn start_windows_inner(
        rx: Receiver<Arc<DynamicImage>>,
        tx: mpsc::SyncSender<Arc<DynamicImage>>,
        stop: Arc<AtomicBool>,
        stop_thread: Arc<AtomicBool>,
        exe: &str,
        interval_ms: u64,
    ) -> Self {
        let capture = Arc::new(Win32Capture::new_without_dpi(exe));
        let capture_thread = capture.clone();

        thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                if let Some(img) = capture_windows(&capture_thread) {
                    let _ = tx.try_send(Arc::new(img));
                }
                sleep_until_next(&stop_thread, interval_ms);
            }
        });

        Self { rx, stop }
    }

    #[cfg(not(windows))]
    fn start_windows_inner(
        rx: Receiver<Arc<DynamicImage>>,
        _tx: mpsc::SyncSender<Arc<DynamicImage>>,
        stop: Arc<AtomicBool>,
        _stop_thread: Arc<AtomicBool>,
        _exe: &str,
        _interval_ms: u64,
    ) -> Self {
        Self { rx, stop }
    }
}

fn sleep_until_next(stop: &AtomicBool, interval_ms: u64) {
    let mut waited = 0u64;
    while waited < interval_ms && !stop.load(Ordering::Relaxed) {
        let step = waited.saturating_add(50).min(interval_ms) - waited;
        thread::sleep(Duration::from_millis(step));
        waited += step;
    }
}

#[cfg(windows)]
fn capture_windows(capture: &Win32Capture) -> Option<DynamicImage> {
    let img = capture.capture_blocking().ok()?;
    if is_black_frame(&img) {
        return None;
    }
    Some(img)
}

impl Drop for LiveCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

const BLACK_FRAME_MEAN_THRESHOLD: f32 = 10.0;

fn is_black_frame(img: &DynamicImage) -> bool {
    let gray: GrayImage = img.to_luma8();
    let pixels: &[u8] = gray.as_raw();
    if pixels.is_empty() {
        return true;
    }
    let sum: u64 = pixels.iter().map(|&v| v as u64).sum();
    let mean = sum as f32 / pixels.len() as f32;
    mean < BLACK_FRAME_MEAN_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_frame_detected_for_pure_black() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([0, 0, 0])));
        assert!(is_black_frame(&img));
    }

    #[test]
    fn black_frame_not_detected_for_bright() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            8,
            8,
            image::Rgb([200, 200, 200]),
        ));
        assert!(!is_black_frame(&img));
    }

    #[test]
    fn bounded_channel_never_accumulates_more_than_one_frame() {
        let (tx, rx) = mpsc::sync_channel::<i32>(1);
        assert!(tx.try_send(1).is_ok());
        assert!(tx.try_send(2).is_err());
        assert!(tx.try_send(3).is_err());
        assert!(tx.try_send(4).is_err());
        let drained: Vec<i32> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(drained, vec![1]);
        assert!(tx.try_send(99).is_ok());
    }

    #[test]
    fn latest_drains_to_newest() {
        let (tx, rx) = mpsc::sync_channel::<i32>(2);
        let _ = tx.try_send(1);
        let _ = tx.try_send(2);
        let mut latest = None;
        while let Ok(v) = rx.try_recv() {
            latest = Some(v);
        }
        assert_eq!(latest, Some(2));
    }

    #[test]
    fn black_frame_threshold_is_pinned_to_10() {
        assert_eq!(BLACK_FRAME_MEAN_THRESHOLD, 10.0);
    }

    #[test]
    fn black_frame_boundary_strictly_below_threshold() {
        let dark =
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([9, 9, 9])));
        assert!(is_black_frame(&dark), "mean=9 must be black (below 10.0)");

        let edge =
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([10, 10, 10])));
        assert!(
            !is_black_frame(&edge),
            "mean=10 must NOT be black (strictly below)"
        );

        let bright =
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([11, 11, 11])));
        assert!(!is_black_frame(&bright), "mean=11 must not be black");
    }

    #[test]
    fn black_frame_for_empty_image() {
        let empty = DynamicImage::ImageRgb8(image::RgbImage::new(0, 0));
        assert!(is_black_frame(&empty));
    }
}
