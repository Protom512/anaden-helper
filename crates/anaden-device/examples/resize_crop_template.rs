#![cfg(windows)]

use image::imageops::FilterType;

struct Args {
    input: String,
    output: String,
    letterbox: u32,
    width: u32,
    height: u32,
    roi: (u32, u32, u32, u32),
}

fn print_usage() {
    eprintln!(
        "usage: resize_crop_template <input.png> <output.png> [--letterbox N] [--roi x,y,w,h] [--size WxH]"
    );
}

fn take_flag(args: &mut Vec<String>, key: &str) -> Option<String> {
    let pos = args.iter().position(|a| a == key)?;
    if pos + 1 >= args.len() {
        return None;
    }
    let value = args.remove(pos + 1);
    args.remove(pos);
    Some(value)
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn parse_roi(s: &str) -> Option<(u32, u32, u32, u32)> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 4 {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
        parts[3].parse().ok()?,
    ))
}

fn parse_args(argv: &[String]) -> Option<Args> {
    if argv.len() < 2 {
        print_usage();
        return None;
    }
    let mut rest: Vec<String> = argv.to_vec();
    let input = rest.remove(0);
    let output = rest.remove(0);

    let letterbox: u32 =
        take_flag(&mut rest, "--letterbox").map_or(Some(10), |v| v.parse().ok())?;
    let size = take_flag(&mut rest, "--size").map_or(Some((1258, 708)), |v| parse_size(&v))?;
    let roi = take_flag(&mut rest, "--roi").map_or(Some((8, 2, 138, 20)), |v| parse_roi(&v))?;

    if !rest.is_empty() {
        print_usage();
        return None;
    }
    Some(Args {
        input,
        output,
        letterbox,
        width: size.0,
        height: size.1,
        roi,
    })
}

fn run(args: &Args) -> Result<(), image::ImageError> {
    let img = image::open(&args.input)?;
    println!("input: {} ({}x{})", args.input, img.width(), img.height());

    let w = img.width().saturating_sub(args.letterbox);
    let cropped = image::imageops::crop_imm(&img, args.letterbox, 0, w, img.height()).to_image();

    let dyn_img = image::DynamicImage::ImageRgba8(cropped);
    let resized = dyn_img.resize_exact(args.width, args.height, FilterType::Triangle);

    let (rx, ry, rw, rh) = args.roi;
    if rx + rw > args.width || ry + rh > args.height {
        eprintln!(
            "error: ROI ({rx},{ry},{rw},{rh}) exceeds resized image {}x{}",
            args.width, args.height
        );
        std::process::exit(2);
    }
    let sub = image::imageops::crop_imm(&resized, rx, ry, rw, rh).to_image();
    sub.save(&args.output)?;
    println!("saved: {} ({}x{})", args.output, sub.width(), sub.height());
    Ok(())
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(args) = parse_args(&argv) else {
        std::process::exit(2);
    };
    if let Err(e) = run(&args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[allow(clippy::panic)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_ok() {
        assert_eq!(parse_size("1258x708"), Some((1258, 708)));
        assert_eq!(parse_size("1x1"), Some((1, 1)));
    }

    #[test]
    fn parse_size_rejects_invalid() {
        assert_eq!(parse_size("1258"), None);
        assert_eq!(parse_size("ax708"), None);
        assert_eq!(parse_size("1258x"), None);
        assert_eq!(parse_size("1258xx708"), None);
    }

    #[test]
    fn parse_roi_ok() {
        assert_eq!(parse_roi("65,7,121,35"), Some((65, 7, 121, 35)));
        assert_eq!(parse_roi("0,0,1,1"), Some((0, 0, 1, 1)));
    }

    #[test]
    fn parse_roi_rejects_invalid() {
        assert_eq!(parse_roi("65,7,121"), None);
        assert_eq!(parse_roi("65,7,121,35,1"), None);
        assert_eq!(parse_roi("65,-7,121,35"), None);
        assert_eq!(parse_roi("a,b,c,d"), None);
        assert_eq!(parse_roi(""), None);
    }

    #[test]
    fn take_flag_removes_key_and_value() {
        let mut args = vec![
            "in.png".to_string(),
            "--roi".to_string(),
            "1,2,3,4".to_string(),
            "out.png".to_string(),
        ];
        let v = take_flag(&mut args, "--roi");
        assert_eq!(v.as_deref(), Some("1,2,3,4"));
        assert_eq!(args, vec!["in.png".to_string(), "out.png".to_string()]);
    }

    #[test]
    fn take_flag_missing_returns_none() {
        let mut args = vec!["in.png".to_string()];
        assert_eq!(take_flag(&mut args, "--roi"), None);
        assert_eq!(args.len(), 1);
    }

    #[test]
    fn parse_args_defaults() {
        let argv = vec!["in.png".to_string(), "out.png".to_string()];
        let a = parse_args(&argv).expect("parse ok");
        assert_eq!(
            (a.letterbox, a.width, a.height, a.roi),
            (10, 1258, 708, (8, 2, 138, 20))
        );
    }

    #[test]
    fn parse_args_explicit_flags() {
        let argv = [
            "in.png",
            "out.png",
            "--letterbox",
            "0",
            "--roi",
            "1,2,100,50",
            "--size",
            "1280x720",
        ]
        .map(String::from)
        .to_vec();
        let a = parse_args(&argv).expect("parse ok");
        assert_eq!(
            (a.letterbox, a.width, a.height, a.roi),
            (0, 1280, 720, (1, 2, 100, 50))
        );
    }

    #[test]
    fn parse_args_rejects_too_few_and_unknown() {
        assert!(parse_args(&["in.png".to_string()]).is_none());
        let bad = vec![
            "in.png".to_string(),
            "out.png".to_string(),
            "--bogus".to_string(),
        ];
        assert!(parse_args(&bad).is_none());
    }

    #[test]
    fn parse_args_rejects_invalid_flag_values() {
        let bad = vec![
            "in.png".to_string(),
            "out.png".to_string(),
            "--roi".to_string(),
            "not-a-roi".to_string(),
        ];
        assert!(parse_args(&bad).is_none());
    }
}
