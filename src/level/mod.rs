//! .gmd level string parser: header (kS38 color channels, kA settings) + `;`-separated objects.

pub mod object;

use object::GdObject;

#[derive(Debug, Clone)]
pub struct ColorChannel {
    pub id: u16,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub opacity: f32,
}

#[derive(Debug, Clone, Default)]
pub struct LevelHeader {
    /// kA2 raw value (speed index; mapping pinned by physics task)
    pub start_speed: u8,
    /// kA3 raw value (start gamemode)
    pub start_gamemode: u8,
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
}

const CHANNEL_KEYS: [&str; 7] = ["1", "2", "3", "6", "7", "5", "8"];

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
        Level { header: LevelHeader::default(), objects: Vec::new() }
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

    let get_ka = |k: &str| {
        ka_raw
            .iter()
            .find(|(pk, _)| pk == k)
            .and_then(|(_, v)| v.parse::<u8>().ok())
            .unwrap_or(0)
    };
    let header = LevelHeader {
        start_speed: get_ka("kA2"),
        start_gamemode: get_ka("kA3"),
        start_pos_index: ka_start_pos(&ka_raw),
        channels,
        ka: ka_raw,
    };

    let objects = rest
        .split(';')
        .filter(|o| !o.trim().is_empty())
        .map(object::parse_object)
        .collect();

    Level { header, objects }
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
    fn grief_start_pos_and_channels() {
        let s = std::fs::read_to_string("levels/149992717.gmd").unwrap();
        let lvl = parse_level(&s);
        assert_eq!(lvl.header.start_pos_index, 25);
        assert!(lvl.header.channels.len() >= 10, "channels: {}", lvl.header.channels.len());
        assert!(lvl.header.channels.iter().any(|c| c.id == 1000));
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
