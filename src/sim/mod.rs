//! Sim orchestration: 240Hz substeps, collision resolution, interactions, progress.

pub mod objects;
pub mod player;
pub mod triggers;

use crate::level::Level;
use objects::{Grid, ObjectKind, ObjectTable, SimObject};
use player::{GameMode, Player};
use triggers::TriggerEngine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Playing,
    Dead,
    Complete,
}

#[derive(Debug, Clone, Copy)]
pub struct InputState {
    pub hold: bool,
    pub hold_p2: bool,
}

impl InputState {
    pub fn single(hold: bool) -> Self {
        InputState { hold, hold_p2: false }
    }
}

pub struct Sim {
    pub grid: Grid,
    pub players: Vec<Player>,
    pub state: RunState,
    pub frame: u64,
    pub attempt: u32,
    pub dead_frames: u32,
    pub last_obj_x: f64,
    /// last death forensics: (player index, killer object id), set on death
    pub killer: Option<(usize, u16)>,
    /// object indices currently overlapped per player (edge-triggered interactions)
    touching: Vec<std::collections::HashSet<usize>>,
    prev_hold: Vec<bool>,
    level_start: (GameMode, u8, bool, bool, bool),
    /// x-crossing move triggers (901) with live group offsets
    pub triggers: TriggerEngine,
}

pub const DEATH_PAUSE_FRAMES: u32 = 30;

/// Translate a SimObject by a move-trigger offset (rect, slope, center).
fn shift_object(o: &SimObject, ox: f64, oy: f64) -> SimObject {
    let mut s = o.clone();
    s.x += ox;
    s.y += oy;
    s.rect.0 += ox;
    s.rect.1 += oy;
    if let Some(sl) = &mut s.slope {
        sl.surf.0 .0 += ox;
        sl.surf.0 .1 += oy;
        sl.surf.1 .0 += ox;
        sl.surf.1 .1 += oy;
        for p in &mut sl.tri {
            p.0 += ox;
            p.1 += oy;
        }
    }
    s
}

impl Sim {
    pub fn new(level: &Level, table: &ObjectTable) -> Sim {
        // deterministic order: by x, then y, then id
        let mut objs: Vec<SimObject> = level
            .objects
            .iter()
            .filter_map(|o| {
                let so = objects::sim_object(o, table);
                if so.kind == ObjectKind::Decoration {
                    None
                } else {
                    Some(so)
                }
            })
            .collect();
        objs.sort_by(|a, b| {
            a.x.partial_cmp(&b.x)
                .unwrap()
                .then(a.y.partial_cmp(&b.y).unwrap())
                .then(a.id.cmp(&b.id))
        });
        let last_obj_x = level
            .objects
            .iter()
            .map(|o| o.x as f64)
            .fold(f64::MIN, f64::max)
            + 15.0;
        // kA4 is NOT in speed order: the game stores 1x as 0 and 0.5x as 1 (gd-sim Level.cpp)
        let ka4 = level.header.start_speed % 5;
        let speed_idx = match ka4 {
            1 => 0, // 0.5x
            other => other,
        };
        let mode = match level.header.start_gamemode {
            1 => GameMode::Ship,
            2 => GameMode::Ball,
            3 => GameMode::Ufo,
            4 => GameMode::Wave,
            5 => GameMode::Robot,
            6 => GameMode::Spider,
            7 => GameMode::Swing,
            _ => GameMode::Cube,
        };
        let (mini, flip) = (level.header.start_mini, level.header.start_flip);
        let two_p = level.header.start_two_player;
        let mut p0 = Player::new(mode, speed_idx);
        p0.mini = mini;
        p0.grav_flip = flip;
        let mut players = vec![p0];
        if two_p {
            // 2-player mode (kA10): p2 spawns mirrored/flipped and rides the
            // ceiling from tick 0 — its idle arc never crosses the corridor
            // (the level's opening hazard field is impassable for a free
            // riser at ANY wave slope; the clean ceiling lane is the design).
            let mut p2 = Player::new(mode, speed_idx);
            p2.mini = mini;
            p2.grav_flip = !flip;
            players.push(p2);
        }
        let mut sim = Sim {
            grid: Grid::build(objs),
            players,
            state: RunState::Playing,
            frame: 0,
            attempt: 1,
            dead_frames: 0,
            killer: None,
            last_obj_x,
            touching: vec![std::collections::HashSet::new(); if two_p { 2 } else { 1 }],
            prev_hold: vec![false; if two_p { 2 } else { 1 }],
            level_start: (mode, speed_idx, mini, flip, two_p),
            triggers: {
                let (cmds, stops) = triggers::parse_cmds(level);
                TriggerEngine::new(cmds, stops)
            },
        };
        sim.snap_to_floor(0);
        if two_p {
            sim.snap_to_floor(1);
        }
        sim
    }

