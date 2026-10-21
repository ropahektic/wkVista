//! The hooks on WA 3.8.1 (the same functions wa-play captures; docs there: docs/RENDERER.md).
//!
//! In game, each frame runs through GameRender 0x533DC0 (wrapped: `pre` / `post`). While the
//! zoom is not 1x, WA's drawing into its framebuffer goes through the hooks below: an operation
//! in world space (the render-queue entries that go through the camera) is drawn by wkVista
//! through the view (raster.rs); everything else, the HUD (screen space) and any drawing that
//! is not into the framebuffer, is WA's own code, untouched. At 1x every hook steps aside at
//! once: WA draws exactly as without wkVista.
//!
//! | Hook | WA function | Convention |
//! |---|---|---|
//! | 0x533DC0 | GameRender | ECX runtime; ret |
//! | 0x542350 | RenderDrawingQueue | EAX queue; display, camera; ret 8 |
//! | 0x56A700 | DisplayGfx::BlitBitmapClipped | EAX display, EDX w; x, y, h, bitmap, flags; ret 0x14 |
//! | 0x56B810 | DisplayGfx fill rectangle | ECX display; x1, y1, x2, y2 (from the centre), colour; ret 0x14 |
//! | 0x4F6910 | BitGrid::BlitSpriteRect | ESI target; x, y, w, h, src, sx, sy, blend table, mode; ret 0x24 |
//! | 0x4F9180 / 0x4F5E40 | BitGrid pixel, clipped / not | ECX grid; x, y, colour; ret 0xC |
//! | 0x4F9110 | BitGrid vertical line | ECX grid; x, y1, y2, colour; ret 0x10 |
//! | 0x4F5D50 | BitGrid fill rectangle | ECX grid; x1, y1, x2, y2, colour; ret 0x14 |
//! | 0x542F10 | ClipContext::ClampCameraToBounds | EAX viewport width, ECX bottom bound; context, viewport height, left, top, right; ret 0x14 |
//! | 0x572660 | WA's window procedure | stdcall: the wheel and wkVista's keys |
//!
//! The camera: WA keeps its camera half a viewport away from the level's edges (GameRender
//! clamps its three camera contexts each frame). Zoomed in, only the middle w/z x h/z of WA's
//! screen is seen, so the clamp is given that size instead: WA's camera can then scroll until
//! the zoomed view reaches the level's edge (the water, the bottom), never past it. The camera
//! is WA's local view, not game state.

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::time::Instant;

use crate::raster::{check, draw_mapped, Clip, Op, Src, Target, View};
use crate::{config::Config, log};

const IMAGE_BASE: usize = 0x0040_0000;
static BASE_DELTA: AtomicU32 = AtomicU32::new(0);

/// A WA address in this process
fn rb(va: u32) -> usize {
    (va as usize).wrapping_add(BASE_DELTA.load(Ordering::Relaxed) as usize)
}

unsafe fn r32(a: usize) -> u32 {
    std::ptr::read_unaligned(a as *const u32)
}
unsafe fn i32_(a: usize) -> i32 {
    std::ptr::read_unaligned(a as *const i32)
}
unsafe fn w32(a: usize, v: u32) {
    std::ptr::write_unaligned(a as *mut u32, v)
}

/// The registers as a hook thunk saved them (pushad), then the return address and the stack
/// arguments
#[repr(C)]
pub struct Regs {
    edi: u32,
    esi: u32,
    ebp: u32,
    esp: u32,
    ebx: u32,
    edx: u32,
    ecx: u32,
    eax: u32,
}

impl Regs {
    /// Stack argument `k` (1 = the first)
    unsafe fn arg(&self, k: usize) -> u32 {
        *(self as *const Regs as *const u32).add(8 + k)
    }

    /// Stack argument `k` changed before WA's function runs
    unsafe fn set_arg(&mut self, k: usize, v: u32) {
        *(self as *mut Regs as *mut u32).add(8 + k) = v;
    }
}

// ---- state (WA renders and pumps messages on one thread) ----

