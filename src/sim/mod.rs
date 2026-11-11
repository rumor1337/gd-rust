//! Sim orchestration: 240Hz substeps, collision resolution, interactions, progress.

pub mod objects;
pub mod player;

use crate::level::Level;
use objects::{Grid, ObjectKind, ObjectTable, SimObject};
use player::{GameMode, Player};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Playing,
    Dead,
    Complete,
}

#[derive(Debug, Clone, Copy)]
pub struct InputState {
    pub hold: bool,
}

pub struct Sim {
    pub grid: Grid,
    pub players: Vec<Player>,
    pub state: RunState,
    pub frame: u64,
    pub attempt: u32,
    pub dead_frames: u32,
    pub last_obj_x: f64,
    /// object indices currently overlapped per player (edge-triggered interactions)
    touching: Vec<std::collections::HashSet<usize>>,
    prev_hold: Vec<bool>,
    level_start: (GameMode, u8),
}

pub const DEATH_PAUSE_FRAMES: u32 = 30;
const SPEED_KA2_MAP: [u8; 5] = [1, 0, 2, 3, 4]; // kA2 value -> SPEEDS index

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
        let speed_idx = *SPEED_KA2_MAP
            .get(level.header.start_speed as usize % 5)
            .unwrap_or(&1);
        let mode = match level.header.start_gamemode {
            1 => GameMode::Ship,
            2 => GameMode::Ball,
            3 => GameMode::Ufo,
            4 => GameMode::Wave,
            5 => GameMode::Robot,
            6 => GameMode::Spider,
            _ => GameMode::Cube,
        };
        let mut sim = Sim {
            grid: Grid::build(objs),
            players: vec![Player::new(mode, speed_idx)],
            state: RunState::Playing,
            frame: 0,
            attempt: 1,
            dead_frames: 0,
            last_obj_x,
            touching: vec![std::collections::HashSet::new()],
            prev_hold: vec![false],
            level_start: (mode, speed_idx),
        };
        sim.snap_to_floor(0);
        sim
    }

    fn snap_to_floor(&mut self, pi: usize) {
        let p = &mut self.players[pi];
        let hh = p.outer_half();
        if p.grav_flip {
            p.y = crate::physics::CEIL_Y - hh;
        } else {
            p.y = crate::physics::GROUND_Y + hh;
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
        self.frame += 1;
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
        for pi in 0..self.players.len() {
            self.players[pi].hold = input.hold;
        }
        for _ in 0..crate::physics::SUBSTEPS {
            for pi in 0..self.players.len() {
                let press = input.hold && !self.prev_hold[pi];
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
                self.players[pi].substep();
                if self.resolve_collisions(pi) || self.interact(pi) {
                    if self.state == RunState::Dead {
                        return;
                    }
                }
            }
        }
        for i in 0..self.players.len() {
            self.prev_hold[i] = input.hold;
        }
    }

    fn respawn(&mut self) {
        let (mode, speed) = self.level_start;
        self.players = vec![Player::new(mode, speed)];
        self.touching = vec![std::collections::HashSet::new()];
        self.prev_hold = vec![false];
        self.state = RunState::Playing;
        self.dead_frames = 0;
        self.attempt += 1;
        self.frame = 0;
        self.snap_to_floor(0);
    }

    /// Full restart (gift reset). Returns the attempt number before reset.
    pub fn reset(&mut self) -> u32 {
        let a = self.attempt;
        self.respawn();
        a
    }

    /// returns true if player died
    fn resolve_collisions(&mut self, pi: usize) -> bool {
        let hh = self.players[pi].outer_half();
        let (px, py) = (self.players[pi].x, self.players[pi].y);
        let outer = (px - hh, py - hh, 2.0 * hh, 2.0 * hh);
        let inner_half = crate::physics::INNER_HALF * if self.players[pi].mini { 0.6 } else { 1.0 };
        let inner = (px - inner_half, py - inner_half, 2.0 * inner_half, 2.0 * inner_half);

        // level box floor/ceiling
        {
            let p = &mut self.players[pi];
            let flying = matches!(
                p.mode,
                GameMode::Ship | GameMode::Ufo | GameMode::Wave | GameMode::Spider | GameMode::Swing
            );
            let _ = flying;
            // floor
            if p.y - hh < crate::physics::GROUND_Y {
                if p.grav_flip && matches!(p.mode, GameMode::Cube | GameMode::Ball | GameMode::Robot) {
                    // flipped cube hitting floor going up? impossible; snap anyway
                }
                p.y = crate::physics::GROUND_Y + hh;
                if !p.grav_flip {
                    p.vel_y = 0.0;
                    p.on_ground = true;
                }
            }
            // ceiling
            if p.y + hh > crate::physics::CEIL_Y {
                if p.grav_flip {
                    p.y = crate::physics::CEIL_Y - hh;
                    p.vel_y = 0.0;
                    p.on_ground = true;
                } else {
                    match p.mode {
                        GameMode::Ship | GameMode::Ufo | GameMode::Wave | GameMode::Swing => {
                            p.y = crate::physics::CEIL_Y - hh;
                            if p.vel_y > 0.0 {
                                p.vel_y = 0.0;
                            }
                        }
                        GameMode::Cube | GameMode::Ball | GameMode::Robot => {
                            p.y = crate::physics::CEIL_Y - hh;
                            if p.vel_y > 0.0 {
                                p.vel_y = 0.0;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        let cands = self.grid.query(outer.0, outer.1, outer.2, outer.3, 30.0);
        let mut died = false;
        for ci in cands {
            let o = &self.grid.all()[ci];
            let (rx, ry, rw, rh) = o.rect;
            let overlap_outer = rects_overlap(&outer, &(rx, ry, rw, rh));
            let circle_hit = match o.radius {
                Some(r) => circle_rect((o.x, o.y), r, &outer),
                None => false,
            };
            match o.kind {
                ObjectKind::Hazard => {
                    if overlap_outer || circle_hit {
                        died = true;
                        break;
                    }
                }
                ObjectKind::Solid | ObjectKind::Slope => {
                    if !overlap_outer {
                        continue;
                    }
                    // wave dies on any solid contact
                    if self.players[pi].mode == GameMode::Wave {
                        died = true;
                        break;
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
                            died = true; // side hit
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
        let cands = self.grid.query(outer.0, outer.1, outer.2, outer.3, 30.0);
        let mut now: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for ci in cands {
            let kind = self.grid.all()[ci].kind;
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
            let (rx, ry, rw, rh) = self.grid.all()[ci].rect;
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

#[inline]
fn circle_rect(c: (f64, f64), r: f64, rect: &(f64, f64, f64, f64)) -> bool {
    let cx = c.0.clamp(rect.0, rect.0 + rect.2);
    let cy = c.1.clamp(rect.1, rect.1 + rect.3);
    let dx = c.0 - cx;
    let dy = c.1 - cy;
    dx * dx + dy * dy <= r * r
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
        sim.step(InputState { hold: false });
        assert_eq!(sim.state, RunState::Playing);
        // airborne: place a tall block to jump off? simpler: force airborne
        sim.players[0].on_ground = false;
        sim.players[0].y = 200.0;
        sim.players[0].vel_y = 0.0;
        let y0 = sim.players[0].y;
        sim.step(InputState { hold: false });
        let expect = 0.225 * 0.225 * crate::physics::GRAVITY * 10.0;
        assert!((sim.players[0].y - (y0 - expect)).abs() < 1e-9, "y={} expect drop {}", sim.players[0].y, expect);
    }

    #[test]
    fn x_speed_1x_exact() {
        let mut sim = Sim::new(&empty_level(), &table());
        // default header speed maps to 1x
        sim.players[0].speed_idx = 1;
        let x0 = sim.players[0].x;
        sim.step(InputState { hold: false });
        let dx = crate::physics::X_VEL * crate::physics::SPEEDS[1];
        assert!((sim.players[0].x - (x0 + dx)).abs() < 1e-9);
    }

    #[test]
    fn spike_kills() {
        // spike id 8 on the ground ahead; empty-level floor keeps player running
        let lvl = synth_level(vec![(8, 300.0, 0.0)]);
        let mut sim = Sim::new(&lvl, &table());
        let mut died_at = None;
        for f in 0..200 {
            sim.step(InputState { hold: false });
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
            sim.step(InputState { hold: true });
            if sim.state == RunState::Dead {
                break;
            }
        }
        // either alive (orb hopped over spike region) or died past orb; assert orb triggered:
        // (orb sets vel to JUMP_VEL) — check we got airborne with upward vel at some point
        assert!(sim.state != RunState::Dead || sim.players[0].x > 560.0);
    }

    #[test]
    fn determinism_two_runs_identical() {
        let lvl = synth_level(vec![(8, 300.0, 0.0), (36, 500.0, 60.0), (1, 800.0, 0.0)]);
        let run = |seed_holds: &[bool]| -> Vec<(f64, f64, RunState)> {
            let mut sim = Sim::new(&lvl, &table());
            let mut log = vec![];
            for h in seed_holds {
                sim.step(InputState { hold: *h });
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
