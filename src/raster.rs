//! WA's drawing operations drawn through a view, straight into WA's framebuffer as WA issues
//! them. The meaning of each operation is wa-play's reference rasterizer (core/src/wa/raster.rs),
//! verified there pixel for pixel against WA's own functions; this is the same rule per pixel
//! (`shade`) and the same integer view mapping, reading the sources where WA keeps them.
//!
//! The view: zoom `n / 16` about WA's screen centre `c`, which lands on the output centre `oc`;
//! an output pixel shows the WA pixel it maps back to: X = cx + floor((16 (ox - ocx) + 8) / n).
//! Variants WA was not seen to use are refused (the caller lets WA draw them itself).

/// A clip rectangle [l, r) x [t, b)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clip {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
}

impl Clip {
    pub const NONE: Clip = Clip { l: i32::MIN, t: i32::MIN, r: i32::MAX, b: i32::MAX };
}

/// 8-bit pixels where WA keeps them: `h` rows of `stride` bytes, `w` used
#[derive(Clone, Copy, Debug)]
pub struct Src {
    pub data: *const u8,
    pub w: i32,
    pub h: i32,
    pub stride: i32,
}

impl Src {
    /// A pixel; 0 outside
    #[inline]
    fn at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return 0;
        }
        unsafe { *self.data.add((y * self.stride + x) as usize) }
    }
}

/// One operation as WA issued it (positions in WA's framebuffer pixels)
#[derive(Clone, Copy, Debug)]
pub enum Op {
    /// DisplayGfx::BlitBitmapClipped 0x56A700: `w` x `h` of a bitmap from its top-left, colour-
    /// keyed with `key` when `keyed`
    Bitmap { x: i32, y: i32, w: i32, h: i32, src: Src, keyed: bool, key: u8, clip: Clip, land: Option<LandTag> },
    /// DisplayGfx fill rectangle 0x56B810 / BitGrid fill 0x4F5D50: [x1, x2) x [y1, y2)
    Fill { x1: i32, y1: i32, x2: i32, y2: i32, colour: u8, clip: Clip },
    /// BitGrid::BlitSpriteRect 0x4F6910: `w` x `h` of `src` from (sx, sy) to (x, y); mode low
    /// 16 bits 0 copy, 1 skip source 0 (with a blend table: table[dst << 8 | src]), 2 only over
    /// non-0, 3 only over 0; high 16 bits the orientation (0 / 0xB as is, 1 / 0xA mirrored)
    Grid { x: i32, y: i32, w: i32, h: i32, src: Src, sx: i32, sy: i32, lut: Option<*const u8>, mode: u32, clip: Clip },
    /// BitGrid pixel 0x4F9180 (clipped) / 0x4F5E40 (Clip::NONE)
    Pixel { x: i32, y: i32, colour: u8, clip: Clip },
    /// BitGrid vertical line 0x4F9110: column x, rows [y1, y2)
    VLine { x: i32, y1: i32, y2: i32, colour: u8 },
}

/// A landscape tile (land.rs): its top-left in the land, the land's size and its pixels at the
/// game's start
#[derive(Clone, Copy, Debug)]
pub struct LandTag {
    pub x0: i32,
    pub y0: i32,
    pub lw: i32,
    pub lh: i32,
    pub snap: *const u8,
}

/// The framebuffer drawn into: `w` x `h` bytes, rows `stride` apart. `side` (null: none), one
/// u32 a pixel (rows `w` apart): 1 + the land pixel it shows when that is untouched since the
/// start (a landscape tile's pixel equal to the start's), else 0
pub struct Target {
    pub data: *mut u8,
    pub w: i32,
    pub h: i32,
    pub stride: i32,
    pub side: *mut u32,
}

/// Zoom `zoom / 16` about WA's screen centre `centre`, landing on `out_centre`; WA's screen is
/// `wa` (width, height)
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub zoom: i32,
    pub centre: (i32, i32),
    pub out_centre: (i32, i32),
    pub wa: (i32, i32),
}

