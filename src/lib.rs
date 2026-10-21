//! wkVista: an in-game renderer for stock Worms Armageddon 3.8.1, a WormKit module
//! (`wkVista.dll` in the WA folder). WA's frontend is untouched; in game, the mouse wheel (or
//! numpad + / -) zooms: wkVista draws WA's world operations through its own view, WA keeps
//! drawing the HUD. At 1x WA draws everything itself, exactly as without the module.
//!
//! Its rules come from wa-play (WA running its own transliterated code), where each drawing
//! operation and the zoom were verified pixel for pixel against WA: docs there,
//! docs/RENDERER.md. Logs for test runs: log.rs.

// WormKit loads modules named wk*.dll: the crate is named for its file
#![allow(non_snake_case)]

pub mod config;
pub mod pngmap;
pub mod raster;

#[cfg(all(windows, target_arch = "x86"))]
mod hooks;
#[cfg(all(windows, target_arch = "x86"))]
mod log;
#[cfg(all(windows, target_arch = "x86"))]
mod present;
#[cfg(all(windows, target_arch = "x86"))]
mod files;
#[cfg(all(windows, target_arch = "x86"))]
mod land;
#[cfg(all(windows, target_arch = "x86"))]
mod smooth;

#[cfg(all(windows, target_arch = "x86"))]
mod dll {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::HINSTANCE;
    use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
    use windows_sys::Win32::System::SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH};

    static mut MODULE: usize = 0;

    #[no_mangle]
    pub unsafe extern "system" fn DllMain(hinst: HINSTANCE, reason: u32, _r: *mut c_void) -> i32 {
        if reason == DLL_PROCESS_ATTACH {
            MODULE = hinst as usize;
            let m = MODULE;
            std::thread::spawn(move || init(m));
        } else if reason == DLL_PROCESS_DETACH {
            crate::log::flush();
        }
        1
    }

    fn module_dir(module: usize) -> std::path::PathBuf {
        let mut buf = [0u16; 1024];
        let n = unsafe { GetModuleFileNameW(module as _, buf.as_mut_ptr(), buf.len() as u32) } as usize;
        let p = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n]));
        p.parent().map(|d| d.to_path_buf()).unwrap_or_default()
    }

    fn init(module: usize) {
        let dir = module_dir(module);
        crate::log::open(&dir);
        crate::log::line(&format!("wkVista {} loaded from {}", env!("CARGO_PKG_VERSION"), dir.display()));
        let (cfg, bad) = crate::config::load(&dir);
        for b in bad {
            crate::log::line(&format!("wkVista.ini: ignored line: {b}"));
        }
        crate::log::line(&format!("settings: {cfg:?}"));
        if cfg.truecolor {
            if let Err(e) = crate::files::install(&dir, cfg.map_theme) {
                crate::log::line(&format!("full-colour maps unavailable: {e}"));
            }
            match crate::present::install() {
                Ok(()) => crate::log::line("true colour: WA's Direct3D 9 backends hooked (used when WA's renderer is a Direct3D 9 option)"),
                Err(e) => crate::log::line(&format!("true colour unavailable: {e}")),
            }
        }
        match crate::hooks::install(cfg, dir) {
            Ok(()) => crate::log::line("hooks on: GameRender, RenderDrawingQueue, the 7 drawing functions, the camera clamp, the window procedure"),
            Err(e) => crate::log::line(&format!("NOT ACTIVE: {e}")),
        }
    }
}
