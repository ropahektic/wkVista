//! Smooth motion between game frames. WA simulates 50 frames a second but draws more often
//! (GameRender runs 1.5 to 2.3 times a game frame at high refresh rates, the camera already
//! easing per render); objects only move on game frames, so between them they stand still and
//! judder against the camera. Before each render, every physics object (the collision
//! manager's list: world +0x528, its DwordList at +0x18: count +8, data +0xC; position 16.16 at
//! +0x84 / +0x88) is put at the point between its last two game-frame positions that the time
//! since the frame began gives; after the render its real position is put back, so the game
//! never sees it (the picture runs up to one game frame behind). Objects that jumped further
//! than 64 px or are new this frame are drawn where they are.

use std::collections::HashMap;
use std::time::Instant;

struct Track {
    vtable: usize,
    prev: (i32, i32),
    cur: (i32, i32),
}

pub struct Smooth {
    frame: u32,
    started: Instant,
    /// a game frame's length (ms), measured
    frame_ms: f64,
    tracks: HashMap<usize, Track>,
    /// the positions moved for this render: (object, its real position)
    moved: Vec<(usize, (i32, i32))>,
}

unsafe fn r32(a: usize) -> u32 {
    std::ptr::read_unaligned(a as *const u32)
}

/// The physics objects of the game (the collision manager's list)
unsafe fn objects(world: usize) -> Vec<usize> {
    let cm = r32(world + 0x528) as usize;
    if cm == 0 {
        return Vec::new();
    }
    let (n, data) = (r32(cm + 0x18 + 8) as usize, r32(cm + 0x18 + 0xC) as usize);
    if data == 0 || n > 4096 {
        return Vec::new();
    }
    (0..n).map(|k| r32(data + 4 * k) as usize).filter(|&e| e != 0).collect()
}

unsafe fn pos(e: usize) -> (i32, i32) {
    (r32(e + 0x84) as i32, r32(e + 0x88) as i32)
}

unsafe fn set_pos(e: usize, p: (i32, i32)) {
    std::ptr::write_unaligned((e + 0x84) as *mut i32, p.0);
    std::ptr::write_unaligned((e + 0x88) as *mut i32, p.1);
}

impl Smooth {
    pub fn new() -> Smooth {
        Smooth { frame: u32::MAX, started: Instant::now(), frame_ms: 20.0, tracks: HashMap::new(), moved: Vec::new() }
    }

    /// Before a render of game frame `frame`: the objects at their in-between positions
    pub unsafe fn before(&mut self, world: usize, frame: u32) {
        let objs = objects(world);
        if frame != self.frame {
            // a new game frame: its length measured (a pause or a jump is not one), each
            // object's last two positions
            let ms = self.started.elapsed().as_secs_f64() * 1e3;
            if frame == self.frame.wrapping_add(1) && (5.0..60.0).contains(&ms) {
                self.frame_ms = self.frame_ms * 0.9 + ms * 0.1;
            }
            let fresh = frame != self.frame.wrapping_add(1);
            self.frame = frame;
            self.started = Instant::now();
            let mut tracks = HashMap::with_capacity(objs.len());
            for &e in &objs {
                let (vt, p) = (r32(e) as usize, pos(e));
                let prev = match self.tracks.get(&e) {
                    Some(t) if t.vtable == vt && !fresh => t.cur,
                    _ => p,
                };
                tracks.insert(e, Track { vtable: vt, prev, cur: p });
            }
            self.tracks = tracks;
        }
        // the share of the frame gone
        let t = (self.started.elapsed().as_secs_f64() * 1e3 / self.frame_ms).clamp(0.0, 1.0);
        let k = (t * 65536.0) as i64;
        for &e in &objs {
            let Some(tr) = self.tracks.get(&e) else { continue };
            let p = pos(e);
            if p != tr.cur || tr.prev == tr.cur {
                continue;
            }
            let (dx, dy) = (tr.cur.0 as i64 - tr.prev.0 as i64, tr.cur.1 as i64 - tr.prev.1 as i64);
            if dx.abs() > 64 << 16 || dy.abs() > 64 << 16 {
                continue;
            }
            let at = ((tr.prev.0 as i64 + dx * k / 65536) as i32, (tr.prev.1 as i64 + dy * k / 65536) as i32);
            if at != p {
                self.moved.push((e, p));
                set_pos(e, at);
            }
        }
    }

    /// After the render: every object back at its real position
    pub unsafe fn after(&mut self) {
        for (e, p) in self.moved.drain(..) {
            set_pos(e, p);
        }
    }

    /// The game left (a new world): nothing kept
    pub fn reset(&mut self) {
        self.frame = u32::MAX;
        self.tracks.clear();
        self.moved.clear();
    }
}