/// the zoom in sixteenths (16 = 1x)
static ZOOM: AtomicI32 = AtomicI32::new(16);
/// after a panic: wkVista steps aside for good
static DISABLED: AtomicBool = AtomicBool::new(false);
/// the time of the last GameRender (ms since start): in game when recent
static LAST_RENDER: AtomicU32 = AtomicU32::new(0);
static MARKS: AtomicU32 = AtomicU32::new(0);
static PENDING_MARK: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Space {
    Screen,
    World,
}

struct Frame {
    /// this frame is drawn through the view
    active: bool,
    space: Space,
    display: usize,
    view: View,
    target: Target,
    // the frame's counts
    zoomed: u32,
    by_wa: u32,
    refused: u32,
    kinds: [u32; 5],
    t0: Instant,
    t_zoomed: f64,
}

struct State {
    cfg: Config,
    dir: std::path::PathBuf,
    start: Instant,
    frame: Option<Frame>,
    world: usize,
    last_frame: u32,
    refused_seen: HashSet<String>,
    logged_target: bool,
}

static mut STATE: Option<State> = None;

#[allow(static_mut_refs)]
fn st() -> &'static mut State {
    unsafe { STATE.as_mut().expect("wkVista state") }
}

fn now_ms() -> u32 {
    st().start.elapsed().as_millis() as u32
}

/// Runs a hook body; a panic is logged and wkVista steps aside from then on
fn guard<T>(default: T, f: impl FnOnce() -> T) -> T {
    if DISABLED.load(Ordering::Relaxed) {
        return default;
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(e) => {
            let what = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
            log::line(&format!("PANIC in a hook ({what}): wkVista is off until WA restarts"));
            DISABLED.store(true, Ordering::Relaxed);
            default
        }
    }
}

// ---- thunks ----

/// A hook thunk: the registers saved, `handler(&mut Regs)`; 1: handled (the registers as the
/// handler left them, `ret N`), 0: WA's own function runs, untouched
macro_rules! thunk {
    ($name:ident, $handler:path, $orig:ident, $ret:literal) => {
        static mut $orig: usize = 0;
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            core::arch::naked_asm!(
                "pushad",
                "push esp",
                "call {h}",
                "add esp, 4",
                "test eax, eax",
                "popad",
                "jnz 2f",
                "jmp dword ptr [{o}]",
                "2:",
                concat!("ret ", $ret),
                h = sym $handler,
                o = sym $orig,
            );
        }
    };
}

thunk!(t_bitmap, h_bitmap, O_BITMAP, "0x14");
thunk!(t_fill, h_fill, O_FILL, "0x14");
thunk!(t_grid, h_grid, O_GRID, "0x24");
thunk!(t_pixel_clipped, h_pixel_clipped, O_PIXEL_CLIPPED, "0xC");
thunk!(t_pixel, h_pixel, O_PIXEL, "0xC");
thunk!(t_vline, h_vline, O_VLINE, "0x10");
thunk!(t_grid_fill, h_grid_fill, O_GRID_FILL, "0x14");
thunk!(t_queue, h_queue, O_QUEUE, "8");
thunk!(t_clamp, h_clamp, O_CLAMP, "0x14");

/// GameRender wrapped: `pre` (ECX the runtime), WA's own, `post`
static mut O_RENDER: usize = 0;
#[unsafe(naked)]
unsafe extern "C" fn t_render() {
    core::arch::naked_asm!(
        "pushad",
        "push ecx",
        "call {pre}",
        "add esp, 4",
        "popad",
        "call dword ptr [{o}]",
        "pushad",
        "call {post}",
        "popad",
        "ret",
        pre = sym pre,
        post = sym post,
        o = sym O_RENDER,
    );
}

