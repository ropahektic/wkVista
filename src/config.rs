//! `wkVista.ini` beside the DLL (written with the defaults when missing): `key = value` lines,
//! `#` comments. Keys are Windows virtual-key codes (hex with 0x, or decimal).

use std::path::Path;

#[derive(Clone, Debug)]
pub struct Config {
    /// the mouse wheel zooms (the wheel is then not passed to WA)
    pub wheel: bool,
    pub key_zoom_in: u32,
    pub key_zoom_out: u32,
    pub key_zoom_reset: u32,
    pub key_mark: u32,
    /// the largest zoom, in sixteenths (64 = 4x)
    pub zoom_max: i32,
    /// one CSV row per drawn frame (logs/render-NNN.csv)
    pub csv: bool,
}

impl Default for Config {
    fn default() -> Self {
        // numpad + / - / *, Scroll Lock
        Config { wheel: true, key_zoom_in: 0x6B, key_zoom_out: 0x6D, key_zoom_reset: 0x6A, key_mark: 0x91, zoom_max: 64, csv: true }
    }
}

const DEFAULT_INI: &str = "\
# wkVista settings. Keys are Windows virtual-key codes.
# The mouse wheel zooms in game (1 = on; the wheel is then not passed to WA)
wheel = 1
# numpad +, numpad -, numpad * (back to 1x)
key_zoom_in = 0x6B
key_zoom_out = 0x6D
key_zoom_reset = 0x6A
# writes a numbered mark into wkVista.log and the game's CSV (Scroll Lock)
key_mark = 0x91
# the largest zoom in sixteenths (64 = 4x)
zoom_max = 64
# one CSV row per drawn frame in logs/render-NNN.csv (1 = on)
csv = 1
";

fn num(v: &str) -> Option<i64> {
    let v = v.trim();
    match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        Some(h) => i64::from_str_radix(h, 16).ok(),
        None => v.parse().ok(),
    }
}

/// The settings (the defaults written when the file is missing); unknown or bad lines are
/// reported in the returned list
pub fn load(dir: &Path) -> (Config, Vec<String>) {
    let p = dir.join("wkVista.ini");
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(_) => {
            let _ = std::fs::write(&p, DEFAULT_INI);
            DEFAULT_INI.to_string()
        }
    };
    let mut c = Config::default();
    let mut bad = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            bad.push(line.to_string());
            continue;
        };
        let (k, n) = (k.trim(), num(v));
        match (k, n) {
            ("wheel", Some(n)) => c.wheel = n != 0,
            ("key_zoom_in", Some(n)) => c.key_zoom_in = n as u32,
            ("key_zoom_out", Some(n)) => c.key_zoom_out = n as u32,
            ("key_zoom_reset", Some(n)) => c.key_zoom_reset = n as u32,
            ("key_mark", Some(n)) => c.key_mark = n as u32,
            ("zoom_max", Some(n)) => c.zoom_max = (n as i32).clamp(16, 128),
            ("csv", Some(n)) => c.csv = n != 0,
            _ => bad.push(line.to_string()),
        }
    }
    (c, bad)
}
