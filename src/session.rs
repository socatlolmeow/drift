use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{is_valid_id, Track};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub queue: Vec<Track>,
    pub current: Option<usize>,
    pub position: f64,
}

impl Session {
    fn path() -> Result<PathBuf> {
        let dir = dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .ok_or_else(|| anyhow!("cannot determine state directory"))?;
        Ok(dir.join("drift").join("session.json"))
    }

    pub fn exists() -> bool {
        Self::path().is_ok_and(|p| p.exists())
    }

    pub fn load() -> Session {
        let Ok(path) = Self::path() else { return Session::default() };
        let Ok(text) = fs::read_to_string(&path) else { return Session::default() };
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
        self.queue.retain(|t| is_valid_id(&t.id));
        if !self.position.is_finite() || self.position < 0.0 {
            self.position = 0.0;
        }
        match self.current {
            Some(_) if self.queue.is_empty() => self.current = None,
            Some(c) if c >= self.queue.len() => self.current = Some(self.queue.len() - 1),
            _ => {}
        }
    }

    pub fn render(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn write(text: &str) -> Result<()> {
        let path = Self::path()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track { id: id.into(), title: "T".into(), artist: "A".into(), album: None, duration: Some(200) }
    }

    #[test]
    fn roundtrip_and_sanitize() {
        let s = Session {
            queue: vec![track("aaaaaaaaaaa"), track("bad")],
            current: Some(7),
            position: f64::NAN,
        };
        let mut back: Session = serde_json::from_str(&s.render().unwrap().replace("NaN", "null")).unwrap_or_default();
        back.queue = s.queue.clone();
        back.current = s.current;
        back.position = s.position;
        back.sanitize();
        assert_eq!((back.queue.len(), back.current, back.position), (1, Some(0), 0.0));

        let ok = Session { queue: vec![track("aaaaaaaaaaa")], current: Some(0), position: 42.5 };
        let again: Session = serde_json::from_str(&ok.render().unwrap()).unwrap();
        assert_eq!((again.current, again.position, again.queue), (Some(0), 42.5, ok.queue));
    }
}
