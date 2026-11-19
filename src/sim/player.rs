//! Player state + per-substep physics (OpenGD PlayerObject model).

use crate::physics::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    Cube,
    Ship,
    Ball,
    Ufo,
    Wave,
    Robot,
    Spider,
    Swing,
}

#[derive(Debug, Clone)]
pub struct Player {
    /// recorded in-game rotation (deg, clockwise) from replay fixes; render only
    pub vis_rot: Option<f64>,
    /// world center
    pub x: f64,
    pub y: f64,
    pub vel_y: f64,
    pub mode: GameMode,
    /// true = gravity pulls up (inverted)
    pub grav_flip: bool,
    pub mini: bool,
    pub speed_idx: u8,
    pub on_ground: bool,
    /// input held this substep
    pub hold: bool,
    /// press buffered for orb activation (cleared on use / ground touch)
    pub queued_hold: bool,
    /// frames since robot jump start (robot sustained jump)
    pub robot_frames: f64,
    /// y movement of the last tick (post-collision) — true travel direction
    pub last_move_dy: f64,
    /// y before this tick's move (collisions: which side the player came from)
    pub prev_y: f64,
    /// riding a slope this tick (GD m_isOnSlope; cleared each pass)
    pub slope_on: bool,
    /// GD m_isCurrentSlopeTop: slopeFloorTop of the last ridden slope
    pub slope_top: Option<bool>,
    /// GD m_collidingWithSlopeId: unique index of the last ridden slope
    pub slope_id: Option<usize>,
    /// player radius on the last ridden slope (rad)
    pub slope_rad: f64,
    /// gravity trigger multiplier (key 148)
    pub gravity_mult: f64,
    /// (floor, ceiling) world y for this mode; cube/robot: ground, no ceiling
    pub band: (f64, f64),
    /// ball: a press is pending (one flip per press; OpenGD clears isHolding)
    pub ball_ready: bool,
    /// dash orb in progress: dy per unit dx (tan of the dash angle)
    pub dash: Option<f64>,
}

/// GD rounds velocity (in these units) to 0.001 after each update
/// (gd-sim roundVel; the real GRIEF run shows 3-decimal ship velocities)
fn round_vel(v: f64) -> f64 {
    let t = v.trunc();
    if v == t { v } else { ((v - t) * 1000.0).round() / 1000.0 + t }
}

/// GD's camera band for a flying-family mode entered at `y` (OpenGD
/// PlayLayer::changeGameMode): ship/ufo/wave/swing get 10 blocks, ball and
/// spider 8, centered on 240/210 low down or on the portal's 30-grid row.
pub fn band_for(mode: GameMode, y: f64) -> (f64, f64) {
    let (low, cut, half) = match mode {
        GameMode::Ship | GameMode::Ufo | GameMode::Wave | GameMode::Swing => (240.0, 270.0, 150.0),
        GameMode::Ball | GameMode::Spider => (210.0, 240.0, 120.0),
        GameMode::Cube | GameMode::Robot => return (GROUND_Y, CEIL_Y),
    };
    let c = if y < cut { low } else { (y / 30.0).floor() * 30.0 };
    ((c - half).max(GROUND_Y), c + half)
}

impl Player {
    pub fn new(mode: GameMode, speed_idx: u8) -> Player {
        Player {
            vis_rot: None,
            x: SPAWN_X,
            y: SPAWN_Y,
            vel_y: 0.0,
            mode,
            grav_flip: false,
            mini: false,
            speed_idx,
            on_ground: true,
            hold: false,
            queued_hold: false,
            robot_frames: 99.0,
            last_move_dy: 0.0,
            prev_y: 0.0,
            slope_on: false,
            slope_top: None,
            slope_id: None,
            slope_rad: 0.0,
            gravity_mult: 1.0,
            band: band_for(mode, SPAWN_Y),
            ball_ready: false,
            dash: None,
        }
    }

    pub fn size(&self) -> f64 {
        match self.mode {
            GameMode::Ship => if self.mini { PLAYER_SIZE_MINI_SHIP } else { PLAYER_SIZE_FULL },
            _ => if self.mini { PLAYER_SIZE_MINI } else { PLAYER_SIZE_FULL },
        }
    }

    pub fn outer_half(&self) -> f64 {
        // wave box is 10×10 (OSGD toggleDartMode), mini-scaled 0.6 → 6
        if self.mode == GameMode::Wave {
            if self.mini { 3.0 } else { 5.0 }
        } else if self.mini { OUTER_HALF_MINI } else { OUTER_HALF }
    }

