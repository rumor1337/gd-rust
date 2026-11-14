//! Windowed visual demo: auto-plays the GRIEF macro through the sim and
//! renders it with the atlas renderer in a minifb window.
//!
//! Run (never against the default target dir — the main session builds there):
//!   CARGO_TARGET_DIR=/tmp/opencode/demo-target cargo run --release --bin demo
//! DEMO_DUMP=1 also writes one mid-run frame to /tmp/opencode/demo-tick240.rgba.

use gdrust::bot::gdr2::parse_gdr2;
use gdrust::bot::Playback;
use gdrust::level::parse_level;
use gdrust::render::{Renderer, FRAME_H, FRAME_W};
use gdrust::sim::objects::ObjectTable;
use gdrust::sim::{InputState, RunState, Sim};
use minifb::{Key, KeyRepeat, Scale, Window, WindowOptions};

fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let level =
        parse_level(&std::fs::read_to_string(format!("{manifest}/levels/149992717.gmd")).unwrap());
    let table =
        ObjectTable::load(std::path::Path::new(&format!("{manifest}/data/objects.json"))).unwrap();
    let m = parse_gdr2(
        &std::fs::read(format!("{manifest}/macros/Cerealmilkbowl-GRIEF-xdBot.gdr2")).unwrap(),
    )
    .expect("gdr2 parse");
    println!("macro: {} fps={} inputs={}", m.level_name, m.framerate, m.inputs.len());

    let mut sim = Sim::new(&level, &table);
    let mut playback = Playback::new(m.clone());
    let mut renderer = Renderer::new(std::path::Path::new(&format!("{manifest}/assets")));

    let mut window = Window::new(
        "gdrust demo — GRIEF auto",
        FRAME_W as usize,
        FRAME_H as usize,
        WindowOptions { scale: Scale::FitScreen, ..WindowOptions::default() },
    )
    .expect("open window");
    window.set_target_fps(60); // each frame = 4 sim substeps = 240Hz sim

    let mut screen = vec![0u32; (FRAME_W * FRAME_H) as usize];
    let mut deaths = 0u32;
    let mut prev_state = sim.state;
    let mut tick = 0u64;
    let dump = std::env::var_os("DEMO_DUMP").is_some();

    loop {
        if !window.is_open() {
            break;
        }
        let keys = window.get_keys_pressed(KeyRepeat::No);
        if keys.contains(&Key::Q) {
            break;
        }
        // R: restart run from the level start (and replay the macro)
        if keys.contains(&Key::R) {
            sim.reset();
            playback = Playback::new(m.clone());
        }

        // 4 substeps per 60fps frame. SWAP wiring (calibrate.rs): macro p2
        // drives sim player 0. Deaths auto-respawn after the sim's pause;
        // the macro keeps playing, which visually restarts the run.
        for _ in 0..4 {
            let (m_p1, m_p2) = playback.step();
            sim.step_single(InputState { hold: m_p2, hold_p2: m_p1 });
            tick += 1;
        }
        if prev_state == RunState::Dead && sim.state == RunState::Playing {
            deaths += 1;
        }
        prev_state = sim.state;

        let mut rgba = renderer.draw(&level, &table, &sim);
        hud(&mut rgba, sim.attempt, sim.progress(), deaths);
        if dump && tick == 240 {
            std::fs::write("/tmp/opencode/demo-tick240.rgba", &rgba).unwrap();
        }
        for (dst, px) in screen.iter_mut().zip(rgba.chunks_exact(4)) {
            *dst = (255u32 << 24) | ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | px[2] as u32;
        }
        window.update_with_buffer(&screen, FRAME_W as usize, FRAME_H as usize).unwrap();

        // 1Hz status line for headless verification
        if tick % 240 == 0 {
            eprintln!(
                "t={tick} attempt={} progress={:.1}% deaths={deaths} state={:?}",
                sim.attempt,
                sim.progress(),
                sim.state
            );
        }
    }
}

// --- HUD: tiny 3x5 bitmap font drawn straight into the RGBA buffer ---

const WHITE: [u8; 3] = [255, 255, 255];
const AMBER: [u8; 3] = [255, 210, 100];

