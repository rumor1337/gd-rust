//! Level renderer: 720×1280 RGBA frames from a Level + ObjectTable + Sim via
//! tiny-skia software raster (Task 7).

pub mod atlas;

use crate::level::object::GdObject;
use crate::level::Level;
use crate::sim::objects::{sim_object, ObjectTable};
use crate::sim::player::{GameMode, Player};
use crate::sim::Sim;
use atlas::SpriteStore;
use tiny_skia::{
    Color, FillRule, FilterQuality, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke,
    Transform,
};

pub const FRAME_W: u32 = 1280;
pub const FRAME_H: u32 = 720;
/// internal render resolution (upscaled ×2 on output — 4× fewer pixels)
const RW: u32 = 640;
const RH: u32 = 360;
/// pixels per world unit on the internal canvas: 360u vertical view
const SCALE: f32 = 1.0;
/// uhd sheets are 4x retina: sprite px per world unit
const SHEET_SCALE: f32 = 4.0;
/// vertical half view in units
const HALF_VIEW_Y: f32 = RH as f32 / SCALE / 2.0;
/// pre-cull margin for the x scan; exact cull uses real sprite extents after
/// shortcut: objects wider than ~800u can be skipped early, none known in these levels
const CULL_PAD: f32 = 400.0;
/// color channel id of the level background
const BG_CHANNEL: u16 = 1000;

pub struct Renderer {
    store: SpriteStore,
    /// lazily-decoded tiled background (png name, texture)
    bg: Option<(String, (u8, u8, u8), Pixmap)>,
    fps: Option<u32>,
    /// prescaled sprites (extracted, tinted, rotated, downscaled to canvas
    /// resolution) cached per (texture, channel, rot, flips, scale)
    prescaled: std::collections::HashMap<
        (String, u32, i32, bool, bool, u64, u64),
        Option<(Pixmap, i32, i32)>,
    >,
    /// per-player recent world positions for the wave trail
    trails: Vec<Vec<(f64, f64)>>,
    /// cached decoration-layer slabs (512u wide, world y -60..964)
    deco_tiles: std::collections::HashMap<i32, Pixmap>,
    /// objects sorted by x, cached per level (invalidated by pointer)
    sorted: Option<Vec<(f32, usize)>>,
    sorted_key: usize,
    /// rot≈0 sprites: extracted, tinted, mirrored, downscaled to canvas
    /// resolution — drawn as translate-only blits (the fast path)
    fast: std::collections::HashMap<(String, u32, bool, bool), Option<Pixmap>>,
    /// live color channels, driven by the level's own color triggers
    colors: Option<ColorEngine>,
    colors_key: usize,
}

/// One placed color trigger (899 / legacy 29|30): x-cross → lerp a channel.
#[derive(Clone)]
struct ColorCmd {
    x: f32,
    ch: u16,
    to: (f32, f32, f32),
    dur: f32,
}

struct Transition {
    ch: u16,
    from: (f32, f32, f32),
    to: (f32, f32, f32),
    t: f32,
    dur: f32,
}

/// Evaluates the level's color triggers (ids 899 / 29 / 30) against the
/// leading player x, so every level's palette comes from its own data.
/// GD's key map: 23 = target channel, 7/8/9 = RGB, 10 = duration.
/// Pulse (1006) / Alpha (1007) triggers and per-channel opacity are not
/// evaluated — channels only.
struct ColorEngine {
    chans: std::collections::HashMap<u16, (f32, f32, f32)>,
    cmds: Vec<ColorCmd>,
    cursor: usize,
    live: Vec<Transition>,
    last_frame: u64,
}

impl ColorEngine {
    fn new(level: &crate::level::Level) -> ColorEngine {
        let mut chans = std::collections::HashMap::new();
        for c in &level.header.channels {
            chans.insert(c.id, (c.r as f32, c.g as f32, c.b as f32));
        }
        let raw_kv = |o: &crate::level::GdObject, k: u16| -> Option<f32> {
            o.raw.iter().find(|(kk, _)| *kk == k).and_then(|(_, v)| v.parse().ok())
        };
        let mut cmds: Vec<ColorCmd> = level
            .objects
            .iter()
            .filter(|o| matches!(o.id, 899 | 29 | 30))
            .filter_map(|o| {
                Some(ColorCmd {
                    x: o.x,
                    ch: raw_kv(o, 23)? as u16,
                    to: (
                        raw_kv(o, 7).unwrap_or(255.0),
                        raw_kv(o, 8).unwrap_or(255.0),
                        raw_kv(o, 9).unwrap_or(255.0),
                    ),
                    dur: raw_kv(o, 10).unwrap_or(0.0),
                })
            })
            .collect();
        cmds.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        ColorEngine { chans, cmds, cursor: 0, live: Vec::new(), last_frame: 0 }
    }

    fn step(&mut self, player_x: f64, frame: u64) {
        let dt = (frame.saturating_sub(self.last_frame)) as f32 / 240.0;
        self.last_frame = frame;
        while self.cursor < self.cmds.len()
            && (self.cmds[self.cursor].x as f64) <= player_x
        {
            let c = &self.cmds[self.cursor];
            let from = self.chans.get(&c.ch).copied().unwrap_or((255.0, 255.0, 255.0));
            self.live.push(Transition { ch: c.ch, from, to: c.to, t: 0.0, dur: c.dur });
            self.cursor += 1;
        }
        // disjoint field borrows; later transitions overwrite earlier ones
        let (live, chans) = (&mut self.live, &mut self.chans);
        live.retain_mut(|tr| {
            tr.t += dt;
            let k = if tr.dur <= 0.0 { 1.0 } else { (tr.t / tr.dur).min(1.0) };
            let col = (
                tr.from.0 + (tr.to.0 - tr.from.0) * k,
                tr.from.1 + (tr.to.1 - tr.from.1) * k,
                tr.from.2 + (tr.to.2 - tr.from.2) * k,
            );
            chans.insert(tr.ch, col);
            tr.t < tr.dur
        });
    }

    fn snapshot(&self) -> std::collections::HashMap<u16, (u8, u8, u8)> {
        self.chans
            .iter()
            .map(|(k, (r, g, b))| (*k, (*r as u8, *g as u8, *b as u8)))
            .collect()
    }
}

