#![cfg(windows)]

use anaden_device::clamped_top_left;

const MONITOR_1080P: (i32, i32, i32, i32) = (0, 0, 1920, 1080);
const MONITOR_4K: (i32, i32, i32, i32) = (0, 0, 3840, 2160);
const MONITOR_LEFT_NEGATIVE: (i32, i32, i32, i32) = (-1920, 0, 0, 1080);

#[test]
fn window_fully_inside_needs_no_clamp() {
    assert_eq!(
        clamped_top_left((100, 100, 500, 400), MONITOR_4K),
        None,
        "a window fully inside its monitor must not move"
    );
}

#[test]
fn window_exactly_filling_monitor_needs_no_clamp() {
    assert_eq!(
        clamped_top_left((0, 0, 3840, 2160), MONITOR_4K),
        None,
        "inclusive edges count as inside"
    );
}

#[test]
fn window_overflowing_left_is_pushed_to_monitor_left() {
    assert_eq!(
        clamped_top_left((-100, 100, 400, 400), MONITOR_4K),
        Some((0, 100))
    );
}

#[test]
fn window_overflowing_top_is_pushed_to_monitor_top() {
    assert_eq!(
        clamped_top_left((100, -100, 500, 300), MONITOR_4K),
        Some((100, 0))
    );
}

#[test]
fn window_overflowing_right_is_pulled_back_by_its_width() {
    assert_eq!(
        clamped_top_left((3500, 100, 4000, 400), MONITOR_4K),
        Some((3340, 100))
    );
}

#[test]
fn window_overflowing_bottom_is_pulled_up_by_its_height() {
    assert_eq!(
        clamped_top_left((100, 2000, 500, 2300), MONITOR_4K),
        Some((100, 1860))
    );
}

#[test]
fn window_on_secondary_monitor_with_negative_origin_is_left_alone() {
    assert_eq!(
        clamped_top_left((-1000, 100, -700, 400), MONITOR_LEFT_NEGATIVE),
        None
    );
}

#[test]
fn window_overflowing_left_of_negative_origin_monitor_is_clamped_to_it() {
    assert_eq!(
        clamped_top_left((-2000, 100, -1700, 400), MONITOR_LEFT_NEGATIVE),
        Some((-1920, 100))
    );
}

#[test]
fn window_on_second_monitor_to_the_right_is_left_alone() {
    assert_eq!(
        clamped_top_left((2000, 100, 2500, 400), (1920, 0, 3840, 1080)),
        None
    );
}

#[test]
fn window_wider_than_monitor_is_left_aligned_not_centered() {
    assert_eq!(
        clamped_top_left((-100, 0, 1300, 500), (0, 0, 1280, 720)),
        Some((0, 0))
    );
}

#[test]
fn window_taller_than_monitor_is_top_aligned() {
    assert_eq!(
        clamped_top_left((100, -100, 600, 900), (0, 0, 1280, 720)),
        Some((100, 0))
    );
}

#[test]
fn the_def012_live_placement_is_clamped_to_the_primary_monitor() {
    assert_eq!(
        clamped_top_left((-2200, -2200, 1599, -33), MONITOR_4K),
        Some((0, 0))
    );
}

#[test]
fn window_below_the_monitor_is_pulled_up() {
    assert_eq!(
        clamped_top_left((300, 2200, 800, 2500), MONITOR_1080P),
        Some((300, 780))
    );
}