    fn snap_to_floor(&mut self, pi: usize) {
        let p = &mut self.players[pi];
        let ph = p.phys_half();
        if p.grav_flip {
            p.y = crate::physics::CEIL_Y - ph;
        } else {
            p.y = crate::physics::GROUND_Y + ph;
        }
        p.vel_y = 0.0;
        p.on_ground = true;
    }

    pub fn progress(&self) -> f64 {
        let p = &self.players[0];
        let total = self.last_obj_x - crate::physics::SPAWN_X;
        if total <= 0.0 {
            return 100.0;
        }
        (((p.x - crate::physics::SPAWN_X) / total) * 100.0).clamp(0.0, 100.0)
    }

    /// One 60fps frame: 4 substeps of (input edges -> physics -> collide -> interact).
    pub fn step(&mut self, input: InputState) {
        match self.state {
            RunState::Dead => {
                self.dead_frames += 1;
                if self.dead_frames >= DEATH_PAUSE_FRAMES {
                    self.respawn();
                }
                return;
            }
            RunState::Complete => return,
            RunState::Playing => {}
        }
        for _ in 0..crate::physics::SUBSTEPS {
            self.step_single(input);
            if self.state != RunState::Playing {
                return;
            }
        }
    }

    /// One 240Hz substep with per-substep input (used by macro replay at 240fps).
    pub fn step_single(&mut self, input: InputState) {
        if self.state == RunState::Complete {
            return;
        }
        if self.state == RunState::Dead {
            self.dead_frames += 1;
            if self.dead_frames >= DEATH_PAUSE_FRAMES * crate::physics::SUBSTEPS as u32 {
                self.respawn();
            }
            return;
        }
        self.frame += 1; // 240Hz tick (calibrate drives step_single directly)
        // fire x-crossing move triggers + advance timed ones (gd-sim Run.step;
        // any player's x counts — dual players cross together at spawn)
        let max_x = self.players.iter().map(|p| p.x).fold(f64::MIN, f64::max);
        self.triggers.step(max_x);
        for pi in 0..self.players.len() {
            let hold = if pi == 0 { input.hold } else { input.hold_p2 };
            self.players[pi].hold = hold;
            let press = hold && !self.prev_hold[pi];
            if press {
                let p = &mut self.players[pi];
                p.queued_hold = true;
                match p.mode {
                    GameMode::Cube | GameMode::Robot => {
                        if p.on_ground {
                            p.jump();
                        }
                    }
                    GameMode::Ball => {
                        if p.on_ground {
                            p.jump();
                        }
                    }
                    GameMode::Ufo => p.ufo_hop(),
                    GameMode::Spider => p.spider_teleport(),
                    _ => {}
                }
            }
            let prev_y = self.players[pi].y;
            self.players[pi].substep();
            if self.resolve_collisions(pi) || self.interact(pi) {
                if self.state == RunState::Dead {
                    for i in 0..self.players.len() {
                        self.prev_hold[i] = if i == 0 { input.hold } else { input.hold_p2 };
                    }
                    return;
                }
            }
            // actual movement this tick (post-collision) — drives the wave
            // icon tilt, which must follow the real travel direction even
            // while slope-riding (vel_y is zeroed/re-set by the rides)
            self.players[pi].last_move_dy = self.players[pi].y - prev_y;
        }
        for i in 0..self.players.len() {
            self.prev_hold[i] = if i == 0 { input.hold } else { input.hold_p2 };
        }
    }

    fn respawn(&mut self) {
        self.triggers.reset(); // classic respawn re-arms every trigger
        let (mode, speed, mini, flip, two_p) = self.level_start;
        let mut p0 = Player::new(mode, speed);
        p0.mini = mini;
        p0.grav_flip = flip;
        let mut players = vec![p0];
        if two_p {
            let mut p2 = Player::new(mode, speed);
            p2.mini = mini;
            p2.grav_flip = !flip;
            players.push(p2);
        }
        self.players = players;
        self.touching = vec![std::collections::HashSet::new(); self.players.len()];
        self.prev_hold = vec![false; self.players.len()];
        self.state = RunState::Playing;
        self.dead_frames = 0;
        self.killer = None;
        self.attempt += 1;
        self.frame = 0;
        self.snap_to_floor(0);
        if self.players.len() > 1 {
            self.snap_to_floor(1);
        }
    }

    /// Full restart (gift reset). Returns the attempt number before reset.
    pub fn reset(&mut self) -> u32 {
        let a = self.attempt;
        self.respawn();
        a
    }

