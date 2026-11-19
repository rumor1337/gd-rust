//! Trigger engine: activates GD triggers (x-cross, touch, spawn) and keeps the
//! group state they drive: move offsets, per-object rotation, toggles and
//! item counts. Keys follow analysis/gddocs/level-string.md; behavior follows
//! OpenGD (EffectGameObject.cpp) where it has the trigger.
//!
//! "Activate group" (touch, count, instant count, collision, on-death, toggle
//! orb) toggles the target group on AND spawns its spawn-triggered triggers;
//! the levels' own designs depend on it (GRIEF's click counter: touch ->
//! spawn-triggered pickup -> count). The plain toggle trigger (1049) does not
//! spawn, as in OpenGD.

use super::fx::{HashMap, HashSet};
use std::sync::Arc;

use crate::level::{GdObject, Level};

const TICK_HZ: f64 = 240.0;
/// spawn chains deeper than this are cut (guards spawn loops with no delay)
const MAX_DEPTH: u32 = 32;

#[derive(Debug, Clone, Copy)]
enum Kind {
    /// to: (group whose anchor is the destination, 0 both / 1 x only / 2 y only)
    Move { dx: f64, dy: f64, lock_x: bool, lock_y: bool, to: Option<(u16, u8)> },
    Toggle { on: bool },
    Spawn { delay: u64 },
    Rotate { deg: f64, center: Option<u16>, lock: bool },
    Follow { other: u16, xm: f64, ym: f64 },
    FollowY { speed: f64, offset: f64, max: f64 },
    Stop,
    OnDeath { on: bool },
    Count { item: u16, count: i64, on: bool },
    InstantCount { item: u16, count: i64, on: bool, cmp: u8 },
    Pickup { item: u16, count: i64 },
    Collision { a: u16, b: u16, on: bool, exit: bool },
    Touch { hold: bool, mode: u8 },
    Gravity(f64),
    End,
    /// 1612 hide / 1613 show player
    ShowPlayer(bool),
    /// 2899 options trigger: OPT_* bits to turn on / off
    Options { on: u16, off: u16 },
    /// 1934 song trigger: play song `id` (0 = the level's) from `offset` s
    Song { id: u32, offset: f64 },
    /// 1934 with "prep" (key 399): only preloads in GD, plays nothing
    Nop,
    /// 1007: fade the target group's opacity to `to` over the trigger time
    Alpha { to: f32 },
    Pulse(Pulse),
}

/// What a pulse blends toward: a fixed color, or an HSV shift of the base.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PulseColor {
    Rgb(u8, u8, u8),
    /// GD HSV: [hue deg, sat, val, sat additive, val additive]
    Hsv([f32; 5]),
}

/// 1006 pulse: target is a group (key 52 = 1) or a color channel.
#[derive(Debug, Clone, Copy)]
pub struct Pulse {
    group: bool,
    color: PulseColor,
    /// fade in / hold / fade out, ticks
    fin: u64,
    hold: u64,
    fout: u64,
}

#[derive(Debug, Clone, Copy)]
struct Trig {
    /// level object index
    obj: usize,
    x: f64,
    /// touch hitbox half size
    half: f64,
    target: u16,
    ticks: usize,
    easing: u8,
    rate: f64,
    spawn: bool,
    touch: bool,
    multi: bool,
    kind: Kind,
}

#[derive(Debug, Clone, Copy)]
enum Act {
    Move { group: u16, dx: f64, dy: f64, lock_x: bool, lock_y: bool },
    Rotate { group: u16, deg: f64, center: Option<u16>, lock: bool },
    Follow { group: u16, other: u16, xm: f64, ym: f64, last: (f64, f64) },
    FollowY { group: u16, speed: f64, offset: f64, max: f64 },
}

#[derive(Debug, Clone, Copy)]
struct Run {
    src: usize,
    done: usize,
    total: usize,
    easing: u8,
    rate: f64,
    act: Act,
}

/// What the sim must apply after a trigger step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    End,
    Gravity(f64),
    /// hide (false) / show (true) the player icons
    ShowPlayer(bool),
    Options { on: u16, off: u16 },
    Song { id: u32, offset: f64 },
}

/// Options trigger (2899) keys, in OPT_* bit order (read off GD's
/// SetupOptionsTriggerPopup). Each key: 1 = enable, -1 = disable, 0 = keep.
/// shortcut: streak additive, unlink dual gravity, respawn time, audio/sfx
/// on death and boost slide are ignored.
pub const OPT_KEYS: [u16; 7] = [161, 195, 162, 163, 165, 199, 532];
pub const OPT_HIDE_GROUND: u16 = 1 << 0;
pub const OPT_HIDE_MG: u16 = 1 << 1;
pub const OPT_HIDE_P1: u16 = 1 << 2;
pub const OPT_NO_CTRL_P1: u16 = 1 << 4;
pub const OPT_HIDE_ATTEMPTS: u16 = 1 << 6;

/// Player state a trigger step reads (player 0 drives locks and follow-Y).
#[derive(Debug, Clone, Copy)]
pub struct PlayerView {
    pub x: f64,
    pub y: f64,
    pub dx: f64,
    pub dy: f64,
    pub half: f64,
}

fn raw<'a>(o: &'a GdObject, k: u16) -> Option<&'a str> {
    o.raw.iter().find(|(key, _)| *key == k).map(|(_, v)| v.as_str())
}

