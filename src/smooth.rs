//! Smooth motion between game frames. WA simulates 50 frames a second but draws more often
//! (GameRender runs 1.5 to 4 times a game frame at high refresh rates, the camera already
//! easing per render); objects only move on game frames, so between them they stand still and
//! judder against the camera.
//!
//! WA's game frames don't come evenly: its frame clock steps in 1/64 s, so frames arrive in
//! uneven bursts. So positions are not placed by "time since this frame began": wkVista keeps
//! each physics object's positions of the last few game frames and its own steady clock of
//! game time (real time, gently pulled toward 0.8 frames behind the frame WA has reached, never
//! past it), and each render shows every object at that clock, between the recorded frames
//! around it. Simulated with WA's 1/64 s frame clock this moves within 9 % of an even pace at
//! about 16 ms behind (0.5 frames behind still stalls). After the render the real positions
//! are put back, so the game never sees it.
//!
//! The objects: the collision manager's list (world +0x528, its DwordList at +0x18: count +8,
//! data +0xC); position 16.16 at +0x84 / +0x88. An object that moved more than 64 px between
//! two frames, or is new, is drawn where it is.

use std::collections::HashMap;
use std::time::Instant;

/// game frames of history kept per object
const KEEP: usize = 4;
/// how far behind the frame reached the clock is aimed (game frames)
const BEHIND: f64 = 0.8;

struct Track {
    vtable: usize,
    /// (game frame, position), oldest first
    hist: Vec<(u32, (i32, i32))>,
}

pub struct Smooth {
    /// the last game frame seen
    frame: u32,
    /// the steady clock, in game frames
    clock: f64,
    last: Instant,
    tracks: HashMap<usize, Track>,
    /// the positions moved for this render: (object, its real position)
    moved: Vec<(usize, (i32, i32))>,
    /// this render's clock minus the frame reached (for the log)
    pub lag: f64,
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
        Smooth { frame: u32::MAX, clock: 0.0, last: Instant::now(), tracks: HashMap::new(), moved: Vec::new(), lag: 0.0 }
    }

    /// Before a render of game frame `frame` (50 a second): the objects at the steady clock
    pub unsafe fn before(&mut self, world: usize, frame: u32) {
        let objs = objects(world);
        let now = Instant::now();
        let real = now.duration_since(self.last).as_secs_f64() * 50.0;
        self.last = now;
        if frame != self.frame {
            // a new game frame: each object's position recorded
            if self.frame == u32::MAX || frame < self.frame || frame > self.frame + 10 {
                // a start, or a jump: the clock set, no history
                self.tracks.clear();
                self.clock = frame as f64;
            }
            self.frame = frame;
            let mut tracks = HashMap::with_capacity(objs.len());
            for &e in &objs {
                let (vt, p) = (r32(e) as usize, pos(e));
                let mut hist = match self.tracks.remove(&e) {
                    Some(t) if t.vtable == vt => t.hist,
                    _ => Vec::new(),
                };
                hist.push((frame, p));
                if hist.len() > KEEP {
                    hist.remove(0);
                }
                tracks.insert(e, Track { vtable: vt, hist });
            }
            self.tracks = tracks;
        }
        // the steady clock: real time, pulled toward BEHIND frames before the frame reached by
        // about 10 % a game frame's time (however often WA renders), never past that frame
        self.clock += real;
        let reached = frame as f64;
        self.clock += (reached - BEHIND - self.clock) * (real * 0.1).min(1.0);
        self.clock = self.clock.clamp(reached - 3.0, reached);
        let at = self.clock;
        self.lag = self.clock - reached;
        for &e in &objs {
            let Some(tr) = self.tracks.get(&e) else { continue };
            let p = pos(e);
            // the recorded frames around `at`
            let Some(&(f1, p1)) = tr.hist.last() else { continue };
            if p != p1 || tr.hist.len() < 2 {
                continue;
            }
            let k = tr.hist.iter().rposition(|&(f, _)| f as f64 <= at).unwrap_or(0);
            let (fa, pa) = tr.hist[k];
            let (fb, pb) = if k + 1 < tr.hist.len() { tr.hist[k + 1] } else { (f1, p1) };
            let (dx, dy) = (pb.0 as i64 - pa.0 as i64, pb.1 as i64 - pa.1 as i64);
            if dx.abs() > 64 << 16 || dy.abs() > 64 << 16 {
                continue;
            }
            let u = if fb > fa { ((at - fa as f64) / (fb - fa) as f64).clamp(0.0, 1.0) } else { 1.0 };
            let s = (u * 65536.0) as i64;
            let q = ((pa.0 as i64 + dx * s / 65536) as i32, (pa.1 as i64 + dy * s / 65536) as i32);
            if q != p {
                self.moved.push((e, p));
                set_pos(e, q);
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
