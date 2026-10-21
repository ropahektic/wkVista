//! WA's screen resolution changed in game (the Resolution dropdown of WA's Esc menu,
//! escmenu.rs), by WA's own path, as WA itself switches between the
//! frontend's 640 x 480 and the game's resolution at every game start
//! (Frontend::LaunchGameSession 0x4EC5E0):
//!
//! 1. RenderContext resize (vtable 0x662EC8 slot 9, 0x5A2530; ECX the RenderContext 0x79D6D4,
//!    EDX the result, width, height; ret 8): a new framebuffer (0x7A0EEC, its size 0x7A0EF0 /
//!    0x7A0EF4), then the backend's own resize (Direct3D 9: slot 9 0x59CB00 -> its setup, slot
//!    6, with the device there -> 0x59CA50: Reset with the new size, WA's windowed flag
//!    0x7A084C, its vsync; its resources made again).
//! 2. DisplayGfx's sizes as DisplayGfx::Init 0x569D00 sets them (+0x3548 / +0x354C, the clip
//!    +0x3550..+0x355C, the centre +0x3560 / +0x3564).
//! 3. The mouse and the window as GameEngine::InitHardware 0x56D5xx and DisplayGfx::Init do:
//!    windowed (0x7A084C set) the mouse centre is the screen's (the session's +0x54 / +0x58 and
//!    +0x80 / +0x84) and the cursor is clipped to the window's client area (the window is not
//!    resized: Direct3D scales the back buffer into it); fullscreen the window is moved to
//!    (0, 0, w, h) and the mouse centre is (w / 2, h / 2).
//!
//! 4. The choice saved as WA's own setting (HKCU\Software\Team17SoftwareLTD\WormsArmageddon\
//!    Options, DisplayXSize / DisplayYSize), so the next game starts with it, and the Esc menu
//!    laid out again (its label current).
//!
//! It runs between frames (before GameRender), never while WA draws. Every step is logged.

use std::sync::Mutex;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplaySettingsA, MapWindowPoints, DEVMODEA};
use windows_sys::Win32::UI::WindowsAndMessaging::{ClipCursor, GetClientRect, GetSystemMetrics, GetWindowRect, MoveWindow, SetCursorPos};

use crate::log;

/// A resolution asked for (applied before the next frame)
static PENDING: Mutex<Option<(u32, u32)>> = Mutex::new(None);

fn rb(va: usize) -> usize {
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    va.wrapping_add(base.wrapping_sub(0x40_0000))
}

unsafe fn r32(a: usize) -> u32 {
    std::ptr::read_unaligned(a as *const u32)
}
unsafe fn w32(a: usize, v: u32) {
    std::ptr::write_unaligned(a as *mut u32, v)
}

/// This PC's display modes (width, height), each once, smallest first (at least 640 x 480)
pub fn modes() -> Vec<(u32, u32)> {
    let mut v = Vec::new();
    let mut k = 0;
    loop {
        let mut dm: DEVMODEA = unsafe { std::mem::zeroed() };
        dm.dmSize = std::mem::size_of::<DEVMODEA>() as u16;
        if unsafe { EnumDisplaySettingsA(std::ptr::null(), k, &mut dm) } == 0 {
            break;
        }
        let m = (dm.dmPelsWidth, dm.dmPelsHeight);
        if m.0 >= 640 && m.1 >= 480 && !v.contains(&m) {
            v.push(m);
        }
        k += 1;
    }
    v.sort_by_key(|&(w, h)| (w * h, w));
    v
}

/// A resolution asked for (applied before the next frame)
pub fn ask(w: u32, h: u32) {
    let cur = unsafe { (r32(rb(0x7A0EF0)), r32(rb(0x7A0EF4))) };
    log::line(&format!("resolution: {}x{} -> {w}x{h} asked", cur.0, cur.1));
    *PENDING.lock().unwrap() = Some((w, h));
}

unsafe fn rects(wnd: HWND) -> String {
    let (mut wr, mut cr): (RECT, RECT) = (std::mem::zeroed(), std::mem::zeroed());
    GetWindowRect(wnd, &mut wr);
    GetClientRect(wnd, &mut cr);
    format!("window ({},{})-({},{}) client {}x{}", wr.left, wr.top, wr.right, wr.bottom, cr.right - cr.left, cr.bottom - cr.top)
}