/// WA's RenderDrawingQueue on a one-entry view of the queue, with the registers it was
/// called with (EAX the queue) and its two arguments
static mut CALL_REGS: [u32; 8] = [0; 8];
static mut CALL_ARGS: [u32; 2] = [0; 2];
static mut CALL_EAX: u32 = 0;
#[unsafe(naked)]
unsafe extern "C" fn call_queue() {
    core::arch::naked_asm!(
        "pushad",
        "push dword ptr [{a}+4]",
        "push dword ptr [{a}]",
        "mov edi, dword ptr [{r}]",
        "mov esi, dword ptr [{r}+4]",
        "mov ebx, dword ptr [{r}+16]",
        "mov edx, dword ptr [{r}+20]",
        "mov ecx, dword ptr [{r}+24]",
        "mov eax, dword ptr [{r}+28]",
        "call dword ptr [{o}]",
        "mov dword ptr [{res}], eax",
        "popad",
        "ret",
        a = sym CALL_ARGS,
        r = sym CALL_REGS,
        o = sym O_QUEUE,
        res = sym CALL_EAX,
    );
}

// ---- the frame ----

extern "C" fn pre(runtime: u32) {
    guard((), || unsafe {
        LAST_RENDER.store(now_ms(), Ordering::Relaxed);
        let s = st();
        let world = r32(runtime as usize + 0x488) as usize;
        if world == 0 {
            s.frame = None;
            return;
        }
        let frame_no = r32(world + 0x5CC);
        if world != s.world || frame_no < s.last_frame {
            s.world = world;
            let csv = if s.cfg.csv { log::new_game(&s.dir) } else { None };
            log::line(&format!("[{frame_no}] game: world {world:#x}{}", csv.map(|p| format!(", per-frame log {}", p.display())).unwrap_or_default()));
            s.logged_target = false;
        }
        s.last_frame = frame_no;
        let display = r32(world + 4) as usize;
        // WA's framebuffer (the RenderContext's software buffer the backend shows)
        let (data, w, h) = (r32(rb(0x007A_0EEC)) as usize, i32_(rb(0x007A_0EF0)), i32_(rb(0x007A_0EF4)));
        let zoom = ZOOM.load(Ordering::Relaxed);
        let active = zoom != 16 && data != 0 && w > 0 && h > 0 && display != 0;
        let centre = (w / 2, h / 2);
        s.frame = Some(Frame {
            active,
            space: Space::Screen,
            display,
            view: View { zoom, centre, out_centre: centre, wa: (w, h) },
            target: Target { data: data as *mut u8, w, h, stride: w },
            zoomed: 0,
            by_wa: 0,
            refused: 0,
            kinds: [0; 5],
            t0: Instant::now(),
            t_zoomed: 0.0,
        });
    })
}

extern "C" fn post() {
    guard((), || unsafe {
        let s = st();
        let Some(f) = s.frame.take() else { return };
        // the frame as colours for the true colour output: WA's finished framebuffer through
        // WA's display palette (DisplayGfx +0x358D: 256 x R, G, B, flags)
        let t_rgba = Instant::now();
        if s.cfg.truecolor && !f.target.data.is_null() && f.display != 0 {
            let mut pal = [0u32; 256];
            for (k, c) in pal.iter_mut().enumerate() {
                let p = (f.display + 0x358D + 4 * k) as *const u8;
                *c = (*p as u32) << 16 | (*p.add(1) as u32) << 8 | *p.add(2) as u32;
            }
            let (w, h, stride) = (f.target.w as usize, f.target.h as usize, f.target.stride as usize);
            crate::present::fill(w as u32, h as u32, |px| {
                for y in 0..h {
                    let src = std::slice::from_raw_parts(f.target.data.add(y * stride), w);
                    for (o, &i) in px[y * w..(y + 1) * w].iter_mut().zip(src) {
                        *o = pal[i as usize];
                    }
                }
            });
            crate::present::SHOW.store(true, Ordering::Relaxed);
        }
        let ms_rgba = t_rgba.elapsed().as_secs_f64() * 1e3;
        let ms = f.t0.elapsed().as_secs_f64() * 1e3;
        let mark = PENDING_MARK.swap(0, Ordering::Relaxed);
        if s.cfg.csv {
            let k = f.kinds;
            log::row(&format!(
                "{},{},{ms:.2},{:.2},{ms_rgba:.2},{},{},{},{},{},{},{},{},{}",
                s.last_frame, f.view.zoom, f.t_zoomed, f.zoomed, f.by_wa, f.refused, k[0], k[1], k[2], k[3], k[4],
                if mark != 0 { mark.to_string() } else { String::new() }
            ));
            if mark != 0 || s.last_frame % 250 == 0 {
                log::flush();
            }
        }
        if f.active && !s.logged_target {
            s.logged_target = true;
            let g = r32(f.display + 0x3D9C) as usize;
            let grid = if g != 0 { format!("data {:#x} {}x{} stride {}", r32(g + 8), r32(g + 0x14), r32(g + 0x18), r32(g + 0x10)) } else { "none".into() };
            log::line(&format!(
                "[{}] first zoomed frame: framebuffer {:#x} {}x{}, display {:#x} centre {:?}, its grid {grid}; {} zoomed, {} by WA, {} refused, {ms:.2} ms",
                s.last_frame, f.target.data as usize, f.target.w, f.target.h, f.display, f.view.centre, f.zoomed, f.by_wa, f.refused
            ));
        }
    })
}