    /// Physical half-height for floor and ceiling contact (bounds_318190: 15.0 * scale)
    pub fn phys_half(&self) -> f64 {
        // wave: measured from xdBot frame fixes, the mini wave rests at
        // floor + 6 (= 10 * 0.6)
        let full = if self.mode == GameMode::Wave { 10.0 } else { 15.0 };
        if self.mini { full * 0.6 } else { full }
    }

    /// gravity sign: +1 pulls down (normal), -1 pulls up
    pub fn grav_sign(&self) -> f64 {
        if self.grav_flip { -1.0 } else { 1.0 }
    }

    pub fn falling(&self) -> bool {
        self.vel_y * self.grav_sign() < -GRAVITY * 0.5
    }

    fn speed_units(&self) -> f64 {
        SPEEDS[self.speed_idx as usize % 5]
    }

    /// dx for this substep (units/s ÷ 240Hz)
    pub fn dx(&self) -> f64 {
        self.speed_units() / TICK_HZ
    }

    /// OpenGD update(): gravity + mode logic + vertical velocity; returns (dx, dy)
    fn integrate(&mut self) -> (f64, f64) {
        let flip = self.grav_sign();
        let dz = DT * DT_SLOW; // 0.225 — the famous 0.9 vertical quirk
        if let Some(slope) = self.dash {
            // dash: straight line at the orb's angle, no gravity
            self.vel_y = 0.0;
            return (self.dx(), slope * self.dx());
        }
        let g = GRAVITY * self.gravity_mult;
        // OpenGD updateJump: falling faster than 2g means the player left the
        // ground (walked off a ledge, left a wave mid-air): no more jumps
        if matches!(self.mode, GameMode::Cube | GameMode::Robot | GameMode::Spider | GameMode::Ball) && self.vel_y * flip < -2.0 * GRAVITY {
            self.on_ground = false;
        }
        let dy;
        match self.mode {
            GameMode::Cube | GameMode::Robot | GameMode::Spider => {
                // jump/teleport edges handled by Sim; here gravity + terminal
                self.vel_y -= g * dz * flip;
                if self.mode == GameMode::Robot && self.hold && self.robot_frames < 12.0 {
                    self.vel_y += g * dz * flip * 0.9; // hold boost cancels most of gravity
                }
                self.vel_y = (self.vel_y * flip).max(-TERMINAL_VEL) * flip; // GD caps falling only: pads launch past 15
                dy = dz * self.vel_y;
            }
            GameMode::Ball => {
                self.vel_y -= g * dz * flip * BALL_GRAV_MULT;
                self.vel_y = (self.vel_y * flip).max(-TERMINAL_VEL) * flip; // GD caps falling only: pads launch past 15
                dy = dz * self.vel_y;
            }
            GameMode::Ufo => {
                // OpenGD: 0.8 while falling, 1.2 rising (GD's recordings: a
                // falling UFO accelerates at 0.4 g); flying modes scale by
                // 0.85 when mini, and clamp vy to -6.4..8 (mirrored flipped)
                // "falling" for the UFO, OpenGD playerIsFalling: vy < g, or
                // world vy > g when flipped (not mirrored); 2g when mini
                // (fits GD's recordings to 0.002)
                let cut = GRAVITY * if self.mini { 2.0 } else { 1.0 } * if self.grav_flip { -1.0 } else { 1.0 };
                let mult = if self.vel_y * flip < cut { UFO_GRAV_RISING } else { UFO_GRAV_FALLING };
                let size = if self.mini { 0.85 } else { 1.0 };
                self.vel_y -= g * dz * flip * mult * UFO_GRAV_SCALE / size;
                let v = (self.vel_y * flip).clamp(-6.4 / size, 8.0 / size);
                self.vel_y = v * flip;
                dy = dz * self.vel_y;
            }
            GameMode::Ship | GameMode::Swing => {
                // GD 2.2 ship, in velocity units of 1/54 unit/s (dy = v * 0.225).
                // Accelerations (unit/s²) and the speed-indexed threshold are
                // gd-sim's; verified tick-exact against the real GRIEF run.
                // shortcut: swing = ship physics with no hold thrust (each
                // click flips gravity, see Sim); 2.2 swing constants are not
                // decompiled. Upgrade from the decomp if a level needs swing.
                const THRESH: [f64; 5] = [101.541492, 103.485494592, 103.377492, 103.809492, 103.809492];
                let thrust = self.hold && self.mode == GameMode::Ship;
                let v = self.vel_y * flip; // gravity-relative
                let thr = THRESH[self.speed_idx as usize % 5] / 54.0;
                let a = if thrust {
                    if v <= thr { 1397.0491 } else { 1117.64328 }
                } else if v >= thr {
                    -1341.1719
                } else {
                    -894.11464
                };
                let a = a / if self.mini { 0.85 } else { 1.0 } * self.gravity_mult;
                let (lo, hi) = if self.mini { (-406.566, 508.248) } else { (-345.6, 432.0) };
                let v = round_vel(v + a / (54.0 * TICK_HZ)).clamp(lo / 54.0, hi / 54.0);
                self.vel_y = v * flip;
                dy = dz * self.vel_y;
            }
            GameMode::Wave => {
                // GD mini dart slope: calibrated 2.0 (mini wave vy = 2 x
                // xV; confirmed against the GRIEF macro zigzag)
                static SLOPE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
                let slope = *SLOPE.get_or_init(|| {
                    std::env::var("CAL_WAVE_SLOPE")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(2.0)
                });
                let slope = if self.mini { slope } else { 1.0 };
                let s = if self.hold { 1.0 } else { -1.0 } * if self.grav_flip { -1.0 } else { 1.0 };
                // stored like GD: dy = v * 0.25 (no 0.9 factor for the wave)
                self.vel_y = s * slope * self.dx() / 0.25;
                dy = s * slope * self.dx();
            }
        }
        (self.dx(), dy)
    }

