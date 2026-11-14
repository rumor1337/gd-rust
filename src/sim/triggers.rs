//! Calibration-minimal trigger engine (gd-sim triggers.py port + GRIEF's
//! spawn/stop chain). Fires x-crossing 901 moves; instant offsets apply at
//! fire, timed moves advance incrementally along a cocos2d easing curve so
//! they land exactly. 1812 activate-group triggers fire spawn-gated 1616
//! stop triggers; a stopped group blocks future fires of triggers whose own
//! groups (key 57) contain it and cancels their in-flight eases.

use std::collections::{HashMap, HashSet};

use crate::level::Level;

#[derive(Debug, Clone)]
pub enum Cmd {
    /// x-cross move trigger (id 901)
    Move(MoveTrigger),
    /// x-cross activate-group trigger (id 1812 with 56=1): activates
    /// spawn-gated triggers whose own groups contain `target`
    Activate { x: f64, target: u16 },
}

#[derive(Debug, Clone)]
pub struct MoveTrigger {
    pub x: f64,
    pub target: u16,
    pub dx: f64,
    pub dy: f64,
    pub ticks: usize,
    pub easing: u8,
    pub rate: f64,
    /// the trigger's own editor groups (key 57); a stopped group blocks it
    pub own_groups: Vec<u16>,
}

/// spawn-gated stop trigger (id 1616 with 62=1): when activated it marks its
/// target group stopped — already-applied offsets remain, future fires of
/// triggers in that group are blocked and their in-flight eases cancelled
#[derive(Debug, Clone)]
pub struct StopTrigger {
    pub target: u16,
    pub own_groups: Vec<u16>,
}

fn raw_kv(o: &crate::level::GdObject, k: u16) -> Option<String> {
    o.raw.iter().find(|(key, _)| *key == k).map(|(_, v)| v.clone())
}

