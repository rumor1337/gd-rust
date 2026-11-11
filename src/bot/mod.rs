pub mod gdr2;

use gdr2::Macro;

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
        Playback { macro_: m, idx: 0, frame: 0, hold: false, hold_p2: false }
    }

    pub fn duration_frames(&self) -> u64 {
        self.macro_.inputs.last().map(|i| i.frame).unwrap_or(0)
    }

    /// Advance one 240Hz tick; returns (p1_hold, p2_hold).
    pub fn step(&mut self) -> (bool, bool) {
        while self.idx < self.macro_.inputs.len()
            && self.macro_.inputs[self.idx].frame == self.frame
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
}

/// Bot drives the sim by replaying a macro.
pub struct Bot {
    pub playback: Playback,
}

impl Bot {
    pub fn from_macro(m: Macro) -> Self {
        Bot { playback: Playback::new(m) }
    }

    pub fn step(&mut self) -> (bool, bool) {
        self.playback.step()
    }

    pub fn restart(&mut self) {
        self.playback.frame = 0;
        self.playback.idx = 0;
        self.playback.hold = false;
        self.playback.hold_p2 = false;
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
            inputs: vec![
                gdr2::MacroInput { frame: 5, button: 1, player2: false, down: true },
                gdr2::MacroInput { frame: 10, button: 1, player2: false, down: false },
            ],
        };
        let mut p = Playback::new(m);
        for _ in 0..5 {
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
