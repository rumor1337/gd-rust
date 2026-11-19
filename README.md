# gd-rust

Geometry Dash rewritten in Rust: 240 Hz physics and a CPU renderer that match
the real game. Play any level by its ID.

## Install

Download a package from **Releases** (Linux, macOS, Windows), unpack it and run `demo`.
macOS builds are unsigned: run `xattr -dr com.apple.quarantine gd-rust-macos-arm64` once.

Or build it: `cargo run --release --bin demo` (Linux needs `libasound2-dev`).

## Requirements

- Geometry Dash installed. The demo draws with the game's own art and plays its
  songs; nothing of RobTop's is bundled. It looks in `--assets <dir>`, then
  `DEMO_ASSETS`, then the folder picked last time, then (Windows) Steam's default
  folder. If none has the game, a folder picker asks once and remembers it.
- Python 3, to download levels and their songs (`tools/fetch_level.py`).
- FFmpeg only for `record`. The demo plays audio itself.

## Demo

```sh
demo [level id] [--assets <GD folder>]   # default 1 (Stereo Madness)
```

A missing level is downloaded with its song (`tools/fetch_level.py`).

| Key | Action |
|---|---|
| Insert, or M+E together | menu: type a level ID, Enter loads it |
| Space / Up / W / mouse | jump |
| R | restart |
| Q / Esc | quit |

You can also type a level ID in the terminal: it loads in the background.

## Other tools

| Binary | Purpose |
|---|---|
| `record [out.mp4]` | render a recorded run to MP4 with the song (`RECORD_LEVEL=<id>`) |
| `frame <tick>` | render one frame to PNG; `FRAME_SWEEP=n` times n frames |
| `fidelity`, `truth`, `calibrate` | compare the sim against real GD recordings (`macros/*.gdr2`) |
| `rig` | hand-driven test level with every orb, pad and portal |

## Layout

```
src/sim/      physics, collisions, triggers
src/render/   renderer (sprite atlas, effects, shaders)
src/replay/   replays of real GD recordings (.gdr2), for the comparison tools
src/level/    level string parser
data/         object table (ids, hitboxes, sprites)
```

`cargo test --release` runs the test suite.