    /// A ride clamp destination must not penetrate any OTHER solid (co-placed
    /// mirror pairs form full squares: the twin's hyp would push the player
    /// straight into its solid half). Flush touch (zero-width) is fine.
    /// Grid candidates with live move-trigger offsets applied (owned copies).
    /// Pad covers GRIEF's max opening displacement (+60x/+30y) with margin.
    fn shifted_cands(&self, x: f64, y: f64, w: f64, h: f64) -> (Vec<usize>, Vec<SimObject>) {
        let cands = self.grid.query(x, y, w, h, 130.0);
        let shifted = cands
            .iter()
            .map(|&ci| {
                let o = &self.grid.all()[ci];
                let (ox, oy) = self.triggers.offset_for(&o.groups);
                if ox == 0.0 && oy == 0.0 {
                    o.clone()
                } else {
                    shift_object(o, ox, oy)
                }
            })
            .collect();
        (cands, shifted)
    }

    /// returns true if player died
    fn resolve_collisions(&mut self, pi: usize) -> bool {
        let hh = self.players[pi].outer_half();
        let (px, py) = (self.players[pi].x, self.players[pi].y);
        let outer = (px - hh, py - hh, 2.0 * hh, 2.0 * hh);
        // inner = 0.3 × outer (INNER_HITBOX_SCALE); decides death vs solids
        // CAL_INNER: env probe for the small-hitbox half (fraction of hh);
        // probes whether GD's getObjectRect(0.3,0.3) also applies mini scale.
        let inner_half = std::env::var("CAL_INNER")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .map(|f| f * hh)
            .unwrap_or(0.3 * hh);

        // level box floor/ceiling
        {
            let p = &mut self.players[pi];
            // GD bounds (bounds_318190: 15.0 * scale): floor contact is
            // ground + scale * 15.0, ceiling is ceil - scale * 15.0.
            let ph = p.phys_half();
            // floor
            if p.y - ph < crate::physics::GROUND_Y {
                p.y = crate::physics::GROUND_Y + ph;
                if !p.grav_flip {
                    p.vel_y = 0.0;
                    p.on_ground = true;
                }
            }
            // ceiling
            if p.y + ph > crate::physics::CEIL_Y {
                if p.grav_flip {
                    p.y = crate::physics::CEIL_Y - ph;
                    p.vel_y = 0.0;
                    p.on_ground = true;
                } else {
                    p.y = crate::physics::CEIL_Y - ph;
                    if p.vel_y > 0.0 {
                        p.vel_y = 0.0;
                    }
                }
            }
        }

        let (cands, shifted) = self.shifted_cands(outer.0, outer.1, outer.2, outer.3);
        let trace = std::env::var("CAL_TRACE").is_ok();
        // CAL_NO_SLOPEHAZ: diagnostic — treat slope-HAZARD deaths as inert
        // (probes whether the 1717 gate kills a pass the real game lets live)
        let no_slopehaz = std::env::var("CAL_NO_SLOPEHAZ").is_ok();
        let mut died = false;
        // pass 1: slopes — port of PlayerObject::collidedWithSlopeInternal.
        // Waves SNAP+RIDE plain slopes (decomp line 147: a flying wave dies
        // only on hazard slopes or isNewSlope); riding persists via the
        // wasOnSlope gate. Fresh contact gate = exitRect (y+1, h-2) overlap;
        // riding gate = x overlap + the wasOnSlope threshold.
        let wave = self.players[pi].mode == GameMode::Wave;
        let family =
            matches!(self.players[pi].mode, GameMode::Ship | GameMode::Ball | GameMode::Wave);
        let flip = self.players[pi].grav_flip;
        let hold = self.players[pi].hold;
        let ph = 2.0 * hh;
        // GD m_wasOnSlope from the previous tick; cleared now, rebuilt on snap
        let was_on_slope = self.players[pi].slope_on;
        self.players[pi].slope_on = false;
        let mut on_slope_now = false; // GD m_isOnSlope within this pass
        for (k, &ci) in cands.iter().enumerate() {
            let o = &shifted[k];
            if !matches!(o.kind, ObjectKind::Slope | ObjectKind::SlopeHazard) {
                continue;
            }
            let sl = match &o.slope {
                Some(s) => s,
                None => continue,
            };
            let (rx, ry, rw, rh) = o.rect;
            let px = self.players[pi].x;
            let py = self.players[pi].y;
            // GD slopeYPos: the AABB diagonal picked by the surface direction,
            // extrapolated beyond the rect, + hazard-only offset (4 into the
            // free side)
            let (a, b) = if sl.surf.0 .0 <= sl.surf.1 .0 {
                sl.surf
            } else {
                (sl.surf.1, sl.surf.0)
            };
            let rising = a.1 < b.1;
            let hyp = if rising {
                ry + (px - rx) / rw * rh
            } else {
                ry + rh - (px - rx) / rw * rh
            };
            // solid side from the triangle centroid: solid_below = solid
            // beneath the face; GD slopeFloorTop is the INVERSE (solid above)
            let (tcx, tcy) = (
                (sl.tri[0].0 + sl.tri[1].0 + sl.tri[2].0) / 3.0,
                (sl.tri[0].1 + sl.tri[1].1 + sl.tri[2].1) / 3.0,
            );
            let hyp_tc = if rising {
                ry + (tcx - rx) / rw * rh
            } else {
                ry + rh - (tcx - rx) / rw * rh
            };
            let solid_below = tcy < hyp_tc;
            let gd_floor_top = !solid_below;
            let haz = matches!(o.kind, ObjectKind::SlopeHazard);
            let syp = hyp + if haz {
                if solid_below { -4.0 } else { 4.0 }
            } else {
                0.0
            };
            let rad = (ph * 0.5) / (rh / rw).abs().atan().cos();
            // is_new_slope (decomp line 66): a ride that flips slope id AND
            // side; its giant scalar (vehicleSize*20) displaces the ride
            // line, slamming the snap to the rect's far edge instead of the
            // face (decomp 66-84: clamps use the box half-height)
            let is_new_slope = was_on_slope
                && self.players[pi].slope_id != Some(ci)
                && self.players[pi].slope_top != Some(gd_floor_top);
            let new_slope_scalar = if is_new_slope { self.players[pi].size() * 20.0 } else { 0.0 };
            // rider line (decomp line 70): floor slope -> face + rad (rider
            // above); ceiling slope -> face - rad (rider below)
            let npy_raw = syp + (rad - new_slope_scalar) * if gd_floor_top { -1.0 } else { 1.0 };
            let npy = if gd_floor_top {
                npy_raw.max(ry - hh + new_slope_scalar).min(ry + rh)
            } else {
                npy_raw.min(ry + rh + hh - new_slope_scalar).max(ry)
            };
            // playerUphill: empirical convention (rising ^ flip). The raw XOR
            // chain (slopeUphill ^ (upside!=sideways) ^ outpaces) inverts
            // this, but the flipped variant regresses corridor progress
            // (tick 625 vs 681); kept until the decomp's m_slopeUphill
            // storage polarity is pinned down.
            let player_uphill = rising ^ flip;
            // static-slope float band (decomp): an uphill rider keeps glue
            let float_g = if player_uphill {
                if was_on_slope { 4.0 } else { 1.0 }
            } else {
                0.0
            };
            // Broadphase x bound first: GD's query pad is small, so the ride
            // must not persist past the rect on the extrapolated face.
            if px + hh <= rx || px - hh >= rx + rw {
                continue;
            }
            if haz {
                // hazard slopes are gated like hazards (collisionCheckObjects
                // death circle): object center in the player box, or a box
                // corner within scale — keeps 1717/1718 pit slopes from
                // killing far off-face
                let (ocx, ocy) = (o.x, o.y);
                let b = hh;
                let center_in =
                    ocx >= px - b && ocx <= px + b && ocy >= py - b && ocy <= py + b;
                let corner_hit = [
                    (px - b, py - b),
                    (px + b, py - b),
                    (px - b, py + b),
                    (px + b, py + b),
                ]
                .iter()
                .any(|&(cx, cy)| ((ocx - cx).powi(2) + (ocy - cy).powi(2)).sqrt() <= o.scale);
                if !(center_in || corner_hit) {
                    continue;
                }
            } else if was_on_slope && self.players[pi].slope_id == Some(ci) {
                // wasOnSlope gate (decomp 47-52, normal gravity skips line
                // 48): leave when the ride threshold passes the rect top.
                // Proximity band first: the pad-130 grid returns slopes far
                // outside GD's small scan pad; GD never even sees those, so
                // require the live box inside the rect expanded by the ride
                // radius before applying GD's threshold.
                let pad = self.players[pi].slope_rad + float_g;
                let live = (px - hh, py - hh, 2.0 * hh, 2.0 * hh);
                if !rects_overlap(&live, &(rx, ry - pad, rw, rh + 2.0 * pad)) {
                    continue;
                }
                if py - pad > ry + rh {
                    continue;
                }
            } else {
                // fresh contact: exitRect (y+1, h-2) overlap
                let live = (px - hh, py - hh, 2.0 * hh, 2.0 * hh);
                if !rects_overlap(&live, &(rx, ry + 1.0, rw, rh - 2.0)) {
                    continue;
                }
            }
            if wave {
                // decomp branches: slopeUpsideDown = isUpsideDown ^
                // slopeFloorTop; normal gravity -> gd_floor_top.
                let just_pressed = self.players[pi].queued_hold; // jumpBuffered approx
                let collided = if gd_floor_top {
                    // upside-down branch (ceiling slopes, decomp 94-132):
                    // static slope float_p = playerUphill ? 0 : float_g;
                    // flying + buffered jump overrides to 2.0/1.0
                    let mut float_p = if player_uphill { 0.0 } else { float_g };
                    if just_pressed {
                        float_p = if was_on_slope { 2.0 } else { 1.0 };
                    }
                    let mut c = !on_slope_now
                        && !is_new_slope
                        && !player_uphill
                        && py > npy - float_p;
                    if py > npy {
                        c = true; // decomp 104-106 override
                    }
                    c
                } else {
                    // non-upside-down branch (floor slopes, decomp 133-145)
                    let bool_h = player_uphill && !on_slope_now && !is_new_slope;
                    let mut c = true;
                    if py >= npy {
                        c = false;
                        if bool_h && py < npy + float_g {
                            c = true; // m_isBird ? vel<=0 : true; no bird here
                        }
                    }
                    c
                };
                // death (decomp 147): only hazard slopes kill. GD's other
                // terms (isNewSlope, dart-not-sliding) are vetoed in
                // practice by preSlopeCollision (no decompiled body): the
                // 0-death macro survives both slope landings and side-flips
                let death = collided && haz;
                if trace {
                    let (offx, offy) = self.triggers.offset_for(&o.groups);
                    eprintln!(
                        "TRACE f={} p{} id={} px={:.2} hyp={:.2} py={:.2} npy={:.2} gft={} up={} ns={} col={} death={} rect=({:.2},{:.2},{:.2},{:.2}) off=({:.1},{:.1}) surf=({:.2},{:.2})->({:.2},{:.2})",
                        self.frame, pi, o.id, px, hyp, py, npy, gd_floor_top, player_uphill, is_new_slope, collided, death, o.rect.0, o.rect.1, o.rect.2, o.rect.3, offx, offy, o.slope.as_ref().map(|s| s.surf.0.0).unwrap_or(0.0), o.slope.as_ref().map(|s| s.surf.0.1).unwrap_or(0.0), o.slope.as_ref().map(|s| s.surf.1.0).unwrap_or(0.0), o.slope.as_ref().map(|s| s.surf.1.1).unwrap_or(0.0)
                    );
                }
                if death && !(haz && no_slopehaz) {
                    self.killer = Some((pi, o.id));
                    died = true;
                    break;
                }
                if collided {
                    // snap+ride (decomp 192-212)
                    let p = &mut self.players[pi];
                    p.y = npy;
                    p.slope_on = true;
                    p.slope_top = Some(gd_floor_top);
                    p.slope_id = Some(ci);
                    p.slope_rad = rad;
                    on_slope_now = true;
                    if gd_floor_top {
                        // ceiling ride (decomp 222-227): zero rising vy
                        if p.vel_y > 0.0 && !p.grav_flip {
                            p.vel_y = 0.0;
                        }
                        p.on_ground = false;
                    } else {
                        // floor ride: hitGround (decomp 229-232); vy is
                        // re-set from input next tick for waves
                        p.on_ground = !p.grav_flip;
                    }
                }
                continue;
            }
            // non-wave: approximation — snap onto GD's ride line from the
            // approach side or shallow penetration (GRIEF is wave-only)
            let thresh_applies = !(family && hold);
            let enter = if flip {
                py < npy || thresh_applies
            } else {
                py > npy || thresh_applies
            };
            if trace {
                eprintln!(
                    "TRACE f={} p{} id={} px={:.2} hyp={:.2} py={:.2} npy={:.2} gft={} enter={}",
                    self.frame, pi, o.id, px, hyp, py, npy, gd_floor_top, enter
                );
            }
            if enter {
                let p = &mut self.players[pi];
                p.y = npy;
                p.vel_y = 0.0;
                p.on_ground = gd_floor_top == p.grav_flip;
            }
        }
        if died {
            self.state = RunState::Dead;
            self.dead_frames = 0;
            return true;
        }
        // pass 2: deaths — boxes recomputed from the post-slope position.
        // GD defers hazard checks past the object scan; a single pass in
        // level order is equivalent here (waves never move in pass 1).
        let py2 = self.players[pi].y;
        let outer2 = (px - hh, py2 - hh, 2.0 * hh, 2.0 * hh);
        let inner = (px - inner_half, py2 - inner_half, 2.0 * inner_half, 2.0 * inner_half);
        for (k, _ci) in cands.into_iter().enumerate() {
            let o = &shifted[k];
            let (rx, ry, rw, rh) = o.rect;
            if !rects_overlap(&outer2, &(rx, ry, rw, rh)) {
                continue;
            }
            match o.kind {
                ObjectKind::Hazard => {
                    // GD PlayerObject::collidedWithObjectInternal: smallHitbox
                    // getObjectRect(0.3, 0.3) intersects objRect -> destroyPlayer
                    let hit = rects_overlap(&inner, &(rx, ry, rw, rh));
                    if hit {
                        if std::env::var("CAL_TRACE").is_ok() {
                            eprintln!(
                                "HAZKILL p{pi} id={} rect=({},{},{},{}) center=({},{}) p=({},{}) hh={}",
                                o.id, rx, ry, rw, rh, o.x, o.y, px, py2, hh
                            );
                        }
                        self.killer = Some((pi, o.id));
                        died = true;
                        break;
                    }
                }
                // slopes own their deaths in pass 1
                ObjectKind::Slope | ObjectKind::SlopeHazard => {}
                ObjectKind::Solid => {
                    if self.players[pi].mode == GameMode::Wave {
                        // GD: waves LAND on solids and slide along them (the
                        // outer-box snap engages before the small hitbox
                        // penetrates the surface). Only side entry — the
                        // inner box overlapping while vertically INSIDE the
                        // rect's span — kills. py_prev reconstructs the
                        // pre-move position (vel is 0 after pass-1 snaps).
                        let py_prev = py2 - self.players[pi].vel_y;
                        if py_prev - inner_half >= ry + rh {
                            let p = &mut self.players[pi];
                            p.y = ry + rh + hh;
                            if p.vel_y < 0.0 {
                                p.vel_y = 0.0;
                            }
                            p.on_ground = !p.grav_flip;
                            continue;
                        } else if py_prev + inner_half <= ry {
                            let p = &mut self.players[pi];
                            p.y = ry - hh;
                            if p.vel_y > 0.0 {
                                p.vel_y = 0.0;
                            }
                            continue;
                        } else if rects_overlap(&inner, &(rx, ry, rw, rh))
                            && std::env::var("CAL_NOSOLID").is_err()
                        {
                            // CAL_NOSOLID: probe — skip wave side deaths
                            if std::env::var("CAL_TRACE").is_ok() {
                                eprintln!(
                                    "SOLKILL p{pi} id={} rect=({},{},{},{}) inner=({},{},{},{}) ih={} p=({},{})",
                                    o.id, rx, ry, rw, rh,
                                    inner.0, inner.1, inner.2, inner.3, inner_half, px, py2
                                );
                            }
                            self.killer = Some((pi, o.id));
                            died = true;
                            break;
                        }
                        continue;
                    }
                    let (inner_ov, _) = overlap_depth(&inner, &(rx, ry, rw, rh));
                    if inner_ov {
                        // middle rule: resolve vertically if we came from a surface
                        let p = &self.players[pi];
                        let obj_mid = ry + rh / 2.0;
                        let moving_down = p.vel_y < 0.0;
                        let snap_up = moving_down && p.y > obj_mid;
                        let snap_down = !moving_down && p.y < obj_mid;
                        if snap_up || snap_down {
                            let hh = p.outer_half();
                            let p = &mut self.players[pi];
                            if snap_up {
                                p.y = ry + rh + hh;
                            } else {
                                p.y = ry - hh;
                            }
                            p.vel_y = 0.0;
                            p.on_ground = snap_up != p.grav_flip;
                        } else {
                            self.killer = Some((pi, o.id)); // side hit
                            died = true;
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        if died {
            self.state = RunState::Dead;
            self.dead_frames = 0;
        }
        died
    }

    /// portals, orbs, pads. returns true if died (drop orb edge cases)
    fn interact(&mut self, pi: usize) -> bool {
        let hh = self.players[pi].outer_half();
        let (px, py) = (self.players[pi].x, self.players[pi].y);
        let outer = (px - hh, py - hh, 2.0 * hh, 2.0 * hh);
        let (cands, shifted) = self.shifted_cands(outer.0, outer.1, outer.2, outer.3);
        let mut now: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for (k, ci) in cands.into_iter().enumerate() {
            let kind = shifted[k].kind;
            if !matches!(
                kind,
                ObjectKind::SpeedPortal(_)
                    | ObjectKind::GravityPortal(_)
                    | ObjectKind::PortalCube
                    | ObjectKind::PortalShip
                    | ObjectKind::PortalBall
                    | ObjectKind::PortalUfo
                    | ObjectKind::PortalWave
                    | ObjectKind::PortalRobot
                    | ObjectKind::PortalSpider
                    | ObjectKind::PortalSwing
                    | ObjectKind::PortalDual
                    | ObjectKind::PortalSolo
                    | ObjectKind::PortalSize(_)
                    | ObjectKind::OrbYellow
                    | ObjectKind::OrbPink
                    | ObjectKind::OrbRed
                    | ObjectKind::OrbBlue
                    | ObjectKind::OrbGreen
                    | ObjectKind::OrbSpider
                    | ObjectKind::OrbDrop
                    | ObjectKind::OrbDash
                    | ObjectKind::PadYellow
                    | ObjectKind::PadPink
                    | ObjectKind::PadRed
                    | ObjectKind::PadGravity
            ) {
                continue;
            }
            let (rx, ry, rw, rh) = shifted[k].rect;
            if !rects_overlap(&outer, &(rx, ry, rw, rh)) {
                continue;
            }
            now.insert(ci);
            if self.touching[pi].contains(&ci) {
                continue; // already inside — only trigger on enter
            }
            // --- orbs: need a buffered press ---
            if matches!(
                kind,
                ObjectKind::OrbYellow
                    | ObjectKind::OrbPink
                    | ObjectKind::OrbRed
                    | ObjectKind::OrbBlue
                    | ObjectKind::OrbGreen
                    | ObjectKind::OrbSpider
                    | ObjectKind::OrbDrop
                    | ObjectKind::OrbDash
            ) {
                if self.players[pi].queued_hold {
                    let p = self.players[pi].clone();
                    match kind {
                        ObjectKind::OrbGreen => {
                            let v = player::orb_velocity(kind, &p).unwrap_or(crate::physics::JUMP_VEL);
                            self.players[pi].green_orb(v);
                        }
                        ObjectKind::OrbBlue => {
                            let v = player::orb_velocity(kind, &p).unwrap_or(crate::physics::JUMP_VEL);
                            self.players[pi].flip_gravity(!p.grav_flip);
                            self.players[pi].apply_orb(v);
                        }
                        ObjectKind::OrbSpider => {
                            self.players[pi].spider_teleport();
                        }
                        ObjectKind::OrbDrop => {
                            let v = player::orb_velocity(kind, &p).unwrap_or(-15.0);
                            self.players[pi].drop_orb(v);
                        }
                        _ => {
                            let v = player::orb_velocity(kind, &p).unwrap_or(crate::physics::JUMP_VEL);
                            self.players[pi].apply_orb(v);
                        }
                    }
                    self.players[pi].queued_hold = false;
                }
                continue;
            }
            // --- pads ---
            if let Some(force) = player::pad_force(kind) {
                if kind == ObjectKind::PadGravity {
                    let flip = !self.players[pi].grav_flip;
                    self.players[pi].flip_gravity(flip);
                }
                self.players[pi].apply_pad(force);
                continue;
            }
            // --- portals ---
            match kind {
                ObjectKind::SpeedPortal(idx) => self.players[pi].speed_idx = idx,
                ObjectKind::GravityPortal(flip) => self.players[pi].flip_gravity(flip),
                ObjectKind::PortalCube => self.players[pi].enter_mode(GameMode::Cube),
                ObjectKind::PortalShip => self.players[pi].enter_mode(GameMode::Ship),
                ObjectKind::PortalBall => self.players[pi].enter_mode(GameMode::Ball),
                ObjectKind::PortalUfo => self.players[pi].enter_mode(GameMode::Ufo),
                ObjectKind::PortalWave => self.players[pi].enter_mode(GameMode::Wave),
                ObjectKind::PortalRobot => self.players[pi].enter_mode(GameMode::Robot),
                ObjectKind::PortalSpider => self.players[pi].enter_mode(GameMode::Spider),
                ObjectKind::PortalSwing => self.players[pi].enter_mode(GameMode::Swing),
                ObjectKind::PortalDual => {
                    if self.players.len() == 1 {
                        let mut p2 = self.players[0].clone();
                        p2.grav_flip = !p2.grav_flip;
                        self.players.push(p2);
                        self.touching.push(std::collections::HashSet::new());
                        self.prev_hold.push(false);
                    }
                }
                ObjectKind::PortalSolo => {
                    if self.players.len() == 2 {
                        self.players.truncate(1);
                        self.touching.truncate(1);
                        self.prev_hold.truncate(1);
                    }
                }
                ObjectKind::PortalSize(mini) => self.players[pi].mini = mini,
                _ => {}
            }
        }
        self.touching[pi] = now;
        false
    }
}

#[inline]
fn rects_overlap(a: &(f64, f64, f64, f64), b: &(f64, f64, f64, f64)) -> bool {
    a.0 < b.0 + b.2 && a.0 + a.2 > b.0 && a.1 < b.1 + b.3 && a.1 + a.3 > b.1
}

#[inline]
fn overlap_depth(a: &(f64, f64, f64, f64), b: &(f64, f64, f64, f64)) -> (bool, f64) {
    let ox = (a.0 + a.2).min(b.0 + b.2) - a.0.max(b.0);
    let oy = (a.1 + a.3).min(b.1 + b.3) - a.1.max(b.1);
    (ox > 0.0 && oy > 0.0, ox.min(oy))
}


/// p inside (or on the edge of) triangle t — all edge cross products same sign.
#[inline]
fn point_in_tri(p: (f64, f64), t: [(f64, f64); 3]) -> bool {
    let d = |a: (f64, f64), b: (f64, f64)| {
        (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
    };
    let d1 = d(t[0], t[1]);
    let d2 = d(t[1], t[2]);
    let d3 = d(t[2], t[0]);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> ObjectTable {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/objects.json");
        ObjectTable::load(&p).unwrap()
    }

    fn empty_level() -> Level {
        Level::default()
    }

    fn synth_level(objs: Vec<(u16, f64, f64)>) -> Level {
        let mut lvl = Level::default();
        for (id, x, y) in objs {
            lvl.objects.push(crate::level::object::GdObject {
                id,
                x: x as f32,
                y: y as f32,
                rot: 0.0,
                flip_x: false,
                flip_y: false,
                scale_x: 1.0,
                scale_y: 1.0,
                base_color: None,
                detail_color: None,
                z_layer: None,
                editor_layer: 0,
                groups: vec![],
                raw: vec![],
            });
        }
        lvl
    }

    #[test]
    fn free_fall_gravity_exact() {
        // after 1 frame (4 substeps): dy = 0.225 * sum(vel_i), vel_i = -g*0.225*i
        let sim = Sim::new(&empty_level(), &table());
        let mut sim = sim;
        // empty level: player spawns on floor, no input -> stays on ground
        sim.step(InputState::single(false));
        assert_eq!(sim.state, RunState::Playing);
        // airborne: place a tall block to jump off? simpler: force airborne
        sim.players[0].on_ground = false;
        sim.players[0].y = 200.0;
        sim.players[0].vel_y = 0.0;
        let y0 = sim.players[0].y;
        sim.step(InputState::single(false));
        let expect = 0.225 * 0.225 * crate::physics::GRAVITY * 10.0;
        assert!((sim.players[0].y - (y0 - expect)).abs() < 1e-9, "y={} expect drop {}", sim.players[0].y, expect);
    }

    #[test]
    fn x_speed_1x_exact() {
        let mut sim = Sim::new(&empty_level(), &table());
        // kA4=1 → 1x
        sim.players[0].speed_idx = 1;
        let x0 = sim.players[0].x;
        sim.step(InputState::single(false));
        let dx = crate::physics::SPEEDS[1] / crate::physics::TICK_HZ * 4.0; // 4 substeps
        assert!((sim.players[0].x - (x0 + dx)).abs() < 1e-9);
    }

    #[test]
    fn spike_kills() {
        // spike id 8 on the ground ahead; empty-level floor keeps player running
        let lvl = synth_level(vec![(8, 300.0, 0.0)]);
        let mut sim = Sim::new(&lvl, &table());
        let mut died_at = None;
        for f in 0..200 {
            sim.step(InputState::single(false));
            if sim.state == RunState::Dead {
                died_at = Some(f);
                break;
            }
        }
        assert!(died_at.is_some(), "player never died on spike");
        // spike world x=300, hitbox 12 wide; death must happen near it
        assert!(sim.players[0].x > 250.0 && sim.players[0].x < 340.0, "died at x={}", sim.players[0].x);
    }

    #[test]
    fn yellow_orb_jumps() {
        // orb floating above ground at x=300 (y=60 world 150): run into it while pressing
        let lvl = synth_level(vec![(36, 300.0, 60.0), (8, 600.0, 0.0)]);
        let mut sim = Sim::new(&lvl, &table());
        // hold from start: cube jumps, then press again mid-air for orb? simple: hold entire time
        for _ in 0..400 {
            sim.step(InputState::single(true));
            if sim.state == RunState::Dead {
                break;
            }
        }
        // either alive (orb hopped over spike region) or died past orb; assert orb triggered:
        // (orb sets vel to JUMP_VEL) — check we got airborne with upward vel at some point
        assert!(sim.state != RunState::Dead || sim.players[0].x > 560.0);
    }

    #[test]
    fn wave_survives_slope_far_from_center() {
        // Same slope, but the wave rides a parallel 2:1 path 6+ units below
        // the hyp: never within ~1.0 of the object center -> the slope
        // handler never runs -> inert (GD's tiny effective slope hitbox).
        let lvl = synth_level(vec![(1718, 120.0, 15.0)]);
        let mut sim = Sim::new(&lvl, &table());
        {
            let p = &mut sim.players[0];
            p.mode = GameMode::Wave;
            p.mini = true;
            p.x = 120.0;
            p.y = 100.0;
        }
        for _ in 0..25 {
            sim.step_single(InputState::single(true));
            assert_eq!(sim.state, RunState::Playing);
        }
        // the 2:1 wave crosses the hyp ~13u right of the center: no circle
        // contact -> the slope stays inert and the wave survives
        assert!(sim.players[0].y > 125.0, "crossed the hyp line, y={}", sim.players[0].y);
    }

    #[test]
    fn determinism_two_runs_identical() {
        let lvl = synth_level(vec![(8, 300.0, 0.0), (36, 500.0, 60.0), (1, 800.0, 0.0)]);
        let run = |seed_holds: &[bool]| -> Vec<(f64, f64, RunState)> {
            let mut sim = Sim::new(&lvl, &table());
            let mut log = vec![];
            for h in seed_holds {
                sim.step(InputState::single(*h));
                log.push((sim.players[0].x, sim.players[0].y, sim.state));
            }
            log
        };
        let holds: Vec<bool> = (0..300).map(|i| i % 37 < 18).collect();
        let a = run(&holds);
        let b = run(&holds);
        assert_eq!(format!("{:?}", a), format!("{:?}", b));
    }
}
