//! Fullscreen <-> windowed in game (the Display item of WA's Esc menu, escmenu.rs), with WA's own
//! pieces. WA itself only switches with a restart (CREW2App::sub_4E48F0: it saves the option and
//! exits), but every part of a switch exists in WA:
//!
//! - The flag: 0x7A084C (windowed). WA reads it live everywhere (mouse, cursor, activation,
//!   the Direct3D present parameters at each reset); WA's option WindowedMode is the options
//!   object's +0xA4 (0x88E42C; the object, app 0x7C03D0 + 0xCDFB8, is what WA saves).
//! - The window: WA makes one window (CREW2App::sub_4ECD40), style 0x900A0000 plus WS_CAPTION
//!   when windowed, WS_EX_TOPMOST when fullscreen.
//! - Windowed: MouseCursorRecenterOnWindow 0x4EC0D0 (stdcall, 1: also the display; ret 4) is
//!   WA's own game window setup for windowed mode (WA runs it when you switch back into a
//!   game): the game's resolution (0x88C7EC / 0x88C7F0) centred on the desktop, scaled down to
//!   fit, the caption put back when 0x6B32AA says it was removed (or removed when nothing fits),
//!   then the display set up again at that resolution (the Direct3D reset, now windowed).
//! - Fullscreen: what WA does at a fullscreen game start: no caption (CWnd::ModifyStyle
//!   0x5C6350, thiscall: the window object 0x6B3908, remove, add, flags; ret 0xC), the display
//!   set up again (RenderContext resize at the same size: the Direct3D reset, now fullscreen),
//!   the window at (0, 0, w, h) and topmost, the mouse centred (InitHardware).
//!
//! It runs when WA's message pump delivers wkVista's message (hooks.rs MSG_SWITCH): between
//! frames, where WA handles activation and display messages itself (a fullscreen mode change
//! makes Windows send them during the Direct3D reset). Every step is logged.

use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{ClipCursor, GetClientRect, GetWindowLongA, GetWindowRect, SetCursorPos, SetWindowPos};

use crate::log;

static PENDING: AtomicBool = AtomicBool::new(false);

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

/// Windowed now
pub fn windowed() -> bool {
    unsafe { r32(rb(0x7A084C)) != 0 }
}

/// A switch asked for (applied before the next frame)
pub fn ask_toggle() {
    log::line(&format!("display: {} asked", if windowed() { "fullscreen" } else { "windowed" }));
    PENDING.store(true, Ordering::Relaxed);
    crate::hooks::post_switch();
}

unsafe fn state(wnd: HWND) -> String {
    let (mut wr, mut cr): (RECT, RECT) = (std::mem::zeroed(), std::mem::zeroed());
    GetWindowRect(wnd, &mut wr);
    GetClientRect(wnd, &mut cr);
    format!(
        "{}, style {:#x} ex {:#x}, window ({},{})-({},{}) client {}x{}, framebuffer {}x{}",
        if windowed() { "windowed" } else { "fullscreen" },
        GetWindowLongA(wnd, -16),
        GetWindowLongA(wnd, -20),
        wr.left, wr.top, wr.right, wr.bottom, cr.right - cr.left, cr.bottom - cr.top,
        r32(rb(0x7A0EF0)), r32(rb(0x7A0EF4))
    )
}

