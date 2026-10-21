//! wkVista's rasterizer against wa-play's, whose pictures were verified against WA itself: the
//! frames wa-play exports for its browser test (draw lists, `<frame>.json` cases) drawn here
//! must give the same pixels at every zoom. Screen-space operations are drawn 1:1, as WA draws
//! them while wkVista zooms in.
//!
//! env FIXTURES (default ../wa-play/web/test/fixtures, made by wa-play's
//! `EXPORT=../web/test/fixtures cargo test --test capture`); skipped when missing.

use wkVista::raster::{check, draw_mapped, Clip, Op, Src, Target, View};

struct Image {
    w: i32,
    h: i32,
    px: Vec<u8>,
}

fn clip(w: &[u32]) -> Clip {
    Clip { l: w[0] as i32, t: w[1] as i32, r: w[2] as i32, b: w[3] as i32 }
}

/// A draw list: WA's screen size, its operations with their spaces (true: world), the images
fn parse(b: &[u8]) -> ((i32, i32), Vec<(bool, Op)>, std::collections::HashMap<u32, Image>) {
    let w: Vec<u32> = b.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    assert_eq!(w[0], 0x4C44_4157, "not a draw list");
    let (size, nops, nimg, ndrop, npal) = ((w[2] as i32, w[3] as i32), w[6], w[7], w[8], w[9]);
    let mut p = 10 + (npal + ndrop) as usize;
    let mut images = std::collections::HashMap::new();
    for _ in 0..nimg {
        let (id, iw, ih) = (w[p], w[p + 1], w[p + 2]);
        let n = (iw * ih) as usize;
        let start = (p + 4) * 4;
        images.insert(id, Image { w: iw as i32, h: ih as i32, px: b[start..start + n].to_vec() });
        p += 4 + n.div_ceil(4);
    }
    let mut ops = Vec::new();
    for _ in 0..nops {
        let (kind, world) = (w[p] & 0xFF, (w[p] >> 8) & 1 == 1);
        let q = &w[p + 1..];
        let (op, len) = match kind {
            1 => (Op::Bitmap { x: q[0] as i32, y: q[1] as i32, w: q[2] as i32, h: q[3] as i32, src: src_of(&images, q[4]), keyed: q[5] != 0, key: q[6] as u8, clip: clip(&q[7..11]) }, 11),
            2 | 6 => (Op::Fill { x1: q[0] as i32, y1: q[1] as i32, x2: q[2] as i32, y2: q[3] as i32, colour: q[4] as u8, clip: clip(&q[5..9]) }, 9),
            3 => {
                let lut = (q[7] != u32::MAX).then(|| images[&q[7]].px.as_ptr());
                (Op::Grid { x: q[0] as i32, y: q[1] as i32, w: q[2] as i32, h: q[3] as i32, src: src_of(&images, q[4]), sx: q[5] as i32, sy: q[6] as i32, lut, mode: q[8], clip: clip(&q[9..13]) }, 13)
            }
            4 => (Op::Pixel { x: q[0] as i32, y: q[1] as i32, colour: q[2] as u8, clip: if q[3] != 0 { clip(&q[4..8]) } else { Clip::NONE } }, 8),
            5 => (Op::VLine { x: q[0] as i32, y1: q[1] as i32, y2: q[2] as i32, colour: q[3] as u8 }, 4),
            k => panic!("op kind {k}"),
        };
        ops.push((world, op));
        p += 1 + len;
    }
    assert_eq!(p, w.len());
    (size, ops, images)
}

fn src_of(images: &std::collections::HashMap<u32, Image>, id: u32) -> Src {
    let im = &images[&id];
    Src { data: im.px.as_ptr(), w: im.w, h: im.h, stride: im.w }
}

