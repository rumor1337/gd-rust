//! 16:9 visual demo: the GRIEF macro replayed in a local window.
//!
//! Assets from ~/Documents/gd-clone (Resources/ resolved by SpriteStore),
//! macro p2 drives sim p0 (SWAP wiring, same as calibrate). R restarts the
//! attempt, Escape/Q quits. Runs 4 sim ticks (240Hz) per displayed frame
//! (~60fps). Progress/attempt live in the window title.

use gdrust::bot::gdr2::parse_gdr2;
use gdrust::bot::Playback;
use gdrust::level::parse_level;
use gdrust::render::{Renderer, FRAME_H, FRAME_W};
use gdrust::sim::objects::ObjectTable;
use gdrust::sim::{InputState, Sim};
use minifb::{Key, Window, WindowOptions};

fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let mut level = parse_level(
        &std::fs::read_to_string(format!("{manifest}/levels/149992717.gmd")).unwrap(),
    );
    level.id = 149992717;
    let table =
        ObjectTable::load(std::path::Path::new(&format!("{manifest}/data/objects.json"))).unwrap();
    let m = parse_gdr2(
        &std::fs::read(format!("{manifest}/macros/Cerealmilkbowl-GRIEF-xdBot.gdr2")).unwrap(),
    )
    .expect("gdr2 parse");

    let assets = std::env::var("DEMO_ASSETS").unwrap_or_else(|_| {
        format!("{}/Documents/gd-clone", std::env::var("HOME").unwrap())
    });
    let mut renderer = Renderer::new(std::path::Path::new(&assets));

    let mut sim = Sim::new(&level, &table);
    let mut pb = Playback::new(m.clone());

    let mut window = Window::new(
        "GRIEF — gd-rust demo",
        FRAME_W as usize,
        FRAME_H as usize,
        WindowOptions::default(),
    )
    .expect("window");
    window.set_target_fps(60);
    let mut fb = vec![0u32; FRAME_W as usize * FRAME_H as usize];
    let mut last = std::time::Instant::now();
    let mut fps_smooth = 60.0f32;

    while window.is_open() {
        // 4 sim ticks per displayed frame = 240Hz at 60fps (step_single =
        // one 240Hz tick; the macro advances one frame per tick)
        for _ in 0..4 {
            // death: restart immediately (the sim's internal respawn would
            // otherwise resume the macro at the death frame, not frame 0)
            if sim.state == gdrust::sim::RunState::Dead {
                sim.reset();
                pb = Playback::new(m.clone());
            }
            let (p1, p2) = pb.step();
            // SWAP wiring: macro p2 (active player) drives sim p0
            sim.step_single(InputState { hold: p2, hold_p2: p1 });
        }
        if window.is_key_pressed(Key::R, minifb::KeyRepeat::No) {
            sim.reset();
            pb = Playback::new(m.clone());
        }
        if window.is_key_pressed(Key::Escape, minifb::KeyRepeat::No)
            || window.is_key_pressed(Key::Q, minifb::KeyRepeat::No)
        {
            break;
        }

        let rgba = renderer.draw(&level, &table, &sim);
        let dt = last.elapsed().as_secs_f32();
        last = std::time::Instant::now();
        if dt > 0.0 {
            fps_smooth = fps_smooth * 0.9 + (1.0 / dt) * 0.1;
        }
        renderer.set_fps(fps_smooth as u32);
        for (i, px) in fb.iter_mut().enumerate() {
            let b = rgba[i * 4];
            let g = rgba[i * 4 + 1];
            let r = rgba[i * 4 + 2];
            *px = (r as u32) << 16 | (g as u32) << 8 | b as u32;
        }
        window.update_with_buffer(&fb, FRAME_W as usize, FRAME_H as usize);
        window.set_title(&format!(
            "GRIEF — attempt {} — {:.2}% — x {:.0}  [R restart, Q quit]",
            sim.attempt,
            sim.progress(),
            sim.players.first().map(|p| p.x).unwrap_or(0.0)
        ));
    }
}
