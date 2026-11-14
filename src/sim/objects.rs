//! Object kinds, the runtime object table (data/objects.json), and the spatial hash.

use crate::level::object::GdObject;
use serde::Deserialize;
use std::collections::HashMap;

/// Interactive object ids (stable in GD). Portals map by texture evidence in objects.json.
pub const SPEED_PORTALS: [(u16, u8); 5] = [(200, 0), (201, 1), (202, 2), (203, 3), (1334, 4)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Solid,
    Hazard,
    Slope, // triangle solid with a ridable hypotenuse (SlopeGeom)
    SlopeHazard, // pit spike slope (1717/1718 family): the diagonal kills
    Decoration,
    SpeedPortal(u8),
    GravityPortal(bool), // true = flip to inverted
    PortalCube,
    PortalShip,
    PortalBall,
    PortalUfo,
    PortalWave,
    PortalRobot,
    PortalSpider,
    PortalSwing,
    PortalTeleport,
    PortalDual,
    PortalSolo,
    PortalMirror,
    PortalSize(bool), // true = mini
    OrbYellow,
    OrbPink,
    OrbRed,
    OrbBlue,  // gravity orb
    OrbGreen, // gravity flip + jump
    OrbSpider,
    OrbDrop,   // 2.2 black/drop orb (3641/3642 family)
    OrbDash,   // 2.2 dash orbs (3646 family) — shortcut: treated as yellow
    PadYellow,
    PadPink,
    PadRed,
    PadGravity,
    Coin,
    Trigger, // no gameplay effect in sim
}

#[derive(Debug, Deserialize)]
pub struct ObjectDef {
    #[serde(default)]
    pub object_type: i64,
    #[serde(default)]
    pub texture_name: Option<String>,
    #[serde(default)]
    pub hitbox: Option<Vec<f64>>, // [w, h, x, y] offsets from object center
    #[serde(default)]
    pub hitbox_radius: Option<f64>,
    #[serde(default)]
    pub is_solid: bool,
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub struct ObjectTable {
    pub objects: HashMap<String, ObjectDef>,
}

impl ObjectTable {
    pub fn load(path: &std::path::Path) -> Result<ObjectTable, String> {
        let f = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&f).map_err(|e| e.to_string())
    }

    fn kind_of(&self, id: u16) -> ObjectKind {
        let def = match self.objects.get(&id.to_string()) {
            Some(d) => d,
            None => return ObjectKind::Decoration,
        };
        let tex = def.texture_name.as_deref().unwrap_or("");
        // explicit interactive ids first (verified against real GD portal textures)
        let kind = match id {
            200 | 201 | 202 | 203 | 1334 => SPEED_PORTALS
                .iter()
                .find(|(pid, _)| *pid == id)
                .map(|(_, idx)| ObjectKind::SpeedPortal(*idx)),
            10 => Some(ObjectKind::GravityPortal(false)),
            11 => Some(ObjectKind::GravityPortal(true)),
            12 => Some(ObjectKind::PortalCube),
            13 => Some(ObjectKind::PortalShip),
            47 => Some(ObjectKind::PortalBall),
            111 => Some(ObjectKind::PortalUfo),
            660 => Some(ObjectKind::PortalWave),
            745 => Some(ObjectKind::PortalRobot),
            1331 => Some(ObjectKind::PortalSpider),
            1933 => Some(ObjectKind::PortalSwing),
            99 => Some(ObjectKind::PortalSize(true)), // mini
            101 => Some(ObjectKind::PortalSize(false)), // big
            747 => Some(ObjectKind::PortalTeleport),
            286 => Some(ObjectKind::PortalDual),
            287 => Some(ObjectKind::PortalSolo),
            45 | 46 => Some(ObjectKind::PortalMirror),
            363 | 366 | 1717 | 1723 | 364 | 367 | 1718 | 1724 => {
                // pit slopes: same object_type 25 as plain slopes; GRIEF's
                // corridor places them superimposed with plain triangles at
                // identical faces — they are rideable, not killers
                Some(ObjectKind::Slope)
            } // 30x30 / 60x30
            1331 => Some(ObjectKind::PortalSize(true)),
            36 => Some(ObjectKind::OrbYellow),
            141 => Some(ObjectKind::OrbPink),
            1333 => Some(ObjectKind::OrbRed),
            84 => Some(ObjectKind::OrbBlue),
            1022 => Some(ObjectKind::OrbGreen),
            3649 => Some(ObjectKind::OrbSpider),
            3641 | 3642 => Some(ObjectKind::OrbDrop),
            3646 | 3647 | 3648 => Some(ObjectKind::OrbDash),
            35 => Some(ObjectKind::PadYellow),
            140 => Some(ObjectKind::PadPink),
            1332 => Some(ObjectKind::PadRed),
            67 => Some(ObjectKind::PadGravity),
            142 | 143 => Some(ObjectKind::Coin), // secret coins
            _ => None,
        };
        if let Some(k) = kind {
            return k;
        }
        match def.object_type {
            0 | 1 if def.is_solid || def.hitbox.is_some() => ObjectKind::Solid,
            2 => ObjectKind::Hazard,
            25 => ObjectKind::Slope,
            _ => ObjectKind::Decoration,
        }
    }
}

