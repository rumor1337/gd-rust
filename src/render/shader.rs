//! GD 2.2 color shaders (2919 grayscale, 2920 sepia, 2921 invert, 2922 hue,
//! 2923 color change), applied to the world canvas in the order of GD's own
//! fragment shader. Values fade linearly over the trigger's time (key 10).
//!
//! Keys (shared ShaderGameObject fields, read off the levels' data): 176 =
//! the effect's value (gray/sepia amount, invert target, hue degrees);
//! 2921 with key 188 (edit RGB) inverts per channel by 179/180/189;
//! 2923: C = 175/176/191, B = 179/180/189.
//!
//! shortcut: easing (key 30), grayscale tint channel and luminance mode,
//! and spawn/touch-activated shader triggers are ignored; the distortion
//! shaders (glitch, chromatic, lens, blur, pixelate, pinch, bulge, split
//! screen, shock waves) are not drawn. Particles and players, drawn later on
//! the output frame, are not tinted.

use crate::level::{GdObject, Level};

// state slots
const GRAY: usize = 0;
const SEPIA: usize = 1;
const INV: usize = 2; // 2..5 = rgb, 5 = on
const HUE: usize = 6;
const CC: usize = 7; // 7..10 = C, 10..13 = B
pub const N: usize = 13;
pub const OFF: [f32; N] = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0];

pub struct Shaders {
    /// (x, slot updates, fade seconds), sorted by x
    cmds: Vec<(f32, Vec<(usize, f32)>, f32)>,
    cur: usize,
    /// per slot: (from, to, elapsed, duration)
    tw: [(f32, f32, f32, f32); N],
}

fn kv(o: &GdObject, k: u16) -> Option<f32> {
    o.raw.iter().find(|(kk, _)| *kk == k).and_then(|(_, v)| v.parse().ok())
}

/// The slot updates one shader trigger makes, or None if it is not one.
fn updates(o: &GdObject) -> Option<Vec<(usize, f32)>> {
    let k = |key, d| kv(o, key).unwrap_or(d);
    let v = k(176, 0.0);
    Some(match o.id {
        2919 => vec![(GRAY, v.clamp(0.0, 1.0))],
        2920 => vec![(SEPIA, v.clamp(0.0, 1.0))],
        2921 => {
            let rgb = if k(188, 0.0) != 0.0 { [k(179, 0.0), k(180, 0.0), k(189, 0.0)] } else { [v; 3] };
            vec![(INV, rgb[0]), (INV + 1, rgb[1]), (INV + 2, rgb[2]), (INV + 3, if rgb == [0.0; 3] { 0.0 } else { 1.0 })]
        }
        2922 => vec![(HUE, v)],
        2923 => [k(175, 1.0), k(176, 1.0), k(191, 1.0), k(179, 0.0), k(180, 0.0), k(189, 0.0)]
            .into_iter()
            .enumerate()
            .map(|(i, x)| (CC + i, x))
            .collect(),
        _ => return None,
    })
}

impl Shaders {
    pub fn new(level: &Level) -> Shaders {
        let mut cmds: Vec<_> = level
            .objects
            .iter()
            .filter(|o| kv(o, 62).unwrap_or(0.0) == 0.0) // spawn-activated: never x-crossed
            .filter_map(|o| Some((o.x, updates(o)?, kv(o, 10).unwrap_or(0.0))))
            .collect();
        cmds.sort_by(|a, b| a.0.total_cmp(&b.0));
        Shaders { cmds, cur: 0, tw: std::array::from_fn(|i| (OFF[i], OFF[i], 0.0, 0.0)) }
    }

    /// Fire the triggers the player crossed, then advance the fades.
    pub fn step(&mut self, x: f64, dt: f32) {
        while self.cur < self.cmds.len() && self.cmds[self.cur].0 as f64 <= x {
            let now = self.now();
            let (_, ups, dur) = &self.cmds[self.cur];
            for &(slot, to) in ups {
                self.tw[slot] = (now[slot], to, 0.0, *dur);
            }
            self.cur += 1;
        }
        for t in &mut self.tw {
            t.2 += dt;
        }
    }

    pub fn now(&self) -> [f32; N] {
        std::array::from_fn(|i| {
            let (from, to, t, dur) = self.tw[i];
            from + (to - from) * if dur <= 0.0 { 1.0 } else { (t / dur).min(1.0) }
        })
    }
}

/// GD's color-effect pass over RGBA pixels (alpha untouched).
pub fn apply(px: &mut [u8], s: [f32; N]) {
    if s == OFF {
        return;
    }
    // per-pixel float math on the whole canvas: split across cores
    use rayon::prelude::*;
    if s[GRAY] <= 0.0 && s[SEPIA] <= 0.0 && s[HUE].to_radians().cos() == 1.0 {
        // invert / color correct act on each channel alone: run the same
        // float code once per value into a table (bit-identical, ~40x less work)
        let mut lut = [0u8; 256 * 4];
        for (v, p) in lut.chunks_exact_mut(4).enumerate() {
            p.copy_from_slice(&[v as u8, v as u8, v as u8, 255]);
        }
        apply_part(&mut lut, s);
        px.par_chunks_mut(64 * 1024).for_each(|part| {
            for p in part.chunks_exact_mut(4) {
                for i in 0..3 {
                    p[i] = lut[p[i] as usize * 4 + i];
                }
            }
        });
        return;
    }
    // one band of rows per task, so the color cache warms up
    px.par_chunks_mut(32 * 1280 * 4).for_each(|part| apply_part_memo(part, s));
}