impl Renderer {
    /// Missing sheets degrade to hitbox-outline rendering (see SpriteStore::load).
    pub fn new(assets_dir: &std::path::Path) -> Renderer {
        Renderer {
            store: SpriteStore::load(assets_dir),
            bg: None,
            fps: None,
            prescaled: std::collections::HashMap::new(),
            trails: Vec::new(),
            deco_tiles: std::collections::HashMap::new(),
            sorted: None,
            sorted_key: 0,
            fast: std::collections::HashMap::new(),
            colors: None,
            colors_key: 0,
        }
    }

    /// Show an fps counter (3x5 digits, top-right).
    pub fn set_fps(&mut self, fps: u32) {
        self.fps = Some(fps);
    }

    /// Render one frame following player 0. Returns straight-alpha RGBA,
    /// `FRAME_W * FRAME_H * 4` bytes (ready for Streamer::submit).
    pub fn draw(&mut self, level: &Level, table: &ObjectTable, sim: &Sim) -> Vec<u8> {
        let prof = std::env::var("PROFILE").is_ok();
        let t_start = std::time::Instant::now();
        let mut canvas = Pixmap::new(RW, RH).expect("static size");
        let view_w = RW as f32 / SCALE;

        // camera: x puts player 0 at 40% from the left; y clamps so the
        // 90..390 corridor always fills the screen (up to 20u of bands)
        let (px, py) = sim
            .players
            .first()
            .map(|p| (p.x as f32, p.y as f32))
            .unwrap_or((0.0, 240.0));
        let camx = px - view_w * 0.4;
        let camy = py.clamp(210.0, 270.0);

        // live channel colors: driven by the level's own color triggers
        // (899/29/30), evaluated against the leading player x
        let lead_x = sim
            .players
            .iter()
            .map(|p| p.x)
            .fold(f64::MIN, f64::max);
        if self.colors_key != level.objects.as_ptr() as usize {
            self.colors = Some(ColorEngine::new(level));
            self.colors_key = level.objects.as_ptr() as usize;
        }
        let colors = self.colors.as_mut().expect("color engine");
        colors.step(lead_x, sim.frame);
        let chan_map = colors.snapshot();
        let chan = move |cid: u16| -> (u8, u8, u8) {
            chan_map.get(&cid).copied().unwrap_or((255, 255, 255))
        };
        let bg_rgb = chan(1000);
        let bg = Color::from_rgba8(bg_rgb.0, bg_rgb.1, bg_rgb.2, 255);
        canvas.fill(bg);

let to_screen = |wx: f32, wy: f32| -> (f32, f32) {
            ((wx - camx) * SCALE, RH as f32 * 0.5 - (wy - camy) * SCALE)
        };

        // tiled background texture: kA6 bg id → game_bg_NN-uhd; one 1x
        // texture tile spans 512u, slow horizontal parallax, bottom of the
        // tile row anchored at the floor line
        let bg_id: u32 = level
            .header
            .ka
            .iter()
            .find(|(k, _)| k == "kA6")
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(1);
        let bg_name = format!("game_bg_{bg_id:02}_001-uhd.png");
        // reload when the texture name OR the live channel tint changes
        // (otherwise the first untinted load gets cached and the bg reads
        // blue through the gradient)
        if self
            .bg
            .as_ref()
            .map(|(n, t, _)| n != &bg_name || t != &bg_rgb)
            .unwrap_or(true)
        {
            let tint = bg_rgb;
            let tiled = Pixmap::load_png(self.store.dir().join(&bg_name)).ok().map(|mut p| {
                if tint != (255, 255, 255) {
                    for px in p.pixels_mut() {
                        *px = tiny_skia::PremultipliedColorU8::from_rgba(
                            (px.red() as u16 * tint.0 as u16 / 255) as u8,
                            (px.green() as u16 * tint.1 as u16 / 255) as u8,
                            (px.blue() as u16 * tint.2 as u16 / 255) as u8,
                            px.alpha(),
                        )
                        .unwrap_or(*px);
                    }
                }
                p
            });
            self.bg = tiled.map(|p| (bg_name, tint, p));
        }
        if let Some((_, _, tex)) = &self.bg {
            let s = 512.0 * SCALE / tex.width() as f32;
            let tile_px = tex.width() as f32 * s;
            let mut paint = PixmapPaint::default();
            paint.quality = FilterQuality::Nearest;
            let mut tx = -(camx * 0.125) % tile_px - tile_px;
            let (_, ty) = to_screen(0.0, crate::physics::GROUND_Y as f32 + 512.0);
            while tx < FRAME_W as f32 {
                let t = Transform::from_translate(tx, ty).pre_concat(Transform::from_scale(s, s));
                canvas.draw_pixmap(0, 0, tex.as_ref(), &paint, t, None);
                tx += tile_px;
            }
        }

        
        // GRIEF-style depth gradient, derived from the live bg channel
        let band_h = RH as f32 / 36.0;
        for b in 0..36 {
            let t = b as f32 / 35.0;
            let m = 0.12 + t * 0.33; // darker up top, brighter toward ground
            let (r, g, bl) = (
                (bg_rgb.0 as f32 * m) as u8,
                (bg_rgb.1 as f32 * m) as u8,
                (bg_rgb.2 as f32 * m) as u8,
            );
            let mut band = Paint::default();
            band.anti_alias = false;
            band.shader =
                tiny_skia::Shader::SolidColor(Color::from_rgba8(r, g, bl, 235));
            if let Some(rect) = Rect::from_xywh(0.0, b as f32 * band_h, RW as f32, band_h + 1.0) {
                canvas.fill_rect(rect, &band, Transform::identity(), None);
            }
        }

        // ground / ceiling bands: live ground channel 1001; surface line =
        // line channel 1002 (GD mirrors ground color on the ceiling)
        let ground_rgb = chan(1001);
        let mut solid = Paint::default();
        solid.anti_alias = false;
        solid.shader = tiny_skia::Shader::SolidColor(Color::from_rgba8(
            ground_rgb.0, ground_rgb.1, ground_rgb.2, 255,
        ));
        let ceil = Paint::default();
        let mut ceil = ceil;
        ceil.anti_alias = false;
        ceil.shader = tiny_skia::Shader::SolidColor(Color::from_rgba8(
            ground_rgb.0, ground_rgb.1, ground_rgb.2, 255,
        ));
        let (_, gy) = to_screen(0.0, crate::physics::GROUND_Y as f32);
        if let Some(r) = Rect::from_xywh(0.0, gy, RW as f32, RH as f32 - gy) {
            canvas.fill_rect(r, &solid, Transform::identity(), None);
        }
        let (_, cy) = to_screen(0.0, crate::physics::CEIL_Y as f32);
        if let Some(r) = Rect::from_xywh(0.0, 0.0, RW as f32, cy) {
            canvas.fill_rect(r, &ceil, Transform::identity(), None);
        }
        // bright surface line (line channel)
        let line_rgb = chan(1002);
        let mut line = Paint::default();
        line.anti_alias = false;
        line.shader = tiny_skia::Shader::SolidColor(Color::from_rgba8(
            line_rgb.0, line_rgb.1, line_rgb.2, 230,
        ));
        if let Some(r) = Rect::from_xywh(0.0, gy - 2.0, RW as f32, 2.0) {
            canvas.fill_rect(r, &line, Transform::identity(), None);
        }
        if let Some(r) = Rect::from_xywh(0.0, cy, RW as f32, 2.0) {
            canvas.fill_rect(r, &line, Transform::identity(), None);
        }

        // id → texture name / hitbox, built once per frame (levels repeat ids heavily)
        let mut by_tex: Vec<Option<&str>> = vec![None; 8192];
        let mut by_hb: Vec<Option<&[f64]>> = vec![None; 8192];
        let mut is_deco: Vec<bool> = vec![false; 8192];
        for (k, def) in &table.objects {
            if let Ok(id) = k.parse::<usize>() {
                if id < 8192 {
                    by_tex[id] = def.texture_name.as_deref();
                    by_hb[id] = def.hitbox.as_deref();
                    // GD object_type 7 = decoration: drawn behind gameplay
                    is_deco[id] = def.object_type == 7;
                }
            }
        }

        let mut sprite_paint = PixmapPaint::default();
        sprite_paint.quality = FilterQuality::Nearest;

        let t_bg = t_start.elapsed();
        let skip_deco = std::env::var("SKIP_DECO").is_ok();
        if self.sorted_key != level.objects.as_ptr() as usize {
            let mut v: Vec<(f32, usize)> =
                level.objects.iter().enumerate().map(|(i, o)| (o.x, i)).collect();
            v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            self.sorted = Some(v);
            self.sorted_key = level.objects.as_ptr() as usize;
        }
        let sorted = self.sorted.as_ref().unwrap();
        let x_lo = camx - CULL_PAD;
        let x_hi = camx + view_w + CULL_PAD;
        let lo = sorted.partition_point(|(x, _)| *x < x_lo);
        let hi = sorted.partition_point(|(x, _)| *x <= x_hi);
        if !skip_deco {
            const TILE_W: f32 = 512.0;
            let t_lo = ((camx - CULL_PAD) / TILE_W).floor() as i32;
            let t_hi = ((camx + view_w + CULL_PAD) / TILE_W).floor() as i32;
            if self.deco_tiles.len() > 24 {
                self.deco_tiles.clear(); // crude cap; hitches only on long levels
            }
            for t in t_lo..=t_hi {
                if self.deco_tiles.contains_key(&t) {
                    continue;
                }
                let mut tile = Pixmap::new(TILE_W as u32, 1024).expect("tile");
                let tx0 = t as f32 * TILE_W;
                let tile_screen = |wx: f32, wy: f32| -> (f32, f32) {
                    (wx - tx0, 1024.0 - (wy + 60.0))
                };
                let tx_lo = tx0 - 400.0;
                let tx_hi = tx0 + TILE_W + 400.0;
                let lo = sorted.partition_point(|(x, _)| *x < tx_lo);
                let hi = sorted.partition_point(|(x, _)| *x <= tx_hi);
                for (_, oi) in &sorted[lo..hi] {
                    let obj = &level.objects[*oi];
                    if !is_deco[obj.id as usize & 8191] {
                        continue;
                    }
                    let wx = obj.x;
                    let wy = obj.y + 105.0;
                    let id = obj.id as usize;
                    let Some(tex) = by_tex.get(id).copied().flatten() else { continue };
                    // GD default: objects without key 21 use the "obj"
                    // channel 1004
                    let tint = chan(obj.base_color.unwrap_or(1004));
                    let tint = if tint != (255, 255, 255) { Some(tint) } else { None };
                    let kx = if obj.scale_x != 0.0 { obj.scale_x.abs() } else { 1.0 };
                    let ky = if obj.scale_y != 0.0 { obj.scale_y.abs() } else { 1.0 };
                    let fx = obj.flip_x;
                    let fy = obj.flip_y;
                    let rot_milli = (obj.rot * 1000.0).round() as i32;
                    let key2 = (
                        tex.to_string(),
                        tint.map(|t| (t.0 as u32) << 16 | (t.1 as u32) << 8 | t.2 as u32).unwrap_or(u32::MAX),
                        rot_milli,
                        fx,
                        fy,
                        (kx.to_bits() as u64) << 32 | ky.to_bits() as u64,
                        0,
                    );
                    if !self.prescaled.contains_key(&key2) {
                        let built = self.store.sprite(tex).and_then(|sp| {
                            let dw = (sp.width() as f32 * 0.25 * kx).ceil() as u32 + 2;
                            let dh = (sp.height() as f32 * 0.25 * ky).ceil() as u32 + 2;
                            let mut off = Pixmap::new(dw, dh)?;
                            let mut tinted = sp.clone();
                            if let Some((tr, tg, tb)) = tint {
                                for px in tinted.pixels_mut() {
                                    *px = tiny_skia::PremultipliedColorU8::from_rgba(
                                        (px.red() as u16 * tr as u16 / 255) as u8,
                                        (px.green() as u16 * tg as u16 / 255) as u8,
                                        (px.blue() as u16 * tb as u16 / 255) as u8,
                                        px.alpha(),
                                    )
                                    .unwrap();
                                }
                            }
                            let mut paint = PixmapPaint::default();
                            paint.quality = FilterQuality::Bilinear;
                            let t = Transform::from_translate(dw as f32 / 2.0, dh as f32 / 2.0)
                                .pre_concat(Transform::from_rotate(obj.rot))
                                .pre_concat(Transform::from_scale(
                                    (if fx { -1.0 } else { 1.0 })
                                        * 0.25
                                        * SCALE
                                        / SHEET_SCALE
                                        * kx,
                                    (if fy { -1.0 } else { 1.0 })
                                        * 0.25
                                        * SCALE
                                        / SHEET_SCALE
                                        * ky,
                                ))
                                .pre_concat(Transform::from_translate(
                                    -(tinted.width() as f32) * 0.5,
                                    -(tinted.height() as f32) * 0.5,
                                ));
                            off.draw_pixmap(0, 0, tinted.as_ref(), &paint, t, None);
                            // alpha-bounds crop
                            let (w, h) = (off.width() as usize, off.height() as usize);
                            let data = off.data();
                            let (mut x0, mut y0, mut x1, mut y1) =
                                (w as i64, h as i64, -1i64, -1i64);
                            for j in 0..h {
                                for i in 0..w {
                                    if data[(j * w + i) * 4 + 3] != 0 {
                                        if (i as i64) < x0 { x0 = i as i64; }
                                        if (i as i64) > x1 { x1 = i as i64; }
                                        if (j as i64) < y0 { y0 = j as i64; }
                                        if (j as i64) > y1 { y1 = j as i64; }
                                    }
                                }
                            }
                            if x1 < 0 {
                                return None;
                            }
                            let cw = (x1 - x0 + 1) as u32;
                            let ch = (y1 - y0 + 1) as u32;
                            let mut crop = Pixmap::new(cw, ch)?;
                            for j in 0..ch as usize {
                                let s = ((y0 as usize + j) * w + x0 as usize) * 4;
                                let d = j as usize * cw as usize * 4;
                                crop.data_mut()[d..d + cw as usize * 4]
                                    .copy_from_slice(&data[s..s + cw as usize * 4]);
                            }
                            Some((crop, x0 as i32, y0 as i32))
                        });
                        self.prescaled.insert(key2.clone(), built);
                    }
                    let Some((sprite, ox, oy)) = self.prescaled.get(&key2).and_then(|o| o.as_ref())
                    else {
                        continue;
                    };
                    let (cx, cy) = tile_screen(wx, wy);
                    let tx = (cx - sprite.width() as f32 * 0.5).round() + *ox as f32;
                    let ty = (cy - sprite.height() as f32 * 0.5).round() + *oy as f32;
                    let t = Transform::from_translate(tx, ty);
                    canvas.draw_pixmap(0, 0, sprite.as_ref(), &sprite_paint, t, None);
                }
                self.deco_tiles.insert(t, tile);
            }
            // blit the visible slabs
            let mut paint = PixmapPaint::default();
            paint.quality = FilterQuality::Nearest;
            for t in t_lo..=t_hi {
                let Some(tile) = self.deco_tiles.get(&t) else { continue };
                let sx = (t as f32 * TILE_W - camx) as f32;
                let sy = RH as f32 / 2.0 - (964.0 - camy) * SCALE;
                let t = Transform::from_translate(sx, sy);
                canvas.draw_pixmap(0, 0, tile.as_ref(), &paint, t, None);
            }
        }
        // decorations: static per level — render 512u slabs once into cached
        // bitmaps (world y -60..964) and blit the visible ones

        let prof_n = std::env::var("PROFILE").is_ok();
        let (mut n_fast, mut n_slow, mut n_out, mut n_win) = (0u32, 0u32, 0u32, 0u32);
        {
        for (_, oi) in &sorted[lo..hi] {
            let obj = &level.objects[*oi];
            if is_deco[obj.id as usize & 8191] {
                continue; // decorations come from the cached slabs
            }

            let wx = obj.x;
            let wy = obj.y + 105.0;
            n_win += 1;
            let id = obj.id as usize;
            let tex = by_tex.get(id).copied().flatten();
            let mut drawn = false;
            // object tint: key 21 names a color channel; GD default is the
            // "obj" channel 1004 — all colors live via the ColorEngine
            let tint = chan(obj.base_color.unwrap_or(1004));
            let tint = if tint != (255, 255, 255) { Some(tint) } else { None };
            if let Some(tex) = tex {
                let kx = if obj.scale_x != 0.0 { obj.scale_x.abs() } else { 1.0 };
                let ky = if obj.scale_y != 0.0 { obj.scale_y.abs() } else { 1.0 };
                // prescaled cache: every (texture, tint, rot, flip, scale)
                // combination is rendered once into a canvas-resolution
                // pixmap; per frame it is only an integer-translate blit
                let fx = obj.flip_x;
                let fy = obj.flip_y;
                let rot_milli = (obj.rot * 1000.0).round() as i32;
                let key2 = (
                    tex.to_string(),
                    tint.map(|t| (t.0 as u32) << 16 | (t.1 as u32) << 8 | t.2 as u32).unwrap_or(u32::MAX),
                    rot_milli,
                    fx,
                    fy,
                    (kx.to_bits() as u64) << 32 | ky.to_bits() as u64,
                    0,
                );
                if !self.prescaled.contains_key(&key2) {
                    // render the full transform once into a margin'd canvas,
                    // then crop to the alpha bounding box (huge deco sprites
                    // carry large transparent margins — blitting those was
                    // the frame-time killer)
                    let built = self.store.sprite(tex).and_then(|sp| {
                        let dw = (sp.width() as f32 * 0.25 * kx).ceil() as u32 + 2;
                        let dh = (sp.height() as f32 * 0.25 * ky).ceil() as u32 + 2;
                        let mut off = Pixmap::new(dw, dh)?;
                        let mut tinted = sp.clone();
                        if let Some((tr, tg, tb)) = tint {
                            for px in tinted.pixels_mut() {
                                *px = tiny_skia::PremultipliedColorU8::from_rgba(
                                    (px.red() as u16 * tr as u16 / 255) as u8,
                                    (px.green() as u16 * tg as u16 / 255) as u8,
                                    (px.blue() as u16 * tb as u16 / 255) as u8,
                                    px.alpha(),
                                )
                                .unwrap();
                            }
                        }
                        let mut paint = PixmapPaint::default();
                        paint.quality = FilterQuality::Bilinear;
                        let t = Transform::from_translate(dw as f32 / 2.0, dh as f32 / 2.0)
                            .pre_concat(Transform::from_rotate(obj.rot))
                            .pre_concat(Transform::from_scale(
                                (if fx { -1.0 } else { 1.0 }) * 0.25 * SCALE / SHEET_SCALE * kx,
                                (if fy { -1.0 } else { 1.0 }) * 0.25 * SCALE / SHEET_SCALE * ky,
                            ))
                            .pre_concat(Transform::from_translate(
                                -(tinted.width() as f32) * 0.5,
                                -(tinted.height() as f32) * 0.5,
                            ));
                        off.draw_pixmap(0, 0, tinted.as_ref(), &paint, t, None);
                        // alpha-bounds crop
                        let (w, h) = (off.width() as usize, off.height() as usize);
                        let data = off.data();
                        let (mut x0, mut y0, mut x1, mut y1) =
                            (w as i64, h as i64, -1i64, -1i64);
                        for j in 0..h {
                            for i in 0..w {
                                if data[(j * w + i) * 4 + 3] != 0 {
                                    if (i as i64) < x0 { x0 = i as i64; }
                                    if (i as i64) > x1 { x1 = i as i64; }
                                    if (j as i64) < y0 { y0 = j as i64; }
                                    if (j as i64) > y1 { y1 = j as i64; }
                                }
                            }
                        }
                        if x1 < 0 {
                            return None; // fully transparent
                        }
                        let cw = (x1 - x0 + 1) as u32;
                        let ch = (y1 - y0 + 1) as u32;
                        let mut crop = Pixmap::new(cw, ch)?;
                        for j in 0..ch as usize {
                            let s = ((y0 as usize + j) * w + x0 as usize) * 4;
                            let d = j as usize * cw as usize * 4;
                            crop.data_mut()[d..d + cw as usize * 4]
                                .copy_from_slice(&data[s..s + cw as usize * 4]);
                        }
                        Some((crop, x0 as i32, y0 as i32))
                    });
                    self.prescaled.insert(key2.clone(), built);
                }
                let pixmap: Option<&(Pixmap, i32, i32)> =
                    self.prescaled.get(&key2).and_then(|o| o.as_ref());
                let dw = (self.store.sprite(tex).map(|s| s.width() as f32).unwrap_or(0.0)
                    * 0.25
                    * kx)
                    .ceil() as f32
                    + 2.0;
                let dh = (self.store.sprite(tex).map(|s| s.height() as f32).unwrap_or(0.0)
                    * 0.25
                    * ky)
                    .ceil() as f32
                    + 2.0;
                if let Some((sprite, ox, oy)) = &pixmap {
                    let ext = sprite.width().max(sprite.height()).max(1) as f32;
                    if wx + ext < camx
                        || wx - ext > camx + view_w
                        || wy + ext < camy - HALF_VIEW_Y
                        || wy - ext > camy + HALF_VIEW_Y
                    {
                        continue;
                    }
                    let (cx, cy) = to_screen(wx, wy);
                    let tx = (cx - dw as f32 / 2.0).round() + *ox as f32;
                    let ty = (cy - dh as f32 / 2.0).round() + *oy as f32;
                    let t = Transform::from_translate(tx, ty);
                    canvas.draw_pixmap(0, 0, sprite.as_ref(), &sprite_paint, t, None);
                    drawn = true;
                }
            }
            if !drawn {
                // no texture: outline the hitbox so nothing silently disappears
                if let Some(hb) = by_hb.get(id).copied().flatten() {
                    if hb.len() == 4 {
                        let so = sim_object(obj, table);
                        let (rx, ry, rw, rh) = so.rect;
                        let (x0, y0) = to_screen(rx as f32, ry as f32 + rh as f32);
                        let wpx = rw as f32 * SCALE;
                        let hpx = rh as f32 * SCALE;
                        if x0 + wpx < 0.0 || x0 > FRAME_W as f32 || hpx <= 0.0 || wpx <= 0.0 {
                            continue;
                        }
                        if let Some(r) = Rect::from_xywh(x0, y0, wpx, hpx) {
                            let mut pb = PathBuilder::new();
                            pb.push_rect(r);
                            if let Some(path) = pb.finish() {
                                let mut paint = Paint::default();
                                paint.shader = tiny_skia::Shader::SolidColor(
                                    Color::from_rgba8(255, 255, 255, 110),
                                );
                                let stroke = Stroke { width: 2.0, ..Default::default() };
                                canvas.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
                            }
                        }
                    }
                }
            }
        }
        }

        let t_objs = t_start.elapsed() - t_bg;
        // wave trails + players: the GD dart icon tinted per player
        // (green p0, red p1); trail = glow stroke + bright core along the
        // recent positions, tinted with the same player color
        while self.trails.len() < sim.players.len() {
            self.trails.push(Vec::new());
        }
        for (i, p) in sim.players.iter().enumerate() {
            let col = if i == 0 {
                // GRIEF reference: pink/magenta wave dart, white trail
                (255u8, 45u8, 140u8)
            } else {
                (255u8, 120u8, 190u8)
            };
            // trail points in world coords; a >60u jump means respawn — reset
            let trail = &mut self.trails[i];
            match trail.last() {
                Some(&(lx, ly)) if (p.x - lx).abs() > 60.0 => *trail = vec![(p.x, p.y)],
                _ => {
                    trail.push((p.x, p.y));
                    if trail.len() > 90 {
                        trail.remove(0);
                    }
                }
            }
            if trail.len() >= 2 {
                let mut pb = PathBuilder::new();
                let (x0, y0) = to_screen(trail[0].0 as f32, trail[0].1 as f32);
                pb.move_to(x0, y0);
                for (wx, wy) in trail.iter().skip(1) {
                    let (x, y) = to_screen(*wx as f32, *wy as f32);
                    pb.line_to(x, y);
                }
                if let Some(path) = pb.finish() {
                    let mut glow = Paint::default();
                    glow.shader =
                        tiny_skia::Shader::SolidColor(Color::from_rgba8(col.0, col.1, col.2, 110));
                    let stroke =
                        Stroke { width: 9.0, line_join: tiny_skia::LineJoin::Miter, ..Default::default() };
                    canvas.stroke_path(&path, &glow, &stroke, Transform::identity(), None);
                    let mut core = Paint::default();
                    // GRIEF reference: bright WHITE core over pink glow
                    core.shader =
                        tiny_skia::Shader::SolidColor(Color::from_rgba8(255, 255, 255, 255));
                    let core_stroke =
                        Stroke { width: 3.5, line_join: tiny_skia::LineJoin::Miter, ..Default::default() };
                    canvas.stroke_path(&path, &core, &core_stroke, Transform::identity(), None);
                }
            }
            let (cx, cy) = to_screen(p.x as f32, p.y as f32);
            draw_player(&mut canvas, &mut self.store, p, col, cx, cy);
        }

        // hud: progress bar + %, fps, death overlay (attempt text + effect)
        let prog = sim.progress();
        let bar_w = (RW as f32 * 0.6) as f32;
        let mut paint = Paint::default();
        paint.anti_alias = false;
        if let Some(r) = Rect::from_xywh((RW as f32 - bar_w) / 2.0, 4.0, bar_w, 3.0) {
            paint.shader = tiny_skia::Shader::SolidColor(Color::from_rgba8(255, 255, 255, 40));
            canvas.fill_rect(r, &paint, Transform::identity(), None);
        }
        if let Some(r) = Rect::from_xywh(
            (RW as f32 - bar_w) / 2.0,
            4.0,
            bar_w * (prog / 100.0).clamp(0.0, 1.0) as f32,
            3.0,
        ) {
            paint.shader = tiny_skia::Shader::SolidColor(Color::from_rgba8(0, 255, 60, 220));
            canvas.fill_rect(r, &paint, Transform::identity(), None);
        }
        let pct = format!("{:.2}%", prog);
        draw_text(&mut canvas, &pct, RW as f32 - 8.0, 12.0, 2.0, Align::Right, (255, 255, 255));
        if let Some(fps) = self.fps {
            draw_text(
                &mut canvas,
                &fps.to_string(),
                RW as f32 - 8.0,
                4.0,
                2.0,
                Align::Right,
                (200, 200, 200),
            );
        }
        if sim.state == crate::sim::RunState::Dead {
            // GD death overlay: attempt text + expanding burst in player color
            let p = &sim.players[0];
            let (dx, dy) = to_screen(p.x as f32, p.y as f32);
            let prog_d = (sim.dead_frames as f32 / 120.0).clamp(0.0, 1.0);
            let fade = (1.0 - prog_d) as u8;
            let (cr, cg, cb) = if p.grav_flip { (255, 45, 45) } else { (0, 255, 60) };
            let ring_r = 6.0 + prog_d * 90.0;
            let mut ring = Paint::default();
            ring.shader =
                tiny_skia::Shader::SolidColor(Color::from_rgba8(cr, cg, cb, (fade / 2).max(1)));
            let stroke = Stroke { width: 3.0, ..Default::default() };
            if let Some(path) = tiny_skia::PathBuilder::from_circle(dx, dy, ring_r) {
                canvas.stroke_path(&path, &ring, &stroke, Transform::identity(), None);
            }
            for i in 0..14 {
                let ang = (i as f32 * 25.7 + sim.attempt as f32 * 13.1).to_radians();
                let dist = 10.0 + prog_d * (30.0 + (i % 5) as f32 * 14.0);
                let px = dx + ang.cos() * dist;
                let py = dy + ang.sin() * dist;
                paint.shader = tiny_skia::Shader::SolidColor(Color::from_rgba8(cr, cg, cb, fade));
                if let Some(r) = Rect::from_xywh(px - 1.5, py - 1.5, 3.0, 3.0) {
                    canvas.fill_rect(r, &paint, Transform::identity(), None);
                }
            }
            let att = format!("ATTEMPT {}", sim.attempt);
            draw_text(
                &mut canvas,
                &att,
                RW as f32 / 2.0,
                RH as f32 * 0.3,
                3.0,
                Align::Center,
                (255, 255, 255),
            );
        }

        let t_pl = t_start.elapsed() - t_bg - t_objs;

        unpremultiply(&canvas);
        let t_un = t_start.elapsed() - t_bg - t_objs - t_pl;

        // upscale 2× into the output framebuffer (nearest — crisp pixels)
        let src = canvas.data();
        let mut out = vec![0u8; (FRAME_W * FRAME_H * 4) as usize];
        let (ow, oh) = (FRAME_W as usize, FRAME_H as usize);
        for y in 0..RH as usize {
            let row = &src[y * RW as usize * 4..(y + 1) * RW as usize * 4];
            for dy in 0..2 {
                let orow = &mut out[(y * 2 + dy) * ow * 4..(y * 2 + dy + 1) * ow * 4];
                for x in 0..RW as usize {
                    let p = &row[x * 4..x * 4 + 4];
                    orow[x * 8..x * 8 + 4].copy_from_slice(p);
                    orow[x * 8 + 4..x * 8 + 8].copy_from_slice(p);
                }
            }
        }
        if prof_n {
        }
        if prof {
            let t_all = t_start.elapsed();
            eprintln!("prof: bg {:?} objs {:?} players {:?} unprem {:?} upscale {:?} total {:?}",
                t_bg, t_objs, t_pl, t_un, t_all - t_bg - t_objs - t_pl - t_un, t_all);
        }
        out
    }

