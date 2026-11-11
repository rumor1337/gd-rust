# GD 1:1 Rust Gameplay Engine + 24/7 TikTok Streamer — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A headless Rust 1:1 recreation of Geometry Dash gameplay (no menus) that auto-plays the current pointercrate top 5 demons 24/7 on a vertical TikTok LIVE stream, where a viewer gift resets the current run.

**Architecture:** Single Rust crate `gdrust` with modules (level parser → fixed-timestep sim → software renderer → ffmpeg RTMP push) plus one Node sidecar for TikTok gift events. Physics constants are extracted from the original GeometryDash.exe via REA/Ghidra and pinned as the source of truth in `docs/physics.md`. The bot auto-plays via replay macros or a segment-search solver; every run is deterministic.

**Tech Stack:** Rust (edition 2021, tiny-skia for CPU rasterization, serde+flate2+base64 for level parsing), ffmpeg as spawned child process, Node 20+ with `tiktok-live-connector` for gifts. Deploy target: staging (Oracle Linux 9.8, aarch64, 2× Neoverse-N1, 10GB RAM, no GPU).

**Spec:** The conversation (2026-11-11): user wants "1:1 gd but rewrite in rust and optimise", gameplay only — no extra menus, no main menu — including death animations; "auto (macros)"; levels = top 5 demons from pointercrate.com; music = Newgrounds (non-copyrighted) tracks; 24/7 TikTok LIVE; gift → reset progress.

## Global Constraints

- 1:1 physics: constants must come from the decompiled GeometryDash.exe (sha256 524265b3…) — community values are cross-checks, not the source.
- Deterministic sim: same seed (level + macro) ⇒ identical run; fixed timestep, no frame-dependent logic.
- Gameplay only: level loading, playing, death, respawn, progress. No menu, editor, shop, icons UI.
- Render 720×1280 (vertical 9:16) @ 30fps via CPU only; must hold 30fps on 2 ARM cores with x264 encoding in parallel.
- Levels are the 5 downloaded .gmd files in `levels/` (GRIEF 149992717, Artemis 13 148598469, Society 127323087, Thinking Space II 119544028, Amethyst 119550490).
- No new heavy dependencies; tiny-skia/serde/flate2 are the render/parse baseline.
- TikTok RTMP URL + stream key supplied via env (`TT_RTMP_URL`, `TT_STREAM_KEY`) — never hardcoded, account-owned.

## Review Focus

- Sim determinism drift (float ops must be IEEE-ordered identically every run) — pinned by Task 4 replay test.
- Level strings from GD 2.2 use base64url+gzip with kVpars format; some fields repeat (kS38 color arrays) — parser must not drop repeats. Task 3 test.
- Object coverage: top demons use ~all object types; unknown IDs must render as placeholder and never crash the parser. Task 3/7.
- Frame pipe backpressure: if sim renders faster than ffmpeg consumes, memory explodes — bounded channel required. Task 11.
- Gift event floods (TikTok gift spam) must debounce to one reset per run, not per event. Task 12.
- ffmpeg exit / RTMP disconnect must reconnect, not kill the 24/7 loop. Task 11/13.

---

### Task 1: Scaffold

**Files:** Create: `Cargo.toml`, `src/main.rs`, `src/lib.rs`, `.gitignore`, `levels/` (exists with .gmd files), `docs/physics.md` (stub).

**Interfaces:** Produces: crate `gdrust` with empty modules `level`, `sim`, `render`, `bot`, `stream`, `bridge`.

- [ ] `cargo new`, git init, module stubs, `cargo test` green, commit.

### Task 2: REA physics extraction

**Files:** Create: `docs/physics.md`, `src/physics.rs` (constants with provenance comments), `analysis/gd.snapshot` (REA Ghidra snapshot, running now).

**Interfaces:** Produces: `pub const TICK_HZ: f32 = 240.0;` `GRAVITY_CUBE`, `JUMP_VEL_CUBE`, `SPEEDS: [f32; 5]`, `PLAYER_HITBOX`, `ORBS`, `PADS` (name → exact value; each entry cites the decompiled function in physics.md).