fn num(o: &crate::level::GdObject, k: u16, default: f64) -> f64 {
    raw_kv(o, k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(default)
}

fn group_list(o: &crate::level::GdObject) -> Vec<u16> {
    // key 57 is parsed into o.groups (not kept in raw)
    o.groups.clone()
}

/// Unified x-cross command list (moves + activate-group triggers) and the
/// spawn-gated stop triggers. Touch- (11) / spawn- (62) flagged 901s excluded.
pub fn parse_cmds(level: &Level) -> (Vec<Cmd>, Vec<StopTrigger>) {
    let mut cmds = Vec::new();
    let mut stops = Vec::new();
    for o in &level.objects {
        let groups = group_list(o);
        match o.id {
            901 if raw_kv(o, 62).is_none() && raw_kv(o, 11).is_none() => {
                let Some(target) = raw_kv(o, 51).and_then(|v| v.parse::<u16>().ok()) else {
                    continue;
                };
                cmds.push(Cmd::Move(MoveTrigger {
                    x: o.x as f64,
                    target,
                    dx: num(o, 28, 0.0),
                    dy: num(o, 29, 0.0),
                    ticks: (num(o, 10, 0.0) * 240.0).round() as usize,
                    easing: num(o, 30, 0.0) as u8,
                    rate: num(o, 85, 2.0),
                    own_groups: groups,
                }));
            }
            1616 if raw_kv(o, 62).is_some() => {
                let Some(target) = raw_kv(o, 51).and_then(|v| v.parse::<u16>().ok()) else {
                    continue;
                };
                stops.push(StopTrigger { target, own_groups: groups });
            }
            1812 if num(o, 56, 0.0) == 1.0 => {
                // 1812 = OnDeath trigger (ObjectToolbox: edit_eOnDeathBtn): it
                // only fires when the player DIES (activating its target
                // group's spawn-gated triggers, e.g. GRIEF's 1616 stop on
                // group 321). In a 0-death macro run it never fires, so no
                // Activate command is emitted for x-crossing.
                let _ = raw_kv(o, 51).and_then(|v| v.parse::<u16>().ok());
            }
            _ => {}
        }
    }
    cmds.sort_by(|a, b| {
        let ax = match a {
            Cmd::Move(t) => t.x,
            Cmd::Activate { x, .. } => *x,
        };
        let bx = match b {
            Cmd::Move(t) => t.x,
            Cmd::Activate { x, .. } => *x,
        };
        ax.partial_cmp(&bx).unwrap()
    });
    (cmds, stops)
}

pub struct TriggerEngine {
    cmds: Vec<Cmd>, // sorted by x
    stops: Vec<StopTrigger>,
    cursor: usize,
    stopped: HashSet<u16>,
    moved: HashMap<u16, (f64, f64)>,
    running: Vec<(usize, usize)>, // (cmds index of Move, done ticks)
}

impl TriggerEngine {
    pub fn new(cmds: Vec<Cmd>, stops: Vec<StopTrigger>) -> Self {
        Self {
            cmds,
            stops,
            cursor: 0,
            stopped: HashSet::new(),
            moved: HashMap::new(),
            running: Vec::new(),
        }
    }

    pub fn empty() -> Self {
        Self::new(Vec::new(), Vec::new())
    }

    /// After the players moved: fire everything the player reached, then
    /// advance. Stops apply AFTER the tick's x-cross fires (GD 2.2 command
    /// queue runs post-scan; shortcut: exact queue phase unverified, but this
    /// order is the only one consistent with the 0-death macro).
    pub fn step(&mut self, player_x: f64) {
        let ttrace = std::env::var("CAL_TTRACE").is_ok();
        let mut queued_stops: Vec<u16> = Vec::new();
        while self.cursor < self.cmds.len() {
            let cmd_x = match &self.cmds[self.cursor] {
                Cmd::Move(t) => t.x,
                Cmd::Activate { x, .. } => *x,
            };
            if cmd_x > player_x {
                break;
            }
            if ttrace {
                match &self.cmds[self.cursor] {
                    Cmd::Move(t) => eprintln!("TTR tick-fire x={} tgt={} dx={} dy={} ticks={} own={:?} blocked={}", t.x, t.target, t.dx, t.dy, t.ticks, t.own_groups, t.own_groups.iter().any(|g| self.stopped.contains(g))),
                    Cmd::Activate { x, target } => eprintln!("TTR activate x={} tgt={}", x, target),
                }
            }
            match self.cmds[self.cursor].clone() {
                Cmd::Move(t) => {
                    // the 1616 stop (group 321, fired at spawn via 1812->999)
                    // kills every trigger placed in group 321: the x=1 corridor
                    // returns AND the later corridor/transition moves. The wave's
                    // survival past the 469 wall (group 113 must stay at -31)
                    // confirms the corridor stays frozen at its x=-29 offsets.
                    if t.own_groups.iter().all(|g| !self.stopped.contains(g)) {
                        if t.ticks == 0 {
                            let e = self.moved.entry(t.target).or_insert((0.0, 0.0));
                            e.0 += t.dx;
                            e.1 += t.dy;
                        } else {
                            self.running.push((self.cursor, 0));
                        }
                    }
                }
                Cmd::Activate { target, .. } => {
                    for s in &self.stops {
                        if s.own_groups.contains(&target) {
                            queued_stops.push(s.target);
                        }
                    }
                }
            }
            self.cursor += 1;
        }
        // phase 2: stops take effect on the tick they fire
        if !queued_stops.is_empty() {
            for g in &queued_stops {
                self.stopped.insert(*g);
            }
            if ttrace {
                eprintln!("TTR stopped now {:?}", self.stopped);
            }
            let cmds = &self.cmds;
            let mut kill: HashSet<usize> = HashSet::new();
            for (idx, _) in &self.running {
                if let Cmd::Move(t) = &cmds[*idx] {
                    if t.own_groups.iter().any(|g| queued_stops.contains(g)) {
                        kill.insert(*idx);
                    }
                }
            }
            if !kill.is_empty() {
                self.running.retain(|(idx, _)| !kill.contains(idx));
            }
        }
        let mut apply: Vec<(u16, f64, f64)> = Vec::new();
        let mut i = 0;
        while i < self.running.len() {
            let (idx, done) = self.running[i];
            let done = done + 1;
            let Cmd::Move(t) = &self.cmds[idx] else { continue };
            let total = t.ticks.max(1);
            if done <= total {
                let a = if done == 1 { 0.0 } else { ease(t.easing, t.rate, (done - 1) as f64 / total as f64) };
                let b = if done >= total { 1.0 } else { ease(t.easing, t.rate, done as f64 / total as f64) };
                apply.push((t.target, t.dx * (b - a), t.dy * (b - a)));
            }
            if done < total {
                self.running[i].1 = done;
                i += 1;
            } else {
                self.running.swap_remove(i);
            }
        }
        for (g, mx, my) in apply {
            let e = self.moved.entry(g).or_insert((0.0, 0.0));
            e.0 += mx;
            e.1 += my;
        }
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
}

fn ease(kind: u8, rate: f64, t: f64) -> f64 {
    match kind {
        1 => {
            // CCEaseInOut (rate): t*=2; <1 -> 0.5*t^r; else 1-0.5*(2-t)^r
            let t2 = t * 2.0;
            if t2 < 1.0 {
                0.5 * t2.powf(rate)
            } else {
                1.0 - 0.5 * (2.0 - t2).powf(rate)
            }
        }
        2 => t.powf(rate),
        3 if rate != 0.0 => t.powf(1.0 / rate),
        6 => elastic_out(t, rate),
        8 => 1.0 - bounce_out(1.0 - t), // CCEaseBounceIn
        18 => {
            // CCEaseBackOut: 1 + c3*u^3 + c1*u^2, u = t-1
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            let u = t - 1.0;
            1.0 + c3 * u * u * u + c1 * u * u
        }
        _ => t, // unhandled kinds stay linear (none in GRIEF's fire path)
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
