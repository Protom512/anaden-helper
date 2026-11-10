#![cfg(windows)]

use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

use anaden_device::point_on_any_monitor;

#[test]
#[allow(clippy::unwrap_used)]
fn primary_monitor_center_is_on_monitor() {
    let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    assert!(w > 0 && h > 0, "primary monitor metrics must be positive");
    let (cx, cy) = (w / 2, h / 2);
    assert!(
        point_on_any_monitor(cx, cy),
        "primary monitor center ({cx},{cy}) must be on a monitor"
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn far_offscreen_points_are_off_monitor() {
    assert!(
        !point_on_any_monitor(-100_000, -100_000),
        "far off-screen point must be off monitor"
    );
    assert!(
        !point_on_any_monitor(10_000_000, 10_000_000),
        "far off-screen point must be off monitor"
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn screen_origin_is_on_monitor() {
    assert!(
        point_on_any_monitor(0, 0),
        "the virtual desktop origin is inside the primary monitor"
    );
}
