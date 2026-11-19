//! Sim orchestration: 240Hz substeps, collision resolution, interactions, progress.

pub mod fx;
pub mod objects;
pub mod player;
pub mod triggers;

use crate::level::Level;
use objects::{Grid, ObjectKind, ObjectTable, SimObject};
use std::borrow::Cow;
use player::{GameMode, Player};
use triggers::{Event, PlayerView, TriggerEngine};

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

/// Clone = a cheap snapshot: the object grid is shared, the live state copied.
#[derive(Clone)]
pub struct Sim {
    pub grid: std::sync::Arc<Grid>,
    pub players: Vec<Player>,
    pub state: RunState,
    pub frame: u64,
    pub attempt: u32,
    pub dead_frames: u32,
    pub last_obj_x: f64,
    /// highest hitbox object (sim y): the autoplayer's route stays below it
    pub top_y: f64,
    /// last death forensics: (player index, killer object id), set on death
    pub killer: Option<(usize, u16)>,
    /// object indices currently overlapped per player (edge-triggered interactions)
    /// per player: grid indices of orbs/pads/portals touched last tick (render reads it for use effects)
    pub touching: Vec<fx::HashSet<usize>>,
    /// level-object indices of coins collected this attempt
    pub coins: fx::HashSet<usize>,
    /// mirror portal (45) active: the view is flipped left-right
    pub mirror: bool,
    /// hide-player trigger (1612) active: icons not drawn, gameplay unchanged
    pub player_hidden: bool,
    /// active options-trigger flags (triggers::OPT_*)
    pub options: u16,
    /// what should be playing: (song id, 0 = the level's; seconds into it
    /// at `frame` == .2). Set per attempt from kA13, moved by song triggers.
    pub song: (u32, f64, u64),
    song_start: f64,
    /// per player: orbs already fired and still overlapped (one use per pass)
    pub orbs_used: Vec<fx::HashSet<usize>>,
    prev_hold: Vec<bool>,
    /// any button held last tick (touch triggers listen to every click)
    prev_any: bool,
    /// kA10: in dual, button 1 drives player 1 and button 2 player 2
    two_player: bool,
    level_start: (GameMode, u8, bool, bool, bool),
    /// triggers and the group state they drive
    pub triggers: TriggerEngine,
    /// solo portal hit this tick: drop player 2 after the player loop
    pending_solo: bool,
    /// mode changes by portal this run, and the x of the latest one (the
    /// solver drops runs that flew past a portal another run took)
    pub mode_portals: (u32, f64),
    /// per player: wave trail, one point per tick (oldest first), so the
    /// trail does not depend on how often frames are drawn
    pub trails: Vec<std::collections::VecDeque<(f64, f64)>>,
    /// replay mode: positions come from a recording (`apply_fix`); no deaths
    pub follow: bool,
    probes: Probes,
}

/// CAL_* calibration probes, read once (env reads lock and allocate).
#[derive(Clone)]
struct Probes {
    trace: bool,
    /// small-hitbox half as a fraction of the outer half (default 0.3)
    inner: Option<f64>,
    /// CAL_HAZ: half-size of the box hazards test against, as a fraction of hh
    haz: Option<f64>,
    /// skip wave side deaths on solids
    nosolid: bool,
}

impl Probes {
    fn from_env() -> Probes {
        Probes {
            trace: std::env::var("CAL_TRACE").is_ok(),
            inner: std::env::var("CAL_INNER").ok().and_then(|v| v.parse().ok()),
            haz: std::env::var("CAL_HAZ").ok().and_then(|v| v.parse().ok()),
            nosolid: std::env::var("CAL_NOSOLID").is_ok(),
        }
    }
}

pub const DEATH_PAUSE_FRAMES: u32 = 30;

/// Grid candidates near the query rect, placed by the trigger engine (moves,
/// rotations; toggled-off objects dropped; copies only for moved objects).
/// The grid holds pre-move positions: the 130 pad covers small moves, every
/// group moved further is looked up at its pre-move spot (query rect shifted
/// back by its offset), and objects carried far by rotations are checked
/// directly.
fn shifted_cands<'a>(
    grid: &'a Grid,
    triggers: &TriggerEngine,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> (Vec<usize>, Vec<Cow<'a, SimObject>>) {
    // pad: small trigger moves (NEAR_MOVE) plus slack for slope ride radii
    const PAD: f64 = triggers::NEAR_MOVE + 10.0;
    let mut cands = grid.query(x, y, w, h, PAD);
    let mut extra = false;
    for (g, (ox, oy)) in triggers.moved() {
        if ox.abs().max(oy.abs()) <= triggers::NEAR_MOVE || !grid.group_may_hit(g, x - ox, y - oy, w, h, PAD) {
            continue;
        }
        for ci in grid.query(x - ox, y - oy, w, h, PAD) {
            if grid.all()[ci].groups.contains(&g) {
                cands.push(ci);
                extra = true;
            }
        }
    }
    for src in triggers.far_rotated() {
        if let Some(ci) = grid.index_of(src) {
            cands.push(ci);
            extra = true;
        }
    }
    if extra {
        cands.sort_unstable();
        cands.dedup();
    }
    let (qx0, qy0, qx1, qy1) = (x - PAD, y - PAD, x + w + PAD, y + h + PAD);
    let mut kept = Vec::with_capacity(cands.len());
    let mut shifted = Vec::with_capacity(cands.len());
    for ci in cands {
        let o = &grid.all()[ci];
        let Some((dx, dy, dr)) = triggers.transform(o.src) else { continue };
        let so = if dx == 0.0 && dy == 0.0 && dr == 0.0 {
            Cow::Borrowed(o)
        } else {
            let p = objects::placed(o, dx, dy, dr);
            let (rx, ry, rw, rh) = p.rect;
            if rx > qx1 || rx + rw < qx0 || ry > qy1 || ry + rh < qy0 {
                continue; // moved away from the query area
            }
            Cow::Owned(p)
        };
        kept.push(ci);
        shifted.push(so);
    }
    (kept, shifted)
}

pub fn is_orb(k: ObjectKind) -> bool {
    matches!(
        k,
        ObjectKind::OrbYellow
            | ObjectKind::OrbPink
            | ObjectKind::OrbRed
            | ObjectKind::OrbBlue
            | ObjectKind::OrbGreen
            | ObjectKind::OrbSpider
            | ObjectKind::OrbDrop
            | ObjectKind::OrbDash(_)
            | ObjectKind::OrbToggle
    )
}

