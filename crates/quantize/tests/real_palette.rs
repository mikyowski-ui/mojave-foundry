//! Calibration against the owner's own Fallout 2 palette. Ignored by
//! default; prints numbers only and stores nothing.
//!
//! ```text
//! MOJAVE_FALLOUT2_DIR=/path/to/fallout2 cargo test -p quantize --test real_palette -- --ignored --nocapture
//! ```

use quantize::{Dither, PaletteMap, Reserved, oklab_from_rgb8, quantize, stats};
use vault_formats::{Dat2Archive, Palette, Vfs};

/// A 256 × 1 ramp through `stops`, evenly spaced.
fn ramp(stops: &[[u8; 3]]) -> Vec<u8> {
    let segments = stops.len() - 1;
    (0..256)
        .flat_map(|x| {
            let t = x as f64 / 255.0 * segments as f64;
            let i = (t.floor() as usize).min(segments - 1);
            let f = t - i as f64;
            let (a, b) = (stops[i], stops[i + 1]);
            let c = |k: usize| {
                (f64::from(a[k]) + (f64::from(b[k]) - f64::from(a[k])) * f).round() as u8
            };
            [c(0), c(1), c(2), 255]
        })
        .collect()
}

#[test]
#[ignore = "needs a Fallout 2 install in MOJAVE_FALLOUT2_DIR"]
fn real_palette_numbers() {
    let Some(dir) = std::env::var_os("MOJAVE_FALLOUT2_DIR") else {
        eprintln!("MOJAVE_FALLOUT2_DIR is not set; nothing to measure");
        return;
    };
    let archive =
        Dat2Archive::open(std::path::Path::new(&dir).join("master.dat")).expect("open master.dat");
    let bytes = archive.read("color.pal").expect("read color.pal");
    let palette = Palette::parse(&bytes).expect("parse color.pal");
    let map = PaletteMap::new(&palette);

    let count = |reason| {
        (0..=255u8)
            .filter(|&i| map.reservation(i) == Some(reason))
            .count()
    };
    println!("allowed indices: {}", map.allowed().len());
    println!(
        "reserved: transparent {}, cycling {}, red pulse {}, unmapped {}",
        count(Reserved::Transparent),
        count(Reserved::ColourCycle),
        count(Reserved::RedPulse),
        count(Reserved::Unmapped)
    );
    // Spacing of the allowed set: mean and median distance from each entry
    // to its nearest allowed neighbour.
    let mut gaps: Vec<f64> = map
        .allowed()
        .iter()
        .map(|&i| {
            map.allowed()
                .iter()
                .filter(|&&j| j != i && map.colour(j) != map.colour(i))
                .map(|&j| {
                    let (a, b) = (map.oklab(i), map.oklab(j));
                    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
                })
                .fold(f64::INFINITY, f64::min)
        })
        .collect();
    gaps.sort_by(f64::total_cmp);
    println!(
        "nearest-neighbour spacing: mean {:.4}, median {:.4}",
        gaps.iter().sum::<f64>() / gaps.len() as f64,
        gaps[gaps.len() / 2]
    );
    let neutral: Vec<f64> = map
        .allowed()
        .iter()
        .filter(|&&i| {
            let [_, a, b] = map.oklab(i);
            a.hypot(b) < 0.02
        })
        .map(|&i| map.oklab(i)[0])
        .collect();
    println!("near-neutral allowed entries: {}", neutral.len());

    let gradients: [(&str, Vec<[u8; 3]>); 9] = [
        ("grey ramp", vec![[0, 0, 0], [255, 255, 255]]),
        (
            "skin-like",
            vec![[70, 40, 28], [160, 105, 80], [240, 200, 175]],
        ),
        (
            "desert-like",
            vec![[90, 70, 45], [170, 140, 95], [235, 215, 175]],
        ),
        ("red", vec![[0, 0, 0], [255, 0, 0]]),
        ("green", vec![[0, 0, 0], [0, 255, 0]]),
        ("blue", vec![[0, 0, 0], [0, 0, 255]]),
        ("yellow", vec![[0, 0, 0], [255, 255, 0]]),
        (
            "hue sweep",
            vec![
                [255, 0, 0],
                [255, 255, 0],
                [0, 255, 0],
                [0, 255, 255],
                [0, 0, 255],
                [255, 0, 255],
                [255, 0, 0],
            ],
        ),
        ("sky-like", vec![[60, 90, 140], [150, 185, 220]]),
    ];
    for (name, stops) in &gradients {
        let rgba = ramp(stops);
        let plain = quantize(&rgba, 256, 1, &map, Dither::None).expect("quantize");
        let s = stats(&rgba, &plain, 256, 1, &map).expect("stats");
        let dithered = quantize(&rgba, 256, 1, &map, Dither::ordered()).expect("quantize");
        let d = stats(&rgba, &dithered, 256, 1, &map).expect("stats");
        println!(
            "{name:12} none: mean {:.4} max {:.4} indices {:3} | ordered: mean {:.4} max {:.4} indices {:3}",
            s.mean_error,
            s.max_error,
            s.used_indices().len(),
            d.mean_error,
            d.max_error,
            d.used_indices().len()
        );
    }
    // The table against the exact search on a fixed colour lattice.
    let mut worst: f64 = 0.0;
    let mut differ = 0;
    let mut total = 0;
    for r in (0..=255u8).step_by(5) {
        for g in (0..=255u8).step_by(5) {
            for b in (0..=255u8).step_by(5) {
                let rgb = [r, g, b];
                let lab = oklab_from_rgb8(rgb);
                let dist = |i: u8| {
                    let o = map.oklab(i);
                    ((lab[0] - o[0]).powi(2) + (lab[1] - o[1]).powi(2) + (lab[2] - o[2]).powi(2))
                        .sqrt()
                };
                let (t, e) = (map.nearest(rgb), map.nearest_exact(rgb));
                total += 1;
                if t != e {
                    differ += 1;
                    worst = worst.max(dist(t) - dist(e));
                }
            }
        }
    }
    println!(
        "table vs exact search: {differ} of {total} lattice colours differ, worst excess {worst:.4}"
    );
}
