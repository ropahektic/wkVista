//! A "Resolution" dropdown in WA's own Esc menu (resize.rs applies the choice).
//!
//! GameRuntime::OpenEscMenu 0x535200 (stdcall, the runtime; ret 4) lays out its items one row
//! under another with MenuPanel::AppendItem 0x5408F0 (ESI the panel, runtime +0x30; EAX the x,
//! centred when the 4th argument is 1; stack: the item's id, its label (kept: item +0x34), the
//! row's y, centred, a slider's value and maximum; ret 0x18), then sizes the panel from the last
//! row. Right after its first item (the call returns to 0x5356D3: EBP that row's y, [esp+0x20]
//! a row's height, [esp+0x2C] the buttons' x; WA's next row is height + EBP + 1) wkVista
//! appends its rows and moves EBP to its last one, so WA's own items follow below (Exit last)
//! and WA's sizing and frame take them in:
//! - the header, "Resolution: 1920x1080 (16:9)"; clicked, it opens or closes the list;
//! - opened, the display modes of this monitor's ratio (and the current mode) in two columns,
//!   the current one in brackets; clicked, it is applied and the list closes.
//!
//! The menu is drawn into a bitmap of a fixed size (the panel's +0: a BitmapImage, its width
//! +0x14, height +0x18): rows past its bottom come out as garbage. So the list takes only the
//! rows left after room for WA's items still to come (at most 4 rows) and the frame; when that
//! is too few, the smallest modes are left out (logged).
//!
//! MenuPanel::ActivateAtCursor 0x540810 (stdcall: the panel, where the id goes; ret 8) gives a
//! clicked button's id with 1. On wkVista's ids it acts and gives 0 (nothing clicked: WA only
//! plays its click sound), and the menu is laid out again (OpenEscMenu) so it shows the change.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::log;

/// the header's id (WA's are 0 to 4); a mode's is MODE + its index in the list
const HEADER: u32 = 0x57;
const MODE: u32 = 0x60;
/// rows: the header and at most this many modes (a panel holds 16 items; WA's take up to 5)
const MAX_MODES: usize = 10;

static mut O_AFTER_FIRST: usize = 0;
static mut O_ACTIVATE: usize = 0;
static APPEND: AtomicUsize = AtomicUsize::new(0);
static OPEN: AtomicUsize = AtomicUsize::new(0);
/// the list shown
static OPENED: AtomicBool = AtomicBool::new(false);
/// the modes listed, by row
static LISTED: Mutex<Vec<(u32, u32)>> = Mutex::new(Vec::new());
/// the labels (WA keeps pointers to them): the header, then the modes
static mut LABELS: [[u8; 48]; MAX_MODES + 1] = [[0; 48]; MAX_MODES + 1];
static mut CALL: [u32; 8] = [0; 8];
static CALL_RET: AtomicU32 = AtomicU32::new(0);
static RUNTIME: AtomicUsize = AtomicUsize::new(0);

fn rb(va: usize) -> usize {
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    va.wrapping_add(base.wrapping_sub(0x40_0000))
}

unsafe fn r32(a: usize) -> u32 {
    std::ptr::read_unaligned(a as *const u32)
}

/// A screen ratio's usual name (16:9, 16:10, 4:3, 5:4, 3:2, 21:9, within 1 %), else w:h reduced
pub fn ratio(w: u32, h: u32) -> String {
    let r = w as f64 / h as f64;
    for (n, v) in [("16:9", 16.0 / 9.0), ("16:10", 1.6), ("4:3", 4.0 / 3.0), ("5:4", 1.25), ("3:2", 1.5), ("21:9", 64.0 / 27.0)] {
        if (r - v).abs() / v < 0.01 {
            return n.to_string();
        }
    }
    let g = (1..=w.min(h)).rev().find(|d| w % d == 0 && h % d == 0).unwrap_or(1);
    format!("{}:{}", w / g, h / g)
}

/// The modes offered: this monitor's ratio (within 1 %), and the current one; at most
/// MAX_MODES, the largest kept
pub fn offered(all: &[(u32, u32)], monitor: (u32, u32), cur: (u32, u32)) -> Vec<(u32, u32)> {
    let want = monitor.0 as f64 / monitor.1 as f64;
    let mut v: Vec<(u32, u32)> = all.iter().copied().filter(|&(w, h)| ((w as f64 / h as f64) - want).abs() / want < 0.01 || (w, h) == cur).collect();
    if !v.contains(&cur) && cur.0 > 0 {
        v.push(cur);
    }
    v.sort_by_key(|&(w, h)| (w * h, w));
    v.dedup();
    if v.len() > MAX_MODES {
        v.drain(..v.len() - MAX_MODES);
    }
    v
}

