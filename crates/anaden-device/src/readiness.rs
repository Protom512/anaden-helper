use std::hash::Hasher;

use image::DynamicImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameDigest(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadinessSample {
    pub digest: FrameDigest,
    pub socket_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessVerdict {
    Active,
    SplashHang,
}

pub fn frame_digest(img: &DynamicImage) -> FrameDigest {
    let mut hasher = std::hash::DefaultHasher::new();
    hasher.write_u32(img.width());
    hasher.write_u32(img.height());
    hasher.write(img.as_bytes());
    FrameDigest(hasher.finish())
}

pub fn classify_readiness(samples: &[ReadinessSample]) -> ReadinessVerdict {
    let Some(first) = samples.first() else {
        return ReadinessVerdict::SplashHang;
    };
    let render_frozen = samples.iter().all(|s| s.digest == first.digest);
    let all_offline = samples.iter().all(|s| s.socket_count == 0);
    if render_frozen && all_offline {
        ReadinessVerdict::SplashHang
    } else {
        ReadinessVerdict::Active
    }
}
