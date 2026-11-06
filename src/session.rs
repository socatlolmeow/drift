use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::model::{is_valid_id, Track};
use crate::store::write_atomic;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub queue: Vec<Track>,
    pub current: Option<usize>,
    pub position: f64,
}

impl Session {
    fn path() -> Result<PathBuf> {
        let dir = crate::store::state_base()
            .ok_or_else(|| anyhow!("cannot determine state directory"))?;
        Ok(dir.join("drift").join("session.json"))
    }

    /* The first drift to start owns the saved session. Later ones neither
    restore nor overwrite it. The lock goes away when the process exits. */
    #[cfg(unix)]
    pub fn claim() -> bool {
        use std::os::unix::io::AsRawFd;
        let Ok(path) = Self::path().map(|p| p.with_file_name("session.lock")) else {
            return true;
        };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let Ok(file) = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
        else {
            return true;
        };
        let ok = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        if ok {
            std::mem::forget(file);
        }
        ok
    }

    #[cfg(not(unix))]
    pub fn claim() -> bool {
        true
    }

    pub fn exists() -> bool {
        Self::path().is_ok_and(|p| p.exists())
    }

    pub fn load() -> Session {
        let Ok(path) = Self::path() else {
            return Session::default();
        };
        let Ok(text) = fs::read_to_string(&path) else {
            return Session::default();
        };
        match serde_json::from_str::<Session>(&text) {
            Ok(mut s) => {
                s.sanitize();
                s
            }
            Err(_) => {
                let _ = fs::rename(&path, path.with_extension("json.bak"));
                Session::default()
            }
        }
    }

    fn sanitize(&mut self) {
        // Keep the selected track selected when invalid queue entries are
        // removed before it.
        if let Some(current) = self.current {
            if current < self.queue.len() {
                if is_valid_id(&self.queue[current].id) {
                    self.current = Some(
                        self.queue
                            .iter()
                            .take(current)
                            .filter(|t| is_valid_id(&t.id))
                            .count(),
                    );
                } else {
                    self.current = None;
                    self.position = 0.0;
                }
            }
        }
        self.queue.retain(|t| is_valid_id(&t.id));
        if !self.position.is_finite() || self.position < 0.0 {
            self.position = 0.0;
        }
        match self.current {
            Some(_) if self.queue.is_empty() => self.current = None,
            Some(c) if c >= self.queue.len() => self.current = Some(self.queue.len() - 1),
            _ => {}
        }
        if self.current.is_none() {
            self.position = 0.0;
        }
    }

    pub fn render(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn write(text: &str) -> Result<()> {
        write_atomic(&Self::path()?, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track {
            id: id.into(),
            title: "T".into(),
            artist: "A".into(),
            artists: Vec::new(),
            album: None,
            duration: Some(200),
        }
    }

    #[test]
    fn sanitize_drops_bad_tracks_and_clamps() {
        let mut s = Session {
            queue: vec![track("aaaaaaaaaaa"), track("bad")],
            current: Some(7),
            position: f64::NAN,
        };
        s.sanitize();
        assert_eq!((s.queue.len(), s.current, s.position), (1, Some(0), 0.0));
    }

    #[test]
    fn sanitize_preserves_current_track_when_dropping_bad_entries() {
        let mut s = Session {
            queue: vec![track("bad"), track("aaaaaaaaaaa"), track("bbbbbbbbbbb")],
            current: Some(1),
            position: 12.0,
        };
        s.sanitize();
        assert_eq!(s.queue.len(), 2);
        assert_eq!(s.current, Some(0));
        assert_eq!(s.queue[0].id, "aaaaaaaaaaa");
        assert_eq!(s.position, 12.0);
    }

    #[test]
    fn sanitize_clears_invalid_current_track() {
        let mut s = Session {
            queue: vec![track("aaaaaaaaaaa"), track("bad"), track("bbbbbbbbbbb")],
            current: Some(1),
            position: 12.0,
        };
        s.sanitize();
        assert_eq!(s.current, None);
        assert_eq!(s.position, 0.0);
        assert_eq!(s.queue.len(), 2);
    }

    #[test]
    fn roundtrip() {
        let ok = Session {
            queue: vec![track("aaaaaaaaaaa")],
            current: Some(0),
            position: 42.5,
        };
        let again: Session = serde_json::from_str(&ok.render().unwrap()).unwrap();
        assert_eq!(
            (again.current, again.position, again.queue),
            (Some(0), 42.5, ok.queue)
        );
    }
}
