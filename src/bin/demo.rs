//! Geometry Dash in a 16:9 window: play a level yourself.
//!
//! Usage: `demo [level id] [--assets <GD folder>]` (default Stereo Madness, 1).
//! A level not in levels/ is downloaded with its song by tools/fetch_level.py.
//!
//! Space, Up, W or the left mouse button jump; a death restarts the attempt.
//! Insert (or M+E together, for keyboards without Insert) opens a menu: type
//! a level id and Enter to load it; Insert, M+E or Escape closes it. A level
//! id typed in the terminal (Enter) loads too. R restarts, Escape/Q quits.
//!
//! Art from the player's Geometry Dash install (see gdrust::assets: asked
//! for once with a folder picker if not found; --assets <dir> / DEMO_ASSETS
//! override). Uncapped frame rate; the sim advances 240Hz ticks by
//! wall-clock time. Progress and attempt live in the window title.
//!
//! The song and GD's death sound play in-process (rodio) from the sim's song
//! cue (level offset, then song triggers); DEMO_MUTE=1 turns them off.

use gdrust::level::parse_level;
use gdrust::render::{Renderer, FRAME_H, FRAME_W};
use gdrust::sim::objects::ObjectTable;
use gdrust::sim::{InputState, Sim};
use minifb::{Key, Window, WindowOptions};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

/// A level ready to play.
struct Loaded {
    level: gdrust::level::Level,
    name: String,
    song: Option<String>,
}

/// Level + metadata, downloading them when missing.
fn load_level(manifest: &str, id: u32) -> Result<Loaded, String> {
    let (gmd, meta) = (format!("{manifest}/levels/{id}.gmd"), format!("{manifest}/levels/{id}.json"));
    if !std::path::Path::new(&gmd).exists() || !std::path::Path::new(&meta).exists() {
        // Python's name differs by OS (Windows: python / py)
        let ok = ["python3", "python", "py"].iter().find_map(|py| {
            Command::new(py).args(["-I", &format!("{manifest}/tools/fetch_level.py"), &id.to_string()]).status().ok()
        });
        if !ok.is_some_and(|s| s.success()) {
            return Err(format!("could not download level {id}"));
        }
    }
    let read = |p: &str| std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"));
    let mut level = parse_level(&read(&gmd)?);
    level.id = id;
    let meta: serde_json::Value = serde_json::from_str(&read(&meta)?).map_err(|e| format!("level {id} meta: {e}"))?;
    let name = meta["name"].as_str().unwrap_or("level").to_string();
    let song = meta["song"].as_str().map(|s| format!("{manifest}/{s}"));
    Ok(Loaded { level, name, song })
}

/// A decoded sound, all in memory so playback can start at any sample.
struct Pcm {
    samples: Vec<i16>,
    channels: u16,
    rate: u32,
}

/// Plays a shared Pcm from one interleaved sample index on.
struct PcmFrom {
    pcm: Arc<Pcm>,
    at: usize,
}

impl Iterator for PcmFrom {
    type Item = i16;
    fn next(&mut self) -> Option<i16> {
        let s = self.pcm.samples.get(self.at).copied();
        self.at += 1;
        s
    }
}