unsafe fn set_label(k: usize, s: &str) {
    let label = &mut (*(&raw mut LABELS))[k];
    let b = s.as_bytes();
    let n = b.len().min(47);
    label[..n].copy_from_slice(&b[..n]);
    label[n] = 0;
}

/// Regs as a pushad saved them
#[repr(C)]
struct Regs {
    edi: u32,
    esi: u32,
    ebp: u32,
    esp: u32,
    ebx: u32,
    edx: u32,
    ecx: u32,
    eax: u32,
}

/// MenuPanel::AppendItem with CALL: ESI, EAX, then its six arguments
#[unsafe(naked)]
unsafe extern "C" fn call_append() {
    core::arch::naked_asm!(
        "pushad",
        "push dword ptr [{c}+28]",
        "push dword ptr [{c}+24]",
        "push dword ptr [{c}+20]",
        "push dword ptr [{c}+16]",
        "push dword ptr [{c}+12]",
        "push dword ptr [{c}+8]",
        "mov esi, dword ptr [{c}]",
        "mov eax, dword ptr [{c}+4]",
        "call dword ptr [{f}]",
        "mov dword ptr [{r}], eax",
        "popad",
        "ret",
        c = sym CALL,
        f = sym APPEND,
        r = sym CALL_RET,
    );
}

/// At 0x5356D3 (WA's first item appended): wkVista's rows
#[unsafe(naked)]
unsafe extern "C" fn t_after_first() {
    core::arch::naked_asm!(
        "pushad",
        "push esp",
        "call {h}",
        "add esp, 4",
        "popad",
        "jmp dword ptr [{o}]",
        h = sym h_after_first,
        o = sym O_AFTER_FIRST,
    );
}

/// One row: its id, its label (LABELS[k]), its y; true when WA took it
unsafe fn append(panel: u32, x: u32, id: u32, k: usize, y: u32) -> bool {
    CALL = [panel, x, id, (&raw const LABELS[k]) as u32, y, 1, 0, 0];
    let call: unsafe extern "C" fn() = call_append;
    call();
    CALL_RET.load(Ordering::Relaxed) != 0
}

extern "C" fn h_after_first(r: &mut Regs) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let runtime = r.ebx as usize;
        RUNTIME.store(runtime, Ordering::Relaxed);
        let panel = r32(runtime + 0x30);
        let esp = r.esp as usize;
        let (row, x) = (r32(esp + 0x20), r32(esp + 0x2C));
        let cur = (r32(rb(0x7A0EF0)), r32(rb(0x7A0EF4)));
        let opened = OPENED.load(Ordering::Relaxed);
        set_label(0, &format!("Resolution: {}x{} ({}){}", cur.0, cur.1, ratio(cur.0, cur.1), if opened { " -" } else { " +" }));
        let mut y = row.wrapping_add(r.ebp).wrapping_add(1);
        if !append(panel, x, HEADER, 0, y) {
            return;
        }
        let mut last = y;
        if opened {
            let monitor = {
                use windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics;
                (GetSystemMetrics(0) as u32, GetSystemMetrics(1) as u32)
            };
            let mut modes = offered(&crate::resize::modes(), monitor, cur);
            // the rows the menu's bitmap has left: after WA's items still to come (4 rows at
            // most) and the frame
            let bitmap = r32(panel as usize) as usize;
            let (bw, bh) = if bitmap != 0 { (r32(bitmap + 0x14), r32(bitmap + 0x18)) } else { (0, 0) };
            let step = row + 1;
            let room = bh.saturating_sub(y + step + 4 * step + 8) / step.max(1);
            let fit = (room as usize * 2).min(MAX_MODES);
            if modes.len() > fit {
                log::line(&format!("Esc menu: room for {fit} of {} modes (menu bitmap {bw}x{bh}, row {step}): the smallest left out", modes.len()));
                let cut = modes.len() - fit;
                modes.drain(..cut);
            }
            // two columns, at the quarter points of the panel
            let (left, right) = (x.wrapping_sub(bw / 4), x.wrapping_add(bw / 4));
            for (k, &(w, h)) in modes.iter().enumerate() {
                set_label(k + 1, &if (w, h) == cur { format!("[{w}x{h}]") } else { format!("{w}x{h}") });
                if k % 2 == 0 {
                    y = y.wrapping_add(step);
                }
                let col = if modes.len() == 1 { x } else if k % 2 == 0 { left } else { right };
                if !append(panel, col, MODE + k as u32, k + 1, y) {
                    log::line(&format!("Esc menu: the panel is full at {w}x{h}"));
                    break;
                }
                last = y;
            }
            *LISTED.lock().unwrap() = modes;
        }
        r.ebp = last;
    }));
}

