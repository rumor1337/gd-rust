// Headless frame dump: render GRIEF at a chosen sim tick to /tmp/opencode/frame.png
// using the real gd-clone assets, for visual inspection.
// usage: frame [tick] [outdir]
use gdrust::bot::gdr2::parse_gdr2;
use gdrust::bot::Playback;
use gdrust::level::parse_level;
use gdrust::render::Renderer;
use gdrust::sim::objects::ObjectTable;
use gdrust::sim::{InputState, Sim};
use tiny_skia::Pixmap;

fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let tick: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(200);
    let level_id: u32 = std::env::var("FRAME_LEVEL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(149992717);
    let mut level = parse_level(
        &std::fs::read_to_string(format!("{manifest}/levels/{level_id}.gmd")).unwrap(),
    );
    level.id = level_id;
    let table =
        ObjectTable::load(std::path::Path::new(&format!("{manifest}/data/objects.json"))).unwrap();
    let m = parse_gdr2(
        &std::fs::read(format!("{manifest}/macros/Cerealmilkbowl-GRIEF-xdBot.gdr2")).unwrap(),
    )
    .unwrap();
    let assets = format!("{}/Documents/gd-clone", std::env::var("HOME").unwrap());
    let mut r = Renderer::new(std::path::Path::new(&assets));

    let mut sim = Sim::new(&level, &table);
    let mut pb = Playback::new(m);
    for _ in 0..tick {
        if sim.state == gdrust::sim::RunState::Dead {
            break;
        }
        let (p1, p2) = pb.step();
        sim.step_single(InputState { hold: p2, hold_p2: p1 });
    }
    let n: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(1);
    let t0 = std::time::Instant::now();
    let mut rgba = Vec::new();
    for _ in 0..n {
        rgba = r.draw(&level, &table, &sim);
    }
    let per = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
    println!("render avg {per:.1} ms/frame over {n}");

    let mut pm = Pixmap::from_vec(
        rgba,
        tiny_skia::IntSize::from_wh(gdrust::render::FRAME_W, gdrust::render::FRAME_H).unwrap(),
    )
    .unwrap();
    std::fs::write("/tmp/opencode/frame.png", pm.encode_png().unwrap()).unwrap();
    println!("wrote /tmp/opencode/frame.png at tick {tick} (x={:.0}, y={:.0}, dead={})", sim.players[0].x, sim.players[0].y, sim.state == gdrust::sim::RunState::Dead);
}
