//! True colour output: wkVista's own RGBA frame shown through WA's Direct3D 9 device (WA's
//! renderer set to one of its Direct3D 9 options). In game, at WA's `Present`, the frame is
//! copied into WA's back buffer (StretchRect, point sampling), so WA's own picture of that frame
//! is replaced inside WA's swap chain: no window of our own, no other API. WA keeps its window,
//! fullscreen, resolution and device resets; the frontend is untouched.
//!
//! The device's functions are found on a throwaway device (every Direct3D 9 device shares
//! them), so it works however and whenever WA creates its own. Device methods by vtable slot:
//! 16 Reset, 17 Present, 18 GetBackBuffer, 34 StretchRect, 36 CreateOffscreenPlainSurface;
//! a surface: 13 LockRect, 14 UnlockRect; any COM object: 2 Release.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExA, DestroyWindow};

use crate::log;

/// An RGBA frame in Direct3D's X8R8G8B8 (0x00RRGGBB), `w` x `h`
pub struct Rgba {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u32>,
}

/// The last in-game frame wkVista made, shown at every Present while in game
static FRAME: Mutex<Option<Rgba>> = Mutex::new(None);
/// wkVista draws at Present (set each in-game frame, cleared when the game is left)
pub static SHOW: AtomicBool = AtomicBool::new(false);
/// our surface and the device it belongs to
static SURFACE: AtomicUsize = AtomicUsize::new(0);
static SURFACE_DEV: AtomicUsize = AtomicUsize::new(0);
static SURFACE_SIZE: Mutex<(u32, u32)> = Mutex::new((0, 0));
static LOGGED: AtomicBool = AtomicBool::new(false);

static mut O_PRESENT: usize = 0;
static mut O_RESET: usize = 0;

type Hr = i32;

unsafe fn vt(obj: usize, slot: usize) -> usize {
    *(*(obj as *const *const usize)).add(slot)
}

unsafe fn release(obj: usize) {
    if obj != 0 {
        let f: extern "system" fn(usize) -> u32 = std::mem::transmute(vt(obj, 2));
        f(obj);
    }
}

/// The frame to show, `w` x `h`, filled by `fill` (the buffer is kept from frame to frame)
pub fn fill(w: u32, h: u32, f: impl FnOnce(&mut [u32])) {
    let mut g = FRAME.lock().unwrap();
    let fr = g.get_or_insert_with(|| Rgba { w, h, px: Vec::new() });
    fr.w = w;
    fr.h = h;
    fr.px.resize((w * h) as usize, 0);
    f(&mut fr.px);
}

