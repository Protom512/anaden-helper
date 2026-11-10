use anaden_vision::{CaptureModeVerdict, capture_mode_verdict};

#[test]
#[allow(clippy::unwrap_used)]
fn clean_captures_from_live_20261110_session_verdict_clean() {
    let clean_pairs: [(u32, u32); 5] = [
        (3783, 3773),
        (3783, 3783),
        (1952, 1950),
        (2880, 2880),
        (3783, 3700),
    ];
    for (raw_w, crop_w) in clean_pairs {
        assert_eq!(
            capture_mode_verdict(raw_w, crop_w),
            CaptureModeVerdict::Clean,
            "clean live capture class (raw={raw_w}, content={crop_w}, band<40px) must stay Clean"
        );
    }
}

#[test]
#[allow(clippy::unwrap_used)]
fn chromed_captures_from_live_20261110_session_verdict_chromed() {
    let chromed_pairs: [(u32, u32); 3] = [(3783, 3646), (3783, 3300), (3783, 2000)];
    for (raw_w, crop_w) in chromed_pairs {
        assert_eq!(
            capture_mode_verdict(raw_w, crop_w),
            CaptureModeVerdict::Chromed,
            "chrome-mode DWM fallback leaves a right black band >=100px (live: content 3646/3783) \
             and must be flagged Chromed"
        );
    }
}
