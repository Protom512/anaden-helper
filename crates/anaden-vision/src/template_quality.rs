use std::path::Path;

use image::DynamicImage;

pub const TEMPLATE_MIN_LUMA_STDDEV: f32 = 20.0;

#[must_use]
pub fn template_luma_stddev(img: &DynamicImage) -> f32 {
    let gray = img.to_luma8();
    let mut n: f64 = 0.0;
    let mut sum: f64 = 0.0;
    let mut sum_sq: f64 = 0.0;
    for p in gray.pixels() {
        let v = f64::from(p.0[0]);
        n += 1.0;
        sum += v;
        sum_sq += v * v;
    }
    if n == 0.0 {
        return 0.0;
    }
    let mean = sum / n;
    let var = (sum_sq / n - mean * mean).max(0.0);
    var.sqrt() as f32
}

#[must_use]
pub fn template_is_structured(img: &DynamicImage) -> bool {
    template_luma_stddev(img) >= TEMPLATE_MIN_LUMA_STDDEV
}

pub(crate) fn unstructured_warning_for_path(path: &Path) -> Option<String> {
    let img = image::open(path).ok()?;
    let stddev = template_luma_stddev(&img);
    if stddev >= TEMPLATE_MIN_LUMA_STDDEV {
        return None;
    }
    Some(format!(
        "template {} is nearly uniform (luma stddev {:.2} < {:.1}): it can never match \
         under TM_CCOEFF_NORMED — permanent NoMatch expected (Issue #182: an unstructured \
         template measured stddev 3.76 and never fired in 65 iterations)",
        path.display(),
        stddev,
        TEMPLATE_MIN_LUMA_STDDEV
    ))
}

#[must_use]
pub fn needle_fits_roi(needle: (u32, u32), roi: Option<[u32; 4]>) -> bool {
    let [_, _, rw, rh] = match roi {
        None => return true,
        Some(r) => r,
    };
    rw == 0 || rh == 0 || (needle.0 <= rw && needle.1 <= rh)
}