/// The frame's state when this hook should draw: in game, zoomed, a world-space operation
fn drawing() -> Option<&'static mut Frame> {
    let f = st().frame.as_mut()?;
    if !f.active {
        return None;
    }
    if f.space == Space::Screen {
        f.by_wa += 1;
        return None;
    }
    Some(f)
}

/// An operation through the view; a refused variant is WA's own to draw (unzoomed), logged once
fn draw(f: &mut Frame, op: Op, kind: usize) -> bool {
    if let Err(why) = check(&op) {
        f.refused += 1;
        let s = st();
        if s.refused_seen.insert(why.clone()) {
            log::line(&format!("[{}] refused (WA draws it unzoomed): {why}", s.last_frame));
        }
        return false;
    }
    let t = Instant::now();
    draw_mapped(&op, &mut f.target, &f.view);
    f.t_zoomed += t.elapsed().as_secs_f64() * 1e3;
    f.zoomed += 1;
    f.kinds[kind] += 1;
    true
}

unsafe fn clip_at(a: usize) -> Clip {
    Clip { l: i32_(a), t: i32_(a + 4), r: i32_(a + 8), b: i32_(a + 12) }
}

/// The framebuffer's BitGrid (DisplayGfx +0x3D9C)
unsafe fn fb_grid(f: &Frame) -> usize {
    r32(f.display + 0x3D9C) as usize
}

/// A zoomed frame, and `grid` is the framebuffer's (drawing into any other grid is WA's own)
unsafe fn on_fb_grid(grid: u32) -> bool {
    st().frame.as_ref().is_some_and(|f| f.active && grid as usize == fb_grid(f))
}

// ---- the hooks ----

extern "C" fn h_bitmap(r: &mut Regs) -> u32 {
    guard(0, || unsafe {
        let Some(f) = drawing() else { return 0 };
        let (x, y, h, bitmap, flags) = (r.arg(1) as i32, r.arg(2) as i32, r.arg(3) as i32, r.arg(4) as usize, r.arg(5));
        let s = r32(bitmap + 4) as usize;
        if s == 0 {
            // no surface yet: WA makes one (and draws nothing)
            f.by_wa += 1;
            return 0;
        }
        let (sw, sh) = (i32_(s + 4), i32_(s + 8));
        let src = Src { data: r32(s + 0xC) as *const u8, w: sw, h: sh, stride: sw };
        let op = Op::Bitmap { x, y, w: r.edx as i32, h, src, keyed: flags & 2 != 0, key: *((s + 0x10) as *const u8), clip: clip_at(r.eax as usize + 0x3550) };
        draw(f, op, 0) as u32
    })
}

