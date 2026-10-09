use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

#[derive(Debug)]
pub enum PlayerEvent {
    Position(Option<f64>),
    Duration(Option<f64>),
    Paused(bool),
    Volume(f64),
    FileLoaded,
    EndOfFile,
    LoadError(String),
    Died,
}

pub struct Player {
    child: Child,
    writer: UnixStream,
    sock_path: PathBuf,
}

impl Player {
    pub fn spawn(
        volume: u8,
        on_event: impl Fn(PlayerEvent) + Send + 'static,
    ) -> Result<Player> {
        let sock_path =
            std::env::temp_dir().join(format!("drift-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&sock_path);

        let mut cmd = Command::new("mpv");
        cmd.args([
            "--no-config",
            "--idle=yes",
            "--no-video",
            "--no-terminal",
            "--really-quiet",
            "--audio-display=no",
            "--force-window=no",
            "--ytdl=yes",
            "--ytdl-format=bestaudio/best",
            "--cache=yes",
            "--demuxer-max-bytes=32MiB",
        ])
        .arg(format!("--volume={volume}"))
        .arg(format!("--input-ipc-server={}", sock_path.display()));

        if let Ok(extra) = std::env::var("DRIFT_MPV_ARGS").or_else(|_| std::env::var("YTM_MPV_ARGS")) {
            cmd.args(extra.split_whitespace());
        }
        cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

        #[cfg(target_os = "linux")]
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                Ok(())
            });
        }

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow!("mpv not found — install it (see README)")
            } else {
                anyhow!("failed to start mpv: {e}")
            }
        })?;

        let start = Instant::now();
        let stream = loop {
            match UnixStream::connect(&sock_path) {
                Ok(s) => break s,
                Err(e) => {
                    if let Ok(Some(status)) = child.try_wait() {
                        bail!("mpv exited immediately ({status})");
                    }
                    if start.elapsed() > Duration::from_secs(6) {
                        let _ = child.kill();
                        let _ = child.wait();
                        bail!("timed out connecting to mpv IPC socket: {e}");
                    }
                    std::thread::sleep(Duration::from_millis(40));
                }
            }
        };

        let reader = stream.try_clone().context("cloning mpv socket")?;
        let mut player = Player {
            child,
            writer: stream,
            sock_path,
        };

        for (id, prop) in [(1, "time-pos"), (2, "duration"), (3, "pause"), (4, "volume")] {
            player.send(json!(["observe_property", id, prop]))?;
        }

        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                if let Some(ev) = parse_event(&v) {
                    on_event(ev);
                }
            }
            on_event(PlayerEvent::Died);
        });

        Ok(player)
    }

    fn send(&mut self, command: Value) -> Result<()> {
        let mut line = serde_json::to_string(&json!({ "command": command }))?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .and_then(|_| self.writer.flush())
            .map_err(|e| anyhow!("lost connection to mpv: {e}"))
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn load(&mut self, url: &str) -> Result<()> {
        self.send(json!(["set_property", "pause", false]))?;
        self.send(json!(["loadfile", url, "replace"]))
    }

    pub fn set_pause(&mut self, pause: bool) -> Result<()> {
        self.send(json!(["set_property", "pause", pause]))
    }

    pub fn set_volume(&mut self, vol: u8) -> Result<()> {
        self.send(json!(["set_property", "volume", vol]))
    }

    pub fn seek_relative(&mut self, secs: f64) -> Result<()> {
        self.send(json!(["seek", secs, "relative"]))
    }

    pub fn seek_absolute(&mut self, secs: f64) -> Result<()> {
        self.send(json!(["seek", secs, "absolute"]))
    }

    pub fn stop(&mut self) -> Result<()> {
        self.send(json!(["stop"]))
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.send(json!(["quit"]));
        for _ in 0..10 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.sock_path);
    }
}

fn parse_event(v: &Value) -> Option<PlayerEvent> {
    match v.get("event")?.as_str()? {
        "property-change" => {
            let data = v.get("data");
            match v.get("name")?.as_str()? {
                "time-pos" => Some(PlayerEvent::Position(data.and_then(Value::as_f64))),
                "duration" => Some(PlayerEvent::Duration(data.and_then(Value::as_f64))),
                "pause" => Some(PlayerEvent::Paused(
                    data.and_then(Value::as_bool).unwrap_or(false),
                )),
                "volume" => data.and_then(Value::as_f64).map(PlayerEvent::Volume),
                _ => None,
            }
        }
        "file-loaded" => Some(PlayerEvent::FileLoaded),
        "end-file" => match v.get("reason").and_then(Value::as_str) {
            Some("eof") => Some(PlayerEvent::EndOfFile),
            Some("error") => Some(PlayerEvent::LoadError(
                v.get("file_error")
                    .and_then(Value::as_str)
                    .unwrap_or("playback failed")
                    .to_string(),
            )),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn mpv_roundtrip() {
        use std::sync::mpsc;
        let wav = std::env::temp_dir().join("drift-test.wav");
        let samples: Vec<u8> = (0..16_000u32).map(|i| if (i / 20) % 2 == 0 { 140 } else { 116 }).collect();
        let mut data = Vec::new();
        data.extend_from_slice(b"RIFF");
        data.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
        data.extend_from_slice(b"WAVEfmt ");
        data.extend_from_slice(&16u32.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&8000u32.to_le_bytes());
        data.extend_from_slice(&8000u32.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&8u16.to_le_bytes());
        data.extend_from_slice(b"data");
        data.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        data.extend_from_slice(&samples);
        std::fs::write(&wav, data).unwrap();

        std::env::set_var("DRIFT_MPV_ARGS", "--ao=null");
        let (tx, rx) = mpsc::channel();
        let mut p = Player::spawn(50, move |e| {
            let _ = tx.send(e);
        })
        .expect("spawn mpv");
        p.load(wav.to_str().unwrap()).unwrap();
        let (mut loaded, mut eof) = (false, false);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !eof {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(PlayerEvent::FileLoaded) => loaded = true,
                Ok(PlayerEvent::EndOfFile) => eof = true,
                _ => {}
            }
        }
        assert!(loaded, "file-loaded not seen");
        assert!(eof, "end-of-file not seen");
    }

    #[test]
    fn event_parsing() {
        let v: Value = serde_json::from_str(
            r#"{"event":"property-change","id":1,"name":"time-pos","data":12.5}"#,
        )
        .unwrap();
        assert!(matches!(parse_event(&v), Some(PlayerEvent::Position(Some(p))) if p == 12.5));

        let v: Value = serde_json::from_str(r#"{"event":"end-file","reason":"eof"}"#).unwrap();
        assert!(matches!(parse_event(&v), Some(PlayerEvent::EndOfFile)));

        let v: Value = serde_json::from_str(r#"{"event":"end-file","reason":"stop"}"#).unwrap();
        assert!(parse_event(&v).is_none());

        let v: Value = serde_json::from_str(r#"{"error":"success","request_id":0}"#).unwrap();
        assert!(parse_event(&v).is_none());
    }
}
