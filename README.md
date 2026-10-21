# wkVista

An in-game renderer for stock **Worms Armageddon 3.8.1**, as a WormKit module. WA's frontend is
untouched; in game:

- **Zoom in**: mouse wheel or numpad + / − (numpad * back to 1×), 1× to 4×, 
- **Full-colour maps**: put any PNG (32-bit with transparency, or 24-bit) or JPEG in
  `User\SavedLevels` 
- **Native true colour** (WA's renderer on one of its Direct3D 9 options): wkVista draws WA's
  frame itself, operation by operation in WA's order, and WA's Direct3D 9 backend shows that
  frame in place of its own palette pass. Nothing is painted over WA's picture.
- At **1×** with an ordinary map, WA draws everything itself, exactly as without the module.

## Install

Download `wkVista.dll` from the [latest release](https://github.com/ropahektic/wkVista/releases/latest)
and copy it into the WA folder.

## Credits

Loading full-colour maps by handing WA a converted proxy when it opens the file was first done
by [wkRGBA](https://github.com/rUNaW4y/wkRGBA) (rUNaW4y). wkVista's implementation is its own;
the idea is theirs.

## Build

```sh
cargo build --release          # target/i686-pc-windows-msvc/release/wkVista.dll (32-bit)
cargo test --release           # rasterizer vs wa-play's fixtures, proxy maps vs WA's rules
```

## License

MIT, see [LICENSE](LICENSE).
