//! Replay the GRIEF macro and compare the sim, tick by tick, with the real
//! player positions xdBot recorded (frame fixes). Prints the first tick
//! where the sim leaves the real path and a window of rows around it.
//!
//! usage: truth [tolerance=0.5] [context=12]

use gdrust::bot::gdr2::parse_gdr2;
use gdrust::bot::Playback;
use gdrust::level::parse_level;
use gdrust::sim::objects::ObjectTable;
use gdrust::sim::{InputState, RunState, Sim};

fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let mut args = std::env::args().skip(1);
    let tol: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0.5);
    let ctx: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(12);
    let level = parse_level(&std::fs::read_to_string(format!("{manifest}/levels/149992717.gmd")).unwrap());
    let table = ObjectTable::load(std::path::Path::new(&format!("{manifest}/data/objects.json"))).unwrap();
    let m = parse_gdr2(&std::fs::read(format!("{manifest}/macros/Cerealmilkbowl-GRIEF-xdBot.gdr2")).unwrap()).unwrap();
    let truth = m.frames.clone();
    let mut sim = Sim::new(&level, &table);
    let mut pb = Playback::new(m);
    let mut rows = Vec::new();
    let mut first_bad = None;
    // frames[k] is the position after k ticks (frames[0] = spawn)
    for k in 1..truth.len() {
        let (a, b) = pb.step();
        if !pb.stalled() {
            sim.step_single(InputState { hold: b, hold_p2: a });
        }
        let p = &sim.players[0];
        let t = truth[k];
        let (dx, dy) = (p.x - t[0] as f64, p.y - t[1] as f64);
        rows.push(format!(
            "t={k:5} hold={} sim=({:9.3},{:8.3}) real=({:9.3},{:8.3}) d=({:+.3},{:+.3}) mode={:?} mini={} flip={} ground={}",
            (a || b) as u8, p.x, p.y, t[0], t[1], dx, dy, p.mode, p.mini, p.grav_flip, p.on_ground
        ));
        if first_bad.is_none() && (dx.abs() > tol || dy.abs() > tol || sim.state != RunState::Playing) {
            first_bad = Some(k);
        }
        if let Some(f) = first_bad {
            if k >= f + ctx || sim.state != RunState::Playing {
                break;
            }
        }
    }
    let f = first_bad.unwrap_or(rows.len());
    println!("first divergence > {tol}: tick {f} of {} (state {:?}, killer {:?})", truth.len() - 1, sim.state, sim.killer);
    for r in &rows[f.saturating_sub(ctx + 1)..] {
        println!("{r}");
    }
    // objects near the real position at the divergence
    let t = truth[f.min(truth.len() - 1)];
    let (tx, ty) = (t[0] as f64, t[1] as f64);
    for o in sim.grid.all() {
        let (rx, ry, rw, rh) = o.rect;
        if rx < tx + 40.0 && rx + rw > tx - 40.0 && ry < ty + 40.0 && ry + rh > ty - 40.0 {
            println!(
                "  obj src={} id={} {:?} at ({:.1},{:.1}) rot={} rect=({:.1},{:.1},{:.1},{:.1}) slope={:?} groups={:?}",
                o.src, o.id, o.kind, o.x, o.y, o.rot, rx, ry, rw, rh, o.slope.map(|s| s.surf), o.groups
            );
        }
    }
}
