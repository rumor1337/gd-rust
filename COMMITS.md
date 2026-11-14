# Recent commits — visual arc summary

Covers the last 8 commits (2741142 → 7d48a6d), the rendering/demo overhaul that turned the
sim into a watchable, level-faithful GRIEF demo.

## 2741142 — tinting, bg texture, dart icon
- Color-channel tinting of objects from key 21 (channel id → RGB).
- Tiled background texture driven by header key kA6 (game_bg_NN).
- Real wave dart icon replaces the drawn triangle.

## 698c249 — bg recolor + layering
- Background texture multiplied by channel 1000 (black for GRIEF → dark look).
- Decorations drawn behind gameplay objects.
- Dart tilt +90 (icon art points up), fps counter overlay.

## 340944e — render perf + correct tilt
- Tinted-sprite cache keyed (texture, channel) — no per-frame pixmap clones.
- Dart art verified right-pointing (52×42 hd) → aim = atan2(vy, vx) with
  tiny-skia CCW screen rotation.
- Icon/sprite access made `pub` for debug bins (frame.rs).

## daa68d6 — wave trail + true-motion tilt + 60fps path
- Wave trail from `Player::last_move_dy` (post-collision y movement — true travel
  direction; slope rides zero vel_y, which lies about motion).
- 60fps render path; frame.rs updated.

## c26bfd9 — triggers re-arm on respawn
- `TriggerEngine::reset()` called from `Sim::respawn()` — classic GD re-arms all
  x-crossing triggers on every attempt (instants fire again, cursor rewinds).

## 9a0f32d — deterministic demo restarts
- Demo no longer lets the sim's internal respawn race the macro rewind: on death it
  immediately `sim.reset()` + fresh `Playback`. Previously inputs resumed at the
  death frame → instant re-death loop.

## 51e217c — HUD + death effects
- Progress bar, %, fps, attempt number (3×5 bitmap `draw_text`).
- Death ring + particle burst on kill.

## 7d48a6d — level-data colors (no hardcoding)
- `ColorEngine` evaluates each level's own color triggers (899 / legacy 29 / 30;
  key 23 = channel, 7/8/9 = RGB, 10 = fade duration) with per-frame lerp;
  channels seed from the kS38 header.
- GRIEF hardcoded palette pin deleted. Objects without key 21 default to the
  "obj" channel 1004. Ground/ceiling = ch 1001, surface line = ch 1002, bg
  gradient derives from live ch 1000.
- Real wave icon (dart_01-uhd + glow) from ~/Documents/gd-clone/Resources/icons;
  pink player per GRIEF reference. `frame.rs FRAME_LEVEL=<id>` dumps any level.
- Tests updated to data-driven expectations; 27/27 green.

## Known gaps
- Pulse (1006) / Alpha (1007) triggers not evaluated — GRIEF's 6,074 pulses drive
  neon strobing; flash sections render flatter/darker than the real game.
- Opacity untracked; touch-gated (key 11) triggers fire on x-cross regardless of y.