const FONT: &[(char, u16)] = &[
    ('0', 0b111_101_101_101_111),
    ('1', 0b010_110_010_010_111),
    ('2', 0b111_001_111_100_111),
    ('3', 0b111_001_111_001_111),
    ('4', 0b101_101_111_001_001),
    ('5', 0b111_100_111_001_111),
    ('6', 0b111_100_111_101_111),
    ('7', 0b111_001_001_010_010),
    ('8', 0b111_101_111_101_111),
    ('9', 0b111_101_111_001_111),
    ('A', 0b111_101_111_101_101),
    ('B', 0b110_101_110_101_110),
    ('C', 0b111_100_100_100_111),
    ('D', 0b110_101_101_101_110),
    ('E', 0b111_100_111_100_111),
    ('F', 0b111_100_111_100_100),
    ('G', 0b111_100_101_101_111),
    ('H', 0b101_101_111_101_101),
    ('I', 0b111_010_010_010_111),
    ('K', 0b101_101_110_101_101),
    ('L', 0b100_100_100_100_111),
    ('M', 0b101_111_111_101_101),
    ('N', 0b110_101_101_101_101),
    ('O', 0b111_101_101_101_111),
    ('P', 0b111_101_111_100_100),
    ('Q', 0b111_101_101_111_001),
    ('R', 0b111_101_110_101_101),
    ('S', 0b111_100_111_001_111),
    ('T', 0b111_010_010_010_010),
    ('U', 0b101_101_101_101_111),
    ('V', 0b101_101_101_101_010),
    ('W', 0b101_101_111_111_101),
    ('X', 0b101_101_010_101_101),
    ('Y', 0b101_101_010_010_010),
    ('Z', 0b111_001_010_100_111),
    ('%', 0b101_001_010_100_101),
    ('.', 0b000_000_000_000_010),
    ('=', 0b000_111_000_111_000),
    (' ', 0),
];

fn glyph(c: char) -> u16 {
    let c = c.to_ascii_uppercase();
    FONT.iter().find(|(k, _)| *k == c).map(|(_, v)| *v).unwrap_or(0)
}

/// 3x5 bitmap text; `scale` px per font pixel, 1px gap between glyphs.
fn draw_text(rgba: &mut [u8], x0: usize, y0: usize, text: &str, col: [u8; 3], scale: usize) {
    let w = FRAME_W as usize;
    let h = FRAME_H as usize;
    let put = |rgba: &mut [u8], x: usize, y: usize, col: [u8; 3]| {
        if x < w && y < h {
            let i = (y * w + x) * 4;
            rgba[i] = col[0];
            rgba[i + 1] = col[1];
            rgba[i + 2] = col[2];
            rgba[i + 3] = 255;
        }
    };
    for (n, ch) in text.chars().enumerate() {
        let g = glyph(ch);
        for row in 0..5 {
            for c in 0..3 {
                if g >> (14 - (row * 3 + c)) & 1 == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        let x = x0 + (n * 4 + c) * scale + sx;
                        let y = y0 + row * scale + sy;
                        // black offset first for readability, then the color
                        put(rgba, x + 1, y + 1, [0, 0, 0]);
                        put(rgba, x, y, col);
                    }
                }
            }
        }
    }
}

fn hud(rgba: &mut [u8], attempt: u32, progress: f64, deaths: u32) {
    let w = FRAME_W as usize;
    // progress bar strip across the top
    let filled = ((progress / 100.0) * w as f64) as usize;
    for y in 0..6 {
        for x in 0..w {
            let (r, g, b) = if x < filled { (0u8, 220u8, 80u8) } else { (15u8, 15u8, 30u8) };
            let i = (y * w + x) * 4;
            rgba[i] = r;
            rgba[i + 1] = g;
            rgba[i + 2] = b;
            rgba[i + 3] = 255;
        }
    }
    let line = format!("ATTEMPT {attempt}  {progress:.1}%  DEATHS {deaths}");
    draw_text(rgba, 8, 12, &line, WHITE, 2);
    draw_text(rgba, 8, 30, "R = RESTART   Q = QUIT", AMBER, 2);
}