/// Before a frame: a pending resolution applied (`runtime`: GameRuntime, `display`: DisplayGfx)
pub unsafe fn apply(runtime: usize, display: usize, frame: u32) {
    let Some((w, h)) = PENDING.lock().unwrap().take() else { return };
    let wnd = r32(rb(0x6B390C)) as HWND;
    let windowed = r32(rb(0x7A084C)) != 0;
    let rc = r32(rb(0x79D6D4)) as usize;
    let backend = if rc != 0 { r32(rc + 0x18) as usize } else { 0 };
    log::line(&format!(
        "[{frame}] resolution {w}x{h}: before: framebuffer {}x{}, display {}x{}, backend {:#x} size {}x{}, {}, {}, render lock {}",
        r32(rb(0x7A0EF0)), r32(rb(0x7A0EF4)), r32(display + 0x3548), r32(display + 0x354C), backend,
        if backend != 0 { r32(backend + 0x1C) } else { 0 }, if backend != 0 { r32(backend + 0x20) } else { 0 },
        if windowed { "windowed" } else { "fullscreen" }, rects(wnd), r32(display + 0x3D98)
    ));
    if rc == 0 {
        log::line("resolution: no RenderContext");
        return;
    }
    // wkVista's own Direct3D surface goes first (a reset needs every default-pool one gone)
    crate::present::release_surface();
    // 1. RenderContext resize (WA's framebuffer, then its backend's reset)
    let resize: extern "fastcall" fn(usize, *mut u32, u32, u32) -> *mut u32 = std::mem::transmute(r32(r32(rc) as usize + 9 * 4) as usize);
    let mut result = 0u32;
    resize(rc, &mut result, w, h);
    let ok = r32(rb(0x8AC8B4));
    if result != ok {
        // an error record: +4 the call's name, +8 its result
        let what = if result != 0 { std::ffi::CStr::from_ptr(r32(result as usize + 4) as *const i8).to_string_lossy().to_string() } else { String::new() };
        let code = if result != 0 { r32(result as usize + 8) } else { 0 };
        log::line(&format!("[{frame}] resolution {w}x{h}: WA's resize failed: {what} {code:#x}"));
    }
    // 2. DisplayGfx's sizes, as DisplayGfx::Init
    for (o, v) in [(0x3548, w), (0x354C, h), (0x3550, 0), (0x3554, 0), (0x3558, w), (0x355C, h), (0x3560, w / 2), (0x3564, h / 2)] {
        w32(display + o, v);
    }
    // 3. the mouse and the window, as InitHardware / DisplayGfx::Init
    let s = r32(rb(0x7A0884)) as usize;
    let (cx, cy) = if windowed { (GetSystemMetrics(0) / 2, GetSystemMetrics(1) / 2) } else { (w as i32 / 2, h as i32 / 2) };
    if !windowed && wnd != std::ptr::null_mut() {
        MoveWindow(wnd, 0, 0, w as i32, h as i32, 0);
    }
    if s != 0 {
        for (o, v) in [(0x54, cx), (0x58, cy), (0x80, cx), (0x84, cy)] {
            w32(s + o, v as u32);
        }
    }
    SetCursorPos(cx, cy);
    if windowed && wnd != std::ptr::null_mut() {
        let mut cr: RECT = std::mem::zeroed();
        GetClientRect(wnd, &mut cr);
        MapWindowPoints(wnd, std::ptr::null_mut(), &mut cr as *mut RECT as *mut POINT, 2);
        ClipCursor(&cr);
    }
    // 4. kept as WA's setting; the Esc menu's label
    if result == ok {
        save(w, h);
        crate::escmenu::refresh(runtime);
    }
    log::line(&format!(
        "[{frame}] resolution {w}x{h}: after: result {}, framebuffer {}x{}, backend size {}x{}, mouse centre ({cx},{cy}), {}",
        if result == ok { "ok".to_string() } else { format!("{result:#x}") },
        r32(rb(0x7A0EF0)), r32(rb(0x7A0EF4)),
        if backend != 0 { r32(backend + 0x1C) } else { 0 }, if backend != 0 { r32(backend + 0x20) } else { 0 },
        rects(wnd)
    ));
}

/// WA's own resolution setting (read at its start)
fn save(w: u32, h: u32) {
    use windows_sys::Win32::System::Registry::{RegSetKeyValueA, HKEY_CURRENT_USER, REG_DWORD};
    let key = b"Software\\Team17SoftwareLTD\\WormsArmageddon\\Options\0";
    for (name, v) in [(&b"DisplayXSize\0"[..], w), (&b"DisplayYSize\0"[..], h)] {
        let r = unsafe { RegSetKeyValueA(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_DWORD, &v as *const u32 as *const _, 4) };
        if r != 0 {
            log::line(&format!("resolution: WA's setting not saved ({r})"));
        }
    }
}