    /// One 240Hz substep: move + integrate. Input edge effects (jump) are applied by Sim
    /// before calling this.
    pub fn substep(&mut self) {
        let (dx, dy) = self.integrate();
        self.x += dx;
        self.y += dy;
        self.robot_frames += 1.0;
    }

    /// cube/robot/ball jump on press
    pub fn jump(&mut self) {
        let flip = self.grav_sign();
        match self.mode {
            GameMode::Cube => {
                self.vel_y = flip * JUMP_VEL * self.size();
                self.on_ground = false;
            }
            GameMode::Robot => {
                self.vel_y = flip * JUMP_VEL * self.size() * 0.6;
                self.on_ground = false;
                self.robot_frames = 0.0;
            }
            GameMode::Ball => {
                // a set speed toward the new side, whatever the old one: jump
                // speed, halved by the flip, x0.6 (OpenGD; the recordings show
                // a constant +-3.1 one tick after every ball click)
                self.vel_y = flip * JUMP_VEL * self.size() * 0.5 * BALL_JUMP_MULT;
                self.grav_flip = !self.grav_flip;
                self.on_ground = false;
            }
            _ => {}
        }
    }

    /// ufo hop on press
    pub fn ufo_hop(&mut self) {
        let flip = self.grav_sign();
        // OpenGD: 8 x 0.85 when mini (flying modes' mini size), 7 otherwise
        let v = if self.mini { UFO_JUMP_VEL_MINI * 0.85 } else { UFO_JUMP_VEL };
        self.vel_y = flip * v;
    }

    /// spider teleport: flip gravity; Sim snaps the player to the opposite surface
    pub fn spider_teleport(&mut self) {
        self.grav_flip = !self.grav_flip;
        self.vel_y = 0.0;
    }

    /// dash orb: dash at `deg` (math angle, CCW from +x), clamped to ±70°
    pub fn start_dash(&mut self, deg: f64, pink: bool) {
        if pink {
            self.grav_flip = !self.grav_flip;
        }
        self.dash = Some(deg.clamp(-70.0, 70.0).to_radians().tan());
        self.on_ground = false;
        self.queued_hold = false;
    }

    /// ringJump — orb activation. force already mode-scaled by Sim via helpers below.
    pub fn apply_orb(&mut self, vel: f64) {
        let flip = self.grav_sign();
        self.vel_y = flip * vel;
        self.on_ground = false;
        self.queued_hold = false;
        if self.mode == GameMode::Ball || self.mode == GameMode::Spider {
            self.vel_y *= ORB_BALL_SPIDER_POST;
            self.hold = false;
        }
    }

