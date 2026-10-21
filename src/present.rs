//! True colour output, native: wkVista's own RGBA frame shown by WA's Direct3D 9 backend in
//! place of WA's palette pass (WA's renderer set to one of its Direct3D 9 options).
//!
//! WA ends a frame with RenderContext::FlushRender (vtable 0x662EC8 slot 13, 0x5A26C0), which
//! calls its backend's "show frame" (slot 12; ECX the backend, EDX the result, the 8-bit
//! framebuffer; ret 4) and then presents. In the Direct3D 9 backends (vtables 0x6767B0 software
//! palette, 0x6767F8 shader palette; the device at +0x40) slot 12 (0x59CB30 / 0x59CDE0) checks
//! the device (TestCooperativeLevel, a reset when lost), uploads the 8-bit frame and the
//! palette, and draws them as one quad through the palette lookup. In game, with a frame of
//! wkVista's and the device fine, that step is wkVista's: its frame goes into the back buffer
//! (StretchRect, point sampling) and the result is WA's success code (0x8AC8B4); WA's upload and
//! palette pass don't run. Otherwise (the frontend, a lost device, true colour off) WA's own
//! step runs. WA keeps its window, fullscreen, resolution and resets.
//!
//! Device methods by vtable slot: 3 TestCooperativeLevel, 16 Reset, 18 GetBackBuffer,
//! 34 StretchRect, 36 CreateOffscreenPlainSurface; a surface: 12 GetDesc, 13 LockRect,
//! 14 UnlockRect; any COM object: 2 Release. Reset is hooked (found on a throwaway device: every
//! Direct3D 9 device shares the functions) to free wkVista's surface first.

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

static mut O_RESET: usize = 0;
static mut O_SHOW_SW: usize = 0;
static mut O_SHOW_SH: usize = 0;


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

/// wkVista's Direct3D surface freed (before WA resets its device)
pub fn release_surface() {
    unsafe { release(SURFACE.swap(0, Ordering::Relaxed)) };
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
        let reset = vt(dev, 16);
        release(dev);
        release(d3d);
        DestroyWindow(hwnd);
        // WA's Direct3D 9 backends' "show frame" (slot 12 of 0x6767B0 / 0x6767F8)
        let base = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as usize;
        let rb = |va: usize| va.wrapping_add(base.wrapping_sub(0x40_0000));
        let (sw, sh) = (*(rb(0x6767B0) as *const usize).add(12), *(rb(0x6767F8) as *const usize).add(12));
        if (sw, sh) != (rb(0x59CB30), rb(0x59CDE0)) {
            return Err(format!("WA's Direct3D 9 backends are not as in WA 3.8.1 ({sw:#x}, {sh:#x})"));
        }
        let hooks = [(reset, h_reset as *mut c_void, &raw mut O_RESET), (sw, t_show_sw as *mut c_void, &raw mut O_SHOW_SW), (sh, t_show_sh as *mut c_void, &raw mut O_SHOW_SH)];
        for &(target, det, orig) in &hooks {
            let tramp = minhook::MinHook::create_hook(target as *mut c_void, det).map_err(|e| format!("hook {target:#x}: {e:?}"))?;
            *orig = tramp as usize;
        }
        for &(target, _, _) in &hooks {
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
        log::line(&format!("true colour: WA's palette pass replaced, device {dev:#x}, back buffer {}x{} format {}, frame {}x{}", desc[6], desc[7], desc[0], f.w, f.h));
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
static NO_DEVICE_LOGGED: AtomicBool = AtomicBool::new(false);

/// The registers a "show frame" thunk saved (pushad), then the return address and the argument
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

/// A backend's "show frame" thunk: `h_show`; 1: wkVista showed its frame (`ret 4`, EAX the
/// result), 0: WA's own runs
macro_rules! show_thunk {
    ($name:ident, $orig:ident) => {
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
                "ret 4",
                h = sym h_show,
                o = sym $orig,
            );
        }
    };
}
show_thunk!(t_show_sw, O_SHOW_SW);
show_thunk!(t_show_sh, O_SHOW_SH);

/// WA's "show frame", in game with a frame of wkVista's: its frame into the back buffer, WA's
/// success code as the result
extern "C" fn h_show(r: &mut Regs) -> u32 {
    if !SHOW.load(Ordering::Relaxed) || !crate::hooks::in_game() || FAILED.load(Ordering::Relaxed) {
        return 0;
    }
    let shown = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        // WA's device as WA uses it (it may be wrapped by another layer: whatever WA calls is
        // called the same way)
        let dev = *((r.ecx as usize + 0x40) as *const usize);
        if dev == 0 || *(dev as *const usize) == 0 {
            // WA released its display (focus lost in fullscreen, a mode change): its own step
            // would use the missing device, so this frame is shown by nobody, as success, until
            // WA has a device again
            if !NO_DEVICE_LOGGED.swap(true, Ordering::Relaxed) {
                log::line(&format!("true colour: backend {:#x} has no device: frames skipped until WA sets it up again", r.ecx));
            }
            let base = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as usize;
            *(r.edx as *mut u32) = *((0x8AC8B4usize).wrapping_add(base.wrapping_sub(0x40_0000)) as *const u32);
            r.eax = r.edx;
            return Ok::<bool, String>(true);
        }
        NO_DEVICE_LOGGED.store(false, Ordering::Relaxed);
        // a lost device: our surface freed (a reset needs every default-pool resource gone),
        // then WA's own step (it resets the device)
        let tcl: extern "system" fn(usize) -> Hr = std::mem::transmute(vt(dev, 3));
        if tcl(dev) != 0 {
            release(SURFACE.swap(0, Ordering::Relaxed));
            return Ok(false);
        }
        draw(dev)?;
        let base = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as usize;
        let ok = *((0x8AC8B4usize).wrapping_add(base.wrapping_sub(0x40_0000)) as *const u32);
        *(r.edx as *mut u32) = ok;
        r.eax = r.edx;
        Ok(true)
    }))
    .unwrap_or_else(|_| Err("panic".into()));
    match shown {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(e) => {
            // off for this run: WA's own picture
            FAILED.store(true, Ordering::Relaxed);
            log::line(&format!("true colour OFF: {e}"));
            0
        }
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