fn num(o: &GdObject, k: u16, default: f64) -> f64 {
    raw(o, k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(default)
}

fn flag(o: &GdObject, k: u16) -> bool {
    num(o, k, 0.0) != 0.0
}

fn group_key(o: &GdObject, k: u16) -> Option<u16> {
    raw(o, k).and_then(|v| v.parse::<u16>().ok()).filter(|&g| g != 0)
}

fn parse_trig(i: usize, o: &GdObject) -> Option<Trig> {
    let target = group_key(o, 51).unwrap_or(0);
    let on = flag(o, 56);
    let item = || num(o, 80, 0.0) as u16;
    let kind = match o.id {
        901 => Kind::Move {
            dx: num(o, 28, 0.0),
            dy: num(o, 29, 0.0),
            lock_x: flag(o, 58),
            lock_y: flag(o, 59),
            to: if flag(o, 100) { group_key(o, 71).map(|g| (g, num(o, 101, 0.0) as u8)) } else { None },
        },
        1049 => Kind::Toggle { on },
        1268 => Kind::Spawn { delay: (num(o, 63, 0.0) * TICK_HZ).round() as u64 },
        1346 => Kind::Rotate {
            deg: num(o, 68, 0.0) + 360.0 * num(o, 69, 0.0),
            center: group_key(o, 71),
            lock: flag(o, 70),
        },
        1347 => Kind::Follow { other: group_key(o, 71)?, xm: num(o, 72, 1.0), ym: num(o, 73, 1.0) },
        1814 => Kind::FollowY { speed: num(o, 90, 1.0), offset: num(o, 92, 0.0), max: num(o, 105, 0.0) },
        1616 => Kind::Stop,
        1812 => Kind::OnDeath { on },
        1611 => Kind::Count { item: item(), count: num(o, 77, 0.0) as i64, on },
        1811 => Kind::InstantCount { item: item(), count: num(o, 77, 0.0) as i64, on, cmp: num(o, 88, 0.0) as u8 },
        1817 => {
            let c = num(o, 77, 1.0) as i64;
            Kind::Pickup { item: item(), count: if flag(o, 78) { -c } else { c } }
        }
        1815 => Kind::Collision { a: item(), b: num(o, 95, 0.0) as u16, on, exit: flag(o, 93) },
        1595 => Kind::Touch { hold: flag(o, 81), mode: num(o, 82, 0.0) as u8 },
        2066 => Kind::Gravity(num(o, 148, 1.0)),
        3600 => Kind::End,
        1612 => Kind::ShowPlayer(false),
        1613 => Kind::ShowPlayer(true),
        // shortcut: volume (406), fade (10) and song channels are ignored
        1934 if flag(o, 399) => Kind::Nop,
        1934 => Kind::Song { id: num(o, 392, 0.0) as u32, offset: num(o, 408, 0.0) / 1000.0 },
        2899 => {
            let bits = |want: f64| OPT_KEYS.iter().enumerate().filter(|(_, &k)| num(o, k, 0.0) == want).fold(0, |m, (i, _)| m | 1 << i);
            Kind::Options { on: bits(1.0), off: bits(-1.0) }
        }
        1007 => Kind::Alpha { to: num(o, 35, 1.0) as f32 },
        1006 => {
            let secs = |k| (num(o, k, 0.0).max(0.0) * TICK_HZ).round() as u64;
            let hsv = || -> Option<[f32; 5]> {
                let mut h = [0.0, 1.0, 1.0, 0.0, 0.0];
                for (i, p) in raw(o, 49)?.split('a').take(5).enumerate() {
                    h[i] = p.parse().ok()?;
                }
                Some(h)
            };
            let color = match hsv().filter(|_| flag(o, 48)) {
                Some(h) => PulseColor::Hsv(h),
                None => PulseColor::Rgb(num(o, 7, 255.0) as u8, num(o, 8, 255.0) as u8, num(o, 9, 255.0) as u8),
            };
            Kind::Pulse(Pulse { group: num(o, 52, 0.0) == 1.0, color, fin: secs(45), hold: secs(46), fout: secs(47) })
        }
        _ => return None,
    };
    let scale = (o.scale_x.abs().max(o.scale_y.abs()) as f64).max(1e-3);
    Some(Trig {
        obj: i,
        x: o.x as f64,

        half: 15.0 * scale,
        target,
        ticks: (num(o, 10, 0.0).max(0.0) * TICK_HZ).round() as usize,
        easing: num(o, 30, 0.0) as u8,
        rate: num(o, 85, 2.0),
        spawn: flag(o, 62),
        touch: flag(o, 11),
        multi: flag(o, 87) || flag(o, 104),
        kind,
    })
}

#[derive(Clone)]
pub struct TriggerEngine {
    // level data, read-only after `new`: shared between snapshots (Arc)
    trigs: Arc<Vec<Trig>>,
    /// x-cross triggers (not spawn/touch) sorted by x
    xcross: Arc<Vec<usize>>,
    /// touch-triggered triggers sorted by x
    touch: Arc<Vec<usize>>,
    max_touch_half: f64,
    trig_at: Arc<HashMap<usize, usize>>,
    members: Arc<HashMap<u16, Vec<usize>>>,
    pos0: Arc<Vec<(f64, f64)>>,
    groups: Arc<Vec<Vec<u16>>>,
    /// collision blocks (1816) by block id: (object, half size)
    blocks: Arc<HashMap<u16, Vec<(usize, f64)>>>,
    // --- live state (cleared by reset)
    cursor: usize,
    tick: u64,
    moved: HashMap<u16, (f64, f64)>,
    /// per-object displacement and extra rotation (deg, CW) from rotate triggers
    extra: HashMap<usize, (f64, f64, f64)>,
    off: HashSet<usize>,
    group_on: HashMap<u16, bool>,
    fired: HashSet<usize>,
    runs: Vec<Run>,
    /// (due tick, group to spawn, source trigger)
    delayed: Vec<(u64, u16, usize)>,
    items: HashMap<u16, i64>,
    /// listener triggers: on-death, count, collision, touch
    armed: Vec<usize>,
    colliding: HashSet<usize>,
    /// hold-mode touch triggers to revert on release: (group, state to restore)
    held: Vec<(u16, bool)>,
    events: Vec<Event>,
    /// group opacity fades: group -> (from, to, start tick, ticks)
    alpha: HashMap<u16, (f32, f32, u64, u64)>,
    /// running pulses: (target, start tick, pulse)
    pulses: Vec<(u16, u64, Pulse)>,
    trace: bool,
}

impl TriggerEngine {
    pub fn new(level: &Level) -> Self {
        let mut trigs = Vec::new();
        let mut members: HashMap<u16, Vec<usize>> = HashMap::default();
        let mut blocks: HashMap<u16, Vec<(usize, f64)>> = HashMap::default();
        for (i, o) in level.objects.iter().enumerate() {
            for &g in &o.groups {
                members.entry(g).or_default().push(i);
            }
            if o.id == 1816 {
                let s = (o.scale_x.abs().max(o.scale_y.abs()) as f64).max(1e-3);
                blocks.entry(num(o, 80, 0.0) as u16).or_default().push((i, 15.0 * s));
            }
            if let Some(t) = parse_trig(i, o) {
                trigs.push(t);
            }
        }
        trigs.sort_by(|a, b| a.x.total_cmp(&b.x));
        let xcross: Vec<usize> = (0..trigs.len()).filter(|&t| !trigs[t].spawn && !trigs[t].touch).collect();
        let touch: Vec<usize> = (0..trigs.len()).filter(|&t| trigs[t].touch && !trigs[t].spawn).collect();
        let max_touch_half = touch.iter().map(|&t| trigs[t].half).fold(0.0, f64::max);
        let trig_at: HashMap<usize, usize> = trigs.iter().enumerate().map(|(t, tr)| (tr.obj, t)).collect();
        TriggerEngine {
            trigs: Arc::new(trigs),
            xcross: Arc::new(xcross),
            touch: Arc::new(touch),
            max_touch_half,
            trig_at: Arc::new(trig_at),
            members: Arc::new(members),
            pos0: Arc::new(level.objects.iter().map(|o| (o.x as f64, o.y as f64 + crate::physics::EDITOR_Y0)).collect()),
            groups: Arc::new(level.objects.iter().map(|o| o.groups.clone()).collect()),
            blocks: Arc::new(blocks),
            cursor: 0,
            tick: 0,
            moved: HashMap::default(),
            extra: HashMap::default(),
            off: HashSet::default(),
            group_on: HashMap::default(),
            fired: HashSet::default(),
            runs: Vec::new(),
            delayed: Vec::new(),
            items: HashMap::default(),
            armed: Vec::new(),
            colliding: HashSet::default(),
            held: Vec::new(),
            events: Vec::new(),
            alpha: HashMap::default(),
            pulses: Vec::new(),
            trace: std::env::var("CAL_TTRACE").is_ok(),
        }
    }

    /// full restart (GD classic respawn): every trigger re-arms
    pub fn reset(&mut self) {
        self.cursor = 0;
        self.tick = 0;
        self.moved.clear();
        self.extra.clear();
        self.off.clear();
        self.group_on.clear();
        self.fired.clear();
        self.runs.clear();
        self.delayed.clear();
        self.items.clear();
        self.armed.clear();
        self.colliding.clear();
        self.held.clear();
        self.events.clear();
        self.alpha.clear();
        self.pulses.clear();
    }

    /// One 240Hz tick: x-cross and touch activations, due spawns, running
    /// actions, collision listeners. `players[0]` drives locks and follow-Y.
    pub fn step(&mut self, players: &[PlayerView]) {
        self.tick += 1;
        let now = self.tick;
        self.pulses.retain(|(_, s, p)| now <= s + p.fin + p.hold + p.fout);
        let lead_x = players.iter().map(|p| p.x).fold(f64::MIN, f64::max);
        while let Some(&t) = self.xcross.get(self.cursor) {
            if self.trigs[t].x > lead_x {
                break;
            }
            self.cursor += 1;
            if !self.off.contains(&self.trigs[t].obj) {
                self.activate(t, players.first(), 0);
            }
        }
        // touch-triggered: the player box meets the trigger's (scaled) box
        for p in players {
            let lo = self.touch.partition_point(|&t| self.trigs[t].x < p.x - p.half - self.max_touch_half);
            let hi = self.touch.partition_point(|&t| self.trigs[t].x <= p.x + p.half + self.max_touch_half);
            for k in lo..hi {
                let t = self.touch[k];
                let tr = self.trigs[t];
                let (tx, ty) = self.current_pos(tr.obj);
                let hit = (p.x - tx).abs() < p.half + tr.half && (p.y - ty).abs() < p.half + tr.half;
                if hit && !self.off.contains(&tr.obj) && (tr.multi || !self.fired.contains(&t)) {
                    self.fired.insert(t);
                    self.activate(t, players.first(), 0);
                }
            }
        }
        let tick = self.tick;
        let due: Vec<u16> = self.delayed.iter().filter(|d| d.0 <= tick).map(|d| d.1).collect();
        self.delayed.retain(|d| d.0 > tick);
        for g in due {
            self.spawn_group(g, players.first(), 1);
        }
        self.advance(players.first());
        self.check_collisions(players.first());
    }

    fn advance(&mut self, p0: Option<&PlayerView>) {
        let mut i = 0;
        while i < self.runs.len() {
            let mut r = self.runs[i];
            r.done += 1;
            let total = r.total.max(1);
            let a = if r.done == 1 { 0.0 } else { ease(r.easing, r.rate, (r.done - 1) as f64 / total as f64) };
            let b = if r.done >= total { 1.0 } else { ease(r.easing, r.rate, r.done as f64 / total as f64) };
            let f = b - a;
            match &mut r.act {
                Act::Move { group, dx, dy, lock_x, lock_y } => {
                    let (lx, ly) = p0.map(|p| (p.dx, p.dy)).unwrap_or((0.0, 0.0));
                    let mx = *dx * f + if *lock_x { lx } else { 0.0 };
                    let my = *dy * f + if *lock_y { ly } else { 0.0 };
                    self.shift_group(*group, mx, my);
                }
                Act::Rotate { group, deg, center, lock } => self.rotate_group(*group, *deg * f, *center, *lock),
                Act::Follow { group, other, xm, ym, last } => {
                    if let Some(cur) = self.anchor(*other) {
                        let (ddx, ddy) = (cur.0 - last.0, cur.1 - last.1);
                        *last = cur;
                        self.shift_group(*group, ddx * *xm, ddy * *ym);
                    }
                }
                Act::FollowY { group, speed, offset, max } => {
                    // shortcut: GD's follow-player-Y smoothing curve is not
                    // decompiled; this closes speed×gap per tick (speed 1 =
                    // locked), capped by max speed. Upgrade from the decomp.
                    if let (Some(p), Some(a)) = (p0, self.anchor(*group)) {
                        let mut dy = (p.y + *offset - a.1) * speed.clamp(0.0, 1.0);
                        if *max > 0.0 {
                            let cap = *max * 30.0 / TICK_HZ;
                            dy = dy.clamp(-cap, cap);
                        }
                        self.shift_group(*group, 0.0, dy);
                    }
                }
            }
            if r.done >= total {
                self.runs.swap_remove(i);
            } else {
                self.runs[i] = r;
                i += 1;
            }
        }
    }

    fn activate(&mut self, t: usize, p0: Option<&PlayerView>, depth: u32) {
        if depth > MAX_DEPTH {
            return;
        }
        let tr = self.trigs[t];
        if self.trace {
            eprintln!("TTR tick={} fire obj={} x={} kind={:?} tgt={}", self.tick, tr.obj, tr.x, tr.kind, tr.target);
        }
        let run = |act| Run { src: t, done: 0, total: tr.ticks, easing: tr.easing, rate: tr.rate, act };
        match tr.kind {
            Kind::Move { dx, dy, lock_x, lock_y, to } => {
                let (dx, dy) = match to.and_then(|(g, mode)| Some((self.anchor(g)?, self.anchor(tr.target)?, mode))) {
                    Some((dst, src, mode)) => {
                        let (ax, ay) = (dst.0 - src.0, dst.1 - src.1);
                        match mode {
                            1 => (ax, 0.0),
                            2 => (0.0, ay),
                            _ => (ax, ay),
                        }
                    }
                    None => (dx, dy),
                };
                if tr.ticks == 0 && !lock_x && !lock_y {
                    self.shift_group(tr.target, dx, dy);
                } else {
                    self.runs.push(run(Act::Move { group: tr.target, dx, dy, lock_x, lock_y }));
                }
            }
            Kind::Toggle { on } => self.set_group(tr.target, on),
            Kind::Spawn { delay } => {
                if delay == 0 {
                    self.spawn_group(tr.target, p0, depth + 1);
                } else {
                    self.delayed.push((self.tick + delay, tr.target, t));
                }
            }
            Kind::Rotate { deg, center, lock } => {
                if tr.ticks == 0 {
                    self.rotate_group(tr.target, deg, center, lock);
                } else {
                    self.runs.push(run(Act::Rotate { group: tr.target, deg, center, lock }));
                }
            }
            Kind::Follow { other, xm, ym } => {
                if let Some(last) = self.anchor(other) {
                    self.runs.push(run(Act::Follow { group: tr.target, other, xm, ym, last }));
                }
            }
            Kind::FollowY { speed, offset, max } => {
                self.runs.push(run(Act::FollowY { group: tr.target, speed, offset, max }));
            }
            Kind::Stop => {
                let g = tr.target;
                let (trigs, groups) = (&self.trigs, &self.groups);
                self.runs.retain(|r| !groups[trigs[r.src].obj].contains(&g));
                self.delayed.retain(|d| !groups[trigs[d.2].obj].contains(&g));
            }
            Kind::OnDeath { .. } | Kind::Count { .. } | Kind::Collision { .. } | Kind::Touch { .. } => {
                self.armed.push(t);
            }
            Kind::InstantCount { item, count, on, cmp } => {
                let v = self.items.get(&item).copied().unwrap_or(0);
                let ok = match cmp {
                    1 => v > count,
                    2 => v < count,
                    _ => v == count,
                };
                if ok {
                    self.activate_group_at(tr.target, on, p0, depth + 1);
                }
            }
            Kind::Pickup { item, count } => {
                *self.items.entry(item).or_insert(0) += count;
                let v = self.items[&item];
                for k in 0..self.armed.len() {
                    let at = self.armed[k];
                    let a = self.trigs[at];
                    if let Kind::Count { item: ci, count: cc, on } = a.kind {
                        if ci == item && cc == v && (a.multi || self.fired.insert(at)) {
                            self.activate_group_at(a.target, on, p0, depth + 1);
                        }
                    }
                }
            }
            Kind::Gravity(g) => self.events.push(Event::Gravity(g)),
            Kind::End => self.events.push(Event::End),
            Kind::ShowPlayer(v) => self.events.push(Event::ShowPlayer(v)),
            Kind::Options { on, off } => self.events.push(Event::Options { on, off }),
            Kind::Song { id, offset } => self.events.push(Event::Song { id, offset }),
            Kind::Nop => {}
            Kind::Alpha { to } => {
                let from = self.group_alpha(tr.target);
                self.alpha.insert(tr.target, (from, to, self.tick, tr.ticks as u64));
            }
            Kind::Pulse(p) => self.pulses.push((tr.target, self.tick, p)),
        }
    }

    fn check_collisions(&mut self, p0: Option<&PlayerView>) {
        for k in 0..self.armed.len() {
            let t = self.armed[k];
            let Kind::Collision { a, b, on, exit } = self.trigs[t].kind else { continue };
            let live = |e: &Self, id: u16| -> Vec<(f64, f64, f64)> {
                e.blocks
                    .get(&id)
                    .map(|v| {
                        v.iter()
                            .filter(|(o, _)| !e.off.contains(o))
                            .map(|&(o, h)| {
                                let (x, y) = e.current_pos(o);
                                (x, y, h)
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let (la, lb) = (live(self, a), live(self, b));
            let hit = la.iter().any(|&(ax, ay, ah)| {
                lb.iter().any(|&(bx, by, bh)| (ax - bx).abs() < ah + bh && (ay - by).abs() < ah + bh)
            });
            let was = self.colliding.contains(&t);
            if hit {
                self.colliding.insert(t);
            } else {
                self.colliding.remove(&t);
            }
            if (!exit && hit && !was) || (exit && !hit && was) {
                self.activate_group_at(self.trigs[t].target, on, p0, 1);
            }
        }
    }

    fn spawn_group(&mut self, g: u16, p0: Option<&PlayerView>, depth: u32) {
        let Some(objs) = self.members.get(&g) else { return };
        let ts: Vec<usize> = objs.iter().filter_map(|o| self.trig_at.get(o).copied()).collect();
        for t in ts {
            let tr = self.trigs[t];
            if tr.spawn && !self.off.contains(&tr.obj) && (tr.multi || self.fired.insert(t)) {
                self.activate(t, p0, depth);
            }
        }
    }

    fn set_group(&mut self, g: u16, on: bool) {
        self.group_on.insert(g, on);
        if let Some(objs) = self.members.get(&g) {
            for &o in objs {
                if on {
                    self.off.remove(&o);
                } else {
                    self.off.insert(o);
                }
            }
        }
    }

    fn activate_group_at(&mut self, g: u16, on: bool, p0: Option<&PlayerView>, depth: u32) {
        self.set_group(g, on);
        if on {
            self.spawn_group(g, p0, depth + 1);
        }
    }

    /// toggle orb / external activation
    pub fn activate_group(&mut self, g: u16, on: bool) {
        self.activate_group_at(g, on, None, 0);
    }

    /// on a click: armed touch triggers act
    pub fn press(&mut self) {
        for k in 0..self.armed.len() {
            let tr = self.trigs[self.armed[k]];
            if let Kind::Touch { hold, mode } = tr.kind {
                let was = self.group_on.get(&tr.target).copied().unwrap_or(true);
                let on = match mode {
                    1 => true,
                    2 => false,
                    _ => !was,
                };
                self.activate_group_at(tr.target, on, None, 0);
                if hold {
                    self.held.push((tr.target, was));
                }
            }
        }
    }

    /// on release: hold-mode touch triggers revert
    pub fn release(&mut self) {
        for (g, was) in std::mem::take(&mut self.held) {
            self.set_group(g, was);
        }
    }

    /// on player death: armed on-death triggers act
    pub fn death(&mut self) {
        for k in 0..self.armed.len() {
            let tr = self.trigs[self.armed[k]];
            if let Kind::OnDeath { on } = tr.kind {
                self.activate_group_at(tr.target, on, None, 0);
            }
        }
    }

    /// Rough heap bytes of the per-run state (what a clone copies; the
    /// level's own data is shared). Grows as a level toggles objects off.
    pub fn heap_bytes(&self) -> usize {
        fn map<K, V>(m: &HashMap<K, V>) -> usize {
            m.capacity() * (std::mem::size_of::<(K, V)>() + 1)
        }
        fn set<K>(s: &HashSet<K>) -> usize {
            s.capacity() * (std::mem::size_of::<K>() + 1)
        }
        fn vec<T>(v: &Vec<T>) -> usize {
            v.capacity() * std::mem::size_of::<T>()
        }
        map(&self.moved) + map(&self.extra) + set(&self.off) + map(&self.group_on) + set(&self.fired) + vec(&self.runs)
            + vec(&self.delayed) + map(&self.items) + vec(&self.armed) + set(&self.colliding) + vec(&self.held)
            + vec(&self.events) + map(&self.alpha) + vec(&self.pulses)
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn shift_group(&mut self, g: u16, dx: f64, dy: f64) {
        if dx != 0.0 || dy != 0.0 {
            let e = self.moved.entry(g).or_insert((0.0, 0.0));
            e.0 += dx;
            e.1 += dy;
        }
    }

    /// rotate every object of `g` by `deg` (CW): around the center group's
    /// anchor, or each around itself when there is no center
    fn rotate_group(&mut self, g: u16, deg: f64, center: Option<u16>, lock: bool) {
        if deg == 0.0 {
            return;
        }
        let pivot = center.and_then(|c| self.anchor(c));
        let Some(objs) = self.members.get(&g).cloned() else { return };
        let (s, c) = (-deg.to_radians()).sin_cos();
        for o in objs {
            let cur = self.current_pos(o);
            let e = self.extra.entry(o).or_insert((0.0, 0.0, 0.0));
            match pivot {
                Some((px, py)) => {
                    let (rx, ry) = (cur.0 - px, cur.1 - py);
                    e.0 += px + rx * c - ry * s - cur.0;
                    e.1 += py + rx * s + ry * c - cur.1;
                    if !lock {
                        e.2 += deg;
                    }
                }
                None => e.2 += deg,
            }
        }
    }

    /// current world position of a level object
    fn current_pos(&self, o: usize) -> (f64, f64) {
        let (ox, oy) = self.offset_for(&self.groups[o]);
        let (ex, ey, _) = self.extra.get(&o).copied().unwrap_or_default();
        (self.pos0[o].0 + ox + ex, self.pos0[o].1 + oy + ey)
    }

    /// a group's reference point: its first object's current position
    fn anchor(&self, g: u16) -> Option<(f64, f64)> {
        self.members.get(&g)?.first().map(|&o| self.current_pos(o))
    }

    pub fn offset_for(&self, groups: &[u16]) -> (f64, f64) {
        let mut d = (0.0, 0.0);
        for g in groups {
            if let Some(m) = self.moved.get(g) {
                d.0 += m.0;
                d.1 += m.1;
            }
        }
        d
    }

    /// live (dx, dy, extra rotation deg CW) of a level object; None = toggled off
    pub fn transform(&self, o: usize) -> Option<(f64, f64, f64)> {
        if self.off.contains(&o) {
            return None;
        }
        let (ox, oy) = self.offset_for(&self.groups[o]);
        let (ex, ey, er) = self.extra.get(&o).copied().unwrap_or_default();
        Some((ox + ex, oy + ey, er))
    }

    /// live opacity of one group (alpha triggers), 1 when never faded
    pub fn group_alpha(&self, g: u16) -> f32 {
        match self.alpha.get(&g) {
            Some(&(from, to, start, ticks)) => {
                let t = if ticks == 0 { 1.0 } else { ((self.tick - start) as f32 / ticks as f32).min(1.0) };
                from + (to - from) * t
            }
            None => 1.0,
        }
    }

    /// live opacity of a level object: product over its groups
    /// current value of an item id (count / pickup triggers)
    pub fn item(&self, id: u16) -> i64 {
        self.items.get(&id).copied().unwrap_or(0)
    }

    pub fn alpha(&self, o: usize) -> f32 {
        if self.alpha.is_empty() {
            return 1.0;
        }
        self.groups[o].iter().map(|&g| self.group_alpha(g)).product()
    }

    /// pulses hitting a level object drawn with channel `chan`, oldest first:
    /// (color to blend toward, blend weight 0..1)
    pub fn pulses(&self, o: usize, chan: u16) -> impl Iterator<Item = (PulseColor, f32)> + '_ {
        self.pulses.iter().filter_map(move |&(target, start, p)| {
            let hit = if p.group { self.groups[o].contains(&target) } else { target == chan };
            if !hit {
                return None;
            }
            let t = self.tick - start;
            let w = if t < p.fin {
                t as f32 / p.fin as f32
            } else if t <= p.fin + p.hold {
                1.0
            } else if p.fout == 0 {
                0.0
            } else {
                1.0 - (t - p.fin - p.hold) as f32 / p.fout as f32
            };
            Some((p.color, w.clamp(0.0, 1.0)))
        })
    }

    /// every group with a live offset
    pub fn moved(&self) -> impl Iterator<Item = (u16, (f64, f64))> + '_ {
        self.moved.iter().map(|(g, d)| (*g, *d))
    }

    /// objects carried far from their spot by rotations (beyond the grid pad)
    pub fn far_rotated(&self) -> impl Iterator<Item = usize> + '_ {
        self.extra.iter().filter(|(_, e)| e.0.abs().max(e.1.abs()) > NEAR_MOVE).map(|(o, _)| *o)
    }
}

/// moves up to this far are found by the collision query's pad; farther
/// moved groups and rotated objects get their own lookups
pub const NEAR_MOVE: f64 = 40.0;

/// cocos2d easing curves by GD easing id (OpenGD actionEasing); rate is the
/// ease rate, or the period for elastic eases
fn ease(kind: u8, rate: f64, t: f64) -> f64 {
    use std::f64::consts::PI;
    const BACK: f64 = 1.70158;
    match kind {
        1 => {
            let t2 = t * 2.0;
            if t2 < 1.0 {
                0.5 * t2.powf(rate)
            } else {
                1.0 - 0.5 * (2.0 - t2).powf(rate)
            }
        }
        2 => t.powf(rate),
        3 if rate != 0.0 => t.powf(1.0 / rate),
        4 => {
            if t == 0.0 || t == 1.0 {
                return t;
            }
            let p = if rate == 0.0 { 0.3 * 1.5 } else { rate };
            let s = p / 4.0;
            let t = t * 2.0 - 1.0;
            if t < 0.0 {
                -0.5 * 2f64.powf(10.0 * t) * ((t - s) * PI * 2.0 / p).sin()
            } else {
                2f64.powf(-10.0 * t) * ((t - s) * PI * 2.0 / p).sin() * 0.5 + 1.0
            }
        }
        5 => {
            if t == 0.0 || t == 1.0 {
                return t;
            }
            let s = rate / 4.0;
            let t = t - 1.0;
            -(2f64.powf(10.0 * t)) * ((t - s) * PI * 2.0 / rate).sin()
        }
        6 => elastic_out(t, rate),
        7 => {
            if t < 0.5 {
                (1.0 - bounce_out(1.0 - t * 2.0)) * 0.5
            } else {
                bounce_out(t * 2.0 - 1.0) * 0.5 + 0.5
            }
        }
        8 => 1.0 - bounce_out(1.0 - t),
        9 => bounce_out(t),
        10 => {
            if t == 0.0 || t == 1.0 {
                return t;
            }
            let t = t * 2.0;
            if t < 1.0 {
                0.5 * 2f64.powf(10.0 * (t - 1.0))
            } else {
                0.5 * (2.0 - 2f64.powf(-10.0 * (t - 1.0)))
            }
        }
        11 => {
            if t == 0.0 {
                0.0
            } else {
                2f64.powf(10.0 * (t - 1.0)) - 0.001
            }
        }
        12 => {
            if t == 1.0 {
                1.0
            } else {
                1.0 - 2f64.powf(-10.0 * t)
            }
        }
        13 => -0.5 * ((PI * t).cos() - 1.0),
        14 => 1.0 - (t * PI / 2.0).cos(),
        15 => (t * PI / 2.0).sin(),
        16 => {
            let o = BACK * 1.525;
            let t = t * 2.0;
            if t < 1.0 {
                t * t * ((o + 1.0) * t - o) / 2.0
            } else {
                let t = t - 2.0;
                t * t * ((o + 1.0) * t + o) / 2.0 + 1.0
            }
        }
        17 => t * t * ((BACK + 1.0) * t - BACK),
        18 => {
            let u = t - 1.0;
            1.0 + (BACK + 1.0) * u * u * u + BACK * u * u
        }
        _ => t,
    }
}

fn bounce_out(t: f64) -> f64 {
    if t < 1.0 / 2.75 {
        7.5625 * t * t
    } else if t < 2.0 / 2.75 {
        let t = t - 1.5 / 2.75;
        7.5625 * t * t + 0.75
    } else if t < 2.25 / 2.75 {
        let t = t - 2.25 / 2.75;
        7.5625 * t * t + 0.9375
    } else {
        let t = t - 2.625 / 2.75;
        7.5625 * t * t + 0.984375
    }
}

fn elastic_out(t: f64, period: f64) -> f64 {
    if t == 0.0 || t == 1.0 {
        return t;
    }
    let s = period / 4.0;
    2f64.powf(-10.0 * t) * ((t - s) * std::f64::consts::PI * 2.0 / period).sin() + 1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::GdObject;

    fn obj(id: u16, x: f32, y: f32, groups: &[u16], raw: &[(u16, &str)]) -> GdObject {
        GdObject {
            id,
            x,
            y,
            groups: groups.to_vec(),
            raw: raw.iter().map(|(k, v)| (*k, v.to_string())).collect(),
            ..Default::default()
        }
    }

    fn level(objs: Vec<GdObject>) -> Level {
        Level { objects: objs, ..Default::default() }
    }

    fn view(x: f64, y: f64) -> PlayerView {
        PlayerView { x, y, dx: 0.0, dy: 0.0, half: 15.0 }
    }

    #[test]
    fn timed_move_eases_lands_exactly_and_resets() {
        let lvl = level(vec![
            obj(1, 500.0, 0.0, &[7], &[]),
            obj(901, 0.0, 0.0, &[], &[(51, "7"), (28, "60"), (10, "1"), (30, "1"), (85, "2")]),
        ]);
        let mut t = TriggerEngine::new(&lvl);
        for _ in 0..120 {
            t.step(&[view(10.0, 105.0)]);
        }
        let (mx, _) = t.offset_for(&[7]);
        assert!((mx - 30.0).abs() < 1e-9, "ease-in-out midpoint {mx}");
        for _ in 0..200 {
            t.step(&[view(10.0, 105.0)]);
        }
        let (mx, my) = t.offset_for(&[7]);
        assert!((mx - 60.0).abs() < 1e-9 && my == 0.0, "landed at {mx}");
        t.reset();
        assert_eq!(t.offset_for(&[7]), (0.0, 0.0));
    }

    #[test]
    fn alpha_fades_group_and_pulse_holds_then_fades() {
        let lvl = level(vec![
            obj(1, 500.0, 0.0, &[4], &[]),
            obj(1007, 0.0, 0.0, &[], &[(51, "4"), (10, "1"), (35, "0")]),
            obj(1006, 0.0, 0.0, &[], &[(51, "4"), (52, "1"), (7, "0"), (8, "0"), (9, "0"), (45, "0"), (46, "1"), (47, "1")]),
        ]);
        let mut t = TriggerEngine::new(&lvl);
        assert_eq!(t.alpha(0), 1.0);
        for _ in 0..120 {
            t.step(&[view(10.0, 105.0)]);
        }
        assert!((t.alpha(0) - 0.5).abs() < 0.01, "half faded {}", t.alpha(0));
        assert_eq!(t.pulses(0, 1004).collect::<Vec<_>>(), vec![(PulseColor::Rgb(0, 0, 0), 1.0)]);
        for _ in 0..240 {
            t.step(&[view(10.0, 105.0)]);
        }
        let w = t.pulses(0, 1004).next().map(|p| p.1).unwrap();
        assert!((w - 0.5).abs() < 0.01, "fading out {w}");
        assert_eq!(t.alpha(0), 0.0);
        for _ in 0..200 {
            t.step(&[view(10.0, 105.0)]);
        }
        assert_eq!(t.pulses(0, 1004).count(), 0, "pulse expired");
    }

    #[test]
    fn every_easing_starts_at_0_and_ends_at_1() {
        for k in 0..=18u8 {
            let rate = if (4..=6).contains(&k) { 0.5 } else { 2.0 };
            assert!(ease(k, rate, 0.0).abs() < 1e-2, "ease {k} at 0");
            assert!((ease(k, rate, 1.0) - 1.0).abs() < 1e-2, "ease {k} at 1");
        }
    }

    #[test]
    fn toggle_off_hides_group_and_blocks_its_triggers() {
        let lvl = level(vec![
            obj(1, 300.0, 0.0, &[3], &[]),
            obj(1049, 0.0, 0.0, &[], &[(51, "3")]), // 56 unset: toggle off
            obj(901, 100.0, 0.0, &[3], &[(51, "8"), (28, "30")]), // in group 3: blocked
        ]);
        let mut t = TriggerEngine::new(&lvl);
        t.step(&[view(200.0, 105.0)]);
        assert_eq!(t.transform(0), None);
        assert_eq!(t.offset_for(&[8]), (0.0, 0.0));
    }

    #[test]
    fn spawn_delay_fires_spawn_triggered_move_once_and_stop_cancels() {
        let lvl = level(vec![
            obj(1268, 0.0, 0.0, &[], &[(51, "5"), (63, "0.5")]),
            obj(901, 900.0, 0.0, &[5], &[(51, "9"), (29, "30"), (62, "1")]),
            obj(901, 900.0, 0.0, &[5], &[(51, "10"), (29, "30"), (10, "10"), (62, "1")]),
            obj(1616, 0.0, 0.0, &[6], &[(51, "5"), (62, "1")]),
            obj(1268, 50.0, 0.0, &[], &[(51, "6"), (63, "1")]),
        ]);
        let mut t = TriggerEngine::new(&lvl);
        for _ in 0..120 {
            t.step(&[view(60.0, 105.0)]);
        }
        assert_eq!(t.offset_for(&[9]), (0.0, 0.0), "fired before its delay");
        t.step(&[view(60.0, 105.0)]);
        assert_eq!(t.offset_for(&[9]), (0.0, 30.0));
        let before = t.offset_for(&[10]).1;
        assert!(before > 0.0);
        for _ in 0..200 {
            t.step(&[view(60.0, 105.0)]); // stop spawns at 240 ticks
        }
        let at_stop = t.offset_for(&[10]).1;
        for _ in 0..240 {
            t.step(&[view(60.0, 105.0)]);
        }
        assert_eq!(t.offset_for(&[10]).1, at_stop, "stop must cancel the running move");
        assert!(at_stop < 30.0);
    }

    #[test]
    fn rotate_around_center_carries_object_and_turns_it() {
        let lvl = level(vec![
            obj(1, 330.0, 0.0, &[2], &[]),   // 30 right of the center
            obj(1, 300.0, 0.0, &[4], &[]),   // center object
            obj(1346, 0.0, 0.0, &[], &[(51, "2"), (71, "4"), (68, "90")]),
        ]);
        let mut t = TriggerEngine::new(&lvl);
        t.step(&[view(10.0, 105.0)]);
        let (dx, dy, dr) = t.transform(0).unwrap();
        // 90° clockwise: (+30, 0) relative -> (0, -30)
        assert!((dx + 30.0).abs() < 1e-9 && (dy + 30.0).abs() < 1e-9, "({dx},{dy})");
        assert_eq!(dr, 90.0);
    }

    #[test]
    fn follow_copies_the_other_groups_motion() {
        let lvl = level(vec![
            obj(1, 300.0, 0.0, &[1], &[]),
            obj(1, 400.0, 0.0, &[2], &[]),
            obj(1347, 0.0, 0.0, &[], &[(51, "2"), (71, "1"), (72, "0.5"), (73, "1"), (10, "10")]),
            obj(901, 5.0, 0.0, &[], &[(51, "1"), (28, "100"), (29, "40"), (10, "1")]),
        ]);
        let mut t = TriggerEngine::new(&lvl);
        for _ in 0..300 {
            t.step(&[view(10.0, 105.0)]);
        }
        let (fx, fy) = t.offset_for(&[2]);
        assert!((fx - 50.0).abs() < 1e-6 && (fy - 40.0).abs() < 1e-6, "({fx},{fy})");
    }

    #[test]
    fn touch_clicks_count_up_to_a_toggle() {
        // GRIEF's click counter: touch -> spawn-triggered pickup -> count
        let lvl = level(vec![
            obj(1595, 0.0, 0.0, &[], &[(51, "20"), (82, "1")]),
            obj(1817, 0.0, 0.0, &[20], &[(80, "2"), (77, "1"), (62, "1"), (87, "1")]),
            obj(1611, 0.0, 0.0, &[], &[(51, "21"), (80, "2"), (77, "3"), (56, "1")]),
            obj(1, 500.0, 0.0, &[21], &[]),
            obj(1049, -10.0, 0.0, &[], &[(51, "21")]), // starts off
        ]);
        let mut t = TriggerEngine::new(&lvl);
        t.step(&[view(10.0, 105.0)]);
        assert_eq!(t.transform(3), None);
        for _ in 0..2 {
            t.press();
            t.release();
        }
        assert_eq!(t.transform(3), None, "two clicks are not enough");
        t.press();
        assert!(t.transform(3).is_some(), "third click must turn the group on");
    }

    #[test]
    fn touch_triggered_fires_on_contact_only() {
        let lvl = level(vec![
            obj(1, 900.0, 0.0, &[3], &[]),
            obj(901, 100.0, 300.0, &[], &[(51, "3"), (29, "30"), (11, "1")]),
        ]);
        let mut t = TriggerEngine::new(&lvl);
        t.step(&[view(100.0, 105.0)]); // passes under it: no contact
        assert_eq!(t.offset_for(&[3]), (0.0, 0.0));
        t.step(&[view(100.0, 400.0)]);
        assert_eq!(t.offset_for(&[3]), (0.0, 30.0));
    }

    #[test]
    fn collision_blocks_meeting_activate_the_target() {
        let lvl = level(vec![
            obj(1816, 300.0, 0.0, &[1], &[(80, "1")]),
            obj(1816, 400.0, 0.0, &[], &[(80, "2")]),
            obj(1815, 0.0, 0.0, &[], &[(80, "1"), (95, "2"), (51, "9"), (56, "1")]),
            obj(1, 0.0, 0.0, &[9], &[]),
            obj(1049, -10.0, 0.0, &[], &[(51, "9")]),
            obj(901, 1.0, 0.0, &[], &[(51, "1"), (28, "100"), (10, "1")]),
        ]);
        let mut t = TriggerEngine::new(&lvl);
        t.step(&[view(10.0, 105.0)]);
        assert_eq!(t.transform(3), None);
        for _ in 0..240 {
            t.step(&[view(10.0, 105.0)]);
        }
        assert!(t.transform(3).is_some());
    }

    #[test]
    fn on_death_activates_its_group() {
        let lvl = level(vec![
            obj(1812, 0.0, 0.0, &[], &[(51, "4"), (56, "1")]),
            obj(901, 0.0, 0.0, &[4], &[(51, "5"), (29, "10"), (62, "1")]),
        ]);
        let mut t = TriggerEngine::new(&lvl);
        t.step(&[view(10.0, 105.0)]);
        assert_eq!(t.offset_for(&[5]), (0.0, 0.0));
        t.death();
        assert_eq!(t.offset_for(&[5]), (0.0, 10.0));
    }
}