fn apply_part(px: &mut [u8], s: [f32; N]) {
    let (ca, sa) = (s[HUE].to_radians().cos(), s[HUE].to_radians().sin());
    for p in px.chunks_exact_mut(4) {
        let o = shade([p[0], p[1], p[2]], &s, ca, sa);
        p[..3].copy_from_slice(&o);
    }
}

/// apply_part through a direct-mapped cache of recent colors: same function,
/// same result; flat fills and sprite interiors repeat colors a lot
fn apply_part_memo(px: &mut [u8], s: [f32; N]) {
    let (ca, sa) = (s[HUE].to_radians().cos(), s[HUE].to_radians().sin());
    const BITS: u32 = 12;
    // key: rgb | 1<<24 (0 = empty); value: shaded rgb
    let mut cache = vec![(0u32, [0u8; 3]); 1 << BITS];
    for p in px.chunks_exact_mut(4) {
        let k = u32::from_le_bytes([p[0], p[1], p[2], 1]);
        let slot = &mut cache[(k.wrapping_mul(0x9E37_79B1) >> (32 - BITS)) as usize];
        if slot.0 != k {
            *slot = (k, shade([p[0], p[1], p[2]], &s, ca, sa));
        }
        p[..3].copy_from_slice(&slot.1);
    }
}

fn shade(px: [u8; 3], s: &[f32; N], ca: f32, sa: f32) -> [u8; 3] {
    let kk = 0.57735f32;
    let mut c = px.map(|v| v as f32 / 255.0);
    if s[GRAY] > 0.0 {
        let g = (c[0] + c[1] + c[2]) * 0.333;
        c = c.map(|x| x * (1.0 - s[GRAY]) + g * s[GRAY]);
    }
    if s[SEPIA] > 0.0 {
        let t = [
            0.393 * c[0] + 0.769 * c[1] + 0.189 * c[2],
            0.349 * c[0] + 0.686 * c[1] + 0.168 * c[2],
            0.272 * c[0] + 0.534 * c[1] + 0.131 * c[2],
        ];
        c = std::array::from_fn(|i| c[i] * (1.0 - s[SEPIA]) + t[i] * s[SEPIA]);
    }
    if s[INV + 3] > 0.0 {
        c = std::array::from_fn(|i| c[i] * (1.0 - s[INV + i]) + (1.0 - c[i]) * s[INV + i]);
    }
    if ca != 1.0 {
        let d = kk * (c[0] + c[1] + c[2]);
        let cross = [kk * (c[2] - c[1]), kk * (c[0] - c[2]), kk * (c[1] - c[0])];
        c = std::array::from_fn(|i| c[i] * ca + cross[i] * sa + kk * d * (1.0 - ca));
    }
    if s[CC] > 0.0 {
        c = std::array::from_fn(|i| c[i] * s[CC + i] + s[CC + 3 + i]);
    }
    c.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_channel_table_matches_the_per_pixel_math() {
        let mut st = OFF;
        st[INV] = 0.3;
        st[INV + 1] = -0.7;
        st[INV + 3] = 1.0;
        st[CC + 1] = 0.8;
        st[CC + 5] = 0.05;
        let px: Vec<u8> = (0..4096u32).flat_map(|i| [(i * 7) as u8, (i * 13) as u8, (i * 31) as u8, 255]).collect();
        let (mut a, mut b) = (px.clone(), px);
        apply(&mut a, st);
        apply_part(&mut b, st);
        assert_eq!(a, b);
    }

    #[test]
    fn color_cache_matches_the_per_pixel_math() {
        let mut st = OFF;
        st[HUE] = 1.0;
        st[INV] = 0.3;
        st[INV + 3] = 1.0;
        // repeats and hash collisions both
        let px: Vec<u8> = (0..20000u32).flat_map(|i| { let i = i % 9000; [(i * 7) as u8, (i * 13) as u8, (i >> 4) as u8, 255] }).collect();
        let (mut a, mut b) = (px.clone(), px);
        apply(&mut a, st);
        apply_part(&mut b, st);
        assert_eq!(a, b);
    }

    #[test]
    fn invert_fades_in_and_effects_match_gd_formulas() {
        // GRIEF-style 2921: edit RGB, -0.23 on red, 1 s fade at x 100
        let lvl = crate::level::parse_level("kS38,1_0_2_0_3_0_6_1000|;1,2921,2,100,3,0,176,1,188,1,179,-0.25,10,1;");
        let mut s = Shaders::new(&lvl);
        s.step(50.0, 0.1);
        assert_eq!(s.now(), OFF);
        s.step(150.0, 0.0);
        s.step(150.0, 0.5);
        assert!((s.now()[INV] + 0.125).abs() < 1e-4 && s.now()[INV + 3] > 0.0, "{:?}", s.now());
        // invert by -0.25: c + v(1 - 2c): 200/255 -> 0.926
        let mut px = [200u8, 100, 50, 255];
        let mut st = OFF;
        st[INV] = -0.25;
        st[INV + 3] = 1.0;
        apply(&mut px, st);
        assert_eq!(px, [236, 100, 50, 255]);
        // grayscale 1: channel mean; color change: c * C + B
        let mut px = [90u8, 30, 0, 255];
        let mut st = OFF;
        st[GRAY] = 1.0;
        apply(&mut px, st);
        assert_eq!(&px[..3], &[40, 40, 40]);
        let mut st = OFF;
        st[CC + 3] = -0.1;
        apply(&mut px, st);
        assert_eq!(&px[..3], &[15, 40, 40]);
    }
}
