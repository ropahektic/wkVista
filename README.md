# wkVista

An in-game renderer for stock **Worms Armageddon 3.8.1**, as a WormKit module. WA's frontend is
untouched; in game you can zoom in.

- **Zoom in**: mouse wheel or numpad + / − (numpad * back to 1×), 1× to 4×, about the centre of
  WA's screen (WA's camera). The game world is zoomed; the HUD stays as WA draws it.
- At **1×** WA draws everything itself, exactly as without the module.

Planned: zooming out (WA drawing a bigger screen), OpenGL presentation, and a palette-free
pipeline for mods (sprites in full colour, WA's own graphics unchanged).

## Install

Copy `wkVista.dll` into the WA folder (WormKit loads every `wk*.dll` there). Settings are in
`wkVista.ini` beside it (written with the defaults on first start).

## Logs (for test runs)

- `wkVista.log`: start-up, hooks, each game, zoom changes, marks, anything wkVista refused to
  draw, errors; each line with WA's game frame. The previous run's is `wkVista.prev.log`.
- `logs/render-NNN.csv`: one per game, one row per drawn frame: zoom, render time, the
  operations wkVista zoomed by kind, those WA drew itself, refusals.
- **Mark**: Scroll Lock writes a numbered mark into both, so a note like "glitch after my 3rd
  mark" points at a frame.

## How it works

The rules come from [wa-play](../wa-play) (WA running its own transliterated code in the
browser), where every drawing operation and the zoom were verified pixel for pixel against WA
(`docs/RENDERER.md` there).

- `src/hooks.rs`: GameRender, RenderDrawingQueue (each entry's space: world or screen), WA's 7
  drawing functions into the framebuffer, WA's window procedure (the wheel and the keys).
- `src/raster.rs`: each operation drawn through the view, straight into WA's framebuffer as WA
  issues it (the same per-pixel rule and integer view mapping as wa-play's rasterizer).
- `tests/fixtures.rs`: wkVista's rasterizer against wa-play's verified pictures (needs wa-play's
  exported fixtures).

## Build

```sh
cargo build --release          # target/i686-pc-windows-msvc/release/wkVista.dll (32-bit)
cargo test --release           # the rasterizer against wa-play's fixtures
```