    /// propellPlayer — pad activation
    pub fn apply_pad(&mut self, force: f64) {
        let flip = self.grav_sign();
        let mut v = PAD_BASE * force;
        if self.size() < 1.0 { v *= ORB_MINI_SCALE; }
        if self.mode == GameMode::Ball || self.mode == GameMode::Spider {
            v *= PAD_BALL_SPIDER_MULT;
        }
        self.vel_y = flip * v;
        self.on_ground = false;
    }

    /// mode entry (portal at height `portal_y`). Ship halves vertical velocity.
    /// shortcut: wave -> ship divides the wave's velocity by 8 (one measured
    /// GRIEF transition); other entries keep the older /2 guess.
    pub fn enter_mode(&mut self, mode: GameMode, portal_y: f64) {
        self.band = band_for(mode, portal_y);
        if self.mode == GameMode::Ship && mode != GameMode::Ship {
            // leaving ship: keep vel
        }
        if mode == GameMode::Ship && self.mode != GameMode::Ship {
            self.vel_y /= if self.mode == GameMode::Wave { 8.0 } else { 2.0 };
        }
        if mode == GameMode::Spider || mode == GameMode::Wave {
            self.vel_y = 0.0;
        }
        self.mode = mode;
        self.robot_frames = 99.0;
    }

    /// flip via gravity portal / blue orb: halves velocity
    pub fn flip_gravity(&mut self, flip: bool) {
        if self.grav_flip != flip {
            self.grav_flip = flip;
            self.vel_y /= 2.0;
        }
    }

    /// green orb: flip + jump
    pub fn green_orb(&mut self, vel: f64) {
        self.grav_flip = !self.grav_flip;
        let flip = self.grav_sign();
        self.vel_y = flip * vel;
        self.on_ground = false;
        self.queued_hold = false;
    }

    /// drop orb (2.2): slam downward
    pub fn drop_orb(&mut self, vel: f64) {
        let flip = self.grav_sign();
        self.vel_y = flip * vel; // push with gravity
        self.on_ground = false;
        self.queued_hold = false;
    }
}

/// Orb velocity by (orb kind, player) — mode multipliers from OpenGD ringJump.
pub fn orb_velocity(kind: crate::sim::objects::ObjectKind, p: &Player) -> Option<f64> {
    use crate::sim::objects::ObjectKind::*;
    let base = JUMP_VEL;
    let v = match kind {
        OrbYellow => match p.mode {
            GameMode::Ship => base * 0.9, // OpenGD: ship ring ~0.9 — calibrate
            GameMode::Ufo => base * UFO_JUMP_VEL / 7.0,
            _ => base,
        },
        OrbPink => match p.mode {
            GameMode::Ship => base * ORB_PINK_MULT_SHIP,
            GameMode::Ufo => base * ORB_PINK_MULT_UFO,
            GameMode::Ball => base * ORB_PINK_MULT_BALL,
            _ => base * ORB_PINK_MULT,
        },
        OrbRed => match p.mode {
            GameMode::Ship => base * ORB_RED_MULT_SHIP,
            GameMode::Ufo => if p.mini { base * ORB_RED_MULT_UFO_MINI } else { base * ORB_RED_MULT_UFO_FULL },
            GameMode::Ball | GameMode::Spider => base * ORB_RED_MULT_BALL_SPIDER,
            GameMode::Robot => base * ORB_RED_MULT_ROBOT,
            _ => base * ORB_RED_MULT,
        },
        OrbBlue => base * ORB_GRAV_MULT,
        OrbGreen => base * 1.0, // handled specially (flip) by Sim
        OrbSpider => base,      // teleport handled by Sim
        OrbDrop => match p.mode {
            GameMode::Ufo => -11.2,
            GameMode::Ship | GameMode::Wave => -14.0,
            GameMode::Spider => -16.5,
            _ => -15.0,
        },
        _ => return None,
    };
    let mut v = v;
    if p.size() < 1.0 && kind != OrbDrop {
        v *= ORB_MINI_SCALE;
    }
    Some(v)
}

/// Pad force by kind.
pub fn pad_force(kind: crate::sim::objects::ObjectKind) -> Option<f64> {
    use crate::sim::objects::ObjectKind::*;
    match kind {
        PadYellow => Some(PAD_FORCE_YELLOW),
        PadPink => Some(PAD_FORCE_PINK),
        PadRed => Some(PAD_FORCE_RED),
        PadGravity => Some(0.8), // OpenGD propellPlayer(0.8)
        _ => None,
    }
}