    fn _unused_prof(&self) {}
}

fn draw_player(
    canvas: &mut Pixmap,
    store: &mut SpriteStore,
    p: &Player,
    tint: (u8, u8, u8),
    cx: f32,
    cy: f32,
) {
    let half = p.outer_half() as f32 * SCALE;
    // glow layer fetched first (white), then the tinted base dart
    let glow = store
        .sprite_sheet("dart_01_glow_001.png")
        .map(|(gpx, gs)| (gpx.clone(), gs));
    let Some((sprite, sscale)) = store.sprite_sheet("dart_01_001.png") else {
        // no icon sheet: fall back to a bright triangle
        let col = Color::from_rgba8(tint.0, tint.1, tint.2, 255);
        draw_fallback(canvas, p, col, cx, cy);
        return;
    };
    // icon px → screen: mini wave draws at 0.6× the icon's world size
    let s = SCALE / sscale * if p.mini { 0.6 } else { 1.0 };
    /// dart icon art points RIGHT natively; aim along the ACTUAL per-tick
    /// movement (vel_y is zeroed/re-set by slope rides, which lies)
    let vx = p.dx() as f32;
    let vy = p.last_move_dy as f32;
    let deg = vy.atan2(vx).to_degrees();
    let mut tinted = sprite.clone();
    for px in tinted.pixels_mut() {
        let c = px.demultiply();
        *px = tiny_skia::PremultipliedColorU8::from_rgba(
            (c.red() as u16 * tint.0 as u16 / 255) as u8,
            (c.green() as u16 * tint.1 as u16 / 255) as u8,
            (c.blue() as u16 * tint.2 as u16 / 255) as u8,
            c.alpha(),
        )
        .unwrap();
    }
    let t = Transform::from_translate(cx, cy)
        .pre_concat(Transform::from_rotate(-deg))
        .pre_concat(Transform::from_scale(s, s))
        .pre_concat(Transform::from_translate(
            -(tinted.width() as f32) * 0.5,
            -(tinted.height() as f32) * 0.5,
        ));
    if let Some((gpx, gs)) = &glow {
        // glow sprite: same geometry, its own sheet scale, drawn white-ish
        let g = SCALE / gs * if p.mini { 0.6 } else { 1.0 };
        let gt = Transform::from_translate(cx, cy)
            .pre_concat(Transform::from_rotate(-deg))
            .pre_concat(Transform::from_scale(g, g))
            .pre_concat(Transform::from_translate(
                -(gpx.width() as f32) * 0.5,
                -(gpx.height() as f32) * 0.5,
            ));
        let mut gpaint = PixmapPaint::default();
        gpaint.quality = FilterQuality::Nearest;
        gpaint.opacity = 0.85;
        canvas.draw_pixmap(0, 0, gpx.as_ref(), &gpaint, gt, None);
    }
    let mut paint = PixmapPaint::default();
    paint.quality = FilterQuality::Nearest;
    canvas.draw_pixmap(0, 0, tinted.as_ref(), &paint, t, None);
    let _ = half;
}

