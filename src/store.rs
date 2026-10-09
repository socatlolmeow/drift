use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::Track;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub playlists: BTreeMap<String, Vec<Track>>,
}

impl Store {
    pub fn path() -> Result<PathBuf> {
        let dir = dirs::config_dir().ok_or_else(|| anyhow!("cannot determine config directory"))?;
        let new_dir = dir.join("drift");
        let old_dir = dir.join("ytm-tui");
        if !new_dir.exists() && old_dir.exists() {
            let _ = fs::rename(&old_dir, &new_dir);
        }
        Ok(new_dir.join("playlists.json"))
    }

    pub fn load() -> (Store, Option<String>) {
        let path = match Self::path() {
            Ok(p) => p,
            Err(e) => return (Store::default(), Some(format!("playlists disabled: {e}"))),
        };
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Store::default(), None),
            Err(e) => {
                return (
                    Store::default(),
                    Some(format!("cannot read {}: {e}", path.display())),
                )
            }
        };
        match serde_json::from_str::<Store>(&text) {
            Ok(s) => (s, None),
            Err(e) => {
                let bak = path.with_extension("json.bak");
                let _ = fs::rename(&path, &bak);
                (
                    Store::default(),
                    Some(format!(
                        "playlists file was corrupt ({e}); moved to {}",
                        bak.display()
                    )),
                )
            }
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    pub fn resolve(&self, name: &str) -> Option<String> {
        if self.playlists.contains_key(name) {
            return Some(name.to_string());
        }
        let lower = name.to_lowercase();
        let ci: Vec<&String> = self
            .playlists
            .keys()
            .filter(|k| k.to_lowercase() == lower)
            .collect();
        if ci.len() == 1 {
            return Some(ci[0].clone());
        }
        let pre: Vec<&String> = self
            .playlists
            .keys()
            .filter(|k| k.to_lowercase().starts_with(&lower))
            .collect();
        if pre.len() == 1 {
            return Some(pre[0].clone());
        }
        None
    }
}