impl rodio::Source for PcmFrom {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.pcm.channels
    }
    fn sample_rate(&self) -> u32 {
        self.pcm.rate
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

/// Interleaved sample index of `secs` into a song (frame-aligned).
fn sample_at(pcm: &Pcm, secs: f64) -> usize {
    (secs.max(0.0) * pcm.rate as f64) as usize * pcm.channels as usize
}

/// Song and sound effects through the system audio device (rodio), from
/// the sim's cue. Songs decode once, in the background, then any restart
/// (new attempt, song trigger, drift from the sim after a hitch) starts
/// at the exact sample with no delay. No device: silent.
/// shortcut: a restart is a hard cut (GD fades), fine for a local preview.
struct Music {
    /// audio/ (downloaded songs) and GD's Resources/ (official songs, sfx)
    dirs: [std::path::PathBuf; 2],
    /// the level's own song (sim song id 0)
    level_song: Option<String>,
    out: Option<(rodio::OutputStream, rodio::OutputStreamHandle)>,
    sink: Option<rodio::Sink>,
    /// (attempt, sim cue) it plays, and when/where it started
    cue: Option<((u32, (u32, f64, u64)), Instant, f64)>,
    /// decoded sounds by path; None while decoding or when it failed
    pcm: Arc<std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, Option<Arc<Pcm>>>>>,
}

impl Music {
    fn new(audio: std::path::PathBuf, resources: std::path::PathBuf, level_song: Option<String>) -> Music {
        let out = rodio::OutputStream::try_default().map_err(|e| eprintln!("no audio output: {e}")).ok();
        Music { dirs: [audio, resources], level_song, out, sink: None, cue: None, pcm: Default::default() }
    }

    /// A sound file: as given, else by name in audio/ then Resources/.
    fn find(&self, file: &str) -> Option<std::path::PathBuf> {
        let p = std::path::Path::new(file);
        let name = p.file_name()?;
        std::iter::once(p.to_path_buf()).chain(self.dirs.iter().map(|d| d.join(name))).find(|c| c.exists())
    }

    /// The decoded sound, or None while it decodes (started here, on a thread).
    fn decoded(&self, path: &std::path::Path) -> Option<Arc<Pcm>> {
        let mut map = self.pcm.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = map.get(path) {
            return p.clone();
        }
        map.insert(path.to_path_buf(), None);
        let (pcm, path) = (self.pcm.clone(), path.to_path_buf());
        std::thread::spawn(move || {
            let dec = std::fs::File::open(&path).map_err(|e| e.to_string()).and_then(|f| {
                rodio::Decoder::new(std::io::BufReader::new(f)).map_err(|e| e.to_string())
            });
            match dec {
                Ok(d) => {
                    use rodio::Source;
                    let (channels, rate) = (d.channels(), d.sample_rate());
                    let p = Pcm { samples: d.collect(), channels, rate };
                    pcm.lock().unwrap_or_else(|e| e.into_inner()).insert(path, Some(Arc::new(p)));
                }
                Err(e) => eprintln!("can't play {}: {e}", path.display()),
            }
        });
        None
    }

    fn sync(&mut self, sim: &Sim) {
        let (id, pos) = sim.song_pos();
        let key = (sim.attempt, sim.song);
        let drift = self.cue.as_ref().map_or(f64::MAX, |(_, t, p)| (p + t.elapsed().as_secs_f64() - pos).abs());
        if self.cue.as_ref().is_some_and(|c| c.0 == key) && drift < 0.25 {
            return;
        }
        // id 0 = the level's song; song triggers name Newgrounds ids
        let file = if id == 0 { self.level_song.clone() } else { Some(format!("{id}.mp3")) };
        let Some(path) = file.and_then(|f| self.find(&f)) else { return };
        let (Some(pcm), Some((_, handle))) = (self.decoded(&path), self.out.as_ref()) else { return };
        let Ok(sink) = rodio::Sink::try_new(handle) else { return };
        self.stop();
        sink.append(PcmFrom { at: sample_at(&pcm, pos), pcm });
        self.sink = Some(sink);
        self.cue = Some((key, Instant::now(), pos));
    }

    /// Fire-and-forget sound effect (GD's own, from Resources/).
    fn sfx(&self, name: &str) {
        use rodio::Source;
        let (Some(path), Some((_, handle))) = (self.find(name), self.out.as_ref()) else { return };
        if let Some(pcm) = self.decoded(&path) {
            let _ = handle.play_raw(PcmFrom { pcm, at: 0 }.convert_samples());
        }
    }

    fn stop(&mut self) {
        if let Some(s) = self.sink.take() {
            s.stop();
        }
        self.cue = None;
    }
}

/// A typed digit (top row or keypad) for the level id field.
fn key_digit(k: Key) -> Option<char> {
    use Key::*;
    let d = match k {
        Key0 | NumPad0 => 0,
        Key1 | NumPad1 => 1,
        Key2 | NumPad2 => 2,
        Key3 | NumPad3 => 3,
        Key4 | NumPad4 => 4,
        Key5 | NumPad5 => 5,
        Key6 | NumPad6 => 6,
        Key7 | NumPad7 => 7,
        Key8 | NumPad8 => 8,
        Key9 | NumPad9 => 9,
        _ => return None,
    };
    char::from_digit(d, 10)
}

fn main() {
    // shipped build: levels/ data/ next to the exe; dev: the source tree
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let manifest: &'static str = match exe_dir.filter(|d| d.join("data/objects.json").exists()) {
        Some(d) => Box::leak(d.to_string_lossy().into_owned().into_boxed_str()),
        None => env!("CARGO_MANIFEST_DIR"),
    };
    let (_, args) = gdrust::assets::split_flag(std::env::args().collect());
    // the player's own Geometry Dash art (RobTop's, never shipped): asked
    // for on the first run, before anything slow happens
    let assets = gdrust::assets::find_or_ask();
    let id: u32 = args.get(1).map(|a| a.parse().expect("level id must be a number")).unwrap_or(1);
    let table = ObjectTable::load(std::path::Path::new(&format!("{manifest}/data/objects.json"))).unwrap();
    let Loaded { mut level, mut name, song: level_song } = load_level(manifest, id).unwrap_or_else(|e| panic!("{e}"));

    // level ids from the terminal or the Insert menu, loaded (downloaded)
    // one at a time in the background, then switched to
    let status = Arc::new(std::sync::Mutex::new(String::new()));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Loaded>();
    let (req_tx, req_rx) = std::sync::mpsc::channel::<String>();
    {
        let req_tx = req_tx.clone();
        std::thread::spawn(move || {
            for line in std::io::stdin().lines().map_while(Result::ok) {
                if req_tx.send(line).is_err() {
                    return;
                }
            }
        });
    }
    {
        let status = status.clone();
        std::thread::spawn(move || {
            let set = |s: String| *status.lock().unwrap_or_else(|e| e.into_inner()) = s;
            println!("type a level id and Enter to load it, or press Insert (or M+E) in the window");
            for line in req_rx {
                let Ok(id) = line.trim().parse::<u32>() else {
                    if !line.trim().is_empty() {
                        println!("not a level id: {line}");
                    }
                    continue;
                };
                set(format!("loading {id}"));
                match load_level(manifest, id) {
                    Ok(l) => {
                        println!("{}: ready, switching", l.name);
                        if ready_tx.send(l).is_err() {
                            return; // window closed
                        }
                    }
                    Err(e) => println!("{e}: keeping the current level"),
                }
                set(String::new());
            }
        });
    }

    let mut renderer = Renderer::new(&assets);
    let mut sim = Sim::new(&level, &table);
    let mut window = Window::new(&format!("{name} — gd-rust"), FRAME_W as usize, FRAME_H as usize, WindowOptions::default())
        .expect("window");
    // uncapped: the sim advances by wall-clock time (240Hz ticks), the
    // renderer draws as fast as it can
    window.set_target_fps(0);
    let tick_dt = 1.0 / 240.0;
    let mut acc = 0.0f64;
    let mut clock = Instant::now();
    let mut ticks = 0u64;
    let mut frames_drawn = 0u64;
    let mut reported = false;
    let mut fb = vec![0u32; FRAME_W as usize * FRAME_H as usize];
    let mut last = Instant::now();
    let mut fps_smooth = 60.0f32;
    let mut music = Music::new(format!("{manifest}/audio").into(), assets.clone(), level_song);
    let mute = std::env::var("DEMO_MUTE").is_ok();
    let mut prof = std::env::var("DEMO_PROF").is_ok().then_some([0.0f64; 5]);
    // Insert menu: the level id being typed (None: closed)
    let mut menu: Option<String> = None;

    while window.is_open() {
        // a requested level finished loading: switch to it now
        if let Ok(next) = ready_rx.try_recv() {
            (level, name) = (next.level, next.name);
            music.stop();
            music.level_song = next.song;
            sim = Sim::new(&level, &table);
            (ticks, reported) = (0, false);
        }
        // as many 240Hz ticks as real time elapsed (capped so a hitch does
        // not snowball)
        let now = Instant::now();
        acc = (acc + (now - clock).as_secs_f64()).min(0.25);
        clock = now;
        let held = menu.is_none()
            && ([Key::Space, Key::Up, Key::W].iter().any(|&k| window.is_key_down(k)) || window.get_mouse_down(minifb::MouseButton::Left));
        while acc >= tick_dt {
            acc -= tick_dt;
            // death (or a finished run): restart from the beginning
            if sim.state == gdrust::sim::RunState::Dead || (reported && sim.progress() >= 100.0) {
                if sim.state == gdrust::sim::RunState::Dead && !mute {
                    music.sfx("explode_11.ogg"); // GD's death sound
                }
                sim.reset();
                (ticks, reported) = (0, false);
            }
            ticks += 1;
            sim.step_single(InputState { hold: held, hold_p2: held });
        }
        let menu_was_open = menu.is_some();
        // M+E together does the same, for keyboards without Insert (60%)
        let pressed = |k| window.is_key_pressed(k, minifb::KeyRepeat::No);
        let combo = window.is_key_down(Key::M) && window.is_key_down(Key::E) && (pressed(Key::M) || pressed(Key::E));
        if pressed(Key::Insert) || combo {
            menu = if menu_was_open { None } else { Some(String::new()) };
        }
        if let Some(code) = menu.as_mut() {
            let mut close = false;
            for k in window.get_keys_pressed(minifb::KeyRepeat::Yes) {
                match k {
                    Key::Backspace => drop(code.pop()),
                    Key::Escape => close = true,
                    Key::Enter | Key::NumPadEnter if !code.is_empty() => {
                        let _ = req_tx.send(code.clone());
                        close = true;
                    }
                    k => {
                        if let Some(d) = key_digit(k).filter(|_| code.len() < 12) {
                            code.push(d);
                        }
                    }
                }
            }
            if close {
                menu = None;
            }
        }
        if !menu_was_open && window.is_key_pressed(Key::R, minifb::KeyRepeat::No) {
            sim.reset();
            (ticks, reported) = (0, false);
        }
        if !reported && (sim.progress() >= 100.0 || sim.state == gdrust::sim::RunState::Complete) {
            println!("{name} complete in {:.2}s, attempt {}", ticks as f64 / 240.0, sim.attempt);
            reported = true;
        }
        if !menu_was_open && (pressed(Key::Escape) || pressed(Key::Q)) {
            break;
        }

        if !mute && sim.state == gdrust::sim::RunState::Playing && !reported {
            music.sync(&sim);
        } else {
            music.stop();
        }
        frames_drawn += 1;
        let dt = last.elapsed().as_secs_f32();
        last = Instant::now();
        if dt > 0.0 {
            fps_smooth = fps_smooth * 0.9 + (1.0 / dt) * 0.1;
        }
        renderer.set_fps(fps_smooth as u32);
        let next = status.lock().map(|s| s.clone()).unwrap_or_default();
        renderer.set_menu(menu.as_ref().map(|code| {
            vec![
                format!("Level ID: {code}_"),
                if next.is_empty() { "Enter: load   Insert / M+E: close".to_string() } else { next.clone() },
            ]
        }));
        // draw this frame on a worker while the window shows the last one:
        // a frame costs max(draw, present), not their sum (one frame later)
        let t_draw = Instant::now();
        let (rgba, present_ms) = std::thread::scope(|sc| {
            let job = sc.spawn(|| renderer.draw(&level, &table, &sim));
            let t = Instant::now();
            let _ = window.update_with_buffer(&fb, FRAME_W as usize, FRAME_H as usize);
            let present_ms = t.elapsed().as_secs_f64() * 1000.0;
            (job.join().expect("render thread"), present_ms)
        });
        let t_conv = Instant::now();
        {
            // renderer output is RGBA; minifb wants 0RGB
            use rayon::prelude::*;
            let w = FRAME_W as usize;
            fb.par_chunks_mut(w * 16).zip(rgba.par_chunks(w * 16 * 4)).for_each(|(d, s)| {
                for (px, c) in d.iter_mut().zip(s.chunks_exact(4)) {
                    *px = (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
                }
            });
        }
        let t_title = Instant::now();
        window.set_title(&format!(
            "{name} — attempt {} — {:.2}%{}  [Insert or M+E menu, R restart, Q quit]",
            sim.attempt,
            sim.progress(),
            if next.is_empty() { String::new() } else { format!(" — {next}") }
        ));
        if let Some(p) = prof.as_mut() {
            // DEMO_PROF: where a frame's wall time goes (sim, draw alongside
            // present, convert, title; present alone shown for reference)
            let e = [t_draw - now, t_conv - t_draw, t_title - t_conv, t_title.elapsed()].map(|d| d.as_secs_f64() * 1000.0);
            for (a, d) in p.iter_mut().zip(e.into_iter().chain([present_ms])) { *a += d; }
            if frames_drawn % 300 == 0 {
                println!("prof ms/frame: sim {:.2} draw||present {:.2} conv {:.2} title {:.2} (present {:.2}) | {:.0} fps", p[0] / 300.0, p[1] / 300.0, p[2] / 300.0, p[3] / 300.0, p[4] / 300.0, 300.0 / (p[..4].iter().sum::<f64>() / 1000.0));
                *p = [0.0; 5];
            }
        }
    }
    music.stop();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn song_starts_at_the_frame_for_its_cue() {
        let pcm = Arc::new(Pcm { samples: (0..20).collect(), channels: 2, rate: 4 });
        // 1.5 s at 4 Hz stereo = frame 6 = sample 12; never mid-frame
        assert_eq!(sample_at(&pcm, 1.5), 12);
        assert_eq!(sample_at(&pcm, 1.6), 12);
        assert_eq!(sample_at(&pcm, -1.0), 0);
        let rest: Vec<i16> = PcmFrom { at: sample_at(&pcm, 2.0), pcm: pcm.clone() }.collect();
        assert_eq!(rest, (16..20).collect::<Vec<i16>>());
    }
}