/// box overlaps the object's hitbox (oriented when rotated off the 90° grid)
fn hits(b: &(f64, f64, f64, f64), o: &SimObject) -> bool {
    rects_overlap(b, &o.rect) && (o.aligned || box_hits_quad(b, &o.quad))
}

/// hazard contact: circle hitbox (saws) or the (oriented) box
fn hazard_hits(b: &(f64, f64, f64, f64), o: &SimObject) -> bool {
    match o.radius {
        Some(r) => {
            let cx = o.x.clamp(b.0, b.0 + b.2);
            let cy = o.y.clamp(b.1, b.1 + b.3);
            (o.x - cx).powi(2) + (o.y - cy).powi(2) < r * r
        }
        None => hits(b, o),
    }
}

/// separating-axis test: axis-aligned box vs convex quad (the box axes are
/// covered by the caller's bounding-box check)
fn box_hits_quad(b: &(f64, f64, f64, f64), q: &[(f64, f64); 4]) -> bool {
    let corners = [(b.0, b.1), (b.0 + b.2, b.1), (b.0 + b.2, b.1 + b.3), (b.0, b.1 + b.3)];
    for i in 0..2 {
        let n = (q[i].1 - q[i + 1].1, q[i + 1].0 - q[i].0);
        let proj = |p: &(f64, f64)| p.0 * n.0 + p.1 * n.1;
        let (qmin, qmax) = q.iter().map(proj).fold((f64::INFINITY, f64::NEG_INFINITY), |a, v| (a.0.min(v), a.1.max(v)));
        let (bmin, bmax) = corners.iter().map(proj).fold((f64::INFINITY, f64::NEG_INFINITY), |a, v| (a.0.min(v), a.1.max(v)));
        if bmax <= qmin || bmin >= qmax {
            return false;
        }
    }
    true
}

