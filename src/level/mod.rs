//! .gmd level string parser: header (kS38 color channels, kA settings) + `;`-separated objects.

pub mod object;

pub use object::GdObject;

#[derive(Debug, Clone)]
pub struct ColorChannel {
    pub id: u16,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub opacity: f32,
    /// key 9: take the color of this channel instead (copy color)
    pub copy: Option<u16>,
    /// key 5: additive blending
    pub blending: bool,
}

#[derive(Debug, Clone, Default)]
pub struct LevelHeader {
    /// kA4 value (speed key 0..4, direct index into SPEEDS: 0=0.5x,1=1x,2=2x,3=3x,4=4x)
    pub start_speed: u8,
    /// kA2 value (gamemode enum: 0 cube,1 ship,2 ball,3 ufo,4 wave,5 robot,6 spider,7 swing)
    pub start_gamemode: u8,
    /// kA3 value (mini mode at start)
    pub start_mini: bool,
    /// kA11 value (flipped gravity at start)
    pub start_flip: bool,
    /// kA8 value (dual mode at start: a second player spawns)
    pub start_dual: bool,
    /// kA10 value (2-player controls: in dual each button drives one player)
    pub two_player: bool,
    /// from base64 kA14 JSON `start-pos-index`, if present
    pub start_pos_index: u16,
    pub channels: Vec<ColorChannel>,
    /// all kA keys verbatim
    pub ka: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct Level {
    pub header: LevelHeader,
    pub objects: Vec<GdObject>,
    /// level id (0 when parsed from an anonymous string; set by loaders)
    pub id: u32,
    /// unique per constructed Level: renderer caches key on it (a pointer
    /// key breaks when a dropped level's allocation is reused)
    pub uid: u64,
}

fn next_uid() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn channel_from_kv(pairs: &[(&str, &str)]) -> Option<ColorChannel> {
    let get = |k: &str| pairs.iter().find(|(pk, _)| *pk == k).map(|(_, v)| *v);
    let id = get("6")?.parse::<u16>().ok()?;
    let ch = |k: &str| get(k).and_then(|v| v.parse::<u8>().ok()).unwrap_or(0);
    Some(ColorChannel {
        id,
        r: ch("1"),
        g: ch("2"),
        b: ch("3"),
        opacity: get("7").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0),
        copy: get("9").and_then(|v| v.parse::<u16>().ok()).filter(|&c| c != 0),
        blending: get("5") == Some("1"),
    })
}

fn ka_start_pos(ka: &[(String, String)]) -> u16 {
    let Some(raw) = ka.iter().find(|(k, _)| k == "kA14") else { return 0 };
    // Value looks like "|<base64>~0.1~": strip leading '|' and any '~' suffix.
    let v = raw.1.split('~').next().unwrap_or("").trim_start_matches('|');
    use base64::Engine;
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(v) else { return 0 };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return 0 };
    // "start-pos-index" may sit at the top level or nested under a
    // player-name key (e.g. {"alphalaneous.tinker":{...,"start-pos-index":25}}).
    fn find_key(v: &serde_json::Value) -> Option<u64> {
        if let Some(x) = v.get("start-pos-index").and_then(|x| x.as_u64()) {
            return Some(x);
        }
        if let Some(map) = v.as_object() {
            for c in map.values() {
                if let Some(x) = find_key(c) {
                    return Some(x);
                }
            }
        }
        None
    }
    find_key(&json).map(|x| x as u16).unwrap_or(0)
}

impl Default for Level {
    fn default() -> Self {
        Level { header: LevelHeader::default(), objects: Vec::new(), id: 0, uid: next_uid() }
    }
}