- [ ] Search snapshot for `PlayerObject::update`, jump/orb/pad/velocity constants via `rea decompile --snapshot analysis/gd.snapshot`.
- [ ] Transcribe into docs/physics.md (value, unit, function, evidence ID).
- [ ] `src/physics.rs` constants + unit test asserting jump height ≈ 2 blocks (cube) and speed values within 1% of community values (10.386 etc.).
- [ ] Commit.

### Task 3: Level parser

**Files:** Create: `src/level/mod.rs`, `src/level/object.rs`, `data/hitboxes.json` (seeded from community object table; REA-checked later).

**Interfaces:** Produces: `parse_level(bytes: &[u8]) -> Level` where `Level { header: LevelHeader (colors kS, speed kA, start_pos), objects: Vec<GdObject> }`, `GdObject { id: u16, x: f32, y: f32, rot: f32, flip_x: bool, flip_y: bool, base_color: u8, detail_color: u8, groups: Vec<u16> }`. Consumes: the 5 `.gmd` files.

- [ ] Test: parse all 5 levels — object counts match `grep -c ';' + 1` from download step (189607 / 51881 / 167385 / 139570 / 147425); no panics on unknown ids.
- [ ] Test: kS38 repeated color fields all captured.
- [ ] Implement gzip+base64url decode → kVpars split → object parse (split on `;`, key-value `,` pairs, `kA`-prefixed object keys: `1` id, `2` x, `3` y, `6` rot, `4` flipx, `5` flipy, `21` base color, `22` detail color, `57` groups).
- [ ] Commit.

### Task 4: Sim core — cube, speed portals, deterministic tick

**Files:** Create: `src/sim/mod.rs`, `src/sim/player.rs`, `src/sim/input.rs`.

**Interfaces:** Produces: `Sim::new(level: &Level) -> Sim`, `sim.step(inputs: &InputState) -> TickResult` (`TickResult::Alive{progress_pct, pos} | Dead{pct} | Complete`), `InputState { hold: bool }` at 240Hz. Fixed `f32` ops, single code path.

- [ ] Test: empty level — cube free-falls with exact gravity displacement per physics.rs; lands on ground plane y=0.
- [ ] Test: 60s of hold-nothing input on GRIEF start — identical positions across 2 runs (byte-identical progress log).
- [ ] Test: speed portals (obj 200–204) switch SPEEDS table.
- [ ] Implement player state machine (grounded/airborne, rotation), camera x = player x.
- [ ] Commit.

### Task 5: Collision + interactive objects

**Files:** Create: `src/sim/collision.rs`, `data/hitboxes.json` (filled), `src/sim/objects.rs` (orbs 36–49, pads 35–…, portals 10–13/45–47, pads).

**Interfaces:** Consumes: hitboxes.json via `load_hitboxes() -> HashMap<u16, Hitbox>`. Produces: death on hazard overlap (inner hitbox 30×30 scaled), orb/pad activation buffering, portal transitions.

- [ ] Test: synthetic level — cube dies exactly when inner box overlaps spike triangle per hitbox table.
- [ ] Test: yellow orb from physics.rs impulse values → apex height matches.
- [ ] Implement spatial hash (bucket by 30-unit grid cell) — 190K objects demand it.
- [ ] Commit.

### Task 6: Remaining gamemodes

**Files:** Modify: `src/sim/player.rs`, `src/physics.rs`.

**Interfaces:** Produces: modes Cube, Ship, Ball, UFO, Wave, Robot, Spider, Swing (portals 12/13/45/47/99…), each with constants from physics.md.

- [ ] Test per mode: one synthetic input sequence → deterministic position log; ship gravity/wave slope values cross-checked vs physics.md.
- [ ] Commit.

### Task 7: Software renderer

**Files:** Create: `src/render/mod.rs`, `src/render/atlas.rs`, `assets/` (copied GJ sprite sheets + plists from ~/Documents/gd-clone/Resources).

**Interfaces:** Consumes: `&Level`, camera window, `TickResult`. Produces: `render_frame(target: &mut tiny_skia::Pixmap)` at 720×1280; culls to camera ± 2 screens on x; only object ids in `render_map.json` get sprites, else debug quad.

- [ ] Test: 100 consecutive frames of GRIEF start render in < wall-time budget on this machine; pixmap bytes differ between camera moves (non-blank).
- [ ] Implement atlas load (plist frame → sheet rect), object quad transform (rot/flip), player, ground, background color from kS, object base/detail color tint.
- [ ] Commit.

