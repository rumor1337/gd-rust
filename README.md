# gd-rust

A Geometry Dash engine in Rust: 240 Hz physics, a CPU renderer that matches the
real game's look, and a bot that solves levels on its own.

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
- FFmpeg only for `record` and `live`. The demo plays audio itself.

## Demo

```sh
demo [level id] [--assets <GD folder>]   # default 149992717 (GRIEF)
```

A missing level is downloaded with its song (`tools/fetch_level.py`). It plays a
recorded xdBot macro from `macros/` if there is one, otherwise the bot's own
solve, which is cached in `macros/<id>.auto.json`.

| Key | Action |
|---|---|
| Insert | menu: type a level ID, Enter loads it, Tab switches Auto / Manual |
| Space / Up / W / mouse | jump (Manual mode) |
| R | restart |
| Q / Esc | quit |

You can also type a level ID in the terminal to queue it: it loads in the background.

## Other tools

| Binary | Purpose |
|---|---|
| `record [out.mp4]` | offline render to MP4 with the song (`RECORD_LEVEL=<id>`) |
| `live` | 24/7 vertical stream to TikTok LIVE (keys in `.env`, see `deploy/`) |
| `frame <tick>` | render one frame to PNG; `FRAME_SWEEP=n` times n frames |
| `fidelity`, `truth`, `autocheck` | compare the sim against real GD recordings |
| `rig` | hand-driven test level with every orb, pad and portal |

## Layout

```
src/sim/      physics, collisions, triggers
src/render/   renderer (sprite atlas, effects, shaders)
src/bot/      macro parsing (.gdr2) and the beam-search solver
src/level/    level string parser
data/         object table (ids, hitboxes, sprites)
```

`cargo test --release` runs the test suite.
