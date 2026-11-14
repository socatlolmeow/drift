use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::config::Audio;

/// Platform-specific transport for mpv's JSON IPC.
/// Unix: a Unix domain socket. Windows: a named pipe (`\\.\pipe\...`).
#[cfg(unix)]
mod ipc {
    use std::io;
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};

    pub type Reader = UnixStream;
    pub type Writer = UnixStream;

    pub fn endpoint() -> PathBuf {
        let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
        let path = dir.join(format!("drift-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    pub fn connect(path: &Path) -> io::Result<(Reader, Writer)> {
        let stream = UnixStream::connect(path)?;
        let reader = stream.try_clone()?;
        Ok((reader, stream))
    }

    pub fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(windows)]
mod ipc {
    use std::ffi::c_void;
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read};
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    pub type Writer = File;

    extern "system" {
        fn PeekNamedPipe(
            pipe: *mut c_void,
            buffer: *mut c_void,
            buffer_size: u32,
            bytes_read: *mut u32,
            total_bytes_avail: *mut u32,
            bytes_left_this_message: *mut u32,
        ) -> i32;
    }

    /// Reader half of the pipe.
    ///
    /// A blocking `ReadFile` on a synchronous pipe handle also blocks any
    /// `WriteFile` on a clone of that handle, which would deadlock the player.
    /// So we poll with `PeekNamedPipe` and only read when data is waiting.
    pub struct Reader(File);

    impl Read for Reader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            loop {
                let mut avail: u32 = 0;
                let ok = unsafe {
                    PeekNamedPipe(
                        self.0.as_raw_handle() as *mut c_void,
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        &mut avail,
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 {
                    // mpv exited / pipe closed: report EOF.
                    return Ok(0);
                }
                if avail > 0 {
                    let n = (avail as usize).min(buf.len());
                    return self.0.read(&mut buf[..n]);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    pub fn endpoint() -> PathBuf {
        PathBuf::from(format!(r"\\.\pipe\drift-{}", std::process::id()))
    }

    pub fn connect(path: &Path) -> io::Result<(Reader, Writer)> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let reader = file.try_clone()?;
        Ok((Reader(reader), file))
    }

    pub fn cleanup(_path: &Path) {
        // Named pipes disappear when mpv exits.
    }
}

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
    writer: ipc::Writer,
    sock_path: PathBuf,
}

impl Player {
    pub fn spawn(volume: u8, on_event: impl Fn(PlayerEvent) + Send + 'static) -> Result<Player> {
        let sock_path = ipc::endpoint();

        let mut cmd = Command::new("mpv");
        cmd.args([
            "--no-config",
            "--idle=yes",
            "--no-video",
            "--no-terminal",
            "--really-quiet",
            "--audio-display=no",
            "--force-window=no",
            "--ytdl=no",
            "--cache=yes",
            "--demuxer-max-bytes=32MiB",
        ])
        .arg(format!("--volume={volume}"))
        .arg(format!("--input-ipc-server={}", sock_path.display()));

        if let Ok(extra) =
            std::env::var("DRIFT_MPV_ARGS").or_else(|_| std::env::var("YTM_MPV_ARGS"))
        {
            cmd.args(extra.split_whitespace());
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::sys::no_window(&mut cmd);

        // Linux only: ask the kernel to SIGTERM mpv if drift dies unexpectedly.
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

        // Windows: tie mpv's lifetime to ours.
        crate::sys::kill_on_exit(&child);

        let start = Instant::now();
        let (reader, writer) = loop {
            match ipc::connect(&sock_path) {
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

        let mut player = Player {
            child,
            writer,
            sock_path,
        };

        for (id, prop) in [
            (1, "time-pos"),
            (2, "duration"),
            (3, "pause"),
            (4, "volume"),
        ] {
            player.send(json!(["observe_property", id, prop]))?;
        }
        player.send(json!(["request_log_messages", "error"]))?;

        std::thread::spawn(move || {
            let mut reason: Option<String> = None;
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                match v.get("event").and_then(Value::as_str) {
                    Some("log-message") => {
                        let prefix = v.get("prefix").and_then(Value::as_str).unwrap_or("");
                        let text = v.get("text").and_then(Value::as_str).unwrap_or("").trim();
                        match text.strip_prefix("ERROR: ") {
                            Some(t) if prefix == "ytdl_hook" => reason = Some(ytdl_reason(t)),
                            _ if reason.is_none()
                                && !text.is_empty()
                                && !text.starts_with("Failed to open") =>
                            {
                                reason = Some(text.lines().next().unwrap_or(text).to_string());
                            }
                            _ => {}
                        }
                        continue;
                    }
                    Some("start-file") => reason = None,
                    _ => {}
                }
                let Some(mut ev) = parse_event(&v) else {
                    continue;
                };
                match &mut ev {
                    PlayerEvent::LoadError(msg) => {
                        if let Some(r) = reason.take() {
                            *msg = r;
                        }
                    }
                    PlayerEvent::FileLoaded => reason = None,
                    _ => {}
                }
                on_event(ev);
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

    pub fn load(&mut self, path: &str) -> Result<()> {
        self.send(json!(["set_property", "pause", false]))?;
        self.send(json!(["loadfile", path, "replace"]))
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
        ipc::cleanup(&self.sock_path);
    }
}

const KEEP_FILES: usize = 40;

/// Base yt-dlp command with the user's cookie setting applied.
fn ytdlp(audio: &Audio) -> Command {
    let mut cmd = Command::new("yt-dlp");
    if let Some(browser) = audio.cookies() {
        cmd.arg("--cookies-from-browser").arg(browser);
    }
    cmd
}

fn download_component(text: &str) -> String {
    let mut name = String::new();
    for c in text.chars() {
        let c = if c.is_control() || "/\\:*?\"<>|%~$".contains(c) {
            '_'
        } else {
            c
        };
        if name.len() + c.len_utf8() > 80 {
            break;
        }
        name.push(c);
    }
    let name = name.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if name.is_empty() {
        "Unknown".into()
    } else {
        name.into()
    }
}

fn download_command(
    track: &crate::model::Track,
    audio: &Audio,
    dir: &std::path::Path,
) -> Result<Command, String> {
    if track.id.len() != 11
        || !track
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err("invalid YouTube track ID".into());
    }
    let filename = format!(
        "{} - {} [{}].%(ext)s",
        download_component(&track.artist),
        download_component(&track.title),
        track.id,
    );
    let mut cmd = ytdlp(audio);
    cmd.args([
        "--ignore-config",
        "-f",
        "bestaudio",
        "--no-playlist",
        "--no-overwrites",
        "--no-progress",
        "--no-warnings",
    ])
    .arg("--paths")
    .arg(dir)
    .arg(format!("--output={filename}"))
    .arg("--")
    .arg(track.url())
    .stdin(Stdio::null())
    .stdout(Stdio::null());
    crate::sys::no_window(&mut cmd);
    Ok(cmd)
}

pub fn download_track(
    track: &crate::model::Track,
    audio: &Audio,
    dir: &std::path::Path,
) -> Result<(), String> {
    let mut cmd = download_command(track, audio, dir)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    let out = cmd
        .output()
        .map_err(|e| format!("could not run yt-dlp ({e}); is it installed and on PATH?"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let reason = stderr
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("ERROR: "))
        .map(ytdl_reason)
        .filter(|reason| !reason.is_empty())
        .unwrap_or_else(|| stderr.trim().to_string());
    if reason.is_empty() {
        Err(format!("yt-dlp failed ({})", out.status))
    } else {
        Err(format!("yt-dlp failed ({}): {reason}", out.status))
    }
}

fn audio_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("drift")
        .join("audio")
}

pub fn cached_audio(id: &str) -> Option<PathBuf> {
    std::fs::read_dir(audio_dir())
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(id))
}

pub fn fetch_audio(id: &str, audio: &Audio) -> Result<PathBuf, String> {
    if let Some(path) = cached_audio(id) {
        return Ok(path);
    }
    let dir = audio_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    let mut cmd = ytdlp(audio);
    cmd.args([
        "-f",
        "bestaudio/best",
        "--no-playlist",
        "--no-warnings",
        "--no-progress",
        "--no-mtime",
    ])
    .arg("-P")
    .arg(&dir)
    .args(["-o", "%(id)s.%(ext)s"])
    .arg(format!("https://music.youtube.com/watch?v={id}"))
    .stdin(Stdio::null())
    .stdout(Stdio::null());
    crate::sys::no_window(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| format!("could not run yt-dlp ({e}); is it installed and on PATH?"))?;
    if let Some(path) = cached_audio(id).filter(|_| out.status.success()) {
        prune(&dir);
        return Ok(path);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(stderr
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("ERROR: "))
        .map(ytdl_reason)
        .unwrap_or_else(|| "yt-dlp downloaded nothing".into()))
}

/// Ask yt-dlp for the direct media URL so mpv can start playing right away
/// instead of waiting for the whole file to download.
pub fn resolve_stream(id: &str, audio: &Audio) -> Result<String, String> {
    let mut cmd = ytdlp(audio);
    cmd.args([
        "-f",
        "bestaudio/best",
        "--no-playlist",
        "--no-warnings",
        "-O",
        "%(available_at|0)s",
        "-g",
    ])
    .arg(format!("https://music.youtube.com/watch?v={id}"))
    .stdin(Stdio::null());
    crate::sys::no_window(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| format!("could not run yt-dlp ({e}); is it installed and on PATH?"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if let Some(url) = stdout
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("http"))
        .filter(|_| out.status.success())
    {
        // YouTube answers 403 on a fresh stream URL until its `available_at` time.
        if let Some(at) = stdout.lines().find_map(|l| l.trim().parse::<u64>().ok()) {
            std::thread::sleep(wait_until(at, SystemTime::now()));
        }
        return Ok(url.to_string());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(stderr
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("ERROR: "))
        .map(ytdl_reason)
        .unwrap_or_else(|| "yt-dlp returned no stream URL".into()))
}

fn wait_until(at: u64, now: SystemTime) -> Duration {
    let elapsed = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    Duration::from_secs(at)
        .saturating_sub(elapsed)
        .min(Duration::from_secs(30))
}

fn prune(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() > KEEP_FILES {
        files.sort();
        for (_, path) in &files[..files.len() - KEEP_FILES] {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub fn ytdl_reason(text: &str) -> String {
    let text = text.split("; please report").next().unwrap_or(text).trim();
    let text = match text.strip_prefix('[').and_then(|t| t.split_once("] ")) {
        Some((_, rest)) => match rest.split_once(": ") {
            Some((id, msg)) if id.len() == 11 && !id.contains(' ') => msg,
            _ => rest,
        },
        None => text,
    };
    text.trim().to_string()
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

    fn download_test_track() -> crate::model::Track {
        crate::model::Track {
            id: "dQw4w9WgXcQ".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            artists: Vec::new(),
            album: None,
            duration: None,
        }
    }

    #[test]
    fn persistent_download_arguments() {
        let audio = Audio {
            cookies_from_browser: " firefox:profile ".into(),
            ..Audio::default()
        };
        let dir = std::env::temp_dir().join("drift downloads");
        let cmd = download_command(&download_test_track(), &audio, &dir).unwrap();
        assert_eq!(cmd.get_program(), "yt-dlp");
        let args: Vec<_> = cmd.get_args().map(|arg| arg.to_string_lossy()).collect();
        assert_eq!(
            args,
            vec![
                "--cookies-from-browser",
                "firefox:profile",
                "--ignore-config",
                "-f",
                "bestaudio",
                "--no-playlist",
                "--no-overwrites",
                "--no-progress",
                "--no-warnings",
                "--paths",
                dir.to_str().unwrap(),
                "--output=Artist - Song [dQw4w9WgXcQ].%(ext)s",
                "--",
                "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
            ]
        );
    }

    #[test]
    fn persistent_download_sanitizes_names_and_preserves_ids() {
        let mut track = download_test_track();
        track.artist = "../../%(title)s\\Artist".into();
        track.title = " /evil:\"*?<>|\n\0%(id)s ".into();
        let output = |track: &crate::model::Track| {
            download_command(track, &Audio::default(), std::path::Path::new("music"))
                .unwrap()
                .get_args()
                .find_map(|arg| arg.to_str()?.strip_prefix("--output=").map(str::to_owned))
                .unwrap()
        };
        let first = output(&track);
        assert_eq!(first.matches('%').count(), 1);
        assert!(first.ends_with(" [dQw4w9WgXcQ].%(ext)s"));
        assert_eq!(std::path::Path::new(&first).components().count(), 1);
        assert!(!first
            .chars()
            .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c)));
        assert!(!first.starts_with('.'));
        track.id = "aaaaaaaaaaa".into();
        assert_ne!(output(&track), first);
        assert_eq!(download_component("...  "), "Unknown");
        assert_eq!(download_component("Björk"), "Björk");
        assert_eq!(
            download_component("~/$HOME/${USER}/%PATH%"),
            "___HOME__{USER}__PATH_"
        );
        let long = download_component(&"音".repeat(100));
        assert!(long.len() <= 80);
        assert!(long.chars().all(|c| c == '音'));
    }

    #[test]
    fn persistent_download_rejects_invalid_ids_before_io() {
        let dir = std::env::temp_dir().join(format!(
            "drift-invalid-download-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let mut track = download_test_track();
        for id in [
            "",
            "short",
            "aaaaaaaaaaaa",
            "../aaaaaaaa",
            "aaaaa/aaaaa",
            "aaaaa%aaaaa",
        ] {
            track.id = id.into();
            assert_eq!(
                download_track(&track, &Audio::default(), &dir).unwrap_err(),
                "invalid YouTube track ID"
            );
        }
        assert!(!dir.exists());
        track.id = "_-abcDEF012".into();
        assert!(download_command(&track, &Audio::default(), &dir).is_ok());
    }

    #[test]
    fn wait_until_available_at() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        assert_eq!(wait_until(1_004, now), Duration::from_secs(4));
        assert_eq!(wait_until(0, now), Duration::ZERO);
        assert_eq!(wait_until(999, now), Duration::ZERO);
        assert_eq!(wait_until(5_000, now), Duration::from_secs(30));
        assert_eq!(wait_until(u64::MAX, now), Duration::from_secs(30));
    }

    #[test]
    #[ignore]
    fn mpv_roundtrip() {
        use std::sync::mpsc;
        let wav = std::env::temp_dir().join("drift-test.wav");
        let samples: Vec<u8> = (0..16_000u32)
            .map(|i| if (i / 20) % 2 == 0 { 140 } else { 116 })
            .collect();
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
    fn ytdl_reason_is_trimmed() {
        assert_eq!(
            ytdl_reason(
                "[youtube] dQw4w9WgXcQ: Sign in to confirm you're not a bot. Use --cookies. \n"
            ),
            "Sign in to confirm you're not a bot. Use --cookies."
        );
        assert_eq!(
            ytdl_reason("[youtube] dQw4w9WgXcQ: Failed to extract any player response; please report this issue on  https://x"),
            "Failed to extract any player response"
        );
        assert_eq!(ytdl_reason("Unsupported URL: foo"), "Unsupported URL: foo");
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