fn draw_fallback(canvas: &mut Pixmap, p: &Player, col: Color, cx: f32, cy: f32) {
    let half = p.outer_half() as f32 * SCALE;
    let mut paint = Paint::default();
    paint.shader = tiny_skia::Shader::SolidColor(col);

    let (path, t) = match p.mode {
        GameMode::Wave => {
            // dart pointing along the velocity
            let vx = p.dx() as f32;
            let vy = p.vel_y as f32;
            let deg = vy.atan2(vx).to_degrees();
            let mut pb = PathBuilder::new();
            pb.move_to(half * 1.4, 0.0);
            pb.line_to(-half * 0.9, half * 0.7);
            pb.line_to(-half * 0.9, -half * 0.7);
            pb.close();
            (pb.finish(), Transform::from_translate(cx, cy).pre_concat(Transform::from_rotate(-deg)))
        }
        GameMode::Ship | GameMode::Ufo | GameMode::Swing => {
            square_path(half * 0.7, cx, cy, 0.0)
        }
        GameMode::Cube | GameMode::Robot => {
            // tumble while airborne, land flat
            let deg = if p.on_ground { 0.0 } else { (p.x as f32 + 20.0) * 0.9 };
            square_path(half, cx, cy, deg)
        }
        GameMode::Ball => square_path(half, cx, cy, p.x as f32 * 2.0),
        GameMode::Spider => square_path(half, cx, cy, 0.0),
    };
    if let Some(path) = path {
        canvas.fill_path(&path, &paint, FillRule::Winding, t, None);
    }
}

