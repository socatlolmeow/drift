use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::Track;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub playlists: BTreeMap<String, Vec<Track>>,
    /* what the file looked like when we last read or wrote it */
    #[serde(skip)]
    base: BTreeMap<String, Vec<Track>>,
}

/* keep whatever another drift wrote, but apply the playlists this one changed */
fn merge(
    base: &BTreeMap<String, Vec<Track>>,
    mine: &BTreeMap<String, Vec<Track>>,
    disk: BTreeMap<String, Vec<Track>>,
) -> BTreeMap<String, Vec<Track>> {
    let mut out = disk;
    for name in base.keys().chain(mine.keys()) {
        if base.get(name) == mine.get(name) {
            continue;
        }
        match mine.get(name) {
            Some(list) => {
                out.insert(name.clone(), list.clone());
            }
            None => {
                out.remove(name);
            }
        }
    }
    out
}

pub fn write_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    // On Windows the rename can fail briefly while an antivirus scanner or
    // indexer holds the destination open, so retry a few times.
    let mut last = None;
    for attempt in 0..6 {
        match fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                if attempt < 5 {
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
            }
        }
    }
    let _ = fs::remove_file(&tmp);
    Err(last.expect("at least one attempt"))
        .with_context(|| format!("replacing {}", path.display()))
}

/// Base directory for settings and playlists (`DRIFT_CONFIG_DIR` overrides it).
pub fn config_base() -> Option<PathBuf> {
    std::env::var_os("DRIFT_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(dirs::config_dir)
}

/// Base directory for the saved session (`DRIFT_STATE_DIR` overrides it).
/// `dirs::state_dir()` is `None` on Windows and macOS, so fall back to local data.
pub fn state_base() -> Option<PathBuf> {
    std::env::var_os("DRIFT_STATE_DIR")
        .map(PathBuf::from)
        .or_else(dirs::state_dir)
        .or_else(dirs::data_local_dir)
}

impl Store {
    pub fn dir() -> Result<PathBuf> {
        let dir = config_base().ok_or_else(|| anyhow!("cannot determine config directory"))?;
        let new_dir = dir.join("drift");
        let old_dir = dir.join("ytm-tui");
        if !new_dir.exists() && old_dir.exists() {
            let _ = fs::rename(&old_dir, &new_dir);
        }
        Ok(new_dir)
    }

    fn path() -> Result<PathBuf> {
        Ok(Self::dir()?.join("playlists.json"))
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
            Ok(mut s) => {
                s.base = s.playlists.clone();
                (s, None)
            }
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

    fn read_disk(path: &Path) -> Result<Option<BTreeMap<String, Vec<Track>>>> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(Some(
                serde_json::from_str::<Store>(&text)
                    .with_context(|| format!("invalid playlist file {}", path.display()))?
                    .playlists,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&mut self) -> Result<()> {
        self.save_to(&Self::path()?)
    }

    fn save_to(&mut self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        #[cfg(unix)]
        let _lock = lock_file(&path.with_extension("lock"))?;
        if let Some(disk) = Self::read_disk(path)? {
            self.playlists = merge(&self.base, &self.playlists, disk);
        }
        write_atomic(path, &serde_json::to_string_pretty(&*self)?)?;
        self.base = self.playlists.clone();
        Ok(())
    }

    pub fn resolve_exact(&self, name: &str) -> Option<String> {
        if self.playlists.contains_key(name) {
            return Some(name.to_string());
        }
        let lower = name.to_lowercase();
        let mut found = self.playlists.keys().filter(|k| k.to_lowercase() == lower);
        match (found.next(), found.next()) {
            (Some(k), None) => Some(k.clone()),
            _ => None,
        }
    }

    pub fn resolve(&self, name: &str) -> Option<String> {
        self.resolve_exact(name).or_else(|| {
            let lower = name.to_lowercase();
            let mut found = self
                .playlists
                .keys()
                .filter(|k| k.to_lowercase().starts_with(&lower));
            match (found.next(), found.next()) {
                (Some(k), None) => Some(k.clone()),
                _ => None,
            }
        })
    }
}

#[cfg(unix)]
fn lock_file(path: &Path) -> Result<fs::File> {
    use std::os::fd::AsRawFd;

    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_other_instances_changes() {
        let t = |id: &str| Track {
            id: id.into(),
            title: "T".into(),
            artist: "A".into(),
            artists: Vec::new(),
            album: None,
            duration: None,
        };
        let base: BTreeMap<String, Vec<Track>> = BTreeMap::from([
            ("a".to_string(), vec![t("aaaaaaaaaaa")]),
            ("b".to_string(), vec![]),
        ]);
        let mut mine = base.clone();
        mine.insert("c".into(), vec![t("ccccccccccc")]);
        mine.remove("b");
        let mut disk = base.clone();
        disk.insert("d".into(), vec![t("ddddddddddd")]);
        disk.get_mut("a").unwrap().push(t("eeeeeeeeeee"));

        let out = merge(&base, &mine, disk);
        let names: Vec<&str> = out.keys().map(String::as_str).collect();
        assert_eq!(names, ["a", "c", "d"]);
        assert_eq!(out["a"].len(), 2);
    }

    #[test]
    fn exact_ignores_prefixes() {
        let mut s = Store::default();
        s.playlists.insert("rock-classics".into(), Vec::new());
        s.playlists.insert("Jazz".into(), Vec::new());
        assert_eq!(s.resolve_exact("rock"), None);
        assert_eq!(s.resolve("rock").as_deref(), Some("rock-classics"));
        assert_eq!(s.resolve_exact("jazz").as_deref(), Some("Jazz"));
    }

    #[test]
    fn save_does_not_overwrite_corrupt_disk_file() {
        let dir = std::env::temp_dir().join(format!("drift-store-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("playlists.json");
        fs::write(&path, "invalid JSON").unwrap();

        let mut store = Store::default();
        store.playlists.insert("mix".into(), Vec::new());
        assert!(store.save_to(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "invalid JSON");

        fs::remove_dir_all(dir).unwrap();
    }
}
