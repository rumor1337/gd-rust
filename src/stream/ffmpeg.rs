//! ffmpeg child process: rawvideo RGBA on stdin (+ optional raw PCM from a
//! FIFO) in, TikTok-shaped x264/AAC out: vertical 720x1280 (the 16:9 game is
//! letterboxed), 30fps-friendly GOP, CBR-ish bitrate. Output is an RTMP URL
//! (flv), a file path (format from the extension), or None = null muxer.
//! Pipe writes block when ffmpeg stalls; that is the backpressure.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

/// PCM format the audio FIFO carries
pub const AUDIO_RATE: u32 = 44_100;
pub const AUDIO_CHANNELS: u32 = 2;

pub struct Streamer {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Streamer {
    /// `out`: `rtmp(s)://…` push URL, a file path, or None for the null muxer.
    /// `pcm`: FIFO carrying s16le `AUDIO_RATE`/`AUDIO_CHANNELS` audio.
    pub fn start(out: Option<&str>, pcm: Option<&Path>, w: u32, h: u32, fps: u32) -> io::Result<Self> {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-y"])
            // raw inputs: no probing (older ffmpeg probes inputs one by one and
            // would wait for MBs of audio while the video pipe is blocked)
            .args(["-probesize", "32", "-analyzeduration", "0"])
            .args(["-thread_queue_size", "64", "-f", "rawvideo", "-pix_fmt", "rgba"])
            .args(["-s", &format!("{w}x{h}"), "-r", &fps.to_string(), "-i", "-"]);
        if let Some(p) = pcm {
            cmd.args(["-probesize", "32", "-analyzeduration", "0"])
                .args(["-thread_queue_size", "256", "-f", "s16le"])
                .args(["-ar", &AUDIO_RATE.to_string(), "-ac", &AUDIO_CHANNELS.to_string()])
                .arg("-i")
                .arg(p);
        }
        // TikTok LIVE ingest: 720x1280, H.264 high, keyframe every 2 s, ~3 Mbps
        cmd.args(["-vf", "scale=720:1280:force_original_aspect_ratio=decrease:force_divisible_by=2,pad=720:1280:(ow-iw)/2:(oh-ih)/2:black"])
            .args(["-c:v", "libx264", "-preset", "veryfast", "-tune", "zerolatency", "-pix_fmt", "yuv420p"])
            .args(["-b:v", "3000k", "-maxrate", "3000k", "-bufsize", "6000k"])
            .args(["-g", &(fps * 2).to_string(), "-keyint_min", &(fps * 2).to_string()]);
        if pcm.is_some() {
            cmd.args(["-c:a", "aac", "-b:a", "128k", "-ar", &AUDIO_RATE.to_string()]);
        }
        match out {
            Some(url) if url.starts_with("rtmp://") || url.starts_with("rtmps://") => {
                cmd.args(["-f", "flv", url]);
            }
            Some(path) => {
                cmd.args(["-movflags", "+faststart", path]);
            }
            None => {
                cmd.args(["-f", "null", "-"]);
            }
        }
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        Ok(Streamer { child, stdin: Some(stdin) })
    }

    /// One full frame, `w*h*4` RGBA bytes.
    pub fn submit(&mut self, frame: &[u8]) -> io::Result<()> {
        self.stdin.as_mut().expect("stdin closed").write_all(frame)
    }

    /// ffmpeg still running? (false = it exited; caller reconnects)
    pub fn is_alive(&mut self) -> bool {
        self.child.try_wait().map(|w| w.is_none()).unwrap_or(false)
    }

    /// Close the frame pipe and wait; true when ffmpeg exited cleanly.
    pub fn finish(mut self) -> bool {
        drop(self.stdin.take());
        self.child.wait().map(|s| s.success()).unwrap_or(false)
    }
}

impl Drop for Streamer {
    fn drop(&mut self) {
        drop(self.stdin.take()); // close pipe so ffmpeg flushes and exits
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_one_second_of_frames_to_null() {
        let (w, h, fps) = (64, 128, 30); // tiny so the test is fast
        let mut s = Streamer::start(None, None, w, h, fps).expect("ffmpeg spawn");
        let mut frame = vec![0u8; (w * h * 4) as usize];
        for i in 0..fps {
            frame[0] = i as u8; // vary content
            s.submit(&frame).expect("submit frame");
        }
        assert!(s.finish(), "ffmpeg exited with an error");
    }
}
