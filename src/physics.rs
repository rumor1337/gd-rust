//! Physics constants — OpenGD 2.1-era reference model.
//! Provenance: analysis/opengd/Source/PlayerObject.cpp|.h (github.com/Open-GD/OpenGD).
//! Gravity 0.958199 cross-verified by byte-scan of GeometryDash.exe (file offset 0x82f01c).
//! Calibration oracle: macros/Cerealmilkbowl-GRIEF-xdBot.gdr2 (real GD 2.2, 240fps recording).

// --- core (PlayerObject.h) ---
pub const GRAVITY: f64 = 0.958199;
pub const JUMP_VEL: f64 = 11.180032;
pub const X_VEL: f64 = 5.770002;
pub const TERMINAL_VEL: f64 = 15.0;

/// speed-portal multipliers (m_playerSpeed), index 0..4 = 0.5x, 1x, 2x, 3x, 4x
pub const SPEEDS: [f64; 5] = [0.7, 0.9, 1.1, 1.3, 1.6];

/// dt multiplier OpenGD applies to vertical integration (the famous 0.9 quirk).
/// Calibration knob: real GD may use 1.0. GRIEF replay decides.
pub const DT_SLOW: f64 = 0.9;

// substep: 4 per 60fps frame (PlayLayer::update: step = dt*60 then /4)
pub const SUBSTEPS: u32 = 4;
/// substep dt in frame-units: 0.25
pub const DT: f64 = 0.25;

// --- player sizes (outer box 30x30, inner hitbox) ---
pub const OUTER_HALF: f64 = 15.0;
pub const OUTER_HALF_MINI: f64 = 9.0;
pub const INNER_HALF: f64 = 3.75;
pub const PLAYER_SIZE_FULL: f64 = 1.0;
pub const PLAYER_SIZE_MINI: f64 = 0.8; // cube/ball; ship 0.85
pub const PLAYER_SIZE_MINI_SHIP: f64 = 0.85;

// --- ship ---
pub const SHIP_ACCEL: f64 = 0.8;
pub const SHIP_HOLD_MULT: f64 = -1.0;
pub const SHIP_RELEASE_MULT: f64 = 1.2;
pub const SHIP_BOOST: f64 = 0.4; // 0.5 while falling
pub const SHIP_BOOST_FALLING: f64 = 0.5;
pub const SHIP_VEL_MAX: f64 = 8.0;
pub const SHIP_VEL_MIN: f64 = -6.4;

// --- ufo ---
pub const UFO_JUMP_VEL: f64 = 7.0; // 8.0 when mini
pub const UFO_JUMP_VEL_MINI: f64 = 8.0;
pub const UFO_GRAV_RISING: f64 = 0.8;
pub const UFO_GRAV_FALLING: f64 = 1.2;
pub const UFO_GRAV_SCALE: f64 = 0.5;

// --- mode gravity multipliers ---
pub const BALL_GRAV_MULT: f64 = 0.6;
pub const BALL_JUMP_MULT: f64 = 0.6; // ball click velocity factor (OpenGD: vel*0.6 on flip)

// --- orb forces (ringJump, base = JUMP_VEL) ---
pub const ORB_RED_MULT: f64 = 1.38;
pub const ORB_RED_MULT_SHIP: f64 = 1.4; // vehicleSize != 1
pub const ORB_RED_MULT_UFO_FULL: f64 = 1.02;
pub const ORB_RED_MULT_UFO_MINI: f64 = 1.36;
pub const ORB_RED_MULT_BALL_SPIDER: f64 = 1.34;
pub const ORB_RED_MULT_ROBOT: f64 = 1.28;
pub const ORB_PINK_MULT: f64 = 0.72;
pub const ORB_PINK_MULT_SHIP: f64 = 0.37;
pub const ORB_PINK_MULT_UFO: f64 = 0.42;
pub const ORB_PINK_MULT_BALL: f64 = 0.77;
pub const ORB_GRAV_MULT: f64 = 0.8; // blue orb
pub const ORB_BALL_SPIDER_POST: f64 = 0.7; // vel scale after ball/spider orb
pub const ORB_MINI_SCALE: f64 = 0.8; // vehicleSize < 1

// --- pads (propellPlayer: vel = flip * 16 * force * (vehicleSize==1 ? 1 : 0.8)) ---
pub const PAD_BASE: f64 = 16.0;
pub const PAD_FORCE_YELLOW: f64 = 1.0;
pub const PAD_FORCE_PINK: f64 = 0.8; // community: pink pad ~ 13.8 → 0.86? calibrate
pub const PAD_FORCE_RED: f64 = 1.4; // community: red pad > yellow; calibrate
pub const PAD_BALL_SPIDER_MULT: f64 = 0.6;

// --- world ---
/// ground surface world y (editor y=0 + 90 world offset)
pub const GROUND_Y: f64 = 90.0;
/// ceiling for flying modes
pub const CEIL_Y: f64 = 390.0;
/// player spawn (PlayLayer::setPosition({-20, 105})) — x,y center
pub const SPAWN_X: f64 = -20.0;
pub const SPAWN_Y: f64 = 105.0;
