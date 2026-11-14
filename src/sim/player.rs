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
    /// riding a slope this tick (GD m_isOnSlope; cleared each pass)
    pub slope_on: bool,
    /// GD m_isCurrentSlopeTop: slopeFloorTop of the last ridden slope
    pub slope_top: Option<bool>,
    /// GD m_collidingWithSlopeId: unique index of the last ridden slope
    pub slope_id: Option<usize>,
    /// player radius on the last ridden slope (rad)
    pub slope_rad: f64,
}

impl Player {
    pub fn new(mode: GameMode, speed_idx: u8) -> Player {
        Player {
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
            slope_on: false,
            slope_top: None,
            slope_id: None,
            slope_rad: 0.0,
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
        if self.mini { 9.0 } else { 15.0 }
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
        let dy;
        match self.mode {
            GameMode::Cube | GameMode::Robot => {
                // hold-edge handled by Sim (jump()); here gravity + terminal
                self.vel_y -= GRAVITY * dz * flip;
                if self.mode == GameMode::Robot && self.hold && self.robot_frames < 12.0 {
                    self.vel_y += GRAVITY * dz * flip.signum() * -1.0 * 0.9;
                }
                self.vel_y = self.vel_y.clamp(-TERMINAL_VEL, TERMINAL_VEL);
                dy = dz * self.vel_y;
            }
            GameMode::Ball => {
                self.vel_y -= GRAVITY * dz * flip * BALL_GRAV_MULT;
                self.vel_y = self.vel_y.clamp(-TERMINAL_VEL, TERMINAL_VEL);
                dy = dz * self.vel_y;
            }
            GameMode::Ufo => {
                let mult = if self.falling() { UFO_GRAV_FALLING } else { UFO_GRAV_RISING };
                self.vel_y -= GRAVITY * dz * flip * mult * UFO_GRAV_SCALE / self.size();
                dy = dz * self.vel_y;
            }
            GameMode::Ship | GameMode::Swing => {
                let accel = SHIP_ACCEL;
                let boost = if self.falling() { SHIP_BOOST_FALLING } else { SHIP_BOOST };
                let dir = if self.hold { SHIP_HOLD_MULT } else if !self.falling() { SHIP_RELEASE_MULT } else { 1.0 };
                // swing: gravity never flips direction of control
                let gflip = if self.mode == GameMode::Swing { 1.0 } else { flip };
                self.vel_y -= GRAVITY * dz * gflip * accel * boost * dir / self.size();
                let (lo, hi) = (SHIP_VEL_MIN, SHIP_VEL_MAX);
                self.vel_y = self.vel_y / self.size();
                self.vel_y = self.vel_y.clamp(lo, hi);
                self.vel_y *= self.size();
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
                self.vel_y = s * slope * self.speed_units();
                dy = s * slope * self.dx();
            }
            GameMode::Spider => {
                // no free physics: sits snapped until teleport
                dy = 0.0;
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
                self.vel_y = -flip * JUMP_VEL * self.size();
                self.on_ground = false;
            }
            GameMode::Robot => {
                self.vel_y = -flip * JUMP_VEL * self.size() * 0.6;
                self.on_ground = false;
                self.robot_frames = 0.0;
            }
            GameMode::Ball => {
                self.vel_y *= BALL_JUMP_MULT;
                self.grav_flip = !self.grav_flip;
                self.on_ground = false;
            }
            _ => {}
        }
    }

    /// ufo hop on press
    pub fn ufo_hop(&mut self) {
        let flip = self.grav_sign();
        let v = if self.mini { UFO_JUMP_VEL_MINI } else { UFO_JUMP_VEL };
        self.vel_y = -flip * v * self.size();
    }

    /// spider teleport: flip gravity and snap to opposite surface (y set by Sim collision)
    pub fn spider_teleport(&mut self) {
        self.grav_flip = !self.grav_flip;
        self.vel_y = 0.0;
    }

    /// ringJump — orb activation. force already mode-scaled by Sim via helpers below.
    pub fn apply_orb(&mut self, vel: f64) {
        let flip = self.grav_sign();
        self.vel_y = -flip * vel;
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
        self.vel_y = -flip * v;
        self.on_ground = false;
    }

    /// mode entry (portal). Ship halves vertical velocity.
    pub fn enter_mode(&mut self, mode: GameMode) {
        if self.mode == GameMode::Ship && mode != GameMode::Ship {
            // leaving ship: keep vel
        }
        if mode == GameMode::Ship && self.mode != GameMode::Ship {
            self.vel_y /= 2.0;
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
        self.vel_y = -flip * vel;
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
        OrbDash => base, // shortcut: treat dash orbs as yellow
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
        PadGravity => Some(1.0),
        _ => None,
    }
}