fn square_path(half: f32, cx: f32, cy: f32, deg: f32) -> (Option<tiny_skia::Path>, Transform) {
    let mut pb = PathBuilder::new();
    if let Some(r) = Rect::from_xywh(-half, -half, half * 2.0, half * 2.0) {
        pb.push_rect(r);
    }
    (
        pb.finish(),
        Transform::from_translate(cx, cy).pre_concat(Transform::from_rotate(deg)),
    )
}

/// tiny-skia stores premultiplied RGBA; ffmpeg rawvideo wants straight alpha.
#[derive(Clone, Copy, PartialEq)]
enum Align {
    Left,
    Right,
    Center,
}

/// 3x5 glyphs for 0-9, A T E M P % and space
fn glyph(ch: u8) -> Option<[u8; 5]> {
    Some(match ch {
        b'0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        b'1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        b'2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        b'3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        b'4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        b'5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        b'6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        b'7' => [0b111, 0b001, 0b001, 0b010, 0b010],
        b'8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        b'9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        b'A' => [0b111, 0b101, 0b111, 0b101, 0b101],
        b'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        b'E' => [0b111, 0b100, 0b111, 0b100, 0b111],
        b'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        b'P' => [0b111, 0b101, 0b111, 0b100, 0b100],
        b'%' => [0b101, 0b001, 0b010, 0b100, 0b101],
        b'.' => [0b000, 0b000, 0b000, 0b000, 0b010],
        b' ' => [0b000, 0b000, 0b000, 0b000, 0b000],
        _ => return None,
    })
}

