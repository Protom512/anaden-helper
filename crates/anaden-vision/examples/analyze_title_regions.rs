use image::GenericImageView;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: analyze_title_regions <input.png> <y0,y1> [x_min x_max]");
        std::process::exit(2);
    }
    let inp = &args[1];
    let band: Vec<u32> = args[2]
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    if band.len() != 2 {
        eprintln!("band must be y0,y1");
        std::process::exit(2);
    }
    let (y0, y1) = (band[0], band[1]);
    let img = image::open(inp).expect("open input");
    let (w, h) = img.dimensions();
    if y1 > h || y0 >= y1 {
        eprintln!("band {y0}..{y1} invalid for image {w}x{h}");
        std::process::exit(2);
    }
    let gray = img.to_luma8();

    let xs: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0).min(w);
    let xe: u32 = args
        .get(4)
        .and_then(|s| s.parse().ok())
        .unwrap_or(w)
        .max(xs + 1)
        .min(w);

    let band_h = (y1 - y0) as usize;

    let mut col_var: Vec<u64> = Vec::with_capacity((xe - xs) as usize);
    for x in xs..xe {
        let mut vals: Vec<u8> = Vec::with_capacity(band_h);
        for y in y0..y1 {
            vals.push(gray.get_pixel(x, y).0[0]);
        }
        let mean = vals.iter().map(|v| *v as u64).sum::<u64>() as f64 / vals.len() as f64;
        let var: u64 = vals
            .iter()
            .map(|v| {
                let d = *v as f64 - mean;
                (d * d) as u64
            })
            .sum();
        col_var.push(var);
    }
    let max_var = *col_var.iter().max().unwrap_or(&1) as f64;
    let sm: Vec<f64> = (0..col_var.len())
        .map(|i| {
            let a = col_var[i.saturating_sub(1)];
            let b = col_var[i];
            let c = col_var[(i + 1).min(col_var.len() - 1)];
            (a + b + c) as f64 / 3.0
        })
        .collect();

    let thr = max_var * 0.10;
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < sm.len() {
        if sm[i] > thr {
            let s = i;
            while i < sm.len() && sm[i] > thr {
                i += 1;
            }
            runs.push((xs as usize + s, xs as usize + i));
        } else {
            i += 1;
        }
    }
    println!("image: {w}x{h} band=y{y0}..y{y1} x={xs}..{xe} max_var={max_var:.0} thr={thr:.0}");
    println!("runs (描画あり連続区間):");
    for (s, e) in &runs {
        let cw = e - s;
        let tag = if (30..=120).contains(&cw) {
            "SUBTEMPLATE_CANDIDATE"
        } else {
            ""
        };
        println!("  x={s}..{e} w={cw} center={} {tag}", (s + e) / 2);
    }
    if runs.is_empty() {
        println!("  (no runs detected in this band)");
    }
}
