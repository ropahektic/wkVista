//! The proxy a full-colour map becomes must pass WA's map loader (wa-play
//! core/src/wa/mapimg.rs: an 8-bit palette PNG, at least 640 x 32, sides multiples of 8,
//! palette entry 0 black, at most 0x71 colours, a 40-byte waLV chunk) and have the original's
//! shape (alpha >= 128 = land) at its place.

use wkVista::pngmap::{is_land, proxy, Rgba};

fn check(img: &Rgba) {
    let p = proxy(img).unwrap();
    let mut d = png::Decoder::new(&p.png[..]);
    d.set_transformations(png::Transformations::IDENTITY);
    let mut r = d.read_info().unwrap();
    let info = r.info().clone();
    assert_eq!((info.color_type, info.bit_depth), (png::ColorType::Indexed, png::BitDepth::Eight));
    assert!(!info.interlaced);
    let (w, h) = (info.width, info.height);
    assert!(w >= 640 && h >= 32 && w % 8 == 0 && h % 8 == 0, "{w}x{h}");
    let pal = info.palette.clone().unwrap();
    assert!(pal.len() / 3 <= 0x71 && pal[..3] == [0, 0, 0], "{} colours, entry 0 {:?}", pal.len() / 3, &pal[..3]);
    let walv = p.png.windows(8).position(|c| &c[4..] == b"waLV").expect("waLV chunk");
    assert_eq!(u32::from_be_bytes(p.png[walv..walv + 4].try_into().unwrap()), 40);
    let mut idx = vec![0u8; r.output_buffer_size()];
    r.next_frame(&mut idx).unwrap();
    for y in 0..h {
        for x in 0..w {
            let (ox, oy) = (x as i64 - p.at.0 as i64, y as i64 - p.at.1 as i64);
            let want = ox >= 0 && oy >= 0 && ox < img.w as i64 && oy < img.h as i64 && is_land(img.px[(oy as u32 * img.w + ox as u32) as usize]);
            assert_eq!(idx[(y * w + x) as usize] != 0, want, "pixel ({x}, {y})");
        }
    }
}

#[test]
fn proxies_are_legal_maps() {
    // a small map with air: padded to 640 x 32, colours across the range, black land
    let (w, h) = (300, 20);
    let px = (0..w * h).map(|i| {
        let (x, y) = (i % w, i / w);
        let a = if (x + y) % 7 == 0 { 0 } else { 255 };
        a << 24 | (x * 37 % 256) << 16 | (y * 91 % 256) << 8 | ((x * y) % 256)
    }).map(|p| if p & 0xFF_FFFF == 0x12 { p & 0xFF00_0000 } else { p }).collect();
    check(&Rgba { w, h, px });
    // a fully opaque photo-like map, odd size: air on top so WA can place worms
    let (w, h) = (1001, 503);
    let px: Vec<u32> = (0..w * h).map(|i| 0xFF00_0000 | (i * 2654435761u32 >> 8)).collect();
    let img = Rgba { w, h, px };
    check(&img);
    assert!(proxy(&img).unwrap().at.1 >= 128);
}

/// env MAP=<file>: that image through decode + proxy + the checks above
#[test]
fn a_real_map() {
    let Ok(path) = std::env::var("MAP") else { return };
    let bytes = std::fs::read(&path).unwrap();
    assert!(wkVista::pngmap::full_colour(&bytes[..26]), "not a full-colour image");
    let img = wkVista::pngmap::decode(&bytes).unwrap();
    let p = proxy(&img).unwrap();
    println!("{path}: {}x{} -> proxy {}x{} at {:?}, {} bytes", img.w, img.h, p.w, p.h, p.at, p.png.len());
    check(&img);
}
