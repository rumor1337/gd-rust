//! How closely one sim tick matches real GD. For every tick of every real
//! xdBot recording (macros/*xdBot.gdr2, which store GD's player position each
//! tick), put player 1 on GD's exact position and velocity, step one tick, and
//! compare the predicted position with GD's next one. Errors are grouped by
//! mode and size; the worst ticks per group are listed so they can be fixed.
//! usage: fidelity [max examples per group]

use gdrust::bot::gdr2::parse_gdr2;
use gdrust::bot::Playback;
use gdrust::level::parse_level;
use gdrust::sim::objects::ObjectTable;
use gdrust::sim::{InputState, RunState, Sim};
use std::collections::BTreeMap;

/// dy = 0.225 * vy (OpenGD's 0.25 tick times the 0.9 vertical factor)
const DZ: f64 = 0.225;

fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let show: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(5);
    let table = ObjectTable::load(std::path::Path::new(&format!("{manifest}/data/objects.json"))).expect("object table");
    // group -> (ticks, ticks off by 0.01..2, ticks off by more, worst small-error examples)
    let mut groups: BTreeMap<String, (usize, usize, usize, Vec<(f64, String)>)> = BTreeMap::new();
    // group -> errors under 0.2 (plain motion): shows a constant bias
    let mut small: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut files: Vec<_> = std::fs::read_dir(format!("{manifest}/macros")).expect("macros/").flatten().map(|e| e.path()).collect();
    files.sort();
    for path in files.iter().filter(|p| p.to_string_lossy().ends_with("xdBot.gdr2")) {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Ok(m) = parse_gdr2(&std::fs::read(path).unwrap()) else { continue };
        let Ok(text) = std::fs::read_to_string(format!("{manifest}/levels/{}.gmd", m.level_id)) else {
            println!("{name}: no level {}", m.level_id);
            continue;
        };
        let level = parse_level(&text);
        let fr = m.frames.clone();
        let fr2 = m.frames_p2.clone();
        let mut pb = Playback::new(m);
        let mut sim = Sim::new(&level, &table);
        sim.follow = true;
        let mut false_kills: Vec<(f64, u16)> = Vec::new();
        for k in 1..fr.len().saturating_sub(1) {
            let (p1, p2) = pb.step();
            if pb.stalled() {
                continue;
            }
            let input = InputState { hold: p2, hold_p2: p1 };
            // GD's state at the end of tick k-1, velocity from its last move
            let (x0, y0) = (fr[k - 1][0] as f64, fr[k - 1][1] as f64);
            let prev_dy = y0 - fr[k.saturating_sub(2)][1] as f64;
            let mut probe = sim.clone();
            probe.apply_fix(x0, y0, fr[k - 1][2] as f64);
            probe.players[0].vel_y = prev_dy / DZ;
            let before = probe.players[0].clone();
            probe.step_single(input);
            let p = &probe.players[0];
            let real_dy = fr[k][1] as f64 - y0;
            let err = (p.y - y0) - real_dy;
            // skip: deaths/respawns (positions jump), and ticks where the
            // recording itself jumps (teleports, respawn)
            // FIDELITY_FLIPS: every gravity flip (portal / blue pad / orb): GD's vy after vs ours
            if before.grav_flip != p.grav_flip && std::env::var("FIDELITY_FLIPS").is_ok() {
                println!("FLIP {name} x {x0:.0} {:?}{} vy before {:.2} | after: real {:.2} ours {:.2}", before.mode, if before.mini { " mini" } else { "" }, before.vel_y, real_dy / DZ, (p.y - y0) / DZ);
            }
            if before.mode != p.mode && std::env::var("FIDELITY_MODES").is_ok() {
                println!("MODE {name} x {x0:.0} {:?}->{:?} flip {} vy before {:.2} | after: real {:.2} ours {:.2}", before.mode, p.mode, before.grav_flip, before.vel_y, real_dy / DZ, (p.y - y0) / DZ);
            }
            let skip = probe.state != RunState::Playing || real_dy.abs() > 40.0 || (fr[k][0] as f64 - x0).abs() > 20.0;
            if !skip {
                let key = format!("{:?}{}{}", before.mode, if before.mini { " mini" } else { "" }, if before.grav_flip { " flipped" } else { "" });
                if err.abs() < 0.2 {
                    // FIDELITY_DUMP=<group>: every inexact plain-motion tick of one group
                    if err.abs() > 0.002 && std::env::var("FIDELITY_DUMP").is_ok_and(|g| g == key) {
                        println!("DUMP {name} x {x0:.0} y {y0:.0} players {} p2flip {:?} tick {k} vy {:.3} hold {p2} real dy {real_dy:.4} ours {:.4} err {err:+.4}", probe.players.len(), probe.players.get(1).map(|q| q.grav_flip), before.vel_y, p.y - y0);
                    }
                    small.entry(key.clone()).or_default().push(err);
                }
                let g = groups.entry(key).or_default();
                g.0 += 1;
                if err.abs() > 2.0 {
                    g.2 += 1; // a snap onto/into something: collision, not motion
                } else if err.abs() > 0.01 {
                    g.1 += 1;
                    g.3.push((
                        err.abs(),
                        format!(
                            "{name} tick {k} x {x0:.0} y {y0:.1} vy {:.2} hold {} | real dy {real_dy:.3} ours {:.3} (vy after: ours {:.2} real {:.2}) ground {}->{} flip {}",
                            before.vel_y,
                            p2,
                            p.y - y0,
                            p.vel_y,
                            real_dy / DZ,
                            before.on_ground,
                            p.on_ground,
                            before.grav_flip
                        ),
                    ));
                }
            }
            // advance the follow sim like the demo does; GD never died in
            // these runs, so any kill our rules report here is a false kill
            sim.killer = None;
            sim.step_single(input);
            if let Some((_, id)) = sim.killer {
                if std::env::var("FIDELITY_KILLS").is_ok() {
                    let q = &sim.players[0];
                    println!("  KILL {name} x {x0:.0} id {id} mode {:?}{}", q.mode, if q.mini { " mini" } else { "" });
                }
                false_kills.push((x0, id));
            }
            sim.apply_fix(fr[k][0] as f64, fr[k][1] as f64, fr[k][2] as f64);
            if let Some(f) = fr2.get(k) {
                sim.apply_fix_p2(f[0] as f64, f[1] as f64, f[2] as f64);
            }
        }
        if std::env::var("FIDELITY_KILLS").is_ok() {
            let mut by: BTreeMap<u16, (usize, f64)> = BTreeMap::new();
            for &(x, id) in &false_kills {
                let e = by.entry(id).or_insert((0, x));
                e.0 += 1;
            }
            println!("  false kills by object id (ticks, first x): {by:?}");
        }
        let first: Vec<String> = false_kills.iter().take(6).map(|(x, id)| format!("x {x:.0} by {id}")).collect();
        println!("{name}: {} ticks our rules would kill on GD's real path; first {first:?}", false_kills.len());
    }
    for (key, (n, bad, snaps, mut ex)) in groups {
        let e = small.get(&key).cloned().unwrap_or_default();
        let mean = e.iter().sum::<f64>() / e.len().max(1) as f64;
        let within = e.iter().filter(|x| x.abs() <= 0.002).count();
        println!(
            "{key}: {n} ticks, {bad} off by 0.01-2 ({:.1}%), {snaps} by more; plain motion: {:.1}% within 0.002, mean error {mean:+.4}",
            100.0 * bad as f64 / n.max(1) as f64,
            100.0 * within as f64 / e.len().max(1) as f64
        );
        ex.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, line) in ex.iter().take(show) {
            println!("   {line}");
        }
    }
}
