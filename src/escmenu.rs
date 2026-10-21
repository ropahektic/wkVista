//! A "Resolution" item in WA's own Esc menu (resize.rs applies the choice).
//!
//! GameRuntime::OpenEscMenu 0x535200 (stdcall, the runtime; ret 4) lays out its items one row
//! under another with MenuPanel::AppendItem 0x5408F0 (ESI the panel, runtime +0x30; EAX the x,
//! centred when the 4th argument is 1; stack: the item's id, its label (kept: item +0x34), the
//! row's y, centred, a slider's value and maximum; ret 0x18). After the last item, the volume
//! slider (its call returns to 0x535805: EBP its row's y, [esp+0x20] a row's height,
//! [esp+0x2C] the buttons' x), WA sizes the panel from EBP; wkVista appends its item one row
//! lower there and moves EBP to it, so WA's own sizing and frame take it in.
//!
//! MenuPanel::ActivateAtCursor 0x540810 (stdcall: the panel, where the id goes; ret 8) gives a
//! clicked button's id with 1. On wkVista's id it switches to the next resolution (Shift: the
//! previous one) and gives 0 (nothing clicked: WA only plays its click sound); once the new
//! resolution is applied, the menu is laid out again (OpenEscMenu) so its label is current.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use crate::log;

/// wkVista's item id (WA's are 0 to 4)
const ID: u32 = 0x57;

static mut O_AFTER_VOLUME: usize = 0;
static mut O_ACTIVATE: usize = 0;
static APPEND: AtomicUsize = AtomicUsize::new(0);
static OPEN: AtomicUsize = AtomicUsize::new(0);
/// the label (WA keeps a pointer to it)
static mut LABEL: [u8; 64] = [0; 64];
static mut CALL: [u32; 8] = [0; 8];
static CALL_RET: AtomicU32 = AtomicU32::new(0);

fn rb(va: usize) -> usize {
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    va.wrapping_add(base.wrapping_sub(0x40_0000))
}

unsafe fn r32(a: usize) -> u32 {
    std::ptr::read_unaligned(a as *const u32)
}

/// A screen ratio's usual name (16:9, 16:10, 4:3, 5:4, 3:2, 21:9), else w:h reduced
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

/// The label for the current resolution
unsafe fn set_label() {
    let (w, h) = (r32(rb(0x7A0EF0)), r32(rb(0x7A0EF4)));
    let s = format!("Resolution: {w}x{h} ({})", ratio(w, h));
    let b = s.as_bytes();
    let n = b.len().min(63);
    let label = &mut *(&raw mut LABEL);
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

/// At 0x535805 (the volume slider appended): wkVista's item one row lower
#[unsafe(naked)]
unsafe extern "C" fn t_after_volume() {
    core::arch::naked_asm!(
        "pushad",
        "push esp",
        "call {h}",
        "add esp, 4",
        "popad",
        "jmp dword ptr [{o}]",
        h = sym h_after_volume,
        o = sym O_AFTER_VOLUME,
    );
}

extern "C" fn h_after_volume(r: &mut Regs) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let runtime = r.ebx as usize;
        let panel = r32(runtime + 0x30);
        let esp = r.esp as usize;
        let (row, x) = (r32(esp + 0x20), r32(esp + 0x2C));
        let y = row.wrapping_add(r.ebp).wrapping_add(1);
        set_label();
        CALL = [panel, x, ID, (&raw const LABEL) as u32, y, 1, 0, 0];
        let call: unsafe extern "C" fn() = call_append;
        call();
        if CALL_RET.load(Ordering::Relaxed) != 0 {
            r.ebp = y;
        }
    }));
}

/// ActivateAtCursor: wkVista's item clicked -> the next resolution (Shift: the previous one)
extern "system" fn h_activate(panel: usize, out: *mut u32) -> i32 {
    let orig: extern "system" fn(usize, *mut u32) -> i32 = unsafe { std::mem::transmute(O_ACTIVATE) };
    let r = orig(panel, out);
    if r == 1 && !out.is_null() && unsafe { *out } == ID {
        let back = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x10) } < 0;
        crate::resize::ask_step(if back { -1 } else { 1 });
        return 0;
    }
    r
}

/// The Esc menu laid out again (its label current), when it is open
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
        // the volume slider's AppendItem call, as in WA 3.8.1
        let at = rb(0x5357F9);
        let code = std::slice::from_raw_parts(at as *const u8, 12);
        if code != [0x6A, 0x04, 0xB8, 0x06, 0x00, 0x00, 0x00, 0xE8, 0xEB, 0xB0, 0x00, 0x00] {
            return Err(format!("WA's Esc menu is not as in WA 3.8.1 ({code:02X?})"));
        }
        let hooks: [(usize, *mut std::ffi::c_void, *mut usize); 2] = [
            (rb(0x535805), t_after_volume as *mut std::ffi::c_void, &raw mut O_AFTER_VOLUME),
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
    log::line("Esc menu: a Resolution item (click: next, Shift+click: previous)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ratio;

    #[test]
    fn ratio_names() {
        for (w, h, r) in [(1920, 1080, "16:9"), (1366, 768, "16:9"), (1920, 1200, "16:10"), (1680, 1050, "16:10"), (1024, 768, "4:3"), (1152, 864, "4:3"), (1280, 1024, "5:4"), (720, 480, "3:2"), (2560, 1080, "21:9"), (1176, 664, "16:9"), (800, 700, "8:7")] {
            assert_eq!(ratio(w, h), r, "{w}x{h}");
        }
    }
}