/// Slope triangle: hypotenuse endpoints + the solid triangle (world space).
/// Mirroring (flipX/flipY) and rotation of the default {BL,BR,TR} triangle
/// under the BL→TR hypotenuse produce every slope variant, so one local
/// definition covers all flip/rot combinations.
#[derive(Debug, Clone, Copy)]
pub struct SlopeGeom {
    /// hypotenuse surface endpoints
    pub surf: ((f64, f64), (f64, f64)),
    /// the 3 solid vertices; the solid side of the surface is the side the
    /// triangle's centroid is on
    pub tri: [(f64, f64); 3],
}

/// A level object preprocessed for the sim.
#[derive(Debug, Clone)]
pub struct SimObject {
    pub id: u16,
    pub kind: ObjectKind,
    /// world-space center
    pub x: f64,
    pub y: f64,
    /// world-space collision rect (min_x, min_y, w, h); already scaled & AABB-rotated
    pub rect: (f64, f64, f64, f64),
    pub radius: Option<f64>,
    pub slope: Option<SlopeGeom>,
    /// true when the object carries nonzero editor rotation (GD's
    /// "isRotated" flag)
    pub rotated: bool,
    /// max(|scale_x|, |scale_y|): GD's hazard "radius" is [0x38c]=1.0 ×
    /// this scale (hazard death = center-in-rect or corner within it)
    pub scale: f64,
    /// object groups for move-trigger displacement (key 57)
    pub groups: Vec<u16>,
}