extern "C" fn h_fill(r: &mut Regs) -> u32 {
    guard(0, || unsafe {
        let Some(f) = drawing() else { return 0 };
        let d = r.ecx as usize;
        let (cx, cy) = (i32_(d + 0x3560), i32_(d + 0x3564));
        let op = Op::Fill {
            x1: r.arg(1) as i32 + cx,
            y1: r.arg(2) as i32 + cy,
            x2: r.arg(3) as i32 + cx,
            y2: r.arg(4) as i32 + cy,
            colour: r.arg(5) as u8,
            clip: clip_at(d + 0x3550),
        };
        draw(f, op, 1) as u32
    })
}

extern "C" fn h_grid(r: &mut Regs) -> u32 {
    guard(0, || unsafe {
        if !on_fb_grid(r.esi) {
            return 0;
        }
        let Some(f) = drawing() else { return 0 };
        let a: Vec<u32> = (1..=9).map(|k| r.arg(k)).collect();
        let (x, y, w, h) = (a[0] as i32, a[1] as i32, a[2] as i32, a[3] as i32);
        let g = r.esi as usize;
        let c = clip_at(g + 0x1C);
        let drawn = w != 0 && h != 0 && x < c.r && x + w > c.l && y < c.b && y + h > c.t && x.max(c.l) != (x + w).min(c.r) && y.max(c.t) != (y + h).min(c.b);
        if drawn {
            let sg = a[4] as usize;
            if sg == g || r32(sg + 0xC) != 8 {
                // from the framebuffer itself, or not 8-bit: WA's own
                f.by_wa += 1;
                return 0;
            }
            let src = Src { data: r32(sg + 8) as *const u8, w: i32_(sg + 0x14), h: i32_(sg + 0x18), stride: i32_(sg + 0x10) };
            let lut = (a[7] != 0).then_some(a[7] as *const u8);
            let op = Op::Grid { x, y, w, h, src, sx: a[5] as i32, sy: a[6] as i32, lut, mode: a[8], clip: c };
            if !draw(f, op, 2) {
                return 0;
            }
        }
        // WA's result: 1 when something is drawn
        r.eax = drawn as u32;
        1
    })
}

/// A BitGrid method on the framebuffer's grid (ECX)
fn grid_call(r: &Regs, kind: usize, op: impl FnOnce() -> Op) -> u32 {
    guard(0, || unsafe {
        if !on_fb_grid(r.ecx) {
            return 0;
        }
        let Some(f) = drawing() else { return 0 };
        draw(f, op(), kind) as u32
    })
}

extern "C" fn h_pixel_clipped(r: &mut Regs) -> u32 {
    let rr = &*r;
    grid_call(rr, 3, || unsafe { Op::Pixel { x: rr.arg(1) as i32, y: rr.arg(2) as i32, colour: rr.arg(3) as u8, clip: clip_at(rr.ecx as usize + 0x1C) } })
}

extern "C" fn h_pixel(r: &mut Regs) -> u32 {
    let rr = &*r;
    grid_call(rr, 3, || unsafe { Op::Pixel { x: rr.arg(1) as i32, y: rr.arg(2) as i32, colour: rr.arg(3) as u8, clip: Clip::NONE } })
}

extern "C" fn h_vline(r: &mut Regs) -> u32 {
    let rr = &*r;
    grid_call(rr, 4, || unsafe { Op::VLine { x: rr.arg(1) as i32, y1: rr.arg(2) as i32, y2: rr.arg(3) as i32, colour: rr.arg(4) as u8 } })
}

extern "C" fn h_grid_fill(r: &mut Regs) -> u32 {
    let rr = &*r;
    grid_call(rr, 1, || unsafe {
        Op::Fill { x1: rr.arg(1) as i32, y1: rr.arg(2) as i32, x2: rr.arg(3) as i32, y2: rr.arg(4) as i32, colour: rr.arg(5) as u8, clip: clip_at(rr.ecx as usize + 0x1C) }
    })
}

