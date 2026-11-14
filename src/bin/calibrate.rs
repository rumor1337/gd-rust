//! GRIEF macro calibration: replay the xdBot 240fps macro against the sim and
//! compare the sim's first death with the macro's recorded first death.
//!
//! Oracle: macros/Cerealmilkbowl-GRIEF-xdBot.gdr2 (real GD 2.2 recording).
//! Model: 1 macro frame = 1 sim substep (240Hz).

use gdrust::bot::gdr2::parse_gdr2;
use gdrust::bot::Playback;
use gdrust::level::parse_level;
use gdrust::sim::objects::ObjectTable;
use gdrust::sim::{InputState, Sim};

fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let level = parse_level(&std::fs::read_to_string(format!("{manifest}/levels/149992717.gmd")).unwrap());
    let table = ObjectTable::load(std::path::Path::new(&format!("{manifest}/data/objects.json"))).unwrap();
    let macro_bytes = std::fs::read(format!("{manifest}/macros/Cerealmilkbowl-GRIEF-xdBot.gdr2")).unwrap();
    let m = parse_gdr2(&macro_bytes).expect("gdr2 parse");

    println!(
        "macro: level {} ({}) fps={} inputs={} deaths={} duration={:.1}s",
        m.level_id, m.level_name, m.framerate, m.inputs.len(), m.deaths.len(), m.duration
    );
    println!("header: mode={} mini={} flip={} speed(kA4)={}", level.header.start_gamemode, level.header.start_mini, level.header.start_flip, level.header.start_speed);

    let first_death = m.deaths.first().copied().unwrap_or(u64::MAX);
    println!("macro first death @ frame {first_death} (x unknown, progress unknown)");

    let mut sim = Sim::new(&level, &table);
    let mut pb = Playback::new(m.clone());

    let noswap = std::env::var("CAL_NOSWAP").is_ok();
    let shift: i64 = std::env::var("CAL_INPUT_SHIFT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    // ruleset toggles removed: wave/slope/solid/hazard rules are now the
    // exact GD port (readable decomp); CAL_TRACE remains for forensics
    println!("wiring: {}", if noswap { "NOSWAP" } else { "SWAP" });

    // print the first 6 input events for sanity
    for i in m.inputs.iter().take(6) {
        println!("  input: frame={} down={} button={}", i.frame, i.down, i.button);
    }

    let mut death_tick: Option<u64> = None;
    let mut completed = false;
    let total = pb.duration_frames().max(6000);
    let mut last_report = 0u64;
    let (lead, lag) = (shift.max(0) as u64, (-shift).max(0) as u64);
    let mut shift_q: std::collections::VecDeque<(bool, bool)> =
        (0..lag).map(|_| (false, false)).collect();
    let event_lag: usize = std::env::var("CAL_EVENT_LAG")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut lag_q: std::collections::VecDeque<(bool, bool)> =
        (0..event_lag).map(|_| (false, false)).collect();
    for _ in 0..lead {
        pb.step();
    }
    println!(
        "wiring: {} shift={}",
        if noswap { "noswap (p1->p0)" } else { "swap (p2->p0)" },
        shift
    );
    let mut seen: Vec<(gdrust::sim::player::GameMode, bool, bool)> = sim
        .players
        .iter()
        .map(|p| (p.mode, p.grav_flip, p.mini))
        .collect();
    for tick in 0..=total {
        if sim.state == gdrust::sim::RunState::Dead && death_tick.is_none() {
            death_tick = Some(tick);
            let p = &sim.players[0];
            eprintln!("SIM DIED @ tick {tick}: p0 x={:.2} y={:.2} vel={:.3} mode={:?} mini={} progress={:.2}%", p.x, p.y, p.vel_y, p.mode, p.mini, sim.progress());
            if let Some((pi, kid)) = sim.killer {
                eprintln!("  killer: player{pi} hit object id={kid}");
            }
            for (pi, pl) in sim.players.iter().enumerate() {
                eprintln!("  player{pi}: x={:.2} y={:.2} mode={:?} flip={}", pl.x, pl.y, pl.mode, pl.grav_flip);
            }
            // probe: what sim-objects overlap each player's boxes?
            for (pi, pl) in sim.players.iter().enumerate() {
                let oh = pl.outer_half();
                let near = sim.grid.query(pl.x - oh, pl.y - oh, oh * 2.0, oh * 2.0, 35.0);
                for idx in near {
                    let so = &sim.grid.all()[idx];
                    let (ox, oy) = sim.triggers.offset_for(&so.groups);
                    eprintln!("  near p{pi}: id={} kind={:?} rect=({:.1},{:.1},{:.1},{:.1}) off=({:.1},{:.1})", so.id, so.kind, so.rect.0, so.rect.1, so.rect.2, so.rect.3, ox, oy);
                    if let Some(sl) = &so.slope {
                        eprintln!("    surf ({:.2},{:.2})->({:.2},{:.2}) tri ({:.2},{:.2}) ({:.2},{:.2}) ({:.2},{:.2})", sl.surf.0.0 + ox, sl.surf.0.1 + oy, sl.surf.1.0 + ox, sl.surf.1.1 + oy, sl.tri[0].0 + ox, sl.tri[0].1 + oy, sl.tri[1].0 + ox, sl.tri[1].1 + oy, sl.tri[2].0 + ox, sl.tri[2].1 + oy);
                    }
                }
            }
        }
        if sim.state == gdrust::sim::RunState::Complete {
            completed = true;
            eprintln!("SIM COMPLETED @ tick {tick}");
            break;
        }
        // CAL_INPUT_SHIFT (signed): +k = macro frame t+k drives sim tick t
        // (inputs act earlier); -k = macro frame t-k drives tick t (later).
        let (m_p1, m_p2) = {
            let mut cur = pb.step();
            // CAL_EVENT_LAG: delay macro input by N extra ticks (models GD
            // frame-edge input buffering: edge applied on the next frame).
            for _ in 0..event_lag {
                lag_q.push_back(cur);
                cur = lag_q.pop_front().unwrap();
            }
            shift_q.push_back(cur);
            shift_q.pop_front().unwrap()
        };
        // CAL_NOSWAP=1: macro p1 → sim p0 (ground), macro p2 → sim p1 (ceiling).
        // Default (swap): macro p2 → ground, macro p1 → ceiling.
        let (h0, h1) = if noswap { (m_p1, m_p2) } else { (m_p2, m_p1) };
        sim.step_single(InputState { hold: h0, hold_p2: h1 });
        for (pi, pl) in sim.players.iter().enumerate() {
            let cur = (pl.mode, pl.grav_flip, pl.mini);
            if pi < seen.len() && cur != seen[pi] {
                seen[pi] = cur;
                eprintln!(
                    "tick {tick}: p{pi} -> {:?} flip={} mini={} (x={:.2} y={:.2})",
                    pl.mode, pl.grav_flip, pl.mini, pl.x, pl.y
                );
            }
        }
        if sim.state == gdrust::sim::RunState::Playing && tick - last_report >= 1200 {
            last_report = tick;
            let p = &sim.players[0];
            eprintln!("tick {tick}: x={:.2} y={:.2} vel={:.3} mode={:?} hold={m_p2} progress={:.2}%", p.x, p.y, p.vel_y, p.mode, sim.progress());
        }
    }
    println!("----");
    println!("sim first death: {:?}", death_tick);
    println!("macro first death: {first_death}");
    println!("completed: {completed}");
    let p = &sim.players[0];
    println!("final: x={:.2} progress={:.2}% attempt={}", p.x, sim.progress(), sim.attempt);
}
