# gd-rust

A Geometry Dash engine in Rust: 240 Hz physics, a CPU renderer that matches the
real game's look, and a bot that solves levels on its own.

## Requirements

- Rust (stable), `ffmpeg` / `ffplay` for video and music, `python3` to download levels
- Geometry Dash's own art. Nothing of RobTop's is bundled. Linux reads
  `~/Documents/gd-clone`, Windows reads the Steam install; `DEMO_ASSETS=<dir>` overrides.

## Demo

```sh
cargo run --release --bin demo [level id]   # default 149992717 (GRIEF)
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