/// RenderDrawingQueue while zoomed: WA's own sort (qsort 0x5D7890 by layer, comparator
/// 0x542330), then each entry in WA's order through WA's own function on a one-entry view of
/// the queue, with the space set from the entry's kind (1, 4, 7: screen, the HUD; the rest go
/// through the camera: world). The queue: +0x10004 count, +0x10008 the entries.
extern "C" fn h_queue(r: &mut Regs) -> u32 {
    guard(0, || unsafe {
        let Some(f) = st().frame.as_mut() else { return 0 };
        if !f.active {
            return 0;
        }
        let queue = r.eax as usize;
        let n = r32(queue + 0x10004);
        let qsort: extern "C" fn(*mut c_void, u32, u32, usize) = std::mem::transmute(rb(0x005D_7890));
        qsort((queue + 0x10008) as *mut c_void, n, 4, rb(0x0054_2330));
        let entries: Vec<u32> = (0..n).map(|k| r32(queue + 0x10008 + 4 * k as usize)).collect();
        CALL_REGS = [r.edi, r.esi, r.ebp, r.esp, r.ebx, r.edx, r.ecx, r.eax];
        CALL_ARGS = [r.arg(1), r.arg(2)];
        let mut last = r.eax;
        for &e in entries.iter().rev() {
            let kind = r32(e as usize);
            if let Some(f) = st().frame.as_mut() {
                f.space = if matches!(kind, 1 | 4 | 7) { Space::Screen } else { Space::World };
            }
            w32(queue + 0x10004, 1);
            w32(queue + 0x10008, e);
            let call: unsafe extern "C" fn() = call_queue;
            call();
            last = CALL_EAX;
        }
        if let Some(f) = st().frame.as_mut() {
            f.space = Space::Screen;
        }
        w32(queue + 0x10004, n);
        for (k, &e) in entries.iter().enumerate() {
            w32(queue + 0x10008 + 4 * k, e);
        }
        r.eax = last;
        1
    })
}

/// ClampCameraToBounds while zoomed: WA's own clamp with the size seen through the zoom (the
/// viewport x 16 / n, rounded up), so the camera reaches the level's edges
extern "C" fn h_clamp(r: &mut Regs) -> u32 {
    guard(0, || unsafe {
        let Some(f) = st().frame.as_ref() else { return 0 };
        if !f.active {
            return 0;
        }
        let z = f.view.zoom;
        let seen = |v: u32| ((v as i32 * 16 + z - 1) / z) as u32;
        r.eax = seen(r.eax);
        let h = seen(r.arg(2));
        r.set_arg(2, h);
        0
    })
}

// ---- input: WA's window procedure ----

static mut O_WNDPROC: usize = 0;

/// in game: a GameRender in the last half second (the frontend never renders a game)
pub(crate) fn in_game() -> bool {
    now_ms().wrapping_sub(LAST_RENDER.load(Ordering::Relaxed)) < 500
}

fn set_zoom(z: i32) {
    let old = ZOOM.swap(z, Ordering::Relaxed);
    if old != z {
        log::line(&format!("[{}] zoom {:.2}x", st().last_frame, z as f64 / 16.0));
    }
}

/// One zoom step in (+1) or out (-1)
fn step_zoom(dir: i32) {
    let max = st().cfg.zoom_max;
    let steps: Vec<i32> = [16, 20, 24, 32, 40, 48, 64, 80, 96, 128].into_iter().filter(|&z| z <= max).collect();
    let z = ZOOM.load(Ordering::Relaxed);
    let i = steps.iter().position(|&s| s >= z).unwrap_or(0) as i32;
    let j = (i + dir).clamp(0, steps.len() as i32 - 1);
    set_zoom(steps[j as usize]);
}