/// draw `text` with the 3x5 font; (x, y) is the anchor per `align`
fn draw_text(
    canvas: &mut Pixmap,
    text: &str,
    x: f32,
    y: f32,
    cell: f32,
    align: Align,
    col: (u8, u8, u8),
) {
    let gw: f32 = text.len() as f32 * (4.0 * cell) - cell;
    let x0 = match align {
        Align::Left => x,
        Align::Right => x - gw,
        Align::Center => x - gw / 2.0,
    };
    let mut paint = Paint::default();
    paint.anti_alias = false;
    for (shadow, col) in [
        (1.0, Color::from_rgba8(0, 0, 0, 200)),
        (0.0, Color::from_rgba8(col.0, col.1, col.2, 255)),
    ] {
        let mut cx = x0 + shadow;
        for ch in text.bytes() {
            if let Some(rows) = glyph(ch.to_ascii_uppercase()) {
                for (row, bits) in rows.iter().enumerate() {
                    for col_i in 0..3 {
                        if bits & (1 << (2 - col_i)) != 0 {
                            if let Some(r) = Rect::from_xywh(
                                cx + col_i as f32 * cell,
                                y + shadow + row as f32 * cell,
                                cell,
                                cell,
                            ) {
                                paint.shader = tiny_skia::Shader::SolidColor(col);
                                canvas.fill_rect(r, &paint, Transform::identity(), None);
                            }
                        }
                    }
                }
            }
            cx += 4.0 * cell;
        }
    }
}