### Task 8: Death/respawn + HUD

**Files:** Create: `src/sim/effects.rs`, `src/render/hud.rs`.

**Interfaces:** Produces: death animation state (explosion particles from player pos, 0.5s), respawn after 1s (GD-style), attempt counter, progress % bar, level name text (font: copy a .fnt from Resources).

- [ ] Test: death at X → particles spawn exactly once, respawn resets sim to level start, attempts +1.
- [ ] Commit.

### Task 9: Bot — macros + solver + grind mode

**Files:** Create: `src/bot/mod.rs`, `src/bot/macro.rs` (format: JSONL `{frame, hold}`), `src/bot/solver.rs` (segment search, offline tool `gdrust solve <level>`), `macros/` dir.

**Interfaces:** Produces: `Bot::next_input(sim: &Sim) -> InputState`; `reset()` on demand. Solver: greedy segment search with backtracking (retries from last checkpoint), emits a macro file. Grind mode default: bot attempts with best-known macro; dies = authentic restart.

- [ ] Test: macro playback is deterministic — replay of recorded inputs reproduces identical progress log.
- [ ] Test: solver completes a synthetic trivial level (flat ground + one spike) emitting a valid macro.
- [ ] Commit.

### Task 10: Audio

**Files:** Create: `tools/fetch_songs.sh` (boomlings getGJSong20 with empty User-Agent), `audio/` dir.

**Interfaces:** Produces: mp3 per level named `<level_id>.mp3`. Note: NG song IDs 482872, 462046, 1569886, 489111, 233860.

- [ ] Step: fetch 5 songs; `ffprobe` each non-empty.
- [ ] Commit.

### Task 11: Streamer — ffmpeg pipe + RTMP

**Files:** Create: `src/stream/ffmpeg.rs`, `src/main.rs` (loop wiring).

**Interfaces:** Produces: `Streamer::start(cfg) -> FrameSink` where `FrameSink::submit(&Pixmap)`; spawns `ffmpeg -f rawvideo -pix_fmt rgba -s 720x1280 -r 30 -i - -i audio/<id>.mp3 -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -f flv $TT_RTMP_URL/$TT_STREAM_KEY`; bounded 2-frame channel; reconnect on ffmpeg exit with 5s backoff.

- [ ] Test: with env pointing to a local ffconcat/null output, 10s of frames flow, process exits 0 on drop.
- [ ] Commit.

### Task 12: TikTok gift bridge

**Files:** Create: `bridge/index.js`, `bridge/package.json` (`tiktok-live-connector`), Rust `src/bridge/mod.rs`.

**Interfaces:** Produces: Rust spawns `node bridge/index.js` as child, reads JSONL events `{"type":"gift","user":...,"count":N}` on stdout → `bot.reset()` debounced 1s; stream status events logged. Env: `TT_USERNAME` (TikTok handle to watch).

- [ ] Test: feed fake JSONL lines to the Rust parser → exactly one reset per debounce window.
- [ ] Commit.

### Task 13: Orchestrator + deploy

**Files:** Create: `src/orchestrator.rs`, `deploy/gdrust.service`, `deploy/deploy.sh`.

**Interfaces:** Produces: level rotation (loop 5 levels), run loop 24/7 with auto-restart, `deploy.sh` = rsync binary+assets+levels+audio to staging + install systemd unit (ffmpeg via dnf).

- [ ] Test: local 5-minute smoke run cycling 2 levels with fake stream sink + fake gift feed → log shows rotation + resets, RSS stable.
- [ ] Commit.

### Task 14: Staging verification

- [ ] Deploy, run 1h: 30fps stable, total CPU < 190% (of 200%), RSS < 1.5GB, no frame-pipe stalls. Record numbers in `docs/staging-run.md`.

## Macro sourcing note (honest limitation)

Macros for the 5 demons are not publicly downloadable as files; the solver is the default path (segment search — weeks of tuning for top demons, may never fully complete GRIEF). Grind mode (dies + restarts, authentic) is the guaranteed 24/7 content; completions happen when the solver or an imported macro succeeds. Import format (Task 9) accepts any JSONL macro the user supplies later (e.g., from GDR replays).