impl View {
    /// The WA pixel an output pixel shows
    #[inline]
    pub fn back(&self, ox: i32, oy: i32) -> (i32, i32) {
        let f = |o: i32, oc: i32, c: i32| c + (16 * (o - oc) + 8).div_euclid(self.zoom);
        (f(ox, self.out_centre.0, self.centre.0), f(oy, self.out_centre.1, self.centre.1))
    }

    /// The output pixels that can show WA's [l, r) x [t, b) (a little wide; the per-pixel test
    /// decides)
    fn forward(&self, l: i32, t: i32, r: i32, b: i32) -> (i32, i32, i32, i32) {
        let f = |v: i32, c: i32, oc: i32| oc + ((v - c) * self.zoom).div_euclid(16);
        (f(l, self.centre.0, self.out_centre.0) - 1, f(t, self.centre.1, self.out_centre.1) - 1, f(r, self.centre.0, self.out_centre.0) + 2, f(b, self.centre.1, self.out_centre.1) + 2)
    }
}

/// An operation's area [l, r) x [t, b) in WA's pixels, cut to its clip and WA's screen
pub fn area(op: &Op, wa: (i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let (l, t, r, b, c) = match *op {
        Op::Bitmap { x, y, w, h, clip, .. } | Op::Grid { x, y, w, h, clip, .. } => (x, y, x + w, y + h, clip),
        Op::Fill { x1, y1, x2, y2, clip, .. } => (x1, y1, x2, y2, clip),
        Op::Pixel { x, y, clip, .. } => (x, y, x + 1, y + 1, clip),
        Op::VLine { x, y1, y2, .. } => (x, y1, x + 1, y2, Clip::NONE),
    };
    let (l, t, r, b) = (l.max(c.l).max(0), t.max(c.t).max(0), r.min(c.r).min(wa.0), b.min(c.b).min(wa.1));
    (l < r && t < b).then_some((l, t, r, b))
}

/// The variants implemented; anything else is refused (named, for the log)
pub fn check(op: &Op) -> Result<(), String> {
    if let Op::Grid { lut, mode, .. } = *op {
        let (kind, orient) = (mode & 0xFFFF, mode >> 16);
        let mirrored = match orient {
            0 | 0xB => false,
            1 | 0xA => true,
            o => return Err(format!("grid blit orientation {o:#x}")),
        };
        let ok = match (kind, mirrored, lut.is_some()) {
            (1, false, _) | (1, true, false) => true,
            (0 | 2 | 3, false, false) => true,
            _ => false,
        };
        if !ok {
            return Err(format!("grid blit mode {kind} orientation {orient:#x} blend table {}", lut.is_some()));
        }
    }
    Ok(())
}

/// The value WA's pixel (X, Y) gets from an operation whose area is `a`, over `d` (the pixel
/// below); None leaves it (wa-play raster.rs `shade`)
#[inline]
fn shade(op: &Op, a: (i32, i32, i32, i32), x_: i32, y_: i32, d: u8) -> Option<u8> {
    let (l, t, r, b) = a;
    if x_ < l || x_ >= r || y_ < t || y_ >= b {
        return None;
    }
    match *op {
        Op::Bitmap { x, y, src, keyed, key, .. } => {
            let s = src.at(x_ - x, y_ - y);
            (!keyed || s != key).then_some(s)
        }
        Op::Fill { colour, .. } | Op::Pixel { colour, .. } | Op::VLine { colour, .. } => Some(colour),
        Op::Grid { x, y, w, src, sx, sy, lut, mode, .. } => {
            let kind = mode & 0xFFFF;
            let mirrored = matches!(mode >> 16, 1 | 0xA);
            let col = if mirrored { sx + x + w - 1 - x_ } else { sx + x_ - x };
            let row = sy + y_ - y;
            let s = src.at(col, row);
            if let (Some(tab), 1) = (lut, kind) {
                // a blend table, forwards: the row's first width - width % 4 pixels in groups
                // of 4 from its left end (a group of four 0s is left, otherwise all 4 are
                // blended), the last width % 4 one by one, source 0 left
                let (i, width) = (x_ - l, r - l);
                if i < width - (width & 3) {
                    let c0 = sx + l - x + i - i % 4;
                    if (0..4).all(|k| src.at(c0 + k, row) == 0) {
                        return None;
                    }
                } else if s == 0 {
                    return None;
                }
                return Some(unsafe { *tab.add(((d as usize) << 8) | s as usize) });
            }
            match kind {
                0 => Some(s),
                1 => (s != 0).then_some(s),
                2 => (s != 0 && d != 0).then_some(s),
                3 => (s != 0 && d == 0).then_some(s),
                _ => None,
            }
        }
    }
}

thread_local! {
    /// the WA column of each output column of the operation being drawn (kept between calls)
    static XS: std::cell::RefCell<Vec<i32>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// One operation onto `t` through `view` (mapped: every output pixel shows the WA pixel it maps
/// back to). The caller has checked it (`check`). The mapping is monotonic and separable, so
/// the output columns showing the operation's area are one run, found once; each row is then
/// drawn by a loop for the operation's kind (the same rule per pixel as `shade`).
pub fn draw_mapped(op: &Op, t: &mut Target, view: &View) {
    let Some(a) = area(op, view.wa) else { return };
    let (ol, ot, or, ob) = view.forward(a.0, a.1, a.2, a.3);
    let (ol, ot, or, ob) = (ol.max(0), ot.max(0), or.min(t.w), ob.min(t.h));
    if ol >= or || ot >= ob {
        return;
    }
    XS.with(|xs| {
        let mut xs = xs.borrow_mut();
        xs.clear();
        let mut c0 = -1;
        for ox in ol..or {
            let x = view.back(ox, 0).0;
            if x >= a.0 && x < a.2 {
                if c0 < 0 {
                    c0 = ox;
                }
                xs.push(x);
            }
        }
        if xs.is_empty() {
            return;
        }
        for oy in ot..ob {
            let y = view.back(0, oy).1;
            if y < a.1 || y >= a.3 {
                continue;
            }
            let row = unsafe { std::slice::from_raw_parts_mut(t.data.add((oy * t.stride + c0) as usize), xs.len()) };
            if t.side.is_null() {
                draw_row(op, a, &xs, y, row);
            } else {
                let side = unsafe { std::slice::from_raw_parts_mut(t.side.add((oy * t.w + c0) as usize), xs.len()) };
                draw_row_side(op, a, &xs, y, row, side);
            }
        }
    })
}

/// One output row of an operation: `row[k]` shows WA's pixel (xs[k], y), all inside the area
#[inline]
fn draw_row(op: &Op, a: (i32, i32, i32, i32), xs: &[i32], y: i32, row: &mut [u8]) {
    match *op {
        Op::Fill { colour, .. } | Op::Pixel { colour, .. } | Op::VLine { colour, .. } => row.fill(colour),
        Op::Bitmap { x: bx, y: by, src, keyed, key, .. } => {
            let sy = y - by;
            if sy < 0 || sy >= src.h {
                // outside the surface the source reads 0
                if !keyed || key != 0 {
                    row.fill(0);
                }
                return;
            }
            let srow = unsafe { src.data.add((sy * src.stride) as usize) };
            // 1:1 columns inside the surface (no zoom): the source row as a slice
            let (c0, n) = (xs[0] - bx, xs.len() as i32);
            if xs[xs.len() - 1] - xs[0] + 1 == n && c0 >= 0 && c0 + n <= src.w {
                let sr = unsafe { std::slice::from_raw_parts(srow.add(c0 as usize), n as usize) };
                if keyed {
                    row.iter_mut().zip(sr).for_each(|(o, &s)| {
                        if s != key {
                            *o = s;
                        }
                    });
                } else {
                    row.copy_from_slice(sr);
                }
                return;
            }
            for (o, &x) in row.iter_mut().zip(xs) {
                let c = x - bx;
                let s = if (c as u32) < src.w as u32 { unsafe { *srow.add(c as usize) } } else { 0 };
                if !keyed || s != key {
                    *o = s;
                }
            }
        }
        Op::Grid { x: gx, y: gy, w: gw, src, sx, sy, lut, mode, .. } => {
            if lut.is_some() {
                // a blend table (its groups of 4 need the whole row): the reference rule
                for (o, &x) in row.iter_mut().zip(xs) {
                    if let Some(v) = shade(op, a, x, y, *o) {
                        *o = v;
                    }
                }
                return;
            }
            let r = sy + y - gy;
            if r < 0 || r >= src.h {
                // source 0: only mode 0 draws it
                if mode & 0xFFFF == 0 {
                    row.fill(0);
                }
                return;
            }
            let srow = unsafe { src.data.add((r * src.stride) as usize) };
            let mirrored = matches!(mode >> 16, 1 | 0xA);
            let (base, step) = if mirrored { (sx + gx + gw - 1, -1) } else { (sx - gx, 1) };
            let at = |x: i32| -> u8 {
                let c = base + step * x;
                if (c as u32) < src.w as u32 { unsafe { *srow.add(c as usize) } } else { 0 }
            };
            // 1:1 columns, not mirrored, inside the source (no zoom): the source row as a slice
            let (c0, n) = (base + xs[0], xs.len() as i32);
            if !mirrored && xs[xs.len() - 1] - xs[0] + 1 == n && c0 >= 0 && c0 + n <= src.w {
                let sr = unsafe { std::slice::from_raw_parts(srow.add(c0 as usize), n as usize) };
                let it = row.iter_mut().zip(sr);
                match mode & 0xFFFF {
                    0 => row.copy_from_slice(sr),
                    1 => it.for_each(|(o, &s)| {
                        if s != 0 {
                            *o = s;
                        }
                    }),
                    2 => it.for_each(|(o, &s)| {
                        if s != 0 && *o != 0 {
                            *o = s;
                        }
                    }),
                    3 => it.for_each(|(o, &s)| {
                        if s != 0 && *o == 0 {
                            *o = s;
                        }
                    }),
                    _ => {}
                }
                return;
            }
            match mode & 0xFFFF {
                0 => row.iter_mut().zip(xs).for_each(|(o, &x)| *o = at(x)),
                1 => row.iter_mut().zip(xs).for_each(|(o, &x)| {
                    let s = at(x);
                    if s != 0 {
                        *o = s;
                    }
                }),
                2 => row.iter_mut().zip(xs).for_each(|(o, &x)| {
                    let s = at(x);
                    if s != 0 && *o != 0 {
                        *o = s;
                    }
                }),
                3 => row.iter_mut().zip(xs).for_each(|(o, &x)| {
                    let s = at(x);
                    if s != 0 && *o == 0 {
                        *o = s;
                    }
                }),
                _ => {}
            }
        }
    }
}

/// `draw_row` keeping the side buffer: every pixel written gets 0 there, except an untouched
/// land pixel from a landscape tile, which gets 1 + its index in the land
fn draw_row_side(op: &Op, a: (i32, i32, i32, i32), xs: &[i32], y: i32, row: &mut [u8], side: &mut [u32]) {
    match *op {
        Op::Fill { colour, .. } | Op::Pixel { colour, .. } | Op::VLine { colour, .. } => {
            row.fill(colour);
            side.fill(0);
        }
        Op::Bitmap { x: bx, y: by, src, keyed, key, land, .. } => {
            let sy = y - by;
            let in_row = sy >= 0 && sy < src.h;
            let srow = if in_row { unsafe { src.data.add((sy * src.stride) as usize) } } else { std::ptr::null() };
            for k in 0..row.len() {
                let c = xs[k] - bx;
                let inside = in_row && (c as u32) < src.w as u32;
                let s = if inside { unsafe { *srow.add(c as usize) } } else { 0 };
                if keyed && s == key {
                    continue;
                }
                row[k] = s;
                side[k] = match land {
                    Some(l) if inside && s != 0 => {
                        let (lx, ly) = (l.x0 + c, l.y0 + sy);
                        if lx < l.lw && ly < l.lh {
                            let i = (ly * l.lw + lx) as usize;
                            if unsafe { *l.snap.add(i) } == s { i as u32 + 1 } else { 0 }
                        } else {
                            0
                        }
                    }
                    _ => 0,
                };
            }
        }
        Op::Grid { .. } => {
            for k in 0..row.len() {
                if let Some(v) = shade(op, a, xs[k], y, row[k]) {
                    row[k] = v;
                    side[k] = 0;
                }
            }
        }
    }
}