pub(crate) fn needle_exceeds_roi_warning_for_path(
    path: &Path,
    roi: Option<[u32; 4]>,
) -> Option<String> {
    let (nw, nh) = image::image_dimensions(path).ok()?;
    let [_, _, rw, rh] = roi?;
    if needle_fits_roi((nw, nh), roi) {
        return None;
    }
    Some(format!(
        "template {} is {nw}x{nh} but roi is {rw}x{rh}: the needle cannot fit inside the \
         roi — permanent NoMatch expected (Issue #182: a regenerated 138px-wide needle \
         against a 121px-wide roi never fired)",
        path.display()
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    fn dyn_img(img: GrayImage) -> DynamicImage {
        DynamicImage::ImageLuma8(img)
    }

    #[test]
    fn flat_image_is_not_structured() {
        let flat = dyn_img(GrayImage::from_pixel(64, 32, Luma([255u8])));
        assert_eq!(template_luma_stddev(&flat), 0.0);
        assert!(!template_is_structured(&flat));
    }

    #[test]
    fn near_flat_low_contrast_image_is_not_structured() {
        let mut img = GrayImage::new(64, 32);
        for y in 0..32 {
            for x in 0..64 {
                let v = 100 + ((x + y) % 5) as u8;
                img.put_pixel(x, y, Luma([v]));
            }
        }
        let img = dyn_img(img);
        let stddev = template_luma_stddev(&img);
        assert!(
            (1.0..2.0).contains(&stddev),
            "near-flat stddev should be ~1.2, got {stddev}"
        );
        assert!(!template_is_structured(&img));
    }

    #[test]
    fn structured_gradient_image_is_structured() {
        let mut gray = GrayImage::new(64, 32);
        for y in 0..32 {
            for x in 0..64 {
                let v = ((x * 2 + y * 3) % 200) as u8;
                gray.put_pixel(x, y, Luma([v]));
            }
        }
        let img = dyn_img(gray);
        let stddev = template_luma_stddev(&img);
        assert!(
            stddev > TEMPLATE_MIN_LUMA_STDDEV,
            "gradient stddev {stddev:.2} must exceed threshold"
        );
        assert!(template_is_structured(&img));
    }

    #[test]
    fn rgba_image_is_judged_after_luma_conversion() {
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            64,
            32,
            image::Rgba([255u8, 255, 255, 255]),
        ));
        assert!(!template_is_structured(&img));
    }

    #[test]
    fn empty_image_returns_zero_stddev() {
        let empty = dyn_img(GrayImage::new(0, 0));
        assert_eq!(template_luma_stddev(&empty), 0.0);
        assert!(!template_is_structured(&empty));
    }

    #[test]
    fn template_min_luma_stddev_sits_strictly_between_measured_values() {
        const ISSUE_182_UNSTRUCTURED_MEASURED: f32 = 3.76;
        const ISSUE_182_STRUCTURED_MEASURED: f32 = 53.1;
        const _: () = assert!(
            ISSUE_182_UNSTRUCTURED_MEASURED < TEMPLATE_MIN_LUMA_STDDEV
                && TEMPLATE_MIN_LUMA_STDDEV < ISSUE_182_STRUCTURED_MEASURED,
            "TEMPLATE_MIN_LUMA_STDDEV must sit strictly between the Issue #182 measured \
             values (unstructured 3.76 / structured 53.1)"
        );
    }

    #[test]
    fn unstructured_warning_for_path_flags_flat_png() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("flat.png");
        dyn_img(GrayImage::from_pixel(64, 32, Luma([250u8])))
            .save(&path)
            .expect("save flat png");

        let warn = unstructured_warning_for_path(&path).expect("flat png must be flagged");
        assert!(
            warn.contains("stddev") && warn.contains("0.00"),
            "warning should cite the measured stddev: {warn}"
        );
        assert!(
            warn.contains("TM_CCOEFF_NORMED"),
            "warning should explain the permanent-NoMatch consequence: {warn}"
        );
    }

    #[test]
    fn unstructured_warning_for_path_passes_structured_png() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("structured.png");
        let mut img = GrayImage::new(64, 32);
        for y in 0..32 {
            for x in 0..64 {
                let v = ((x * 2 + y * 3) % 200) as u8;
                img.put_pixel(x, y, Luma([v]));
            }
        }
        dyn_img(img).save(&path).expect("save structured png");
        assert!(unstructured_warning_for_path(&path).is_none());
    }

    #[test]
    fn unstructured_warning_for_path_tolerates_missing_and_invalid_files() {
        let missing = unstructured_warning_for_path(Path::new("/nonexistent/no-such.png"));
        assert!(missing.is_none(), "missing file must not be flagged");

        let tmp = tempfile::tempdir().expect("tempdir");
        let not_png = tmp.path().join("not-a-png.png");
        std::fs::write(&not_png, b"this is not a png").expect("write junk");
        assert!(
            unstructured_warning_for_path(&not_png).is_none(),
            "undecodable file must not be flagged"
        );
    }

    #[test]
    fn needle_fits_roi_when_dims_do_not_exceed_roi() {
        assert!(needle_fits_roi((80, 40), Some([820, 470, 200, 90])));
        assert!(needle_fits_roi((100, 50), Some([10, 20, 100, 50])));
        assert!(needle_fits_roi((100, 40), Some([10, 20, 100, 50])));
        assert!(needle_fits_roi((90, 50), Some([10, 20, 100, 50])));
    }

    #[test]
    fn needle_fits_roi_none_means_fullscreen() {
        assert!(needle_fits_roi((4000, 2000), None));
        assert!(needle_fits_roi((0, 0), None));
    }

    #[test]
    fn needle_exceeding_roi_does_not_fit() {
        assert!(!needle_fits_roi((138, 20), Some([8, 2, 121, 35])));
        assert!(!needle_fits_roi((80, 91), Some([820, 470, 200, 90])));
        assert!(!needle_fits_roi((300, 200), Some([820, 470, 200, 90])));
    }

    #[test]
    fn needle_fits_roi_treats_zero_wh_as_fullscreen() {
        assert!(needle_fits_roi((100, 50), Some([10, 20, 0, 90])));
        assert!(needle_fits_roi((100, 50), Some([10, 20, 100, 0])));
        assert!(needle_fits_roi((100, 50), Some([10, 20, 0, 0])));
    }

    #[test]
    fn needle_exceeds_roi_warning_for_path_flags_oversized_png() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("big.png");
        dyn_img(GrayImage::new(138, 20))
            .save(&path)
            .expect("save png");

        let warn = needle_exceeds_roi_warning_for_path(&path, Some([8, 2, 121, 35]))
            .expect("oversized needle must be flagged");
        assert!(
            warn.contains("138x20") && warn.contains("121x35"),
            "warning should cite needle and roi dims: {warn}"
        );
        assert!(
            warn.contains("permanent NoMatch"),
            "warning should explain the permanent-NoMatch consequence: {warn}"
        );
    }

    #[test]
    fn needle_exceeds_roi_warning_for_path_passes_fitting_cases() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("fit.png");
        dyn_img(GrayImage::new(100, 50))
            .save(&path)
            .expect("save png");

        assert!(needle_exceeds_roi_warning_for_path(&path, Some([10, 20, 100, 50])).is_none());
        assert!(needle_exceeds_roi_warning_for_path(&path, Some([10, 20, 200, 90])).is_none());
        assert!(needle_exceeds_roi_warning_for_path(&path, None).is_none());
        assert!(
            needle_exceeds_roi_warning_for_path(
                Path::new("/nonexistent/no-such.png"),
                Some([10, 20, 1, 1])
            )
            .is_none(),
            "missing file must not be flagged"
        );
    }
}
