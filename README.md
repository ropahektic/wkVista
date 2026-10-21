# wkVista

An in-game renderer for stock **Worms Armageddon 3.8.1**, as a WormKit module. WA's frontend is
untouched; in game:

- **Zoom in**: mouse wheel or numpad + / − (numpad * back to 1×), 1× to 4×, about the centre of
  WA's screen (WA's camera, which can then reach the level's edges). The game world is zoomed;
  the HUD stays as WA draws it.
- **Full-colour maps**: put any PNG (32-bit with transparency, or 24-bit) or JPEG in
  `User\SavedLevels` and pick it in WA's map list. WA gets a map it accepts (up to 112 colours,
  what opponents without wkVista see); wkVista shows the original's colours on all land WA hasn't
  changed. Alpha ≥ 128 is land; an image with no transparency gets air on top for placing worms.
- **Native true colour** (WA's renderer on one of its Direct3D 9 options): wkVista draws WA's
  frame itself, operation by operation in WA's order, and WA's Direct3D 9 backend shows that
  frame in place of its own palette pass. Nothing is painted over WA's picture.
- At **1×** with an ordinary map, WA draws everything itself, exactly as without the module.

## Install

Copy `wkVista.dll` into the WA folder (WormKit loads every `wk*.dll` there). Settings are in
`wkVista.ini` beside it (written with the defaults on first start). Converted maps are kept in
`wkVista\maps\`; your originals are never written.

## Logs (for test runs)

- `wkVista.log`: start-up, hooks, maps converted, each game (its land, whether a full-colour map
  matched), zoom changes, marks, anything wkVista refused to draw, errors; each line with WA's
  game frame. The previous run's is `wkVista.prev.log`.
- `logs/render-NNN.csv`: one per game, one row per drawn frame: zoom, render time, wkVista's
  drawing time, the colour pass time, the operations wkVista drew by kind, those WA drew itself.
- **Mark**: Scroll Lock writes a numbered mark into both, so a note like "glitch after my 3rd
  mark" points at a frame.

## How it works

The rules come from wa-play (WA running its own transliterated code in the browser), where
every drawing operation and the zoom were verified pixel for pixel against WA.

- `src/hooks.rs`: GameRender, RenderDrawingQueue (each entry's space: world or screen), WA's 7
  drawing functions into the framebuffer, the camera clamp, WA's window procedure.
- `src/raster.rs`: each operation drawn through the view straight into WA's framebuffer as WA
  issues it; for full-colour maps a side buffer marks the screen pixels showing untouched land.
- `src/present.rs`: WA's Direct3D 9 backends' "show frame" step, replaced by wkVista's frame.
- `src/files.rs`, `src/pngmap.rs`: full-colour maps opened by WA as converted proxies.
- `src/land.rs`: the game's land tiles, matched to the converted map.

## Credits

Loading full-colour maps by handing WA a converted proxy when it opens the file was first done
by [wkRGBA](https://github.com/rUNaW4y/wkRGBA) (rUNaW4y). wkVista's implementation is its own;
the idea is theirs.

## Build

```sh
cargo build --release          # target/i686-pc-windows-msvc/release/wkVista.dll (32-bit)
cargo test --release           # rasterizer vs wa-play's fixtures, proxy maps vs WA's rules
```
