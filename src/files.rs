//! Full-colour PNG maps in WA's map list. WA opens map files with CreateFileA / CreateFileW
//! (its import table is patched): when it opens a full-colour PNG under `User\SavedLevels`
//! (8-bit RGB or RGBA, which WA can't load), it gets a handle to the map's proxy instead
//! (pngmap.rs: the same map as a palette PNG WA accepts). Opened for writing (WA's editor saving
//! it), the proxy is written, never the original.
//!
//! The proxies are kept in `wkVista\maps\` beside the DLL with `index.txt` (one line a map:
//! proxy width, height, the original's place in it, the original's path), which the game uses
//! to find the original's colours (land.rs).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{VirtualProtect, PAGE_READWRITE};

use crate::{log, pngmap};

static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
static mut O_CFA: usize = 0;
static mut O_CFW: usize = 0;

/// A map this module made a proxy for (index.txt)
#[derive(Clone, Debug)]
pub struct Entry {
    pub w: u32,
    pub h: u32,
    pub at: (u32, u32),
    pub source: PathBuf,
}

fn maps_dir() -> Option<PathBuf> {
    DIR.lock().unwrap().as_ref().map(|d| d.join("wkVista").join("maps"))
}

/// The maps with proxies, the most recently made first
pub fn entries() -> Vec<Entry> {
    let Some(d) = maps_dir() else { return Vec::new() };
    let Ok(t) = std::fs::read_to_string(d.join("index.txt")) else { return Vec::new() };
    let mut v: Vec<Entry> = t
        .lines()
        .filter_map(|l| {
            let mut f = l.splitn(5, '\t');
            let n = |s: Option<&str>| s?.parse::<u32>().ok();
            Some(Entry { w: n(f.next())?, h: n(f.next())?, at: (n(f.next())?, n(f.next())?), source: PathBuf::from(f.next()?) })
        })
        .collect();
    v.reverse();
    v
}

fn add_entry(e: &Entry) {
    let Some(d) = maps_dir() else { return };
    let mut all = entries();
    all.retain(|x| x.source != e.source);
    all.reverse();
    all.push(e.clone());
    let text: String = all.iter().map(|x| format!("{}\t{}\t{}\t{}\t{}\n", x.w, x.h, x.at.0, x.at.1, x.source.display())).collect();
    let _ = std::fs::write(d.join("index.txt"), text);
}

/// The proxy for a full-colour map under SavedLevels (made when missing or older than the
/// original); None for any other file
fn proxy_for(path: &Path) -> Option<PathBuf> {
    let lower = path.to_string_lossy().to_lowercase().replace('/', "\\");
    if !lower.ends_with(".png") || !lower.contains("\\savedlevels\\") {
        return None;
    }
    let mut head = [0u8; 26];
    use std::io::Read;
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    pngmap::full_colour(&head)?;
    let dir = maps_dir()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    let out = dir.join(&name);
    let fresh = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    if out.exists() && fresh(&out) >= fresh(path) && entries().iter().any(|e| e.source == path) {
        return Some(out);
    }
    let t = std::time::Instant::now();
    let made = std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| pngmap::decode(&b)).and_then(|img| pngmap::proxy(&img).map(|p| (img, p)));
    match made {
        Ok((img, p)) => {
            let _ = std::fs::create_dir_all(&dir);
            if let Err(e) = std::fs::write(&out, &p.png) {
                log::line(&format!("map {name}: proxy not written: {e}"));
                return None;
            }
            add_entry(&Entry { w: p.w, h: p.h, at: p.at, source: path.to_path_buf() });
            log::line(&format!("map {name}: {}x{} full colour -> WA map {}x{} (original at {:?}), {:.0} ms", img.w, img.h, p.w, p.h, p.at, t.elapsed().as_secs_f64() * 1e3));
            Some(out)
        }
        Err(e) => {
            log::line(&format!("map {name}: not converted: {e}"));
            None
        }
    }
}

fn wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain([0]).collect()
}

type CreateW = extern "system" fn(*const u16, u32, u32, *mut c_void, u32, u32, *mut c_void) -> *mut c_void;
type CreateA = extern "system" fn(*const u8, u32, u32, *mut c_void, u32, u32, *mut c_void) -> *mut c_void;

/// WA's CreateFile on `path`: the proxy's path instead for a full-colour map (written to when
/// WA writes)
fn swap(path: &Path, access: u32) -> Option<PathBuf> {
    let p = std::panic::catch_unwind(|| proxy_for(path)).ok().flatten()?;
    if access & 0x4000_0000 != 0 {
        log::line(&format!("map {}: WA writes it: the proxy is written, the original kept", path.display()));
    }
    Some(p)
}

extern "system" fn cfw(name: *const u16, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, tmpl: *mut c_void) -> *mut c_void {
    let orig: CreateW = unsafe { std::mem::transmute(O_CFW) };
    if !name.is_null() {
        let n = unsafe { (0..).take_while(|&i| *name.add(i) != 0).count() };
        let path = PathBuf::from(String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, n) }));
        if let Some(p) = swap(&path, access) {
            return orig(wide(&p).as_ptr(), access, share, sa, disp, flags, tmpl);
        }
    }
    orig(name, access, share, sa, disp, flags, tmpl)
}

extern "system" fn cfa(name: *const u8, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, tmpl: *mut c_void) -> *mut c_void {
    if !name.is_null() {
        let n = unsafe { (0..).take_while(|&i| *name.add(i) != 0).count() };
        // WA's paths are in the ANSI code page; a full-colour map's path goes the wide way
        let path = PathBuf::from(String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(name, n) }).to_string());
        if let Some(p) = swap(&path, access) {
            let orig: CreateW = unsafe { std::mem::transmute(O_CFW) };
            return orig(wide(&p).as_ptr(), access, share, sa, disp, flags, tmpl);
        }
    }
    let orig: CreateA = unsafe { std::mem::transmute(O_CFA) };
    orig(name, access, share, sa, disp, flags, tmpl)
}

/// Patches WA.exe's imports of kernel32 CreateFileA / CreateFileW
pub fn install(dir: &Path) -> Result<(), String> {
    *DIR.lock().unwrap() = Some(dir.to_path_buf());
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
