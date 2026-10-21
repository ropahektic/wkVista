//! What a test run leaves for the developer, beside the DLL:
//! - `wkVista.log`: start-up, hooks, each game, zoom changes, marks, refusals and errors, each
//!   line with WA's game frame; the previous run's is kept as `wkVista.prev.log`.
//! - `logs/render-NNN.csv`: one per game, one row per drawn frame (zoom, the operations drawn
//!   by kind, those zoomed by wkVista and those WA drew itself, refusals, times, marks).
//!
//! A mark (the mark key, Scroll Lock by default) writes a numbered line to both, so a note like
//! "glitch right after my 3rd mark" points at a frame.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static LOG: Mutex<Option<PathBuf>> = Mutex::new(None);
static CSV: Mutex<Option<File>> = Mutex::new(None);

pub fn open(dir: &Path) {
    let _ = std::fs::rename(dir.join("wkVista.log"), dir.join("wkVista.prev.log"));
    let p = dir.join("wkVista.log");
    let _ = File::create(&p);
    *LOG.lock().unwrap() = Some(p);
}

pub fn line(s: &str) {
    if let Some(p) = LOG.lock().unwrap().as_ref() {
        if let Ok(mut f) = OpenOptions::new().append(true).create(true).open(p) {
            let _ = writeln!(f, "{s}");
        }
    }
}

/// The CSV header (one row per drawn frame, `Frame::row`)
pub const CSV_HEADER: &str = "frame,zoom,ms_render,ms_zoomed,zoomed,by_wa,refused,bitmap,fill,grid,pixel,vline,mark";

/// A new game's CSV: logs/render-NNN.csv (the next free number)
pub fn new_game(dir: &Path) -> Option<PathBuf> {
    let logs = dir.join("logs");
    let _ = std::fs::create_dir_all(&logs);
    let n = (0..1000).find(|n| !logs.join(format!("render-{n:03}.csv")).exists())?;
    let p = logs.join(format!("render-{n:03}.csv"));
    let mut f = File::create(&p).ok()?;
    let _ = writeln!(f, "{CSV_HEADER}");
    *CSV.lock().unwrap() = Some(f);
    Some(p)
}

pub fn row(s: &str) {
    if let Some(f) = CSV.lock().unwrap().as_mut() {
        let _ = writeln!(f, "{s}");
    }
}

pub fn flush() {
    if let Some(f) = CSV.lock().unwrap().as_mut() {
        let _ = f.flush();
    }
}
