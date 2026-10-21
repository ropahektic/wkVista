//! Full-colour maps in WA's map list. WA opens map files with CreateFileA / CreateFileW (its
//! import table is patched): when it opens a full-colour image under `User\SavedLevels` (a PNG
//! with 8-bit RGB or RGBA, or a JPEG, which WA can't load), it gets a handle to the map's proxy
//! instead (pngmap.rs: the same map as a palette PNG WA accepts). The proxy is a temporary file
//! Windows deletes when WA closes it: nothing is kept on disk, the original is never written
//! (WA's editor saving it writes the temporary file). Conversions are kept in memory for the
//! session.

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{VirtualProtect, PAGE_READWRITE};

use crate::{log, pngmap};

static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
static mut O_CFA: usize = 0;
static mut O_CFW: usize = 0;
/// the session's conversions: original -> (its modification time, the proxy's bytes)
static CACHE: Mutex<Option<HashMap<PathBuf, (std::time::SystemTime, Arc<Vec<u8>>)>>> = Mutex::new(None);
/// the originals WA opened this session, the latest last (tried first in game)
static RECENT: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static TEMP_N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// the theme converted maps get (config map_theme)
static THEME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(11);

/// WA's `User\SavedLevels`
pub fn saved_levels() -> Option<PathBuf> {
    Some(DIR.lock().unwrap().as_ref()?.join("User").join("SavedLevels"))
}

/// The full-colour images that could be a land of `w` x `h` (their proxy has that size): the
/// ones WA opened this session first, then the rest of SavedLevels
pub fn candidates(w: u32, h: u32) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = RECENT.lock().unwrap().iter().rev().cloned().collect();
    if let Some(dir) = saved_levels() {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if !v.contains(&p) {
                    v.push(p);
                }
            }
        }
    }
    v.retain(|p| {
        let Ok(b) = std::fs::read(p) else { return false };
        if b.len() < 26 || !pngmap::full_colour(&b[..26]) {
            return false;
        }
        let Some((iw, ih)) = pngmap::size(&b) else { return false };
        [true, false].iter().any(|&air| {
            let (pw, ph, _) = pngmap::layout(iw, ih, air);
            (pw, ph) == (w, h)
        })
    });
    v
}

/// The proxy's bytes for a full-colour image under SavedLevels; None for any other file
fn proxy_for(path: &Path) -> Option<Arc<Vec<u8>>> {
    let lower = path.to_string_lossy().to_lowercase().replace('/', "\\");
    if !lower.contains("\\savedlevels\\") {
        return None;
    }
    let mut head = [0u8; 26];
    use std::io::Read;
    let n = std::fs::File::open(path).ok()?.read(&mut head).ok()?;
    if !pngmap::full_colour(&head[..n]) {
        return None;
    }
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    {
        let mut recent = RECENT.lock().unwrap();
        recent.retain(|p| p != path);
        recent.push(path.to_path_buf());
    }
    if let Some((t, b)) = CACHE.lock().unwrap().get_or_insert_with(HashMap::new).get(path) {
        if *t == modified {
            return Some(b.clone());
        }
    }
    let name = path.file_name()?.to_string_lossy().to_string();
    let t = std::time::Instant::now();
    let made = std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| pngmap::decode(&b)).and_then(|img| pngmap::proxy(&img, THEME.load(std::sync::atomic::Ordering::Relaxed)).map(|p| (img, p)));
    match made {
        Ok((img, p)) => {
            log::line(&format!(
                "map {name}: {}x{} full colour -> WA map {}x{} (original at {:?}, theme {}), {:.0} ms",
                img.w,
                img.h,
                p.w,
                p.h,
                p.at,
                pngmap::THEMES[THEME.load(std::sync::atomic::Ordering::Relaxed) as usize % pngmap::THEMES.len()],
                t.elapsed().as_secs_f64() * 1e3
            ));
            let b = Arc::new(p.png);
            CACHE.lock().unwrap().get_or_insert_with(HashMap::new).insert(path.to_path_buf(), (modified, b.clone()));
            Some(b)
        }
        Err(e) => {
            log::line(&format!("map {name}: not converted: {e}"));
            None
        }
    }
}

/// The proxy as a temporary file deleted when WA closes the handle, at its start
fn temp_handle(bytes: &[u8]) -> Option<*mut c_void> {
    use std::io::{Seek, Write};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::IntoRawHandle;
    let n = TEMP_N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("wkVista-{}-{n}.png", std::process::id()));
    // FILE_SHARE_READ | WRITE | DELETE; FILE_ATTRIBUTE_TEMPORARY; FILE_FLAG_DELETE_ON_CLOSE
    let mut f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).share_mode(7).attributes(0x100).custom_flags(0x0400_0000).open(&p).ok()?;
    f.write_all(bytes).ok()?;
    f.rewind().ok()?;
    Some(f.into_raw_handle())
}