/// Hooks Present and Reset of every Direct3D 9 device (found on a throwaway one)
pub fn install() -> Result<(), String> {
    unsafe {
        let lib = LoadLibraryA(b"d3d9.dll\0".as_ptr());
        if lib.is_null() {
            return Err("no d3d9.dll".into());
        }
        let create = GetProcAddress(lib, b"Direct3DCreate9\0".as_ptr()).ok_or("no Direct3DCreate9")?;
        let create: extern "system" fn(u32) -> usize = std::mem::transmute(create);
        let d3d = create(32);
        if d3d == 0 {
            return Err("Direct3DCreate9 failed".into());
        }
        let hwnd = CreateWindowExA(0, b"STATIC\0".as_ptr(), b"wkVista\0".as_ptr(), 0, 0, 0, 8, 8, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
        // D3DPRESENT_PARAMETERS: windowed, discard, the window
        let mut pp = [0u32; 14];
        pp[6] = 1; // SwapEffect DISCARD
        pp[7] = hwnd as u32; // hDeviceWindow
        pp[8] = 1; // Windowed
        let mut dev: usize = 0;
        // IDirect3D9::CreateDevice (slot 16): adapter 0, HAL, software vertex processing
        let cd: extern "system" fn(usize, u32, u32, *mut c_void, u32, *mut u32, *mut usize) -> Hr = std::mem::transmute(vt(d3d, 16));
        let mut hr = cd(d3d, 0, 1, hwnd, 0x20, pp.as_mut_ptr(), &mut dev);
        if hr < 0 {
            // no hardware device here: the reference device has the same functions
            hr = cd(d3d, 0, 4, hwnd, 0x20, pp.as_mut_ptr(), &mut dev);
        }
        if hr < 0 || dev == 0 {
            release(d3d);
            DestroyWindow(hwnd);
            return Err(format!("throwaway device: {hr:#x}"));
        }
        let (present, reset) = (vt(dev, 17), vt(dev, 16));
        release(dev);
        release(d3d);
        DestroyWindow(hwnd);
        for (target, det, orig) in [(present, h_present as *mut c_void, &raw mut O_PRESENT), (reset, h_reset as *mut c_void, &raw mut O_RESET)] {
            let tramp = minhook::MinHook::create_hook(target as *mut c_void, det).map_err(|e| format!("hook {target:#x}: {e:?}"))?;
            *orig = tramp as usize;
        }
        for target in [present, reset] {
            minhook::MinHook::enable_hook(target as *mut c_void).map_err(|e| format!("enable {target:#x}: {e:?}"))?;
        }
    }
    Ok(())
}

/// Our frame into WA's back buffer (before WA presents it)
unsafe fn draw(dev: usize) -> Result<(), String> {
    let g = FRAME.lock().unwrap();
    let Some(f) = g.as_ref() else { return Ok(()) };
    // our surface: X8R8G8B8 in the default pool, made again for a new device or size
    let mut s = SURFACE.load(Ordering::Relaxed);
    if s == 0 || SURFACE_DEV.load(Ordering::Relaxed) != dev || *SURFACE_SIZE.lock().unwrap() != (f.w, f.h) {
        release(s);
        s = 0;
        let mk: extern "system" fn(usize, u32, u32, u32, u32, *mut usize, *mut c_void) -> Hr = std::mem::transmute(vt(dev, 36));
        let hr = mk(dev, f.w, f.h, 22, 0, &mut s, std::ptr::null_mut());
        if hr < 0 {
            SURFACE.store(0, Ordering::Relaxed);
            return Err(format!("CreateOffscreenPlainSurface {}x{}: {hr:#x}", f.w, f.h));
        }
        SURFACE.store(s, Ordering::Relaxed);
        SURFACE_DEV.store(dev, Ordering::Relaxed);
        *SURFACE_SIZE.lock().unwrap() = (f.w, f.h);
    }
    // the pixels
    let mut lr = [0usize; 2]; // D3DLOCKED_RECT: pitch, bits
    let lock: extern "system" fn(usize, *mut usize, *const c_void, u32) -> Hr = std::mem::transmute(vt(s, 13));
    let hr = lock(s, lr.as_mut_ptr(), std::ptr::null(), 0);
    if hr < 0 {
        return Err(format!("LockRect: {hr:#x}"));
    }
    let (pitch, bits) = (lr[0], lr[1] as *mut u8);
    for y in 0..f.h as usize {
        let row = &f.px[y * f.w as usize..(y + 1) * f.w as usize];
        std::ptr::copy_nonoverlapping(row.as_ptr() as *const u8, bits.add(y * pitch), row.len() * 4);
    }
    let unlock: extern "system" fn(usize) -> Hr = std::mem::transmute(vt(s, 14));
    unlock(s);
    // onto the whole back buffer
    let mut bb: usize = 0;
    let gbb: extern "system" fn(usize, u32, u32, u32, *mut usize) -> Hr = std::mem::transmute(vt(dev, 18));
    let hr = gbb(dev, 0, 0, 0, &mut bb);
    if hr < 0 || bb == 0 {
        return Err(format!("GetBackBuffer: {hr:#x}"));
    }
    if !LOGGED.swap(true, Ordering::Relaxed) {
        // D3DSURFACE_DESC: format, type, usage, pool, multisample, quality, width, height
        let mut desc = [0u32; 8];
        let gd: extern "system" fn(usize, *mut u32) -> Hr = std::mem::transmute(vt(bb, 12));
        gd(bb, desc.as_mut_ptr());
        log::line(&format!("true colour: drawing at Present, device {dev:#x}, back buffer {}x{} format {}, frame {}x{}", desc[6], desc[7], desc[0], f.w, f.h));
    }
    let sr: extern "system" fn(usize, usize, *const c_void, usize, *const c_void, u32) -> Hr = std::mem::transmute(vt(dev, 34));
    let hr = sr(dev, s, std::ptr::null(), bb, std::ptr::null(), 1);
    release(bb);
    if hr < 0 {
        return Err(format!("StretchRect: {hr:#x}"));
    }
    Ok(())
}

static FAILED: AtomicBool = AtomicBool::new(false);

extern "system" fn h_present(dev: usize, src: *const c_void, dst: *const c_void, wnd: *mut c_void, dirty: *const c_void) -> Hr {
    if SHOW.load(Ordering::Relaxed) && crate::hooks::in_game() && !FAILED.load(Ordering::Relaxed) {
        if let Err(e) = std::panic::catch_unwind(|| unsafe { draw(dev) }).unwrap_or_else(|_| Err("panic".into())) {
            // off for this run: WA's own picture is shown
            FAILED.store(true, Ordering::Relaxed);
            log::line(&format!("true colour OFF: {e}"));
        }
    }
    unsafe {
        let orig: extern "system" fn(usize, *const c_void, *const c_void, *mut c_void, *const c_void) -> Hr = std::mem::transmute(O_PRESENT);
        orig(dev, src, dst, wnd, dirty)
    }
}

extern "system" fn h_reset(dev: usize, pp: *mut c_void) -> Hr {
    // default-pool surfaces must go before a reset
    let s = SURFACE.swap(0, Ordering::Relaxed);
    unsafe { release(s) };
    unsafe {
        let orig: extern "system" fn(usize, *mut c_void) -> Hr = std::mem::transmute(O_RESET);
        orig(dev, pp)
    }
}
