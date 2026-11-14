//! GD level objects: `,`-separated key-value pairs after each `;`.

/// One placed object. Keys follow the community .gmd map:
/// 1=id, 2=x, 3=y, 4=flipX, 5=flipY, 6=rotation(deg),
/// 20=editorLayer, 21=baseColor, 22=detailColor, 24=zLayer,
/// 25=groups ('.'-separated), 128=scaleX, 129=scaleY.
/// Everything else lands in `raw` unmodified.
#[derive(Debug, Clone)]
pub struct GdObject {
    pub id: u16,
    pub x: f32,
    pub y: f32,
    pub rot: f32,
    pub flip_x: bool,
    pub flip_y: bool,
    pub scale_x: f32,
    pub scale_y: f32,
    pub base_color: Option<u16>,
    pub detail_color: Option<u16>,
    pub z_layer: Option<u8>,
    pub editor_layer: u8,
    pub groups: Vec<u16>,
    pub raw: Vec<(u16, String)>,
}

impl Default for GdObject {
    fn default() -> Self {
        GdObject {
            id: 0,
            x: 0.0,
            y: 0.0,
            rot: 0.0,
            flip_x: false,
            flip_y: false,
            scale_x: 1.0,
            scale_y: 1.0,
            base_color: None,
            detail_color: None,
            z_layer: None,
            editor_layer: 0,
            groups: Vec::new(),
            raw: Vec::new(),
        }
    }
}

pub fn parse_object(s: &str) -> GdObject {
    let mut o = GdObject::default();
    let parts: Vec<&str> = s.split(',').collect();
    let mut i = 0;
    while i + 1 < parts.len() {
        let k = parts[i];
        let v = parts[i + 1];
        i += 2;
        // Only well-formed numeric keys advance in pairs; odd tails are ignored.
        let Ok(key) = k.parse::<u16>() else { continue };
        match key {
            1 => {
                let _ = v.parse::<u16>().map(|x| o.id = x);
            }
            2 => {
                let _ = v.parse::<f32>().map(|x| o.x = x);
            }
            3 => {
                let _ = v.parse::<f32>().map(|x| o.y = x);
            }
            4 => o.flip_x = v == "1",
            5 => o.flip_y = v == "1",
            6 => {
                let _ = v.parse::<f32>().map(|x| o.rot = x);
            }
            20 => {
                let _ = v.parse::<u8>().map(|x| o.editor_layer = x);
            }
            21 => o.base_color = v.parse::<u16>().ok(),
            22 => o.detail_color = v.parse::<u16>().ok(),
            24 => o.z_layer = v.parse::<u8>().ok(),
            25 => { /* zOrder, not groups (key 57 is the real group key) */ }
            33 => {
                if let Ok(g) = v.parse::<u16>() {
                    o.groups = vec![g];
                }
            }
            57 => {
                o.groups = v
                    .split('.')
                    .filter_map(|g| g.parse::<u16>().ok())
                    .collect();
            }
            128 => {
                let _ = v.parse::<f32>().map(|x| o.scale_x = x);
            }
            129 => {
                let _ = v.parse::<f32>().map(|x| o.scale_y = x);
            }
            _ => o.raw.push((key, v.to_string())),
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_object() {
        let o = parse_object("1,914,2,25322.8,3,2042.2,6,-90,21,25,128,0.25,129,0.25");
        assert_eq!(o.id, 914);
        assert_eq!(o.x, 25322.8);
        assert_eq!(o.y, 2042.2);
        assert_eq!(o.rot, -90.0);
        assert_eq!(o.base_color, Some(25));
        assert_eq!(o.scale_x, 0.25);
    }

    #[test]
    fn unknown_keys_kept_raw() {
        let o = parse_object("1,914,31,a2V2aXg=,488,2");
        assert_eq!(o.id, 914);
        assert!(o.raw.contains(&(31u16, "a2V2aXg=".to_string())));
        assert!(o.raw.contains(&(488u16, "2".to_string())));
    }

    #[test]
    fn malformed_object_never_panics() {
        let o = parse_object("1,notanumber,garbage");
        assert_eq!(o.id, 0);
        let o2 = parse_object("");
        assert_eq!(o2.id, 0);
    }

    #[test]
    fn dot_separated_groups() {
        let o = parse_object("1,1,57,3.4.5");
        assert_eq!(o.groups, vec![3, 4, 5]);
        let o2 = parse_object("1,1,33,7");
        assert_eq!(o2.groups, vec![7]);
    }
}