pub fn sim_object(obj: &GdObject, table: &ObjectTable) -> SimObject {
    let kind = table.kind_of(obj.id);
    let def = table.objects.get(&obj.id.to_string());
    // world position: editor y=0 is the CENTER of ground-row cell [90,120],
    // i.e. world 105 (floor surface = GROUND_Y = 90). x stays raw editor units.
    let wx = obj.x as f64;
    let wy = obj.y as f64 + 105.0;
    let (rect, radius, slope, scale) = match def.and_then(|d| d.hitbox.as_ref()) {
        Some(hb) if hb.len() == 4 => {
            // objects.json hitbox = [h, w, ox, oy]: ox/oy is the rect's
            // bottom-left corner relative to the object center.
            let (h, w, ox, oy) = (hb[0], hb[1], hb[2], hb[3]);
            let sx = if obj.scale_x != 0.0 { obj.scale_x.abs() as f64 } else { 1.0 };
            let sy = if obj.scale_y != 0.0 { obj.scale_y.abs() as f64 } else { 1.0 };
            let w = w * sx;
            let h = h * sy;
            // hitbox rect corners (BL, BR, TL, TR) around the object center
            let x0 = wx + ox * sx;
            let y0 = wy + oy * sy;
            let mut pts = [
                (x0, y0),           // 0 BL
                (x0 + w, y0),       // 1 BR
                (x0, y0 + h),       // 2 TL
                (x0 + w, y0 + h),   // 3 TR
            ];
            // flips mirror points around the object center
            for q in pts.iter_mut() {
                if obj.flip_x {
                    q.0 = 2.0 * wx - q.0;
                }
                if obj.flip_y {
                    q.1 = 2.0 * wy - q.1;
                }
            }
            // rotate around the object center
            // GD editor rotation is CLOCKWISE on the y-up grid (decompiled
            // GCCRotation transform); rot_pt math is CCW, so negate.
            let t = -(obj.rot as f64).to_radians();
            let (c, s) = (t.cos(), t.sin());
            let rot_pt = |(px, py): (f64, f64)| -> (f64, f64) {
                let dx = px - wx;
                let dy = py - wy;
                (wx + dx * c - dy * s, wy + dx * s + dy * c)
            };
            let pts = pts.map(rot_pt);
            let min_x = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
            let max_x = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
            let min_y = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
            let max_y = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
            let rect = (min_x, min_y, max_x - min_x, max_y - min_y);
            let slope = if matches!(kind, ObjectKind::Slope | ObjectKind::SlopeHazard) {
                Some(SlopeGeom { surf: (pts[0], pts[3]), tri: [pts[0], pts[1], pts[3]] })
            } else {
                None
            };
            (rect, def.and_then(|d| d.hitbox_radius), slope, sx.max(sy))
        }
        _ => (
            (wx - 15.0, wy - 15.0, 30.0, 30.0),
            def.and_then(|d| d.hitbox_radius),
            None,
            1.0,
        ),
    };
    SimObject {
        id: obj.id,
        kind,
        x: wx,
        y: wy,
        rect,
        radius,
        slope,
        rotated: obj.rot != 0.0,
        scale,
        groups: obj.groups.clone(),
    }
}

/// Uniform spatial hash; cell 120 units covers the widest portal hitbox.
pub struct Grid {
    cell: f64,
    map: HashMap<(i64, i64), Vec<usize>>,
    objects: Vec<SimObject>,
}

impl Grid {
    pub fn build(objects: Vec<SimObject>) -> Grid {
        let cell = 120.0;
        let mut map: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, o) in objects.iter().enumerate() {
            let (rx, ry, rw, rh) = o.rect;
            let x0 = ((rx - 120.0) / cell).floor() as i64; // pad: scaled/rotated hitboxes
            let x1 = ((rx + rw + 120.0) / cell).floor() as i64;
            let y0 = ((ry - 120.0) / cell).floor() as i64;
            let y1 = ((ry + rh + 120.0) / cell).floor() as i64;
            for cx in x0..=x1 {
                for cy in y0..=y1 {
                    map.entry((cx, cy)).or_default().push(i);
                }
            }
        }
        Grid { cell, map, objects }
    }

    pub fn all(&self) -> &[SimObject] {
        &self.objects
    }

    /// Indices whose cell rectangle is near the query rect (padded by pad units).
    pub fn query(&self, x: f64, y: f64, w: f64, h: f64, pad: f64) -> Vec<usize> {
        let x0 = ((x - pad) / self.cell).floor() as i64;
        let x1 = ((x + w + pad) / self.cell).floor() as i64;
        let y0 = ((y - pad) / self.cell).floor() as i64;
        let y1 = ((y + h + pad) / self.cell).floor() as i64;
        let mut seen = Vec::new();
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                if let Some(v) = self.map.get(&(cx, cy)) {
                    seen.extend_from_slice(v);
                }
            }
        }
        seen.sort_unstable();
        seen.dedup();
        seen
    }
}
