//! Fidelity check on Auto levels: GD finishes them with no input, so any
//! death here is a sim bug. Prints per level how far it got and, on death,
//! where and what killed it.
//! usage: autocheck <level id>... (levels/<id>.gmd, see tools/fetch_level.py)

use gdrust::level::parse_level;
use gdrust::sim::objects::ObjectTable;
use gdrust::sim::{InputState, RunState, Sim};

fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let table = ObjectTable::load(std::path::Path::new(&format!("{manifest}/data/objects.json"))).expect("object table");
    let mut failed = 0;
    for id in std::env::args().skip(1) {
        let Ok(text) = std::fs::read_to_string(format!("{manifest}/levels/{id}.gmd")) else {
            println!("{id}: no levels/{id}.gmd");
            continue;
        };
        let level = parse_level(&text);
        let (score, raw) = gdrust::bot::auto::difficulty(&level, &table);
        println!("{id}: difficulty {score:.2} (raw {raw:.3})");
        let mut sim = Sim::new(&level, &table);
        let mut path = std::collections::VecDeque::new();
        // 10 minutes of ticks is far past any auto level
        for _ in 0..240 * 600 {
            let p = &sim.players[0];
            path.push_back((p.x, p.y, p.vel_y, p.on_ground, p.grav_flip));
            if path.len() > 200 {
                path.pop_front();
            }
            sim.step_single(InputState::single(false));
            if sim.state != RunState::Playing || sim.progress() >= 100.0 {
                break;
            }
        }
        let p = &sim.players[0];
        if sim.state == RunState::Dead {
            failed += 1;
            println!(
                "{id}: DIES at {:.1}% (tick {}, x {:.0}, y {:.0}, mode {:?}, killer {:?})",
                sim.progress(),
                sim.frame,
                p.x,
                p.y,
                p.mode,
                sim.killer
            );
            if std::env::var_os("AUTOCHECK_TRACE").is_some() {
                // every 8th tick of the last 200, then the objects around the death
                for (x, y, vy, g, f) in path.iter().step_by(8) {
                    println!("  x {x:.1} y {y:.1} vy {vy:.2} ground {g} flip {f}");
                }
                for o in sim.grid.all().iter().filter(|o| (o.x - p.x).abs() < 70.0 && (o.y - p.y).abs() < 70.0) {
                    println!("  obj {} at ({:.1}, {:.1})", o.id, o.x, o.y);
                }
            }
        } else {
            println!("{id}: ok {:.1}% ({:?})", sim.progress(), sim.state);
        }
    }
    std::process::exit(failed);
}