extern "system" fn wndproc(hwnd: isize, msg: u32, wp: usize, lp: isize) -> isize {
    let handled = guard(false, || {
        let cfg = st().cfg.clone();
        match msg {
            // WM_MOUSEWHEEL
            0x020A if cfg.wheel && in_game() => {
                let delta = (wp >> 16) as u16 as i16;
                step_zoom(if delta > 0 { 1 } else { -1 });
                true
            }
            // WM_KEYDOWN (not a repeat for the mark)
            0x0100 => {
                let vk = wp as u32;
                if vk == cfg.key_mark && lp & (1 << 30) == 0 {
                    let n = MARKS.fetch_add(1, Ordering::Relaxed) + 1;
                    PENDING_MARK.store(n, Ordering::Relaxed);
                    log::line(&format!("[{}] MARK {n} (zoom {:.2}x)", st().last_frame, ZOOM.load(Ordering::Relaxed) as f64 / 16.0));
                    log::flush();
                    false
                } else if in_game() && (vk == cfg.key_zoom_in || vk == cfg.key_zoom_out || vk == cfg.key_zoom_reset) {
                    if vk == cfg.key_zoom_reset {
                        set_zoom(16);
                    } else {
                        step_zoom(if vk == cfg.key_zoom_in { 1 } else { -1 });
                    }
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    });
    if handled {
        return 0;
    }
    unsafe {
        let orig: extern "system" fn(isize, u32, usize, isize) -> isize = std::mem::transmute(O_WNDPROC);
        orig(hwnd, msg, wp, lp)
    }
}

// ---- install ----

/// The hooks on WA.exe 3.8.1 (checked by GameRender's prologue and its call of
/// RenderDrawingQueue first)
pub fn install(cfg: Config, dir: std::path::PathBuf) -> Result<(), String> {
    unsafe {
        let base = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as usize;
        BASE_DELTA.store(base.wrapping_sub(IMAGE_BASE) as u32, Ordering::Relaxed);
        STATE = Some(State { cfg, dir, start: Instant::now(), frame: None, world: 0, last_frame: 0, refused_seen: HashSet::new(), logged_target: false });
        let p = std::slice::from_raw_parts(rb(0x0053_3DC0) as *const u8, 3);
        let call = std::slice::from_raw_parts(rb(0x0053_40AF) as *const u8, 5);
        let rel = i32::from_le_bytes([call[1], call[2], call[3], call[4]]);
        if p != [0x55, 0x8B, 0xEC] || call[0] != 0xE8 || (rb(0x0053_40B4) as i32).wrapping_add(rel) as usize != rb(0x0054_2350) {
            return Err(format!("not WA 3.8.1: GameRender starts {p:02X?}, its queue call {call:02X?}"));
        }
        let hooks: [(u32, *mut c_void, *mut usize); 11] = [
            (0x0053_3DC0, t_render as *mut c_void, &raw mut O_RENDER),
            (0x0054_2350, t_queue as *mut c_void, &raw mut O_QUEUE),
            (0x0056_A700, t_bitmap as *mut c_void, &raw mut O_BITMAP),
            (0x0056_B810, t_fill as *mut c_void, &raw mut O_FILL),
            (0x004F_6910, t_grid as *mut c_void, &raw mut O_GRID),
            (0x004F_9180, t_pixel_clipped as *mut c_void, &raw mut O_PIXEL_CLIPPED),
            (0x004F_5E40, t_pixel as *mut c_void, &raw mut O_PIXEL),
            (0x004F_9110, t_vline as *mut c_void, &raw mut O_VLINE),
            (0x004F_5D50, t_grid_fill as *mut c_void, &raw mut O_GRID_FILL),
            (0x0054_2F10, t_clamp as *mut c_void, &raw mut O_CLAMP),
            (0x0057_2660, wndproc as *mut c_void, &raw mut O_WNDPROC),
        ];
        // every trampoline stored before any hook is on
        for &(va, det, orig) in &hooks {
            let tramp = minhook::MinHook::create_hook(rb(va) as *mut c_void, det).map_err(|e| format!("hook {va:#x}: {e:?}"))?;
            *orig = tramp as usize;
        }
        for &(va, _, _) in &hooks {
            minhook::MinHook::enable_hook(rb(va) as *mut c_void).map_err(|e| format!("enable {va:#x}: {e:?}"))?;
        }
    }
    Ok(())
}
