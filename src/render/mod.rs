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
}

impl Renderer {
    /// Missing sheets degrade to hitbox-outline rendering (see SpriteStore::load).
    pub fn new(assets_dir: &std::path::Path) -> Renderer {
        Renderer { store: SpriteStore::load(assets_dir) }
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
        let bg = level
            .header
            .channels
            .iter()
            .find(|c| c.id == BG_CHANNEL)
            .map(|c| Color::from_rgba8(c.r, c.g, c.b, 255))
            .unwrap_or_else(|| Color::from_rgba8(25, 50, 100, 255));
        canvas.fill(bg);

        let to_screen = |wx: f32, wy: f32| -> (f32, f32) {
            ((wx - camx) * SCALE, FRAME_H as f32 * 0.5 - (wy - camy) * SCALE)
        };

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
        for (k, def) in &table.objects {
            if let Ok(id) = k.parse::<usize>() {
                if id < 8192 {
                    by_tex[id] = def.texture_name.as_deref();
                    by_hb[id] = def.hitbox.as_deref();
                }
            }
        }

        let mut sprite_paint = PixmapPaint::default();
        sprite_paint.quality = FilterQuality::Bilinear;

        for obj in &level.objects {
            let wx = obj.x;
            let wy = obj.y + 105.0;
            if wx < camx - CULL_PAD || wx > camx + view_w + CULL_PAD {
                continue;
            }
            let id = obj.id as usize;
            let tex = by_tex.get(id).copied().flatten();
            let mut drawn = false;
            if let Some(tex) = tex {
                if let Some(sprite) = self.store.sprite(tex) {
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

        // players: bright shapes (atlas icons are a later task)
        for (i, p) in sim.players.iter().enumerate() {
            let col = if i == 0 {
                Color::from_rgba8(0, 255, 60, 255)
            } else {
                Color::from_rgba8(255, 45, 45, 255)
            };
            let (cx, cy) = to_screen(p.x as f32, p.y as f32);
            draw_player(&mut canvas, p, col, cx, cy);
        }

        unpremultiply(&canvas)
    }
}

fn draw_player(canvas: &mut Pixmap, p: &Player, col: Color, cx: f32, cy: f32) {
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
        let bg = (25u8, 50u8, 100u8, 255u8);
        // A = upper-right of the cell (outside an up-pointing spike), B = lower-right
        let a = (587usize, 555usize);
        let b = (587, 588);
        let buf0 = render(0.0);
        assert_eq!(pixel(&buf0, a.0, a.1), bg, "tip must be up at rot 0");
        assert_ne!(pixel(&buf0, b.0, b.1), bg, "base must be drawn at rot 0");
        let buf180 = render(180.0);
        assert_ne!(pixel(&buf180, a.0, a.1), bg, "tip must be down at rot 180");
        assert_eq!(pixel(&buf180, b.0, b.1), bg, "top must be empty at rot 180");
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
