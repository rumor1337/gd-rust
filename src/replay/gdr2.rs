//! GDR2 binary replay format parser.
//! Spec: maxnut/GDReplayFormat, branch `gdr2`, include/gdr/gdr.hpp + binarystream.hpp.
//! Layout: "GDR" magic, varint version (2), varint-len-prefixed strings, BE floats,
//! LEB128 varints, extension block, death deltas, then delta-packed inputs:
//! non-platformer packed = (frame_delta << 1) | down, platformer = (delta << 3) | (button << 1) | down.

#[derive(Debug, Clone)]
pub struct MacroInput {
    pub frame: u64,
    pub button: u8,
    pub player2: bool,
    pub down: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Macro {
    pub author: String,
    pub bot_name: String,
    pub level_id: u32,
    pub level_name: String,
    pub duration: f32,
    pub framerate: f64,
    pub platformer: bool,
    pub deaths: Vec<u64>,
    pub inputs: Vec<MacroInput>,
    /// xdBot frame fixes: real player-1 (x, y, rotation) per 240Hz frame,
    /// index = frame - 1 (empty when the macro has none)
    pub frames: Vec<[f32; 3]>,
    /// the same for player 2 (dual sections; meaningless while solo)
    pub frames_p2: Vec<[f32; 3]>,
}

/// xdBot's header extension: varint count, then per frame
/// varint frame, p1 (x, y, rot) f32be, u8, p2 (x, y, rot) f32be, u8.
fn xdbot_frames(ext: &[u8]) -> Option<(Vec<[f32; 3]>, Vec<[f32; 3]>)> {
    let mut r = Reader::new(ext);
    let n = r.varint().ok()? as usize;
    let (mut out, mut out2) = (Vec::with_capacity(n.min(ext.len() / 27)), Vec::new());
    for _ in 0..n {
        let f = r.varint().ok()? as usize;
        let p = [r.f32be().ok()?, r.f32be().ok()?, r.f32be().ok()?];
        r.skip(1).ok()?;
        let p2 = [r.f32be().ok()?, r.f32be().ok()?, r.f32be().ok()?];
        r.skip(1).ok()?;
        if f != out.len() + 1 {
            return None;
        }
        out.push(p);
        out2.push(p2);
    }
    r.empty().then_some((out, out2))
}

struct Reader<'a> {
    b: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Reader { b }
    }

    fn u8(&mut self) -> Result<u8, String> {
        let (v, rest) = self.b.split_first().ok_or("eof")?;
        self.b = rest;
        Ok(*v)
    }

    fn varint(&mut self) -> Result<u64, String> {
        let mut out: u64 = 0;
        for i in 0..10 {
            let byte = self.u8()?;
            out |= ((byte & 0x7F) as u64) << (i * 7);
            if byte & 0x80 == 0 {
                return Ok(out);
            }
        }
        Err("varint too long".into())
    }

    fn string(&mut self) -> Result<String, String> {
        let len = self.varint()? as usize;
        if self.b.len() < len {
            return Err("string overrun".into());
        }
        let (s, rest) = self.b.split_at(len);
        self.b = rest;
        Ok(String::from_utf8_lossy(s).into_owned())
    }

    fn f32be(&mut self) -> Result<f32, String> {
        if self.b.len() < 4 {
            return Err("eof".into());
        }
        let (s, rest) = self.b.split_at(4);
        self.b = rest;
        Ok(f32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn f64be(&mut self) -> Result<f64, String> {
        if self.b.len() < 8 {
            return Err("eof".into());
        }
        let (s, rest) = self.b.split_at(8);
        self.b = rest;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Ok(f64::from_be_bytes(a))
    }

    fn skip(&mut self, n: usize) -> Result<(), String> {
        if self.b.len() < n {
            return Err("skip overrun".into());
        }
        self.b = &self.b[n..];
        Ok(())
    }

    fn empty(&self) -> bool {
        self.b.is_empty()
    }
}

pub fn parse_gdr2(data: &[u8]) -> Result<Macro, String> {
    let mut r = Reader::new(data);
    if !data.starts_with(b"GDR") {
        return Err("bad magic".into());
    }
    r.skip(3)?;
    let version = r.varint()? as i32;
    if version != 2 {
        return Err(format!("unsupported version {version}"));
    }
    let input_tag = r.string()?; // non-empty => per-input extensions exist
    let author = r.string()?;
    let _description = r.string()?;
    let duration = r.f32be()?;
    let _game_version = r.varint()?;
    let framerate = r.f64be()?;
    let _seed = r.varint()?;
    let _coins = r.varint()?;
    let _ldm = r.u8()? != 0;
    let platformer = r.u8()? != 0;
    let bot_name = r.string()?;
    let _bot_version = r.varint()?;
    let level_id = r.varint()? as u32;
    let level_name = r.string()?;

    let ext_len = r.varint()? as usize;
    let (frames, frames_p2) = if bot_name == "xdBot" {
        r.b.get(..ext_len).and_then(xdbot_frames).unwrap_or_default()
    } else {
        Default::default()
    };
    r.skip(ext_len)?;

    let death_count = r.varint()? as usize;
    // counts come from the file: cap by what the remaining bytes can hold
    let mut deaths = Vec::with_capacity(death_count.min(r.b.len()));
    let mut p = 0u64;
    for _ in 0..death_count {
        let delta = r.varint()?;
        p += delta;
        deaths.push(p);
    }

    let input_count = r.varint()? as usize;
    let p1_inputs = r.varint()? as usize;

    let mut inputs = Vec::with_capacity(input_count.min(r.b.len()));
    let mut frame = 0u64;
    let mut remaining_p1 = p1_inputs;
    let mut in_p2_section = false;
    while !r.empty() {
        let packed = r.varint()?;
        let (delta, button, down) = if platformer {
            ((packed >> 3) as u64, ((packed >> 1) & 3) as u8, packed & 1 == 1)
        } else {
            (packed >> 1, 1u8, packed & 1 == 1)
        };
        frame += delta;
        if !in_p2_section && remaining_p1 == 0 {
            // switch to player-2 section; frame resets
            in_p2_section = true;
            frame = delta;
        }
        let player2 = in_p2_section;
        inputs.push(MacroInput { frame, button, player2, down });
        if !in_p2_section {
            remaining_p1 -= 1;
            if remaining_p1 == 0 && p1_inputs > 0 {
                frame = 0;
            }
        }
        if !input_tag.is_empty() {
            let iext = r.varint()? as usize;
            r.skip(iext)?;
        }
    }
    inputs.sort_by_key(|i| i.frame);

    if inputs.len() != input_count {
        return Err(format!(
            "input count mismatch: header says {input_count}, parsed {}",
            inputs.len()
        ));
    }

    Ok(Macro {
        author,
        bot_name,
        level_id,
        level_name,
        duration,
        framerate,
        platformer,
        deaths,
        inputs,
        frames,
        frames_p2,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn macro_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("macros")
            .join(name)
    }

    #[test]
    fn parses_grief_xdbot() {
        let data = std::fs::read(macro_path("Cerealmilkbowl-GRIEF-xdBot.gdr2")).unwrap();
        let m = parse_gdr2(&data).unwrap();
        assert_eq!(m.level_id, 149992717);
        assert_eq!(m.level_name, "GRIEF");
        assert_eq!(m.bot_name, "xdBot");
        assert!((m.framerate - 240.0).abs() < 1e-9, "framerate {}", m.framerate);
        assert!(m.inputs.len() > 1000, "inputs {}", m.inputs.len());
        assert!(m.inputs.windows(2).all(|w| w[0].frame <= w[1].frame));
        assert!(!m.platformer);
        // frame fixes: one real position per frame, spawn at (0, 105)
        assert_eq!(m.frames.len(), 40101);
        assert_eq!(m.frames[0], [0.0, 105.0, 0.0]);
    }

    #[test]
    fn rejects_short_and_lying_files() {
        assert!(parse_gdr2(b"").is_err());
        assert!(parse_gdr2(b"G").is_err());
        // valid header up to the death count, which claims ~2^63 deaths
        let mut d = b"GDR\x02".to_vec();
        d.extend([0, 0, 0]); // input tag, author, description
        d.extend([0; 4]); // duration
        d.push(0); // game version
        d.extend([0; 8]); // framerate
        d.extend([0, 0, 0, 0, 0]); // seed, coins, ldm, platformer, bot name
        d.extend([0, 0, 0, 0]); // bot version, level id, level name, ext len
        d.extend([0xff; 9]);
        d.push(0x7f); // death count varint
        assert!(parse_gdr2(&d).is_err());
    }

    #[test]
    fn parses_all_xdbot_macros() {
        for f in [
            "Cerealmilkbowl-GRIEF-xdBot.gdr2",
            "ChesszDC-Artemis-13-xdBot.gdr2",
            "ChesszDC-Society-xdBot.gdr2",
            "ChesszDC-Thinking-Space-II-xdBot.gdr2",
        ] {
            let data = std::fs::read(macro_path(f)).unwrap();
            let m = parse_gdr2(&data).unwrap_or_else(|e| panic!("{f}: {e}"));
            println!("{f}: {} inputs, {} deaths, dur {:.1}s", m.inputs.len(), m.deaths.len(), m.duration);
        }
    }
}
