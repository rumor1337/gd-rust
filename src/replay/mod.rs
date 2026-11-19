//! Replays of real Geometry Dash recordings (xdBot .gdr2 macros), used by
//! the tools that check our physics and visuals against the real game.

pub mod gdr2;

use gdr2::Macro;

/// A recording of level `level_id` in `dir` (*.gdr2), ones with recorded
/// positions (xdBot frame fixes) first.
pub fn recorded_for(dir: &std::path::Path, level_id: u32) -> Option<Macro> {
    let mut found: Vec<Macro> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "gdr2"))
        .filter_map(|e| gdr2::parse_gdr2(&std::fs::read(e.path()).ok()?).ok())
        .filter(|m| m.level_id == level_id)
        .collect();
    found.sort_by_key(|m| m.frames.is_empty());
    found.into_iter().next()
}

/// Input state at a given 240Hz frame, derived from a macro's press/release events.
pub struct Playback {
    macro_: Macro,
    idx: usize,
    frame: u64,
    hold: bool,
    hold_p2: bool,
}

impl Playback {
    pub fn new(m: Macro) -> Self {
        // GD frames are 1-based: the first sim tick runs macro frame 1
        // (xdBot frame fixes show a frame-f click moving the tick-f position)
        Playback { macro_: m, idx: 0, frame: 1, hold: false, hold_p2: false }
    }

    pub fn duration_frames(&self) -> u64 {
        self.macro_.inputs.last().map(|i| i.frame).unwrap_or(0)
    }

    /// Advance one 240Hz tick; returns (p1_hold, p2_hold).
    pub fn step(&mut self) -> (bool, bool) {
        while self.idx < self.macro_.inputs.len()
            && self.macro_.inputs[self.idx].frame <= self.frame
        {
            let i = &self.macro_.inputs[self.idx];
            if i.player2 {
                self.hold_p2 = i.down;
            } else {
                self.hold = i.down;
            }
            self.idx += 1;
        }
        self.frame += 1;
        (self.hold, self.hold_p2)
    }

    /// The frame `step` just returned ran no physics in the real game (its
    /// recorded position repeats the previous one). Replays skip the sim
    /// tick there to stay frame-aligned with the macro.
    pub fn stalled(&self) -> bool {
        let f = (self.frame - 1) as usize;
        let fr = &self.macro_.frames;
        f >= 1 && f < fr.len() && fr[f][0] == fr[f - 1][0] && fr[f][1] == fr[f - 1][1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_toggles_hold() {
        let m = Macro {
            author: "t".into(),
            bot_name: "t".into(),
            level_id: 1,
            level_name: "t".into(),
            duration: 0.0,
            framerate: 240.0,
            platformer: false,
            deaths: vec![],
            frames: vec![],
            frames_p2: vec![],
            inputs: vec![
                gdr2::MacroInput { frame: 5, button: 1, player2: false, down: true },
                gdr2::MacroInput { frame: 10, button: 1, player2: false, down: false },
            ],
        };
        let mut p = Playback::new(m);
        for _ in 1..5 {
            let (h, _) = p.step();
            assert!(!h);
        }
        let (h, _) = p.step(); // frame 5: press
        assert!(h);
        for _ in 0..4 {
            let (h, _) = p.step();
            assert!(h);
        }
        let (h, _) = p.step(); // frame 10: release
        assert!(!h);
    }
}