/// ActivateAtCursor: wkVista's rows clicked
extern "system" fn h_activate(panel: usize, out: *mut u32) -> i32 {
    let orig: extern "system" fn(usize, *mut u32) -> i32 = unsafe { std::mem::transmute(O_ACTIVATE) };
    let r = orig(panel, out);
    if r != 1 || out.is_null() {
        return r;
    }
    let id = unsafe { *out };
    if id == HEADER {
        OPENED.fetch_xor(true, Ordering::Relaxed);
        unsafe { refresh(RUNTIME.load(Ordering::Relaxed)) };
        return 0;
    }
    if (MODE..MODE + MAX_MODES as u32).contains(&id) {
        if let Some(&(w, h)) = LISTED.lock().unwrap().get((id - MODE) as usize) {
            crate::resize::ask(w, h);
        }
        OPENED.store(false, Ordering::Relaxed);
        unsafe { refresh(RUNTIME.load(Ordering::Relaxed)) };
        return 0;
    }
    r
}

/// The Esc menu laid out again (it shows the list's state and the current resolution), when it
/// is open
pub unsafe fn refresh(runtime: usize) {
    if runtime == 0 || r32(runtime + 0x434) != 1 {
        return;
    }
    let open: extern "system" fn(usize) = std::mem::transmute(OPEN.load(Ordering::Relaxed));
    open(runtime);
}

pub fn install() -> Result<(), String> {
    unsafe {
        APPEND.store(rb(0x5408F0), Ordering::Relaxed);
        OPEN.store(rb(0x535200), Ordering::Relaxed);
        // the first item's AppendItem call, as in WA 3.8.1
        let at = rb(0x5356C8);
        let code = std::slice::from_raw_parts(at as *const u8, 11);
        if code != [0x6A, 0x03, 0x89, 0x44, 0x24, 0x44, 0xE8, 0x1D, 0xB2, 0x00, 0x00] {
            return Err(format!("WA's Esc menu is not as in WA 3.8.1 ({code:02X?})"));
        }
        let hooks: [(usize, *mut std::ffi::c_void, *mut usize); 2] = [
            (rb(0x5356D3), t_after_first as *mut std::ffi::c_void, &raw mut O_AFTER_FIRST),
            (rb(0x540810), h_activate as *mut std::ffi::c_void, &raw mut O_ACTIVATE),
        ];
        for &(target, det, orig) in &hooks {
            let tramp = minhook::MinHook::create_hook(target as *mut std::ffi::c_void, det).map_err(|e| format!("hook {target:#x}: {e:?}"))?;
            *orig = tramp as usize;
        }
        for &(target, _, _) in &hooks {
            minhook::MinHook::enable_hook(target as *mut std::ffi::c_void).map_err(|e| format!("enable {target:#x}: {e:?}"))?;
        }
    }
    log::line("Esc menu: a Resolution dropdown after the first item");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{offered, ratio};

    #[test]
    fn ratio_names() {
        for (w, h, r) in [(1920, 1080, "16:9"), (1366, 768, "16:9"), (1920, 1200, "16:10"), (1680, 1050, "16:10"), (1024, 768, "4:3"), (1152, 864, "4:3"), (1280, 1024, "5:4"), (720, 480, "3:2"), (2560, 1080, "21:9"), (1176, 664, "16:9"), (800, 700, "8:7")] {
            assert_eq!(ratio(w, h), r, "{w}x{h}");
        }
    }

    #[test]
    fn modes_of_the_monitors_ratio() {
        // the list this PC reported, a 3840 x 2160 desktop, playing at 1152 x 864 (kept, listed
        // in size order with the monitor's 16:9 modes)
        let all = [(640, 480), (720, 480), (720, 576), (800, 600), (1176, 664), (1024, 768), (1280, 720), (1280, 768), (1152, 864), (1280, 800), (1360, 768), (1366, 768), (1280, 960), (1280, 1024), (1600, 900), (1440, 1080), (1600, 1024), (1680, 1050), (1600, 1200), (1920, 1080), (1920, 1200), (1920, 1440), (2048, 1536)];
        let v = offered(&all, (3840, 2160), (1152, 864));
        assert_eq!(v, [(1176, 664), (1280, 720), (1152, 864), (1360, 768), (1366, 768), (1600, 900), (1920, 1080)]);
    }
}
