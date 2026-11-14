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
/// pixels per world unit: 360u vertical view = the standard GD 12-block
/// height, corridor 90..390 centered
const SCALE: f32 = 2.0;
/// uhd sheets are 4x retina: sprite px per world unit
const SHEET_SCALE: f32 = 4.0;
/// vertical half view in units
const HALF_VIEW_Y: f32 = FRAME_H as f32 / SCALE / 2.0;
/// pre-cull margin for the x scan; exact cull uses real sprite extents after
/// shortcut: objects wider than ~800u can be skipped early, none known in these levels
const CULL_PAD: f32 = 400.0;
/// color channel id of the level background
const BG_CHANNEL: u16 = 1000;

pub struct Renderer {
    store: SpriteStore,
    /// lazily-decoded tiled background (png name, texture)
    bg: Option<(String, Pixmap)>,
    fps: Option<u32>,
    /// color-tinted sprites, cached per (texture, channel id)
    tinted: std::collections::HashMap<(String, u16), Pixmap>,
}

impl Renderer {
    /// Missing sheets degrade to hitbox-outline rendering (see SpriteStore::load).
    pub fn new(assets_dir: &std::path::Path) -> Renderer {
        Renderer {
            store: SpriteStore::load(assets_dir),
            bg: None,
            fps: None,
            tinted: std::collections::HashMap::new(),
        }
    }

    /// Show an fps counter (3x5 digits, top-right).
    pub fn set_fps(&mut self, fps: u32) {
        self.fps = Some(fps);
    }