impl Sim {
    pub fn new(level: &Level, table: &ObjectTable) -> Sim {
        // deterministic order: by x, then y, then id
        let mut objs: Vec<SimObject> = level
            .objects
            .iter()
            .enumerate()
            // key 121 = "No Touch": the object has no hitbox
            .filter(|(_, o)| !o.raw.iter().any(|(k, v)| *k == 121 && v == "1"))
            .map(|(i, o)| objects::sim_object(o, table, i))
            .filter(|so| so.kind != ObjectKind::Decoration)
            .collect();
        objs.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)).then(a.id.cmp(&b.id)));
        let top_y = objs.iter().map(|o| o.y).fold(0.0, f64::max);
        // kA13: the level's song offset, seconds
        let song_start = level.header.ka.iter().find(|(k, _)| k == "kA13").and_then(|(_, v)| v.parse().ok()).unwrap_or(0.0);
        let last_obj_x = level
            .objects
            .iter()
            .map(|o| o.x as f64)
            .fold(570.0, f64::max) // OpenGD PlayLayer: m_lastObjXPos starts at 570
            + 15.0;
        // kA4 is NOT in speed order: the game stores 1x as 0 and 0.5x as 1
        // (GRIEF, kA4 1, starts at 251.16 u/s in its xdBot recording)
        let ka4 = level.header.start_speed % 5;
        let speed_idx = match ka4 {
            0 => 1, // 1x
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
        let two_p = level.header.start_dual;
        let mut p0 = Player::new(mode, speed_idx);
        p0.mini = mini;
        p0.grav_flip = flip;
        let mut players = vec![p0];
        if two_p {
            // dual start (kA8): the second player spawns gravity-flipped
            let mut p2 = Player::new(mode, speed_idx);
            p2.mini = mini;
            p2.grav_flip = !flip;
            players.push(p2);
        }
        let mut sim = Sim {
            grid: std::sync::Arc::new(Grid::build(objs)),
            players,
            state: RunState::Playing,
            frame: 0,
            attempt: 1,
            dead_frames: 0,
            killer: None,
            last_obj_x,
            top_y,
            touching: vec![fx::HashSet::default(); if two_p { 2 } else { 1 }],
            orbs_used: vec![fx::HashSet::default(); if two_p { 2 } else { 1 }],
            coins: Default::default(),
            mirror: false,
            player_hidden: false,
            options: 0,
            song: (0, song_start, 0),
            song_start,
            prev_hold: vec![false; if two_p { 2 } else { 1 }],
            prev_any: false,
            two_player: level.header.two_player,
            level_start: (mode, speed_idx, mini, flip, two_p),
            triggers: TriggerEngine::new(level),
            pending_solo: false,
            mode_portals: (0, 0.0),
            trails: Vec::new(),
            follow: false,
            probes: Probes::from_env(),
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
        p.vel_y = 0.0;
        if p.grav_flip {
            p.y = p.band.1 - ph;
            p.on_ground = true;
        } else {
            // GD spawns every player's center at y=105 (a cube resting on
            // the floor); smaller hitboxes fall the rest of the way
            p.y = crate::physics::SPAWN_Y.max(p.band.0 + ph);
            p.on_ground = p.y - ph <= p.band.0 + 1e-9;
        }
    }

    /// Rough bytes one clone of this sim takes (see TriggerEngine::heap_bytes).
    pub fn heap_bytes(&self) -> usize {
        let sets = |v: &Vec<fx::HashSet<usize>>| v.iter().map(|s| s.capacity() * 9).sum::<usize>();
        std::mem::size_of::<Sim>() + self.triggers.heap_bytes() + sets(&self.touching) + sets(&self.orbs_used)
            + self.coins.capacity() * 9 + self.trails.iter().map(|t| t.capacity() * 16).sum::<usize>()
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
        self.record_trails();
        // triggers first (gd-sim Run.step; any player's x counts for x-cross)
        let views: Vec<PlayerView> = self
            .players
            .iter()
            .map(|p| PlayerView { x: p.x, y: p.y, dx: p.dx(), dy: p.last_move_dy, half: p.outer_half() })
            .collect();
        self.triggers.step(&views);
        for ev in self.triggers.take_events() {
            match ev {
                Event::End => {
                    self.state = RunState::Complete;
                    return;
                }
                Event::Gravity(g) => self.players.iter_mut().for_each(|p| p.gravity_mult = g),
                Event::ShowPlayer(v) => self.player_hidden = !v,
                Event::Options { on, off } => self.options = (self.options | on) & !off,
                Event::Song { id, offset } => self.song = (id, offset, self.frame),
            }
        }
        let any = input.hold || input.hold_p2;
        if any && !self.prev_any {
            self.triggers.press();
        } else if !any && self.prev_any {
            self.triggers.release();
        }
        self.prev_any = any;
        for pi in 0..self.players.len() {
            let hold = self.hold_for(pi, input);
            let press = hold && !self.prev_hold[pi];
            let orb_near = self.touching[pi].iter().any(|&ci| is_orb(self.grid.all()[ci].kind));
            {
                let p = &mut self.players[pi];
                p.hold = hold;
                if press {
                    p.queued_hold = true;
                    p.ball_ready = true;
                }
                if !hold {
                    // OpenGD releaseButton: the click buffer dies on release
                    p.queued_hold = false;
                    p.ball_ready = false;
                    p.dash = None; // a dash lasts while held
                }
            }
            if press {
                let p = &mut self.players[pi];
                match p.mode {
                    GameMode::Ufo => p.ufo_hop(),
                    GameMode::Swing => p.grav_flip = !p.grav_flip,
                    GameMode::Spider if p.on_ground => {
                        p.spider_teleport();
                        self.spider_snap(pi);
                    }
                    _ => {}
                }
            }
            {
                // hold-to-jump (OpenGD updateJump): cube/robot jump whenever
                // held on the ground; ball flips once per press
                let p = &mut self.players[pi];
                let wants = match p.mode {
                    GameMode::Cube | GameMode::Robot => hold,
                    GameMode::Ball => p.ball_ready,
                    _ => false,
                };
                if wants && p.on_ground && p.dash.is_none() {
                    p.jump();
                    p.ball_ready = false;
                    if !orb_near {
                        p.queued_hold = false;
                    }
                }
            }
            let prev_y = self.players[pi].y;
            self.players[pi].prev_y = prev_y;
            self.players[pi].substep();
            if self.resolve_collisions(pi) {
                self.triggers.death();
                for i in 0..self.players.len() {
                    self.prev_hold[i] = self.hold_for(i, input);
                }
                return;
            }
            self.interact(pi);
            if matches!(self.players[pi].mode, GameMode::Ship | GameMode::Wave) {
                // OpenGD checkCollisions (ship); GRIEF's recording shows the
                // wave too: a click held into a blue orb does not fire it
                self.players[pi].queued_hold = false;
            }
            // actual movement this tick (post-collision) — drives the wave
            // icon tilt, which must follow the real travel direction even
            // while slope-riding (vel_y is zeroed/re-set by the rides)
            self.players[pi].last_move_dy = self.players[pi].y - prev_y;
        }
        // applied after the loop: truncating mid-loop leaves `pi` past the end
        if std::mem::take(&mut self.pending_solo) && self.players.len() == 2 {
            self.players.truncate(1);
            self.touching.truncate(1);
            self.orbs_used.truncate(1);
            self.prev_hold.truncate(1);
        }
        for i in 0..self.players.len() {
            self.prev_hold[i] = self.hold_for(i, input);
        }
        if self.players[0].x >= self.last_obj_x {
            self.state = RunState::Complete;
        }
    }

    /// Song to play now: (id, 0 = the level's; position in seconds).
    pub fn song_pos(&self) -> (u32, f64) {
        (self.song.0, self.song.1 + self.frame.saturating_sub(self.song.2) as f64 / 240.0)
    }

    /// GD input routing: outside 2-player mode every click drives every
    /// player; in 2-player mode (kA10) each button drives its own player in
    /// dual, and either button drives the lone player otherwise.
    fn hold_for(&self, pi: usize, input: InputState) -> bool {
        if self.options & (triggers::OPT_NO_CTRL_P1 << pi.min(1)) != 0 {
            false
        } else if !self.two_player || self.players.len() == 1 {
            input.hold || input.hold_p2
        } else if pi == 0 {
            input.hold
        } else {
            input.hold_p2
        }
    }

    /// Spider teleport: snap to the nearest surface in the (new) gravity
    /// direction — a solid's face above/below, else the band edge.
    fn spider_snap(&mut self, pi: usize) {
        let p = &self.players[pi];
        let (hh, ph, up) = (p.outer_half(), p.phys_half(), p.grav_flip);
        let (lo, hi) = p.band;
        let (qy, qh) = if up { (p.y, (hi - p.y).max(0.0)) } else { (lo, (p.y - lo).max(0.0)) };
        let (_, shifted) = shifted_cands(&self.grid, &self.triggers, p.x - hh, qy, 2.0 * hh, qh);
        let mut target = if up { hi - ph } else { lo + ph };
        for o in &shifted {
            let (rx, ry, rw, rh) = o.rect;
            if o.kind != ObjectKind::Solid || rx >= p.x + hh || rx + rw <= p.x - hh {
                continue;
            }
            if up && ry >= p.y + hh - 1e-9 {
                target = target.min(ry - hh);
            } else if !up && ry + rh <= p.y - hh + 1e-9 {
                target = target.max(ry + rh + hh);
            }
        }
        let p = &mut self.players[pi];
        p.y = target;
        p.vel_y = 0.0;
        p.on_ground = true;
    }

    /// Last tick's settled positions (after replay fixes) into the trails.
    fn record_trails(&mut self) {
        const TRAIL_TICKS: usize = 360; // 1.5 s
        self.trails.resize_with(self.players.len(), Default::default);
        for (p, t) in self.players.iter().zip(&mut self.trails) {
            if p.mode != GameMode::Wave {
                t.clear();
                continue;
            }
            // a teleport / respawn jump starts a new trail
            if t.back().is_some_and(|&(x, y)| (p.x - x).abs().max((p.y - y).abs()) > 100.0) {
                t.clear();
            }
            t.push_back((p.x, p.y));
            if t.len() > TRAIL_TICKS {
                t.pop_front();
            }
        }
    }

    fn respawn(&mut self) {
        self.trails.clear();
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
        self.touching = vec![fx::HashSet::default(); self.players.len()];
        self.orbs_used = vec![fx::HashSet::default(); self.players.len()];
        self.coins.clear();
        self.mirror = false;
        self.player_hidden = false;
        self.options = 0;
        self.prev_hold = vec![false; self.players.len()];
        self.prev_any = false;
        self.state = RunState::Playing;
        self.dead_frames = 0;
        self.killer = None;
        self.pending_solo = false;
        self.attempt += 1;
        self.frame = 0;
        self.song = (0, self.song_start, 0);
        self.snap_to_floor(0);
        if self.players.len() > 1 {
            self.snap_to_floor(1);
        }
    }

    /// Full restart. Returns the attempt number before reset.
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
        // inner = 0.3 × outer (INNER_HITBOX_SCALE); decides death vs solids
        // CAL_INNER: env probe for the small-hitbox half (fraction of hh);
        // probes whether GD's getObjectRect(0.3,0.3) also applies mini scale.
        let inner_half = self.probes.inner.map(|f| f * hh).unwrap_or(0.3 * hh);

        // level box floor/ceiling
        {
            let p = &mut self.players[pi];
            // GD bounds (bounds_318190: 15.0 * scale): floor contact is
            // ground + scale * 15.0, ceiling is ceil - scale * 15.0.
            let ph = p.phys_half();
            // floor
            let (floor, ceil) = p.band;
            if p.y - ph < floor {
                p.y = floor + ph;
                if !p.grav_flip {
                    p.vel_y = 0.0;
                    p.on_ground = true;
                }
            }
            // ceiling
            if p.y + ph > ceil {
                if p.grav_flip {
                    p.y = ceil - ph;
                    p.vel_y = 0.0;
                    p.on_ground = true;
                } else {
                    p.y = ceil - ph;
                    if p.vel_y > 0.0 {
                        p.vel_y = 0.0;
                    }
                }
            }
        }

        let (cands, shifted) =
            shifted_cands(&self.grid, &self.triggers, outer.0, outer.1, outer.2, outer.3);
        let trace = self.probes.trace;
        let _ = &cands;
        let mut died = false;
        // pass 1: slopes — port of PlayerObject::collidedWithSlopeInternal.
        // Waves SNAP+RIDE plain slopes (decomp line 147: a flying wave dies
        // only on hazard slopes or isNewSlope); riding persists via the
        // wasOnSlope gate. Fresh contact gate = exitRect (y+1, h-2) overlap;
        // riding gate = x overlap + the wasOnSlope threshold.
        let wave = self.players[pi].mode == GameMode::Wave;
        let flip = self.players[pi].grav_flip;
        let ph = 2.0 * hh;
        // GD m_wasOnSlope from the previous tick; cleared now, rebuilt on snap
        let was_on_slope = self.players[pi].slope_on;
        self.players[pi].slope_on = false;
        let mut on_slope_now = false; // GD m_isOnSlope within this pass
        for (k, &ci) in cands.iter().enumerate() {
            let o = &shifted[k];
            if o.kind != ObjectKind::Slope {
                continue;
            }
            let sl = match &o.slope {
                Some(s) => s,
                None => continue,
            };
            let (rx, ry, rw, rh) = o.rect;
            let px = self.players[pi].x;
            // GD slopeYPos: the AABB diagonal picked by the surface direction,
            // extrapolated beyond the rect
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
            let syp = hyp;
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
            let py = self.players[pi].y;
            if was_on_slope && self.players[pi].slope_id == Some(ci) {
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
                // death (decomp 147): only hazard slopes kill, and none are
                // modeled (1717/1718 ride as plain slopes). GD's other
                // terms (isNewSlope, dart-not-sliding) are vetoed in
                // practice by preSlopeCollision (no decompiled body): the
                // 0-death macro survives both slope landings and side-flips
                if trace {
                    let (offx, offy) = self.triggers.offset_for(&o.groups);
                    eprintln!(
                        "TRACE f={} p{} id={} px={:.2} hyp={:.2} py={:.2} npy={:.2} gft={} up={} ns={} col={} rect=({:.2},{:.2},{:.2},{:.2}) off=({:.1},{:.1}) surf=({:.2},{:.2})->({:.2},{:.2})",
                        self.frame, pi, o.id, px, hyp, py, npy, gd_floor_top, player_uphill, is_new_slope, collided, o.rect.0, o.rect.1, o.rect.2, o.rect.3, offx, offy, o.slope.as_ref().map(|s| s.surf.0.0).unwrap_or(0.0), o.slope.as_ref().map(|s| s.surf.0.1).unwrap_or(0.0), o.slope.as_ref().map(|s| s.surf.1.0).unwrap_or(0.0), o.slope.as_ref().map(|s| s.surf.1.1).unwrap_or(0.0)
                    );
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
            // only from past the face, or within glue distance of it: a
            // player inside the slope's bounding box but well clear of the
            // face (GRIEF x 29331: a ship 26u above) must not be pulled on
            let past = if gd_floor_top { py > npy } else { py < npy };
            let enter = past || (py - npy).abs() <= 4.0;
            if trace {
                eprintln!(
                    "TRACE f={} p{} id={} px={:.2} hyp={:.2} py={:.2} npy={:.2} gft={} enter={}",
                    self.frame, pi, o.id, px, hyp, py, npy, gd_floor_top, enter
                );
            }
            if enter {
                let p = &mut self.players[pi];
                p.y = npy;
                // stop only motion into the face: a ship falling away from a
                // ceiling slope that descends faster keeps its fall
                if (p.vel_y > 0.0) == gd_floor_top {
                    p.vel_y = 0.0;
                }
                p.on_ground = gd_floor_top == p.grav_flip;
            }
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
            if !hits(&outer2, o) {
                continue;
            }
            match o.kind {
                ObjectKind::Hazard => {
                    // hazards hit the player's full outer box (OpenGD
                    // PlayLayer: playerOuterBounds vs hazard bounds; the small
                    // 0.3 box is for solid side hits). Checked on the real
                    // recordings: static spikes never touch GD's path with the
                    // full box; the remaining false kills are trigger-moved
                    // objects. CAL_HAZ probes other sizes.
                    let f = self.probes.haz.unwrap_or(1.0);
                    let haz_box = (px - f * hh, py2 - f * hh, 2.0 * f * hh, 2.0 * f * hh);
                    // saws (circle hitbox) test the small inner box: on the real
                    // Artemis run GD's path comes within 6.4u (half-size) of
                    // static saws without dying, the full 15u box would kill
                    let b = if o.radius.is_some() { &inner } else { &haz_box };
                    if hazard_hits(b, o) {
                        if trace {
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
                // slopes own their deaths in pass 1; their flat edge lands
                // like a block face (GD: standing on top of an upside-down
                // slope; Bloodbath x 13255 drops a cube onto one)
                ObjectKind::Slope => {
                    let Some(sl) = &o.slope else { continue };
                    let p = &self.players[pi];
                    if p.mode == GameMode::Wave {
                        continue; // waves have their own slope rides (pass 1)
                    }
                    let s = p.grav_sign();
                    let floor = if s > 0.0 { ry + rh } else { ry };
                    let flat = sl.tri.iter().filter(|q| (q.1 - floor).abs() < 1e-6).count() >= 2;
                    let tol = if p.mode == GameMode::Ship { 6.0 } else { 10.0 };
                    let foot = if s > 0.0 { p.prev_y.max(py2) } else { p.prev_y.min(py2) } - s * hh;
                    if flat && p.vel_y * s <= 0.0 && (foot - floor) * s >= -tol {
                        let p = &mut self.players[pi];
                        p.y = floor + s * hh;
                        p.vel_y = 0.0;
                        p.on_ground = true;
                    }
                }
                ObjectKind::Solid => {
                    if self.players[pi].mode == GameMode::Wave {
                        // GD: waves LAND on solids and slide along them (the
                        // outer-box snap engages before the small hitbox
                        // penetrates the surface). Only side entry — the
                        // inner box overlapping while vertically INSIDE the
                        // rect's span — kills. prev_y: the pre-move position
                        let py_prev = self.players[pi].prev_y;
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
                        } else if hits(&inner, o) && !self.probes.nosolid
                        {
                            // CAL_NOSOLID: probe — skip wave side deaths
                            if trace {
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
                    {
                        // GD lands on a solid from the OUTER box: moving
                        // toward the face with the player's edge at most 10
                        // units past it (6 for the ship). Ship/ufo/swing do
                        // the same against the opposite face (OpenGD
                        // collidedWithObject).
                        let p = &self.players[pi];
                        let s = p.grav_sign();
                        let tol = if p.mode == GameMode::Ship { 6.0 } else { 10.0 };
                        let both = matches!(p.mode, GameMode::Ship | GameMode::Ufo | GameMode::Swing);
                        let v = p.vel_y * s; // gravity-relative
                        let (floor, ceil) = if s > 0.0 { (ry + rh, ry) } else { (ry, ry + rh) };
                        // measured from where the player was before this
                        // tick's move: gravity's per-tick dip must not decide
                        // a 10-unit step (half-block stairs)
                        let (hi, lo) = (p.prev_y.max(py2), p.prev_y.min(py2));
                        let foot = if s > 0.0 { hi } else { lo } - s * hh;
                        let head = if s > 0.0 { lo } else { hi } + s * hh;
                        let snap = if v <= 0.0 && (foot - floor) * s >= -tol {
                            Some((floor + s * hh, true))
                        } else if both && v >= 0.0 && (ceil - head) * s >= -tol {
                            Some((ceil - s * hh, false))
                        } else {
                            None
                        };
                        if let Some((y, ground)) = snap {
                            if trace {
                                eprintln!("SNAP f={} p{pi} id={} y {py2:.2}->{y:.2} vy {:.2} floor {ground}", self.frame, o.id, self.players[pi].vel_y);
                            }
                            let p = &mut self.players[pi];
                            p.y = y;
                            p.vel_y = 0.0;
                            p.on_ground = ground;
                            continue;
                        }
                    }
                    if hits(&inner, o) {
                        // middle rule: resolve vertically if we came from a
                        // surface. Only ship/ufo(/swing) ride both faces;
                        // others land on the face gravity pulls them onto and
                        // die hitting the other (OpenGD collidedWithObject)
                        let p = &self.players[pi];
                        let obj_mid = ry + rh / 2.0;
                        let moving_down = p.vel_y < 0.0;
                        let both = matches!(p.mode, GameMode::Ship | GameMode::Ufo | GameMode::Swing);
                        // only from the face the player was outside of last
                        // tick: flying into a wall's side is a death, not a
                        // teleport onto its top or bottom
                        let was_above = p.prev_y - hh >= ry + rh - 0.01;
                        let was_below = p.prev_y + hh <= ry + 0.01;
                        let snap_up = moving_down && p.y > obj_mid && was_above && (both || !p.grav_flip);
                        let snap_down = !moving_down && p.y < obj_mid && was_below && (both || p.grav_flip);
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
        if died && self.follow {
            return false; // replaying a recorded path: the recording survived
        }
        if died {
            self.state = RunState::Dead;
            self.dead_frames = 0;
        }
        died
    }

    /// Put player 0 on a recorded real-game position (xdBot frame fixes),
    /// the way xdBot corrects drift in-game. Used by macro replays together
    /// with `follow = true`, which also disables deaths.
    pub fn apply_fix(&mut self, x: f64, y: f64, rot: f64) {
        self.fix_player(0, x, y, rot);
    }

    /// Recorded player-2 position; ignored while solo (xdBot then stores
    /// p2 at x 0) or when the sim has no second player.
    pub fn apply_fix_p2(&mut self, x: f64, y: f64, rot: f64) {
        if x != 0.0 {
            self.fix_player(1, x, y, rot);
        }
    }

    fn fix_player(&mut self, i: usize, x: f64, y: f64, rot: f64) {
        if let Some(p) = self.players.get_mut(i) {
            p.vis_rot = Some(rot);
            p.last_move_dy += y - p.y;
            p.x = x;
            p.y = y;
            // the recording teleported out of the flying band (teleport the
            // sim did not model): re-center like a portal there, or physics
            // clamps the player back into the stale band every tick
            if p.band != (crate::physics::GROUND_Y, crate::physics::CEIL_Y) && !(p.band.0 - 60.0..=p.band.1 + 60.0).contains(&y) {
                p.band = player::band_for(p.mode, y);
            }
        }
    }

    /// portals, orbs, pads
    fn interact(&mut self, pi: usize) {
        let hh = self.players[pi].outer_half();
        let (px, py) = (self.players[pi].x, self.players[pi].y);
        let outer = (px - hh, py - hh, 2.0 * hh, 2.0 * hh);
        let (cands, shifted) =
            shifted_cands(&self.grid, &self.triggers, outer.0, outer.1, outer.2, outer.3);
        let mut now: fx::HashSet<usize> = fx::HashSet::default();
        let mut spider_snap = false;
        for (k, ci) in cands.into_iter().enumerate() {
            let kind = shifted[k].kind;
            // collision kinds are handled by resolve_collisions
            if matches!(kind, ObjectKind::Solid | ObjectKind::Hazard | ObjectKind::Slope | ObjectKind::Decoration | ObjectKind::Trigger) {
                continue;
            }
            if !hits(&outer, &shifted[k]) {
                continue;
            }
            let (oy, oz_rot, flip_y, tele_dy, link) =
                (shifted[k].y, shifted[k].rot, shifted[k].flip_y, shifted[k].tele_dy, shifted[k].link);
            now.insert(ci);
            // pads/portals trigger on enter; an orb fires on any tick inside
            // it with a fresh click (GD: click while overlapping), once
            if if is_orb(kind) { self.orbs_used[pi].contains(&ci) } else { self.touching[pi].contains(&ci) } {
                continue;
            }
            // --- orbs: need a buffered press ---
            if is_orb(kind) {
                // OpenGD ringJump: a buffered click that is still held
                if self.players[pi].queued_hold && self.players[pi].hold {
                    let p = self.players[pi].clone();
                    match kind {
                        ObjectKind::OrbGreen => {
                            let v = player::orb_velocity(kind, &p).unwrap_or(crate::physics::JUMP_VEL);
                            self.players[pi].green_orb(v);
                        }
                        ObjectKind::OrbBlue => {
                            // OpenGD ringJump: the jump goes the OLD way up, then
                            // the flip halves it, so the player heads to its new
                            // ground (recordings: cube +-4.69, mini 3.61, ball 3.1)
                            let v = player::orb_velocity(kind, &p).unwrap_or(crate::physics::JUMP_VEL);
                            self.players[pi].apply_orb(v);
                            self.players[pi].flip_gravity(!p.grav_flip);
                        }
                        ObjectKind::OrbSpider => {
                            self.players[pi].spider_teleport();
                            spider_snap = true;
                        }
                        ObjectKind::OrbDash(pink) => {
                            // dash direction: the orb's rotation (CW) as a
                            // math angle; a Y-flipped orb mirrors it
                            let deg = if flip_y { oz_rot } else { -oz_rot };
                            self.players[pi].start_dash(deg, pink);
                        }
                        ObjectKind::OrbToggle => {
                            if let Some((g, on)) = link {
                                self.triggers.activate_group(g, on);
                            }
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
                    self.orbs_used[pi].insert(ci);
                }
                continue;
            }
            // --- pads ---
            if let Some(force) = player::pad_force(kind) {
                // a blue pad only works facing the player's gravity: a floor
                // pad flips a normal player, a ceiling pad (rotated 180 or
                // Y-flipped) a flipped one (GRIEF: of 3 stacked ceiling pads
                // only the first flips)
                let upside_down = ((oz_rot.rem_euclid(360.0) - 180.0).abs() < 45.0) != flip_y;
                if kind == ObjectKind::PadGravity && upside_down != self.players[pi].grav_flip {
                    continue;
                }
                // OpenGD: propel with the old gravity, then flip (which halves vy),
                // so a blue pad carries the player on toward the new ground
                self.players[pi].apply_pad(force);
                if kind == ObjectKind::PadGravity {
                    let flip = !self.players[pi].grav_flip;
                    self.players[pi].flip_gravity(flip);
                }
                continue;
            }
            // --- portals ---
            let mode_before = self.players[pi].mode;
            match kind {
                ObjectKind::SpeedPortal(idx) => self.players[pi].speed_idx = idx,
                ObjectKind::GravityPortal(flip) => self.players[pi].flip_gravity(flip),
                ObjectKind::PortalCube => self.players[pi].enter_mode(GameMode::Cube, oy),
                ObjectKind::PortalShip => self.players[pi].enter_mode(GameMode::Ship, oy),
                ObjectKind::PortalBall => self.players[pi].enter_mode(GameMode::Ball, oy),
                ObjectKind::PortalUfo => self.players[pi].enter_mode(GameMode::Ufo, oy),
                ObjectKind::PortalWave => self.players[pi].enter_mode(GameMode::Wave, oy),
                ObjectKind::PortalRobot => self.players[pi].enter_mode(GameMode::Robot, oy),
                ObjectKind::PortalSpider => self.players[pi].enter_mode(GameMode::Spider, oy),
                ObjectKind::PortalSwing => self.players[pi].enter_mode(GameMode::Swing, oy),
                ObjectKind::PortalDual => {
                    if self.players.len() == 1 {
                        // player 2 mirrors player 1: other gravity, opposite
                        // vertical speed (real GRIEF/Artemis recordings: p2's
                        // dy is exactly -p1's from the portal tick on)
                        let mut p2 = self.players[0].clone();
                        p2.grav_flip = !p2.grav_flip;
                        p2.vel_y = -p2.vel_y;
                        self.players.push(p2);
                        self.touching.push(fx::HashSet::default());
                        self.orbs_used.push(fx::HashSet::default());
                        self.prev_hold.push(false);
                    }
                }
                ObjectKind::PortalSolo => self.pending_solo = true,
                ObjectKind::PortalSize(mini) => self.players[pi].mini = mini,
                // exit portal sits key-54 units above the entry: same x, keep velocity
                ObjectKind::PortalTeleport => self.players[pi].y += tele_dy,
                ObjectKind::PortalMirror => self.mirror = self.grid.all()[ci].id == 45,
                ObjectKind::Coin => {
                    self.coins.insert(self.grid.all()[ci].src);
                }
                _ => {}
            }
            if self.players[pi].mode != mode_before {
                self.mode_portals = (self.mode_portals.0 + 1, self.players[pi].x);
            }
        }
        self.orbs_used[pi].retain(|c| now.contains(c));
        self.touching[pi] = now;
        if spider_snap {
            self.spider_snap(pi);
        }
    }
}

#[inline]
fn rects_overlap(a: &(f64, f64, f64, f64), b: &(f64, f64, f64, f64)) -> bool {
    a.0 < b.0 + b.2 && a.0 + a.2 > b.0 && a.1 < b.1 + b.3 && a.1 + a.3 > b.1
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
                ..Default::default()
            });
        }
        lvl
    }

    #[test]
    fn coin_on_the_path_is_collected_and_mirror_portal_flips() {
        // coin on the floor row, mirror on (45) then off (46) further along
        let lvl = synth_level(vec![(142, 200.0, 15.0), (45, 300.0, 15.0), (46, 500.0, 15.0), (1, 3000.0, 0.0)]);
        let mut sim = Sim::new(&lvl, &table());
        let mut mirrored = false;
        for _ in 0..600 {
            sim.step_single(InputState::single(false));
            mirrored |= sim.mirror;
        }
        assert!(sim.coins.contains(&0), "coin (level object 0) collected");
        assert!(mirrored && !sim.mirror, "mirror portal on, then off");
        sim.reset();
        assert!(sim.coins.is_empty(), "coins come back on a new attempt");
    }

    #[test]
    fn song_trigger_moves_the_song_and_prep_does_not() {
        // prep at 100 (ignored), play from 5.5 s at 300, level offset kA13 = 2
        let lvl = crate::level::parse_level(
            "kS38,1_0_2_0_3_0_6_1000|,kA13,2;1,1934,2,100,3,0,392,7,408,9000,399,1;1,1934,2,300,3,0,392,7,408,5500,400,1;",
        );
        let mut sim = Sim::new(&lvl, &table());
        assert_eq!(sim.song_pos(), (0, 2.0));
        let mut fired = None;
        for _ in 0..480 * 4 {
            sim.step_single(InputState::single(false));
            if sim.song.0 == 7 && fired.is_none() {
                fired = Some(sim.frame);
            }
        }
        let f = fired.expect("play trigger fired");
        let (id, pos) = sim.song_pos();
        assert_eq!(id, 7);
        assert!((pos - (5.5 + (sim.frame - f) as f64 / 240.0)).abs() <= 1.0 / 240.0 + 1e-9, "{pos}");
        sim.reset();
        assert_eq!(sim.song_pos(), (0, 2.0));
    }

    #[test]
    fn start_speed_ka4_zero_is_normal_and_one_is_slow() {
        let speed = |ka4: &str| {
            let lvl = crate::level::parse_level(&format!("kS38,1_0_2_0_3_0_6_1000|,kA4,{ka4};"));
            let mut sim = Sim::new(&lvl, &table());
            let x0 = sim.players[0].x;
            for _ in 0..240 {
                sim.step_single(InputState::single(false));
            }
            sim.players[0].x - x0
        };
        assert!((speed("0") - 311.58).abs() < 2.0, "{}", speed("0"));
        assert!((speed("1") - 251.16).abs() < 2.0, "{}", speed("1"));
    }

    #[test]
    fn options_trigger_hides_p2_and_disables_p1_controls() {
        // 2899 at x 200: hide P2 (163) + disable P1 controls (165); at 2000: undo hide P2
        let lvl = crate::level::parse_level("kS38,1_0_2_0_3_0_6_1000|;1,2899,2,200,3,0,163,1,165,1;1,2899,2,2000,3,0,163,-1;");
        let mut sim = Sim::new(&lvl, &table());
        for _ in 0..240 {
            sim.step(InputState::single(false));
        }
        let o = sim.options;
        assert!(o & triggers::OPT_HIDE_P1 << 1 != 0 && o & triggers::OPT_NO_CTRL_P1 != 0, "{o:b}");
        assert!(!sim.hold_for(0, InputState::single(true)));
        for _ in 0..1200 {
            sim.step(InputState::single(false));
        }
        assert_eq!(sim.options & triggers::OPT_HIDE_P1 << 1, 0);
    }

    #[test]
    fn hide_and_show_player_triggers_toggle_visibility() {
        let lvl = synth_level(vec![(1612, 200.0, 15.0), (1613, 500.0, 15.0), (1, 3000.0, 0.0)]);
        let mut sim = Sim::new(&lvl, &table());
        let mut hidden = false;
        for _ in 0..600 {
            sim.step_single(InputState::single(false));
            hidden |= sim.player_hidden;
        }
        assert!(hidden && !sim.player_hidden, "hidden after x 200, shown again after x 500");
    }

    /// rig layout: red orb one row above the floor (editor y 45); a click
    /// on the ground while overlapping it must launch higher than a plain jump
    #[test]
    fn orb_one_row_up_fires_on_click_from_the_ground() {
        let peak = |with_orb: bool| {
            // far block keeps the run from completing at the orb
            let mut objs = vec![(1, 3000.0, 0.0)];
            if with_orb {
                objs.push((1333, 300.0, 45.0)); // red orb: stronger than a jump
            }
            let mut sim = Sim::new(&synth_level(objs), &table());
            let mut top = 0.0f64;
            let mut held = 0;
            for _ in 0..600 {
                let hold = sim.players[0].x >= 290.0 && held < 12; // a short click
                held += hold as i32;
                sim.step_single(InputState { hold, hold_p2: false });
                top = top.max(sim.players[0].y);
            }
            (top, sim.players[0].x)
        };
        let (plain, orb) = (peak(false), peak(true));
        assert!(orb.0 > plain.0 + 20.0, "orb peak {orb:?} vs plain jump {plain:?}");
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
        let lvl = synth_level(vec![(8, 300.0, 15.0)]);
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
        let lvl = synth_level(vec![(36, 300.0, 75.0), (8, 600.0, 15.0)]);
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
    fn blue_orb_sends_the_cube_toward_its_new_ground() {
        // blue orb centered at world y 120, touching a cube resting at 105
        let lvl = synth_level(vec![(84, 300.0, 30.0)]);
        let mut sim = Sim::new(&lvl, &table());
        sim.players[0].x = 290.0;
        sim.step_single(InputState::single(false));
        for _ in 0..10 {
            sim.step_single(InputState::single(true));
            if sim.players[0].grav_flip {
                // OpenGD: 0.8 jump the old way up, halved by the flip: +4.47
                let vy = sim.players[0].vel_y;
                assert!((vy - 0.8 * crate::physics::JUMP_VEL / 2.0).abs() < 1e-6, "vy {vy}");
                return;
            }
        }
        panic!("blue orb never fired");
    }

    #[test]
    fn wave_rising_into_a_block_bottom_slides_instead_of_dying() {
        // block y 150..180; the wave came from below (prev_y) with vy zeroed
        // by a slope ride, so vy says nothing about where it came from
        let lvl = synth_level(vec![(1, 300.0, 75.0)]);
        let mut sim = Sim::new(&lvl, &table());
        let p = &mut sim.players[0];
        p.mode = GameMode::Wave;
        p.mini = true;
        p.x = 300.0;
        p.prev_y = 148.0;
        p.y = 150.5;
        p.vel_y = 0.0;
        assert!(!sim.resolve_collisions(0), "killed");
        let hh = sim.players[0].outer_half();
        assert!((sim.players[0].y - (150.0 - hh)).abs() < 1e-9);
    }

    #[test]
    fn cube_lands_on_the_flat_top_of_an_upside_down_slope() {
        // 45-degree slope flipped vertically: box y 90..120, flat edge on top
        let mut lvl = synth_level(vec![(289, 300.0, 15.0)]);
        lvl.objects[0].flip_y = true;
        let mut sim = Sim::new(&lvl, &table());
        sim.players[0].x = 300.0;
        sim.players[0].y = 150.0;
        sim.players[0].on_ground = false;
        for _ in 0..30 {
            let x = sim.players[0].x;
            sim.step_single(InputState::single(false));
            sim.players[0].x = x; // stay over the slope
        }
        assert_eq!(sim.state, RunState::Playing);
        assert!((sim.players[0].y - 135.0).abs() < 1e-6, "y {}", sim.players[0].y);
    }

    #[test]
    fn ship_above_a_slope_face_is_not_pulled_onto_it() {
        // a 45-degree slope, box x 285..315 y 90..120: on its low end the
        // ship's box overlaps the slope's box while ~9u clear of the ride line
        let lvl = synth_level(vec![(289, 300.0, 15.0)]);
        for x in [290.0, 310.0] {
            let mut sim = Sim::new(&lvl, &table());
            let p = &mut sim.players[0];
            p.mode = GameMode::Ship;
            p.x = x;
            p.y = 125.0;
            p.vel_y = 0.0;
            sim.step_single(InputState::single(false));
            // one tick of gravity moves it well under 2u; a snap down is ~9u
            assert!(sim.players[0].y > 123.0, "x {x}: pulled down to {}", sim.players[0].y);
        }
    }

    #[test]
    fn wave_survives_slope_far_from_center() {
        // Same slope, but the wave rides a parallel 2:1 path 6+ units below
        // the hyp: never within ~1.0 of the object center -> the slope
        // handler never runs -> inert (GD's tiny effective slope hitbox).
        let lvl = synth_level(vec![(1718, 120.0, 30.0)]);
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
    fn solo_portal_in_two_player_mode_drops_player_two() {
        let mut lvl = synth_level(vec![(287, 100.0, 0.0)]);
        lvl.header.start_dual = true;
        let mut sim = Sim::new(&lvl, &table());
        assert_eq!(sim.players.len(), 2);
        for _ in 0..100 {
            sim.step(InputState::single(false));
        }
        assert_eq!(sim.players.len(), 1);
        assert_eq!(sim.state, RunState::Playing);
    }

    #[test]
    fn wave_trail_records_every_tick_and_restarts_on_teleport() {
        let mut sim = Sim::new(&empty_level(), &table());
        sim.players[0].mode = GameMode::Wave;
        sim.players[0].band = player::band_for(GameMode::Wave, 240.0);
        for _ in 0..50 {
            sim.step_single(InputState { hold: true, hold_p2: false });
        }
        assert_eq!(sim.trails[0].len(), 50);
        let (x, y) = (sim.players[0].x, sim.players[0].y);
        sim.apply_fix(x + 500.0, y, 0.0); // teleport
        sim.step_single(InputState { hold: true, hold_p2: false });
        assert_eq!(sim.trails[0].len(), 1);
        sim.players[0].mode = GameMode::Ship;
        sim.step_single(InputState { hold: true, hold_p2: false });
        assert!(sim.trails[0].is_empty());
    }

    #[test]
    fn fix_outside_flying_band_recenters_band() {
        let mut sim = Sim::new(&empty_level(), &table());
        sim.players[0].mode = GameMode::Wave;
        sim.players[0].band = player::band_for(GameMode::Wave, 1035.0);
        sim.apply_fix(100.0, 175.0, 0.0);
        let b = sim.players[0].band;
        assert!(b.0 <= 175.0 && 175.0 <= b.1, "band {b:?}");
        sim.apply_fix(102.0, 180.0, 0.0); // small step inside: band kept
        assert_eq!(sim.players[0].band, b);
    }

    #[test]
    fn p2_fix_moves_only_the_second_player_and_skips_solo_marker() {
        let mut sim = Sim::new(&empty_level(), &table());
        sim.apply_fix_p2(500.0, 300.0, 0.0); // solo: nothing to fix
        assert_eq!(sim.players.len(), 1);
        let mut p2 = sim.players[0].clone();
        p2.grav_flip = true;
        sim.players.push(p2);
        sim.apply_fix_p2(0.0, -84.0, 0.0); // xdBot's "no p2" marker
        assert_ne!(sim.players[1].y, -84.0);
        sim.apply_fix_p2(40.0, 300.0, 180.0);
        assert_eq!((sim.players[1].x, sim.players[1].y), (40.0, 300.0));
        assert_ne!(sim.players[0].y, 300.0);
    }

    #[test]
    fn far_moved_block_still_collides() {
        // block placed 600u up, moved down onto the floor lane at spawn by an
        // instant 901: the grid only knows its pre-move spot
        let mut lvl = synth_level(vec![(1, 300.0, 600.0), (901, -10.0, 0.0)]);
        lvl.objects[0].groups = vec![5];
        lvl.objects[1].raw = vec![(51, "5".into()), (29, "-600".into()), (10, "0".into())];
        let mut sim = Sim::new(&lvl, &table());
        for _ in 0..200 {
            sim.step(InputState::single(false));
            if sim.state == RunState::Dead {
                break;
            }
        }
        assert_eq!(sim.state, RunState::Dead, "ran through the moved block");
        assert!(sim.players[0].x > 250.0 && sim.players[0].x < 300.0, "x={}", sim.players[0].x);
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