/// Parse a decompressed level string (plain text, as stored in levels/*.gmd).
pub fn parse_level(s: &str) -> Level {
    let (header, rest) = match s.split_once(';') {
        Some(x) => x,
        None => (s, ""),
    };

    // Header: "kS38,CH|CH|...|CH|,kA13,val,kA14,|<base64-with-pipes>~0.1~,kA6,9,..."
    // The kA14 value contains '|' characters, so reassemble the kA section
    // from the first segment that begins with "kA" (after an optional ',').
    let segs: Vec<&str> = header.split('|').collect();
    let mut channels = Vec::new();
    let mut ka_parts: Vec<&str> = Vec::new();
    let mut ka_started = false;
    for seg in &segs {
        if ka_started {
            ka_parts.push(seg);
        } else if seg.trim_start_matches(',').starts_with("kA") {
            ka_started = true;
            ka_parts.push(seg.trim_start_matches(','));
        } else {
            let seg = seg.strip_prefix("kS38,").unwrap_or(seg);
            if seg.is_empty() {
                continue;
            }
            let kv: Vec<(&str, &str)> = seg
                .split('_')
                .collect::<Vec<&str>>()
                .chunks(2)
                .filter_map(|c| if c.len() == 2 { Some((c[0], c[1])) } else { None })
                .collect();
            if let Some(c) = channel_from_kv(&kv) {
                channels.push(c);
            }
        }
    }
    // kA values (incl. the pipe-bearing kA14 blob) contain no commas, so
    // comma-splitting the rejoined section yields clean key/value pairs.
    let ka_joined = ka_parts.join("|");
    let mut ka_raw = Vec::new();
    let flat: Vec<&str> = ka_joined.split(',').filter(|x| !x.is_empty()).collect();
    for pair in flat.chunks(2) {
        if pair.len() == 2 {
            ka_raw.push((pair[0].to_string(), pair[1].to_string()));
        }
    }

    // pre-2.0 headers (gddocs level-start.md): colors as kS29..kS37 ("1_r_2_g_3_b..." per channel)
    // or, older still, kS1..kS6 (bg rgb, ground rgb), all comma pairs with
    // the kA settings (Stereo Madness and other official levels, 1.x levels)
    if !header.contains("kS38,") {
        channels.clear();
        ka_raw.clear();
        let (mut bg, mut gr) = ([None; 3], [None; 3]);
        for pair in header.split(',').collect::<Vec<_>>().chunks(2) {
            let [k, v] = pair else { continue };
            if k.starts_with("kA") {
                ka_raw.push((k.to_string(), v.to_string()));
                continue;
            }
            let Some(n) = k.strip_prefix("kS").and_then(|n| n.parse::<u16>().ok()) else { continue };
            match n {
                1..=3 => bg[n as usize - 1] = v.parse::<u8>().ok(),
                4..=6 => gr[n as usize - 4] = v.parse::<u8>().ok(),
                29..=37 => {
                    let id = [1000u16, 1001, 1002, 1004, 1, 2, 3, 4, 1003][n as usize - 29].to_string();
                    let mut kv: Vec<(&str, &str)> =
                        v.split('_').collect::<Vec<_>>().chunks(2).filter_map(|c| (c.len() == 2).then(|| (c[0], c[1]))).collect();
                    kv.retain(|(k, _)| *k != "6");
                    kv.push(("6", &id));
                    channels.extend(channel_from_kv(&kv));
                }
                _ => {}
            }
        }
        for (id, rgb) in [(1000u16, bg), (1001, gr)] {
            if let [Some(r), Some(g), Some(b)] = rgb {
                if !channels.iter().any(|c| c.id == id) {
                    channels.push(ColorChannel { id, r, g, b, opacity: 1.0, copy: None, blending: false });
                }
            }
        }
    }

    let get_ka = |k: &str| {
        ka_raw
            .iter()
            .find(|(pk, _)| pk == k)
            .and_then(|(_, v)| v.parse::<u8>().ok())
            .unwrap_or(0)
    };
    let header = LevelHeader {
        start_speed: get_ka("kA4"),
        start_gamemode: get_ka("kA2"),
        start_mini: get_ka("kA3") == 1,
        start_flip: get_ka("kA11") == 1,
        start_dual: get_ka("kA8") == 1,
        two_player: get_ka("kA10") == 1,
        start_pos_index: ka_start_pos(&ka_raw),
        channels,
        ka: ka_raw,
    };

    let objects = rest
        .split(';')
        .filter(|o| !o.trim().is_empty())
        .map(object::parse_object)
        .collect();

    Level { header, objects, id: 0, uid: next_uid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED: [(&str, usize); 5] = [
        ("149992717", 189605), // GRIEF
        ("148598469", 51879),  // Artemis 13
        ("127323087", 167383), // Society
        ("119544028", 139568), // Thinking Space II
        ("119550490", 147423), // Amethyst
    ];

    #[test]
    fn parses_all_five_demons() {
        for (id, count) in EXPECTED {
            let s = std::fs::read_to_string(format!("levels/{id}.gmd")).unwrap();
            let lvl = parse_level(&s);
            assert_eq!(lvl.objects.len(), count, "level {id}");
        }
    }

    #[test]
    fn old_headers_give_colors_and_settings() {
        // 2.0 style (official levels): kS29 = background, kS30 = ground
        let lvl = parse_level("kS29,1_40_2_62_3_255_4_0_5_0,kS30,1_0_2_19_3_200,kA13,0,kA4,1;1,1,2,15,3,15;");
        let bg = lvl.header.channels.iter().find(|c| c.id == 1000).expect("bg");
        assert_eq!((bg.r, bg.g, bg.b), (40, 62, 255));
        assert!(lvl.header.channels.iter().any(|c| c.id == 1001 && c.b == 200));
        assert_eq!(lvl.header.start_speed, 1);
        // 1.x style: kS1..kS3 background rgb
        let lvl = parse_level("kS1,64,kS2,99,kS3,255,kS4,32,kS5,93,kS6,241,kA1,7;1,1,2,15,3,15;");
        let bg = lvl.header.channels.iter().find(|c| c.id == 1000).expect("bg");
        assert_eq!((bg.r, bg.g, bg.b), (64, 99, 255));
    }

    #[test]
    fn grief_start_pos_and_channels() {
        let s = std::fs::read_to_string("levels/149992717.gmd").unwrap();
        let lvl = parse_level(&s);
        assert_eq!(lvl.header.start_pos_index, 25);
        assert!(lvl.header.channels.len() >= 10, "channels: {}", lvl.header.channels.len());
        assert!(lvl.header.channels.iter().any(|c| c.id == 1000));
        let ch = |id| lvl.header.channels.iter().find(|c| c.id == id).unwrap();
        assert_eq!(ch(155).copy, Some(140));
        assert!(ch(140).blending && !ch(141).blending);
    }

    #[test]
    fn parse_is_deterministic() {
        let s = std::fs::read_to_string("levels/148598469.gmd").unwrap();
        let a = parse_level(&s);
        let b = parse_level(&s);
        assert_eq!(a.objects.len(), b.objects.len());
        assert_eq!(a.objects[1000].x.to_bits(), b.objects[1000].x.to_bits());
    }

    #[test]
    fn rotation_and_groups_survive() {
        let s = std::fs::read_to_string("levels/148598469.gmd").unwrap();
        let lvl = parse_level(&s);
        assert!(lvl.objects.iter().any(|o| o.rot != 0.0));
        assert!(lvl.objects.iter().any(|o| !o.groups.is_empty()));
    }
}
