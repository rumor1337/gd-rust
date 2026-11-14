# Amethyst macro candidates — found 2026-11-14

All three are **xdBot v2 replays** (bot `xdBot v2.3.11` embedded in file).
**Format flag:** none start with the legacy `"GDR "` magic — xdBot v2 saves replays as
**CBOR** (extension `.gdr2`). The `"GDR "` header described in the task belongs to the old
xdBot v1 / MegaHack-compat `.gdr` format. Check the 4 existing macros with `head -c 4`: if
they also start with CBOR (`\xa5author...`), the project already handles v2; if they start
with `GDR `, these v2 files need a CBOR path or a converter.

All are CBOR-replay .gdr2 or JSON dumps, 240 Hz, full-length runs (level is ~81.7–81.9 s
≈ 1:21; all three durations agree → full-completion macros, not partial).

---

## 1. xdBot-Amethyst-pieGD122.gdr2 — PRIMARY
- Demon: Amethyst, level ID 119550490
- Format: CBOR xdBot v2 replay, 743,032 bytes
- Embedded header: author `pieGD122`, bot `xdBot v2.3.11`
- Embedded level block: `level {id: 119550490, name: "Amethyst"}` — **exact ID match** ✔
- Timing: last frame 19667, duration ≈ 81.90 s → 19667/81.90 ≈ **240 fps** ✔
- Deaths/completion: full run (duration == level length); showcase video, death count not stated
- Recorder: pieGD122 (YouTube channel "pie_ [GD]")
- Source: video https://www.youtube.com/watch?v=NHtnsmZhAAc
  (desc: "Amethyst.re3/file xdbot Macro download link")
  → https://www.mediafire.com/file/rb8r0a21z53ywvk/Amethyst.gdr/file
- Sanity: header author/bot/version clean; description empty; level id+name correct; no red flags.
- Notes: contains a large `frameFixes` array (~19.5k entries) — every frame has a fix entry;
  typical of converted/high-precision captures, not a red flag but worth a test playback.

## 2. xdBot-Amethyst-Abenex.gdr2.json — BACKUP (JSON dump, needs conversion)
- Demon: Amethyst, level ID 119550490
- Format: **JSON** dump of an xdBot v2.3.11 replay (`Amethyst.gdr.json`), 2,171,715 bytes
- Fields: author `Abenex`, bot `xdBot v2.3.11`, `framerate: 240.0` (explicit ✔),
  `level {id: 119550490, name: "Amethyst"}` ✔, `ldm: true`, gameVersion 2.206
- Timing: 516 input events, last at frame 19605; duration 81.6875 s; 81.6875 × 240 = 19605 → **exact 240 fps** ✔
- Deaths/completion: full run; recorder states "ALL OF MY MACROS HAVE BEEN CHECKED BY ME";
  video says "ONLY WORKS ON 240 FPS/HZ (impossible on 144)"
- Recorder: Abenex (@abenex2175)
- Source: video https://www.youtube.com/watch?v=JlmrMYPVfY8
  → https://www.mediafire.com/file/gmn9un3uszinnh1/Amethyst.gdr.json/file
- Sanity: cleanest metadata of the three (explicit framerate field). JSON, not binary CBOR —
  must be serialized to .gdr2 CBOR before use.

## 3. xdBot-Amethyst2-cebolot.gdr2 — usable but FLAGGED
- Demon: Amethyst (macro says **"Amethyst 2"**, **level id 0**) ⚠
- Format: CBOR xdBot v2 replay, 894,951 bytes
- Embedded header: author `cebolot`, bot `xdBot v2.3.11`, `ldm: false`
- Timing: last frame 19601; duration 81.67083 s; 81.67083 × 240 = 19601 → **exact 240 fps** ✔
- Deaths/completion: video claims "current top 1, verified by wpopoff. NOCLIP: no. FPS/TPS: 240"
  (wPopoff verified the level itself, not this macro). Full run.
- Recorder: cebolot
- Source: video https://www.youtube.com/watch?v=8dusH3CvGFE
  → https://www.mediafire.com/file/mhwuv4ek8kdjcc5/Amethyst_.gdr/file
- Sanity flags: recorded from a local copy renamed "Amethyst 2" with id 0 (no online ID match).
  Run length matches Amethyst exactly (~81.7 s vs 81.7–81.9 s of the other two), so it is very
  likely the same level, but the id/name mismatch means the project's level-vs-macro check
  must not key on the macro's id field.

---

## Sources checked, nothing downloaded
- Telegram GD Macros channel (12k subs, posts t.me/gdmacros/8517, /8518, /10684):
  "Amethyst 2.gdr" + "Thinking Space II.gdr" by Карельский exist but t.me web preview hides
  document CDN URLs — no direct download without the Telegram API.
- https://www.gdmacros.com/ — 394 macros across 140 levels, xdBot/zBot/MegaHack; returned HTTP 402 on every fetch.
- https://github.com/downupstudio/macrodash — real .gdr/.slc/.ybot/.tcm files but **no Amethyst**
  (has thinking_space_ii.gdr, which is redundant — project already covers Thinking Space II).
- YouTube "GD Macros" playlist showcases confirm the same files, no new sources.

## Recommendation
- **First choice:** `xdBot-Amethyst-pieGD122.gdr2` — only candidate with the correct level
  id + name embedded in a native CBOR .gdr2 file at 240 Hz.
- **Backup:** `xdBot-Amethyst-Abenex.gdr2.json` — most precise (framerate stored explicitly,
  duration↔frame math exact); needs a JSON→CBOR convert step.
- No substitute demon needed: verified-completing Amethyst macros exist. Current top-5
  confirmed unchanged on pointercrate (2026-11-14): **1 GRIEF · 2 Artemis 13 · 3 Society ·
  4 Thinking Space II · 5 Amethyst** (next: 6 Flamewall) — identical to the project's parsed set.

## Not verified (honest limits)
- 0-death claim of any macro: cannot be proven statically. Evidence is indirect: all three
  durations equal the level length and each is published as a full-completion showcase.
  Only cebolot explicitly writes "NOCLIP: no". Abenex says macros were self-checked. None
  were re-played by me (no GD client here).
- Actual gameplay correctness (click timing vs current level version): Amethyst is rated 2.2-era;
  RobTop can tweak hitboxes in updates — test-play the macro in-game before trusting it.
- `.gmd` for level 119550490: no straightforward public mirror found (gdbrowser is viewer-only);
  level ID noted here instead.