/// Before a frame: a pending switch applied (`runtime`: GameRuntime, `display`: DisplayGfx)
pub unsafe fn apply(runtime: usize, display: usize, frame: u32) {
    if !PENDING.swap(false, Ordering::Relaxed) {
        return;
    }
    let wnd = r32(rb(0x6B390C)) as HWND;
    let cwnd = r32(rb(0x6B3908)) as usize;
    let rc = r32(rb(0x79D6D4)) as usize;
    let options = rb(0x88E388);
    let (w, h) = (r32(rb(0x7A0EF0)), r32(rb(0x7A0EF4)));
    log::line(&format!("[{frame}] display switch: before: {}", state(wnd)));
    if wnd.is_null() || cwnd == 0 || rc == 0 {
        log::line("display switch: no window or RenderContext");
        return;
    }
    // wkVista's own Direct3D surface goes first (a reset needs every default-pool one gone)
    crate::present::release_surface();
    let s = r32(rb(0x7A0884)) as usize;
    if !windowed() {
        // to windowed: WA's flag and option, not topmost, the caption marked removed (it is) so
        // WA's windowed setup puts it back, then WA's windowed setup with the display
        w32(rb(0x7A084C), 1);
        w32(options + 0xA4, 1);
        SetWindowPos(wnd, -2isize as HWND, 0, 0, 0, 0, 0x1 | 0x2 | 0x10);
        *(rb(0x6B32AA) as *mut u8) = 1;
        let setup: extern "system" fn(u32) = std::mem::transmute(rb(0x4EC0D0));
        setup(1);
        // the mouse as InitHardware: the screen's centre, the cursor kept in the client area
        let (cx, cy) = {
            use windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics;
            (GetSystemMetrics(0) / 2, GetSystemMetrics(1) / 2)
        };
        if s != 0 {
            for (o, v) in [(0x54, cx), (0x58, cy), (0x80, cx), (0x84, cy)] {
                w32(s + o, v as u32);
            }
        }
        SetCursorPos(cx, cy);
        let mut cr: RECT = std::mem::zeroed();
        GetClientRect(wnd, &mut cr);
        windows_sys::Win32::Graphics::Gdi::MapWindowPoints(wnd, std::ptr::null_mut(), &mut cr as *mut RECT as *mut windows_sys::Win32::Foundation::POINT, 2);
        ClipCursor(&cr);
    } else {
        // to fullscreen: WA's flag and option, no caption, the display set up again at the same
        // size (the Direct3D reset, now fullscreen), the window over the screen and topmost
        w32(rb(0x7A084C), 0);
        w32(options + 0xA4, 0);
        let modify: extern "thiscall" fn(usize, u32, u32, u32) -> i32 = std::mem::transmute(rb(0x5C6350));
        modify(cwnd, 0x00C0_0000, 0, 0);
        *(rb(0x6B32AA) as *mut u8) = 0;
        let resize: extern "fastcall" fn(usize, *mut u32, u32, u32) -> *mut u32 = std::mem::transmute(r32(r32(rc) as usize + 9 * 4) as usize);
        let mut result = 0u32;
        resize(rc, &mut result, w, h);
        let ok = r32(rb(0x8AC8B4));
        if result != ok {
            let what = if result != 0 { std::ffi::CStr::from_ptr(r32(result as usize + 4) as *const i8).to_string_lossy().to_string() } else { String::new() };
            log::line(&format!("[{frame}] display switch: WA's display setup failed: {what} {:#x}", if result != 0 { r32(result as usize + 8) } else { 0 }));
        }
        SetWindowPos(wnd, -1isize as HWND, 0, 0, w as i32, h as i32, 0x20 | 0x40);
        let (cx, cy) = (w as i32 / 2, h as i32 / 2);
        if s != 0 {
            for (o, v) in [(0x54, cx), (0x58, cy), (0x80, cx), (0x84, cy)] {
                w32(s + o, v as u32);
            }
        }
        SetCursorPos(cx, cy);
        ClipCursor(std::ptr::null());
    }
    // DisplayGfx's sizes as WA's display setup leaves them (the same resolution)
    for (o, v) in [(0x3548, w), (0x354C, h), (0x3550, 0), (0x3554, 0), (0x3558, w), (0x355C, h), (0x3560, w / 2), (0x3564, h / 2)] {
        w32(display + o, v);
    }
    log::line(&format!("[{frame}] display switch: after: {}", state(wnd)));
    crate::escmenu::refresh(runtime);
}
