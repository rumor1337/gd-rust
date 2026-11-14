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
    let level = parse_level(
        &std::fs::read_to_string(format!("{manifest}/levels/149992717.gmd")).unwrap(),
    );
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
    let rgba = r.draw(&level, &table, &sim);
    let mut pm = Pixmap::from_vec(
        rgba,
        tiny_skia::IntSize::from_wh(gdrust::render::FRAME_W, gdrust::render::FRAME_H).unwrap(),
    )
    .unwrap();
    std::fs::write("/tmp/opencode/frame.png", pm.encode_png().unwrap()).unwrap();
    println!("wrote /tmp/opencode/frame.png at tick {tick} (x={:.0}, y={:.0}, dead={})", sim.players[0].x, sim.players[0].y, sim.state == gdrust::sim::RunState::Dead);
}
