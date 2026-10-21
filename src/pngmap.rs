//! A full-colour map (a PNG, or a JPEG even under a .png name) turned into a map WA accepts (its
//! "proxy"), keeping the original.
//!
//! WA's map loader (wa-play core/src/wa/mapimg.rs) takes only: an 8-bit palette PNG, at least
//! 640 x 32, width and height multiples of 8, palette entry 0 black (air), at most 0x71 colours,
//! and a 40-byte `waLV` settings chunk. The proxy:
//! - land = alpha >= 128 (any colour, black included), air = the rest;
//! - colours: the land's, reduced to 112 by median cut (entry 0 stays air; a land colour of pure
//!   black is stored as (8, 8, 8), it is land);
//! - size: padded with air to multiples of 8 and at least 640 x 32, on the right and on top (the
//!   land keeps the bottom); a map with no air at all gets 128 rows of air on top, so WA can
//!   place worms on it;
//! - settings: an open map (no borders), its seed from the image, a theme (the soil behind
//!   craters and the backdrop come from it).
//!
//! The original's colours are what wkVista shows in game on the land WA has not changed.

/// A decoded full-colour map: `w` x `h` pixels, 0xAARRGGBB
pub struct Rgba {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u32>,
}

/// A full-colour image WA can't load itself, by its first bytes (whatever the file's name): a
/// PNG with 8-bit RGB or RGBA, or a JPEG (no transparency: all land). WA's own maps (palette
/// PNGs) and anything else: false.
pub fn full_colour(head: &[u8]) -> bool {
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return true;
    }
    head.len() >= 26 && head[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] && &head[12..16] == b"IHDR" && head[24] == 8 && (head[25] == 6 || head[25] == 2)
}

/// A full-colour image (PNG or JPEG) as 0xAARRGGBB pixels
pub fn decode(bytes: &[u8]) -> Result<Rgba, String> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return decode_jpeg(bytes);
    }
    decode_png(bytes)
}

fn decode_jpeg(bytes: &[u8]) -> Result<Rgba, String> {
    let mut d = jpeg_decoder::Decoder::new(bytes);
    let px = d.decode().map_err(|e| e.to_string())?;
    let info = d.info().ok_or("no JPEG info")?;
    let (w, h) = (info.width as u32, info.height as u32);
    let px = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => px.chunks(3).map(|p| 0xFF00_0000 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32).collect(),
        jpeg_decoder::PixelFormat::L8 => px.iter().map(|&g| 0xFF00_0000 | (g as u32) * 0x01_0101).collect(),
        f => return Err(format!("JPEG pixel format {f:?}")),
    };
    Ok(Rgba { w, h, px })
}

fn decode_png(bytes: &[u8]) -> Result<Rgba, String> {
    let mut d = png::Decoder::new(bytes);
    d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = d.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width, info.height);
    let ch = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        c => return Err(format!("colour type {c:?}")),
    };
    let px = buf[..(w * h) as usize * ch]
        .chunks(ch)
        .map(|p| {
            let a = if ch == 4 { p[3] } else { 255 } as u32;
            a << 24 | (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32
        })
        .collect();
    Ok(Rgba { w, h, px })
}

#[inline]
pub fn is_land(p: u32) -> bool {
    p >> 24 >= 128
}

/// The proxy: where the original lies in it, and its PNG bytes
pub struct Proxy {
    pub w: u32,
    pub h: u32,
    /// the original's top-left in the proxy
    pub at: (u32, u32),
    pub png: Vec<u8>,
}

/// Median cut: at most `n` colours for `cols` (0x00RRGGBB), and each colour's box index
fn median_cut(cols: &[u32], n: usize) -> (Vec<u32>, std::collections::HashMap<u32, u8>) {
    use std::collections::HashMap;
    // the distinct colours with their counts
    let mut count: HashMap<u32, u32> = HashMap::new();
    for &c in cols {
        *count.entry(c & 0xFF_FFFF).or_default() += 1;
    }
    let mut boxes: Vec<Vec<(u32, u32)>> = vec![count.into_iter().collect()];
    let ch = |c: u32, k: usize| (c >> (16 - 8 * k)) & 0xFF;
    while boxes.len() < n {
        // split the box with the widest channel range (of those with 2+ colours)
        let mut best = None;
        for (i, b) in boxes.iter().enumerate() {
            if b.len() < 2 {
                continue;
            }
            for k in 0..3 {
                let (lo, hi) = b.iter().fold((255, 0), |(lo, hi), &(c, _)| (lo.min(ch(c, k)), hi.max(ch(c, k))));
                if best.is_none_or(|(_, _, r)| hi - lo > r) {
                    best = Some((i, k, hi - lo));
                }
            }
        }
        let Some((i, k, _)) = best else { break };
        let mut b = boxes.swap_remove(i);
        b.sort_by_key(|&(c, _)| ch(c, k));
        let total: u64 = b.iter().map(|&(_, n)| n as u64).sum();
        let (mut acc, mut cut) = (0u64, 1);
        for (j, &(_, n)) in b.iter().enumerate() {
            acc += n as u64;
            if acc * 2 >= total {
                cut = (j + 1).clamp(1, b.len() - 1);
                break;
            }
        }
        let rest = b.split_off(cut);
        boxes.push(b);
        boxes.push(rest);
    }
    let mut pal = Vec::new();
    let mut map = HashMap::new();
    for (i, b) in boxes.iter().enumerate() {
        let total: u64 = b.iter().map(|&(_, n)| n as u64).sum::<u64>().max(1);
        let avg = |k: usize| (b.iter().map(|&(c, n)| ch(c, k) as u64 * n as u64).sum::<u64>() / total) as u32;
        pal.push(avg(0) << 16 | avg(1) << 8 | avg(2));
        for &(c, _) in b {
            map.insert(c, i as u8);
        }
    }
    (pal, map)
}

