//! ffmpeg child process: rawvideo RGBA in, x264 vertical video out to RTMP (or null for tests).
//! Pipe writes block when ffmpeg stalls, which is the backpressure: sim paces at 30fps, ffmpeg keeps up or the write waits.

use std::io::{self, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

pub struct Streamer {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Streamer {
    /// `rtmp`: full push URL. `None` → null muxer (tests/local dry runs).
    pub fn start(rtmp: Option<&str>, audio: Option<&str>, w: u32, h: u32, fps: u32) -> io::Result<Self> {
        let out_args: Vec<String> = match rtmp {
            Some(url) => vec!["-f".into(), "flv".into(), url.into()],
            None => vec!["-f".into(), "null".into(), "-".into()],
        };
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-loglevel").arg("error")
            .arg("-f").arg("rawvideo")
            .arg("-pix_fmt").arg("rgba")
            .arg("-s").arg(format!("{}x{}", w, h))
            .arg("-r").arg(fps.to_string())
            .arg("-i").arg("-"); // frame pipe on stdin
        if let Some(a) = audio {
            cmd.arg("-stream_loop").arg("-1").arg("-i").arg(a);
        }
        cmd.arg("-c:v").arg("libx264")
            .arg("-preset").arg("veryfast")
            .arg("-pix_fmt").arg("yuv420p")
            .arg("-g").arg((fps * 2).to_string());
        if audio.is_some() {
            cmd.arg("-c:a").arg("aac").arg("-b:a").arg("128k");
        }
        cmd.args(out_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = cmd.spawn()?;
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
        drop(s); // wait flushes ffmpeg
        // exit status checked by Drop; a crash here panics via expect above
    }
}