    /// Render one frame following player 0. Returns straight-alpha RGBA,
    /// `FRAME_W * FRAME_H * 4` bytes (ready for Streamer::submit).
    pub fn draw(&mut self, level: &Level, table: &ObjectTable, sim: &Sim) -> Vec<u8> {
        let mut canvas = Pixmap::new(FRAME_W, FRAME_H).expect("static size");
        let view_w = FRAME_W as f32 / SCALE;

        // camera: x puts player 0 at 40% from the left; y clamps so the
        // 90..390 corridor always fills the screen (up to 20u of bands)
        let (px, py) = sim
            .players
            .first()
            .map(|p| (p.x as f32, p.y as f32))
            .unwrap_or((0.0, 240.0));
        let camx = px - view_w * 0.4;
        let camy = py.clamp(210.0, 270.0);

        // background: level bg channel (id 1000), dark blue fallback
        let bg_rgb = level
            .header
            .channels
            .iter()
            .find(|c| c.id == BG_CHANNEL)
            .map(|c| (c.r, c.g, c.b))
            .unwrap_or((25, 50, 100));
        let bg = Color::from_rgba8(bg_rgb.0, bg_rgb.1, bg_rgb.2, 255);
        canvas.fill(bg);

let to_screen = |wx: f32, wy: f32| -> (f32, f32) {
            ((wx - camx) * SCALE, FRAME_H as f32 * 0.5 - (wy - camy) * SCALE)
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
        if self.bg.as_ref().map(|(n, _)| n != &bg_name).unwrap_or(true) {
            // GD paints texture × bg-channel color (kS38 ch 1000; GRIEF's is
            // black, hence the dark look)
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
            self.bg = tiled.map(|p| (bg_name, p));
        }
        if let Some((_, tex)) = &self.bg {
            let s = 512.0 * SCALE / tex.width() as f32;
            let tile_px = tex.width() as f32 * s;
            let mut paint = PixmapPaint::default();
            paint.quality = FilterQuality::Bilinear;
            let mut tx = -(camx * 0.125) % tile_px - tile_px;
            let (_, ty) = to_screen(0.0, crate::physics::GROUND_Y as f32 + 512.0);
            while tx < FRAME_W as f32 {
                let t = Transform::from_translate(tx, ty).pre_concat(Transform::from_scale(s, s));
                canvas.draw_pixmap(0, 0, tex.as_ref(), &paint, t, None);
                tx += tile_px;
            }
        }

        
        // floor / ceiling bands
        let ground = Color::from_rgba8(12, 12, 20, 255);
        let mut solid = Paint::default();
        solid.anti_alias = false;
        solid.shader = tiny_skia::Shader::SolidColor(ground);
        let (_, gy) = to_screen(0.0, crate::physics::GROUND_Y as f32);
        if let Some(r) = Rect::from_xywh(0.0, gy, FRAME_W as f32, FRAME_H as f32 - gy) {
            canvas.fill_rect(r, &solid, Transform::identity(), None);
        }
        let (_, cy) = to_screen(0.0, crate::physics::CEIL_Y as f32);
        if let Some(r) = Rect::from_xywh(0.0, 0.0, FRAME_W as f32, cy) {
            canvas.fill_rect(r, &solid, Transform::identity(), None);
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
        sprite_paint.quality = FilterQuality::Bilinear;

        // decorations first, gameplay on top (GD layering)
        for deco_pass in [true, false] {
        for obj in &level.objects {
            if is_deco[obj.id as usize & 8191] != deco_pass {
                continue;
            }

            let wx = obj.x;
            let wy = obj.y + 105.0;
            if wx < camx - CULL_PAD || wx > camx + view_w + CULL_PAD {
                continue;
            }
            let id = obj.id as usize;
            let tex = by_tex.get(id).copied().flatten();
            let mut drawn = false;
            // object tint: key 21 names a color channel (GRIEF tints most of
            // the corridor black/gray through 1004-family channels)
            let tint = obj
                .base_color
                .and_then(|cid| {
                    level
                        .header
                        .channels
                        .iter()
                        .find(|c| c.id == cid)
                        .map(|c| (cid, c.r, c.g, c.b))
                })
                .filter(|(_, r, g, b)| (*r, *g, *b) != (255, 255, 255));
            if let Some(tex) = tex {
                // tinted sprites are cached per (texture, channel): the clone +
                // pixel multiply is a one-time cost, else the frame rate dies
                let pixmap: Option<&Pixmap> = match tint {
                    Some((cid, tr, tg, tb)) => {
                        let key = (tex.to_string(), cid);
                        if !self.tinted.contains_key(&key) {
                            if let Some(mut sp) = self.store.sprite(tex).cloned() {
                                // channels are premultiplied; scaling them by
                                // the tint keeps the premultiplied invariant
                                for px in sp.pixels_mut() {
                                    *px = tiny_skia::PremultipliedColorU8::from_rgba(
                                        (px.red() as u16 * tr as u16 / 255) as u8,
                                        (px.green() as u16 * tg as u16 / 255) as u8,
                                        (px.blue() as u16 * tb as u16 / 255) as u8,
                                        px.alpha(),
                                    )
                                    .unwrap();
                                }
                                self.tinted.insert(key.clone(), sp);
                            }
                        }
                        self.tinted.get(&key)
                    }
                    None => self.store.sprite(tex),
                };
                if let Some(sprite) = &pixmap {
                    let kx = if obj.scale_x != 0.0 { obj.scale_x.abs() } else { 1.0 };
                    let ky = if obj.scale_y != 0.0 { obj.scale_y.abs() } else { 1.0 };
                    let ext = sprite
                        .width()
                        .max(sprite.height())
                        .max(1) as f32
                        / SHEET_SCALE
                        * kx.max(ky);
                    if wx + ext < camx
                        || wx - ext > camx + view_w
                        || wy + ext < camy - HALF_VIEW_Y
                        || wy - ext > camy + HALF_VIEW_Y
                    {
                        continue;
                    }
                    let (cx, cy) = to_screen(wx, wy);
                    // texture px → screen: center, scale to units, flip, rotate CCW
                    let t = Transform::from_translate(cx, cy)
                        .pre_concat(Transform::from_rotate(-obj.rot))
                        .pre_concat(Transform::from_scale(
                            if obj.flip_x { -1.0 } else { 1.0 } * SCALE / SHEET_SCALE * kx,
                            if obj.flip_y { -1.0 } else { 1.0 } * SCALE / SHEET_SCALE * ky,
                        ))
                        .pre_concat(Transform::from_translate(
                            -(sprite.width() as f32) * 0.5,
                            -(sprite.height() as f32) * 0.5,
                        ));
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

        // players: the GD dart icon tinted per player (green p0, red p1)
        for (i, p) in sim.players.iter().enumerate() {
            let col = if i == 0 {
                (0u8, 255u8, 60u8)
            } else {
                (255u8, 45u8, 45u8)
            };
            let (cx, cy) = to_screen(p.x as f32, p.y as f32);
            draw_player(&mut canvas, &mut self.store, p, col, cx, cy);
        }

        // fps counter (3x5 digits, top-right, black shadow)
        if let Some(fps) = self.fps {
            const D: [[u8; 5]; 10] = [
                [0b111, 0b101, 0b101, 0b101, 0b111],
                [0b010, 0b110, 0b010, 0b010, 0b111],
                [0b111, 0b001, 0b111, 0b100, 0b111],
                [0b111, 0b001, 0b111, 0b001, 0b111],
                [0b101, 0b101, 0b111, 0b001, 0b001],
                [0b111, 0b100, 0b111, 0b001, 0b111],
                [0b111, 0b100, 0b111, 0b101, 0b111],
                [0b111, 0b001, 0b001, 0b010, 0b010],
                [0b111, 0b101, 0b111, 0b101, 0b111],
                [0b111, 0b101, 0b111, 0b001, 0b111],
            ];
            let digits: Vec<u8> = fps
                .to_string()
                .bytes()
                .map(|b| (b - b'0') as u8)
                .collect();
            let cell = 2.0f32;
            let gw = digits.len() as f32 * (3.0 * cell + cell) - cell;
            let mut x0 = FRAME_W as f32 - gw - 10.0;
            let y0 = 10.0;
            let mut paint = Paint::default();
            paint.anti_alias = false;
            for (shadow, col) in
                [(1.0, Color::from_rgba8(0, 0, 0, 200)), (0.0, Color::from_rgba8(255, 255, 255, 255))]
            {
                let mut x = x0 + shadow;
                for d in &digits {
                    for (row, bits) in D[*d as usize].iter().enumerate() {
                        for col_i in 0..3 {
                            if bits & (1 << (2 - col_i)) != 0 {
                                if let Some(r) = Rect::from_xywh(
                                    x + col_i as f32 * cell,
                                    y0 + shadow + row as f32 * cell,
                                    cell,
                                    cell,
                                ) {
                                    paint.shader = tiny_skia::Shader::SolidColor(col);
                                    canvas.fill_rect(r, &paint, Transform::identity(), None);
                                }
                            }
                        }
                    }
                    x += 4.0 * cell;
                }
            }
            let _ = x0;
        }

        unpremultiply(&canvas)
    }
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
    let Some((sprite, sscale)) = store.sprite_sheet("dart_01_001.png") else {
        // no icon sheet: fall back to a bright triangle
        let col = Color::from_rgba8(tint.0, tint.1, tint.2, 255);
        draw_fallback(canvas, p, col, cx, cy);
        return;
    };
    // icon px → screen: mini wave draws at 0.6× the icon's world size
    let s = SCALE / sscale * if p.mini { 0.6 } else { 1.0 };
    // dart icon art points RIGHT natively; tiny_skia's rotate() turns
    // counter-clockwise on screen, so the plain atan2(vy, vx) aims it
    let vx = p.dx() as f32;
    let vy = p.vel_y as f32;
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
    let mut paint = PixmapPaint::default();
    paint.quality = FilterQuality::Bilinear;
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

        // player 0: 40% from the left at ground level → bright green square
        // (16:9: camx = -256, camy = 210 → spawn (0,99) at (512, 582))
        let (r8, g8, b8, a8) = pixel(&buf, 512, 582);
        assert_eq!((r8, g8, b8, a8), (0, 255, 60, 255));

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
        // A = upper-right of the cell (outside an up-pointing spike), B = lower-right
        let a = (587usize, 555usize);
        let b = (587, 588);
        let buf0 = render(0.0);
        let buf180 = render(180.0);
        // the bg is now a tiled texture — assert the sprite content flips
        // with rotation rather than comparing against a flat color
        assert_ne!(
            pixel(&buf0, a.0, a.1),
            pixel(&buf180, a.0, a.1),
            "tip/base must flip between rot 0 and 180 at the cell's upper-right"
        );
        assert_ne!(
            pixel(&buf0, b.0, b.1),
            pixel(&buf180, b.0, b.1),
            "tip/base must flip between rot 0 and 180 at the cell's lower-right"
        );
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
        std::fs::write("/tmp/opencode/grief.rgba", &buf).unwrap(); // TEMP debug dump
        assert!(buf.iter().any(|&b| b != buf[0]), "frame is uniform");
        // bg channel 1000 must drive the background, not the fallback blue
        let ch = lvl.header.channels.iter().find(|c| c.id == 1000).unwrap();
        let (r8, g8, b8, _) = pixel(&buf, 10, 10);
        assert_eq!((r8, g8, b8), (ch.r, ch.g, ch.b));
    }
}
