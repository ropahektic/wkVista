//! The game's land and the full-colour original it came from.
//!
//! WA keeps the land as tiles (DisplayGfx +0x4DD8: an object whose +4 is the tile bitmaps, row
//! by row; +0x4DDC / +0x4DE0 the land's width and height, +0x4DE4 / +0x4DE8 a tile's; drawn by
//! DrawTiledTerrain 0x56C5A0 as BlitBitmapClipped calls). At a game's first frame the tiles are
//! copied (the land as WA made it) and the land's shape is matched against the full-colour maps
//! this module converted (files.rs): where it matches, each untouched land pixel (the same as at
//! the start) shows the original's colour; anything WA changed shows WA's.

use std::collections::HashMap;

use crate::{files, log, pngmap};

pub struct Land {
    pub w: u32,
    pub h: u32,
    /// tile bitmap -> its top-left in the land
    pub tiles: HashMap<usize, (u32, u32)>,
    /// the land at the start (palette indices), `w` x `h`
    pub snap: Vec<u8>,
    /// the original's colour per land pixel (0x00RRGGBB | 1 << 24), 0 where it has none
    pub rgba: Vec<u32>,
}

unsafe fn r32(a: usize) -> u32 {
    std::ptr::read_unaligned(a as *const u32)
}

/// The land from DisplayGfx `display`, matched to a converted map; None (logged) when there is
/// no tile table yet, or no map matches
pub unsafe fn build(display: usize, frame: u32) -> Option<Land> {
    let terrain = r32(display + 0x4DD8) as usize;
    if terrain == 0 {
        return None;
    }
    let arr = r32(terrain + 4) as usize;
    let (w, h, tw, th) = (r32(display + 0x4DDC), r32(display + 0x4DE0), r32(display + 0x4DE4), r32(display + 0x4DE8));
    if arr == 0 || w == 0 || h == 0 || tw == 0 || th == 0 || w > 0x8000 || h > 0x8000 {
        log::line(&format!("[{frame}] land: no tile table (terrain {terrain:#x}, {w}x{h}, tiles {tw}x{th})"));
        return None;
    }
    let cols = w.div_ceil(tw);
    let rows = h.div_ceil(th);
    let mut snap = vec![0u8; (w * h) as usize];
    let mut tiles = HashMap::new();
    for k in 0..cols * rows {
        let bm = r32(arr + 4 * k as usize) as usize;
        if bm == 0 {
            continue;
        }
        let (x0, y0) = ((k % cols) * tw, (k / cols) * th);
        tiles.insert(bm, (x0, y0));
        let s = r32(bm + 4) as usize;
        if s == 0 {
            continue;
        }
        let (sw, sh, data) = (r32(s + 4), r32(s + 8), r32(s + 0xC) as usize);
        for y in 0..sh.min(h - y0) {
            let src = std::slice::from_raw_parts((data + (y * sw) as usize) as *const u8, sw.min(w - x0) as usize);
            let at = ((y0 + y) * w + x0) as usize;
            snap[at..at + src.len()].copy_from_slice(src);
        }
    }
    let land_px = snap.iter().filter(|&&p| p != 0).count();
    log::line(&format!("[{frame}] land {w}x{h} in {} tiles of {tw}x{th}, {land_px} land pixels", tiles.len()));
    // the converted maps, most recent first: the original's shape at a few places in the land
    for e in files::entries() {
        let Ok(bytes) = std::fs::read(&e.source) else { continue };
        let Ok(img) = pngmap::decode(&bytes) else { continue };
        // the proxy's top-left in the land: the same place, bottom-aligned, centred
        let (pw, ph) = (e.w, e.h);
        let cands = [(0i64, 0i64), (0, h as i64 - ph as i64), ((w as i64 - pw as i64) / 2, h as i64 - ph as i64), ((w as i64 - pw as i64) / 2, (h as i64 - ph as i64) / 2)];
        let mut tried = Vec::new();
        for &(px, py) in &cands {
            let (ox, oy) = (px + e.at.0 as i64, py + e.at.1 as i64);
            let (mut same, mut total) = (0u64, 0u64);
            for y in (0..img.h as i64).step_by(3) {
                for x in (0..img.w as i64).step_by(3) {
                    let (lx, ly) = (ox + x, oy + y);
                    if lx < 0 || ly < 0 || lx >= w as i64 || ly >= h as i64 {
                        continue;
                    }
                    total += 1;
                    let land = snap[(ly as u32 * w + lx as u32) as usize] != 0;
                    same += (land == pngmap::is_land(img.px[(y as u32 * img.w + x as u32) as usize])) as u64;
                }
            }
            let ratio = if total > 0 { same as f64 / total as f64 } else { 0.0 };
            tried.push(format!("({ox},{oy}) {:.2}%", ratio * 100.0));
            if total > 1000 && ratio > 0.995 {
                let mut rgba = vec![0u32; (w * h) as usize];
                for y in 0..img.h as i64 {
                    for x in 0..img.w as i64 {
                        let (lx, ly) = (ox + x, oy + y);
                        let p = img.px[(y as u32 * img.w + x as u32) as usize];
                        if lx >= 0 && ly >= 0 && lx < w as i64 && ly < h as i64 && pngmap::is_land(p) {
                            rgba[(ly as u32 * w + lx as u32) as usize] = p & 0xFF_FFFF | 1 << 24;
                        }
                    }
                }
                log::line(&format!("[{frame}] land is {} at {:?} (tried {}): full colour on", e.source.display(), (ox, oy), tried.join(", ")));
                return Some(Land { w, h, tiles, snap, rgba });
            }
        }
        log::line(&format!("[{frame}] land is not {} (tried {})", e.source.display(), tried.join(", ")));
    }
    None
}