#[test]
fn same_pictures_as_wa_play() {
    let dir = std::env::var("FIXTURES").unwrap_or_else(|_| "../wa-play/web/test/fixtures".into());
    let Ok(index) = std::fs::read_to_string(format!("{dir}/index.json")) else {
        println!("no fixtures in {dir}: skipped");
        return;
    };
    let names: Vec<String> = index.split('"').skip(3).step_by(2).map(String::from).filter(|s| !s.is_empty() && s != "]}").collect();
    let mut checked = 0;
    for name in &names {
        let b = std::fs::read(format!("{dir}/{name}.wadl")).unwrap();
        let (wa, ops, _images) = parse(&b);
        let cases = std::fs::read_to_string(format!("{dir}/{name}.json")).unwrap();
        for case in cases.split('{').skip(1) {
            let num = |key: &str| -> Vec<i32> {
                let Some(i) = case.find(&format!("\"{key}\"")) else { return Vec::new() };
                let rest = &case[i + key.len() + 3..];
                let end = rest.find(|c| c == ',' && !rest.starts_with('[') || c == '}').unwrap_or(rest.len());
                let v = if rest.trim_start().starts_with('[') { &rest[rest.find('[').unwrap() + 1..rest.find(']').unwrap()] } else { &rest[..end] };
                v.split(',').filter_map(|x| x.trim().parse().ok()).collect()
            };
            let (zoom, out, focus) = (num("zoom")[0], num("out"), num("focus"));
            let expect = case.split("\"expect\": \"").nth(1).unwrap().split('"').next().unwrap();
            // wkVista only zooms in on WA's own screen size (zooming out is not built here yet)
            if zoom < 16 || (out[0], out[1]) != wa {
                continue;
            }
            let oc = (out[0] / 2, out[1] / 2);
            let centre = match focus.as_slice() {
                [px, py] => (px - (16 * (px - oc.0) + 8).div_euclid(zoom), py - (16 * (py - oc.1) + 8).div_euclid(zoom)),
                _ => (wa.0 / 2, wa.1 / 2),
            };
            let view = View { zoom, centre, out_centre: oc, wa };
            let native = View { zoom: 16, centre: oc, out_centre: oc, wa };
            let mut px = vec![0u8; (out[0] * out[1]) as usize];
            let mut t = Target { data: px.as_mut_ptr(), w: out[0], h: out[1], stride: out[0] };
            for (world, op) in &ops {
                check(op).unwrap_or_else(|e| panic!("{name}: {e}"));
                draw_mapped(op, &mut t, if *world && zoom != 16 { &view } else { &native });
            }
            let want = std::fs::read(format!("{dir}/{expect}")).unwrap();
            let diff = px.iter().zip(&want).filter(|(a, b)| a != b).count();
            assert_eq!(diff, 0, "{name} zoom {zoom}/16 focus {focus:?}: {diff} pixels differ from wa-play's");
            checked += 1;
        }
    }
    println!("{checked} pictures the same as wa-play's");
    assert!(checked > 0);
}

/// env BENCH: the time a whole fixture frame takes at 1x (identity), 2x and 4x
#[test]
fn bench() {
    if std::env::var("BENCH").is_err() {
        return;
    }
    let dir = std::env::var("FIXTURES").unwrap_or_else(|_| "../wa-play/web/test/fixtures".into());
    for name in ["20240211104942On-500", "ropabeatsdeanoin-500"] {
        let Ok(b) = std::fs::read(format!("{dir}/{name}.wadl")) else { continue };
        let (wa, ops, _images) = parse(&b);
        for zoom in [16, 32, 64] {
            let c = (wa.0 / 2, wa.1 / 2);
            let view = View { zoom, centre: c, out_centre: c, wa };
            let native = View { zoom: 16, centre: (c.0 + 1, c.1), out_centre: (c.0 + 1, c.1), wa };
            let mut px = vec![0u8; (wa.0 * wa.1) as usize];
            let mut t = Target { data: px.as_mut_ptr(), w: wa.0, h: wa.1, stride: wa.0 };
            let n = 20;
            let t0 = std::time::Instant::now();
            for _ in 0..n {
                for (world, op) in &ops {
                    draw_mapped(op, &mut t, if *world || zoom == 16 { &view } else { &native });
                }
            }
            println!("{name} zoom {zoom}/16: {:.2} ms a frame ({} ops)", t0.elapsed().as_secs_f64() * 1e3 / n as f64, ops.len());
        }
    }
}