/// WA's themes by number (the settings' theme word, +0x20): WA's own table, 0x649BA0 (the
/// first five are the drawn-map themes)
pub const THEMES: [&str; 29] = [
    "-Beach", "-Desert", "-Farm", "-Forest", "-Hell", "Art", "Cheese", "Construction", "Desert", "Dungeon", "Easter", "Forest",
    "Fruit", "Gulf", "Hell", "Hospital", "Jungle", "Manhattan", "Medieval", "Music", "Pirate", "Snow", "Space", "Sports",
    "Tentacle", "Time", "Tools", "Tribal", "Urban",
];

/// The settings chunk of an open map (wa-play maps.rs reads them as WA's loader does: +0 and
/// +4 the seeds, +0x10 no borders, +0x20 the theme word, split by 0x491530 into its number,
/// low 16 bits, and 1 in the high ones as WA's editor writes it); the seed from the image
fn settings(seed: u64, theme: u32) -> [u8; 40] {
    let mut s = [0u8; 40];
    s[..8].copy_from_slice(&seed.to_le_bytes());
    s[0x10] = 1;
    s[0x20..0x24].copy_from_slice(&(0x1_0000 | theme).to_le_bytes());
    s
}

pub fn hash(px: &[u32]) -> u64 {
    // FNV-1a
    px.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &p| (h ^ p as u64).wrapping_mul(0x100_0000_01b3))
}

/// The proxy's size for an image of `w` x `h` (with or without any air) and the original's
/// top-left in it
pub fn layout(w: u32, h: u32, any_air: bool) -> (u32, u32, (u32, u32)) {
    let top = if any_air { 0 } else { 128 };
    let round = |v: u32, min: u32| v.max(min).div_ceil(8) * 8;
    let (pw, ph) = (round(w, 640), round(h + top, 32));
    (pw, ph, (0, ph - h))
}

/// A full-colour image's size from its file, without decoding the pixels
pub fn size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        let mut d = jpeg_decoder::Decoder::new(bytes);
        d.read_info().ok()?;
        let i = d.info()?;
        return Some((i.width as u32, i.height as u32));
    }
    (bytes.len() >= 24).then(|| (u32::from_be_bytes(bytes[16..20].try_into().unwrap()), u32::from_be_bytes(bytes[20..24].try_into().unwrap())))
}

/// The proxy of `img` with WA theme number `theme` (THEMES)
pub fn proxy(img: &Rgba, theme: u32) -> Result<Proxy, String> {
    let any_air = img.px.iter().any(|&p| !is_land(p));
    let (w, h, at) = layout(img.w, img.h, any_air);
    if w >= 0x6F00 || h >= 0x7760 {
        return Err(format!("{}x{} is bigger than WA's maps", img.w, img.h));
    }
    let land: Vec<u32> = img.px.iter().copied().filter(|&p| is_land(p)).collect();
    let (mut pal, map) = median_cut(&land, 112);
    for c in pal.iter_mut() {
        if *c == 0 {
            *c = 0x08_0808;
        }
    }
    let mut idx = vec![0u8; (w * h) as usize];
    for y in 0..img.h {
        for x in 0..img.w {
            let p = img.px[(y * img.w + x) as usize];
            if is_land(p) {
                idx[((at.1 + y) * w + at.0 + x) as usize] = map[&(p & 0xFF_FFFF)] + 1;
            }
        }
    }
    let mut plte = vec![0u8; 3];
    for c in &pal {
        plte.extend([(c >> 16) as u8, (c >> 8) as u8, *c as u8]);
    }
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Indexed);
        e.set_depth(png::BitDepth::Eight);
        e.set_palette(plte);
        let mut wr = e.write_header().map_err(|e| e.to_string())?;
        wr.write_chunk(png::chunk::ChunkType(*b"waLV"), &settings(hash(&img.px), theme)).map_err(|e| e.to_string())?;
        wr.write_image_data(&idx).map_err(|e| e.to_string())?;
    }
    Ok(Proxy { w, h, at, png: out })
}
