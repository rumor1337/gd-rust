//! Reads JSONL gift events from the Node bridge child process.

use std::io::{self, BufRead, BufReader};
use std::process::{Child, Command, Stdio};

#[derive(Debug, PartialEq)]
pub enum BridgeEvent {
    Gift { user: String, name: String, count: u32 },
    Status { state: String },
}

pub struct GiftBridge {
    _child: Child,
    reader: BufReader<std::process::ChildStdout>,
}

impl GiftBridge {
    /// Spawns `node bridge/index.js`; requires TT_USERNAME in the environment.
    pub fn spawn(project_dir: &std::path::Path) -> io::Result<Self> {
        let mut child = Command::new("node")
            .arg(project_dir.join("bridge/index.js"))
            .current_dir(project_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        Ok(GiftBridge { _child: child, reader: BufReader::new(stdout) })
    }

    /// Next event; None = bridge exited (caller restarts it).
    pub fn next_event(&mut self) -> io::Result<Option<BridgeEvent>> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => Ok(None),
            Ok(_) => Ok(parse_gift_line(line.trim())),
            Err(e) => Err(e),
        }
    }
}

pub fn parse_gift_line(line: &str) -> Option<BridgeEvent> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    match v.get("type")?.as_str()? {
        "gift" => Some(BridgeEvent::Gift {
            user: v.get("user").and_then(|u| u.as_str()).unwrap_or("?").to_string(),
            name: v.get("name").and_then(|u| u.as_str()).unwrap_or("?").to_string(),
            count: v.get("count").and_then(|u| u.as_u64()).unwrap_or(1) as u32,
        }),
        "status" => Some(BridgeEvent::Status {
            state: v.get("state").and_then(|u| u.as_str()).unwrap_or("?").to_string(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gift_and_status_lines() {
        assert_eq!(
            parse_gift_line(r#"{"type":"gift","user":"bob","name":"Rose","count":3}"#),
            Some(BridgeEvent::Gift { user: "bob".into(), name: "Rose".into(), count: 3 })
        );
        assert_eq!(
            parse_gift_line(r#"{"type":"status","state":"connected","roomId":"x"}"#),
            Some(BridgeEvent::Status { state: "connected".into() })
        );
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_gift_line("not json"), None);
        assert_eq!(parse_gift_line(r#"{"type":"other"}"#), None);
    }
}