fn unpremultiply(pm: &Pixmap) -> Vec<u8> {
    let mut out = pm.data().to_vec();
    for px in out.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 0 || a == 255 {
            continue;
        }
        let un = |c: u8| ((c as u32 * 255 + a / 2) / a).min(255) as u8;
        px[0] = un(px[0]);
        px[1] = un(px[1]);
        px[2] = un(px[2]);
    }
    out
}

/// One-shot convenience: builds a Renderer over `assets_dir` and renders a frame.
pub fn render_frame(
    assets_dir: &std::path::Path,
    level: &Level,
    table: &ObjectTable,
    sim: &Sim,
) -> Vec<u8> {
    Renderer::new(assets_dir).draw(level, table, sim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::parse_level;
    use std::path::{Path, PathBuf};

    fn assets() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets")
    }

    fn table() -> ObjectTable {
        ObjectTable::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("data/objects.json")).unwrap()
    }

    fn synth(objs: Vec<(u16, f32, f32, f32)>) -> Level {
        let mut lvl = Level::default();
        for (id, x, y, rot) in objs {
            lvl.objects.push(GdObject {
                id,
                x,
                y,
                rot,
                scale_x: 1.0,
                scale_y: 1.0,
                ..GdObject::default()
            });
        }
        lvl
    }

    fn pixel(buf: &[u8], x: usize, y: usize) -> (u8, u8, u8, u8) {
        let i = (y * FRAME_W as usize + x) * 4;
        (buf[i], buf[i + 1], buf[i + 2], buf[i + 3])
    }

    #[test]
    fn texture_lookup_for_common_ids() {
        let mut store = SpriteStore::load(&assets());
        let t = table();
        for (id, name, w, h) in
            [(1u16, "square_01_001", 120u32, 120u32), (8, "spike_01_001", 120, 120), (218, "colorSpike_03_001", 80, 76)]
        {
            let tex = t.objects.get(&id.to_string()).unwrap().texture_name.clone().unwrap();
            assert_eq!(tex, name);
            let pm = store.sprite(tex.as_str()).unwrap();
            assert_eq!((pm.width(), pm.height()), (w, h));
        }
    }

    #[test]
    fn renders_synthetic_frame() {
        let lvl = synth(vec![(1, 60.0, 0.0, 0.0), (8, 120.0, 0.0, 0.0)]);
        let t = table();
        let sim = Sim::new(&lvl, &t);
        let mut r = Renderer::new(&assets());
        let buf = r.draw(&lvl, &t, &sim);
        assert_eq!(buf.len(), (FRAME_W * FRAME_H * 4) as usize);
        assert!(buf.iter().any(|&b| b != buf[0]), "frame is uniform");

        // player 0: 40% from the left at ground level → pink dart icon
        // (16:9: camx = -256, camy = 210 → spawn (0,99) at (512, 582))
        let (r8, g8, b8, a8) = pixel(&buf, 512, 582);
        assert_eq!((r8, g8, b8, a8), (255, 45, 140, 255));

        // block 1 at world x=60 (y=105): dark square_01 sprite over the bg
        let (br, bgc, bb, _) = pixel(&buf, 632, 570);
        let (fr, fg, fb, _) = pixel(&buf, 632, 300);
        assert!(br != fr || bgc != fg || bb != fb, "block not drawn");
    }

    #[test]
    fn object_rotation_flips_spike_on_screen() {
        // spike_01_001's tip points up; rot=180 (world CCW) must point it down
        let t = table();
        let mut r = Renderer::new(&assets());
        // spike world center (30, 105) → screen (572, 570) (16:9 camera);
        // 30u sprite = 60px cell → x [542, 602], y [540, 600]
        let mut render = |rot: f32| -> Vec<u8> {
            let lvl = synth(vec![(8, 30.0, 0.0, rot)]);
            let sim = Sim::new(&lvl, &t);
            r.draw(&lvl, &t, &sim)
        };
        let buf0 = render(0.0);
        let buf180 = render(180.0);
        // the bg is a tiled texture — assert the sprite flips by counting
        // differing pixels across the whole 60px cell
        let mut diff = 0usize;
        for y in (536..604).step_by(2) {
            for x in (538..606).step_by(2) {
                if pixel(&buf0, x, y) != pixel(&buf180, x, y) {
                    diff += 1;
                }
            }
        }
        assert!(diff > 40, "rot 0 vs 180 must flip the sprite (diff={diff})");
    }

    #[test]
    fn renders_grief_first_frame() {
        let s = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("levels/149992717.gmd"),
        )
        .unwrap();
        let lvl = parse_level(&s);
        let t = table();
        let sim = Sim::new(&lvl, &t);
        let mut r = Renderer::new(&assets());
        let buf = r.draw(&lvl, &t, &sim);
        assert_eq!(buf.len(), (FRAME_W * FRAME_H * 4) as usize);
        // bg channel 1000 drives the background: GRIEF's opening 899 sets it
        // to (0,0,0) instantly, so the top-left pixel is near-black
        let (r8, g8, b8, _) = pixel(&buf, 10, 10);
        assert!(r8 < 130 && g8 < 40, "bg must come from GRIEF's own 899 trigger data, got ({r8},{g8},{b8})");
    }
}