type CreateW = extern "system" fn(*const u16, u32, u32, *mut c_void, u32, u32, *mut c_void) -> *mut c_void;
type CreateA = extern "system" fn(*const u8, u32, u32, *mut c_void, u32, u32, *mut c_void) -> *mut c_void;

/// WA's CreateFile on `path`: a handle to the proxy for a full-colour map (WA writing goes to
/// the temporary file: the original is kept)
fn swap(path: &Path, access: u32) -> Option<*mut c_void> {
    let h = std::panic::catch_unwind(|| proxy_for(path).and_then(|b| temp_handle(&b))).ok().flatten()?;
    if access & 0x4000_0000 != 0 {
        log::line(&format!("map {}: WA writes it: not kept (the original is never written)", path.display()));
    }
    Some(h)
}

extern "system" fn cfw(name: *const u16, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, tmpl: *mut c_void) -> *mut c_void {
    let orig: CreateW = unsafe { std::mem::transmute(O_CFW) };
    if !name.is_null() {
        let n = unsafe { (0..).take_while(|&i| *name.add(i) != 0).count() };
        let path = PathBuf::from(String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, n) }));
        if let Some(h) = swap(&path, access) {
            return h;
        }
    }
    orig(name, access, share, sa, disp, flags, tmpl)
}

extern "system" fn cfa(name: *const u8, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, tmpl: *mut c_void) -> *mut c_void {
    if !name.is_null() {
        let n = unsafe { (0..).take_while(|&i| *name.add(i) != 0).count() };
        // WA's paths are in the ANSI code page; a full-colour map's path goes the wide way
        let path = PathBuf::from(String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(name, n) }).to_string());
        if let Some(h) = swap(&path, access) {
            return h;
        }
    }
    let orig: CreateA = unsafe { std::mem::transmute(O_CFA) };
    orig(name, access, share, sa, disp, flags, tmpl)
}

/// Patches WA.exe's imports of kernel32 CreateFileA / CreateFileW; converted maps get WA theme
/// `theme`
pub fn install(dir: &Path, theme: u32) -> Result<(), String> {
    *DIR.lock().unwrap() = Some(dir.to_path_buf());
    THEME.store(theme, std::sync::atomic::Ordering::Relaxed);
    unsafe {
        let base = GetModuleHandleW(std::ptr::null()) as usize;
        let rd = |a: usize| std::ptr::read_unaligned(a as *const u32) as usize;
        let nt = base + rd(base + 0x3C);
        let imports = base + rd(nt + 0x80);
        // CreateFileW's own address (for a CreateFileA swap when WA imports only the A form)
        let k32 = windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(b"kernel32.dll\0".as_ptr());
        let w_addr = windows_sys::Win32::System::LibraryLoader::GetProcAddress(k32, b"CreateFileW\0".as_ptr()).ok_or("no CreateFileW")? as usize;
        O_CFW = w_addr;
        let mut patched = Vec::new();
        let mut d = imports;
        while rd(d + 12) != 0 {
            let dll = std::ffi::CStr::from_ptr((base + rd(d + 12)) as *const i8).to_string_lossy().to_lowercase();
            if dll == "kernel32.dll" {
                let (names, thunks) = (rd(d), rd(d + 16));
                let names = if names != 0 { names } else { thunks };
                let mut k = 0;
                loop {
                    let n = rd(base + names + 4 * k);
                    if n == 0 {
                        break;
                    }
                    if n & 0x8000_0000 == 0 {
                        let fname = std::ffi::CStr::from_ptr((base + n + 2) as *const i8).to_string_lossy().to_string();
                        let slot = base + thunks + 4 * k;
                        let new = match fname.as_str() {
                            "CreateFileA" => {
                                O_CFA = rd(slot);
                                Some(cfa as *const () as usize)
                            }
                            "CreateFileW" => {
                                O_CFW = rd(slot);
                                Some(cfw as *const () as usize)
                            }
                            _ => None,
                        };
                        if let Some(new) = new {
                            let mut old = 0;
                            VirtualProtect(slot as *mut c_void, 4, PAGE_READWRITE, &mut old);
                            std::ptr::write_unaligned(slot as *mut u32, new as u32);
                            VirtualProtect(slot as *mut c_void, 4, old, &mut old);
                            patched.push(fname);
                        }
                    }
                    k += 1;
                }
            }
            d += 20;
        }
        if patched.is_empty() {
            return Err("WA imports no CreateFile".into());
        }
        log::line(&format!("full-colour maps: {} swapped in WA's imports", patched.join(", ")));
    }
    Ok(())
}
