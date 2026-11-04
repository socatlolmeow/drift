use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{is_valid_id, Track};
use crate::store::{state_base, write_atomic};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub track: Track,
    pub play_count: u64,
    /// Unix time in milliseconds.
    pub last_played: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub entries: BTreeMap<String, HistoryEntry>,
    #[serde(skip)]
    base: BTreeMap<String, HistoryEntry>,
}

impl History {
    fn path() -> Result<PathBuf> {
        let dir = state_base().ok_or_else(|| anyhow!("cannot determine state directory"))?;
        Ok(dir.join("drift").join("history.json"))
    }

    fn read(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let mut history: Self = serde_json::from_str(&text)
                    .with_context(|| format!("invalid history file {}", path.display()))?;
                history.entries.retain(|id, entry| {
                    is_valid_id(id) && entry.track.id == *id && entry.play_count > 0
                });
                history.base = history.entries.clone();
                Ok(history)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn load() -> (Self, Option<String>) {
        match Self::path().and_then(|path| Self::read(&path)) {
            Ok(history) => (history, None),
            Err(e) => (Self::default(), Some(format!("cannot load history: {e:#}"))),
        }
    }

    pub fn record(&mut self, track: &Track) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64;
        // Preserve playback order even for starts in the same millisecond.
        let last = self
            .entries
            .values()
            .map(|e| e.last_played)
            .max()
            .unwrap_or(0);
        self.record_at(track, now.max(last.saturating_add(1)));
    }

    fn record_at(&mut self, track: &Track, last_played: u64) {
        if !is_valid_id(&track.id) {
            return;
        }
        let entry = self
            .entries
            .entry(track.id.clone())
            .or_insert(HistoryEntry {
                track: track.clone(),
                play_count: 0,
                last_played,
            });
        entry.track = track.clone();
        entry.play_count = entry.play_count.saturating_add(1);
        entry.last_played = last_played;
    }

    pub fn sorted(&self, most_played: bool) -> Vec<&HistoryEntry> {
        let mut entries: Vec<_> = self.entries.values().collect();
        entries.sort_by(|a, b| {
            let count = if most_played {
                b.play_count.cmp(&a.play_count)
            } else {
                std::cmp::Ordering::Equal
            };
            count
                .then_with(|| b.last_played.cmp(&a.last_played))
                .then_with(|| a.track.id.cmp(&b.track.id))
        });
        entries
    }

    fn merged(&self, mut disk: Self) -> Self {
        for (id, mine) in &self.entries {
            let previous = self.base.get(id).map_or(0, |e| e.play_count);
            let added = mine.play_count.saturating_sub(previous);
            if added == 0 {
                continue;
            }
            let entry = disk.entries.entry(id.clone()).or_insert(HistoryEntry {
                track: mine.track.clone(),
                play_count: 0,
                last_played: mine.last_played,
            });
            entry.play_count = entry.play_count.saturating_add(added);
            if mine.last_played >= entry.last_played {
                entry.track = mine.track.clone();
                entry.last_played = mine.last_played;
            }
        }
        disk
    }

    pub fn save(&mut self) -> Result<()> {
        self.save_to(&Self::path()?)
    }

    fn save_to(&mut self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        // Serialize read/merge/write between running instances on Unix.
        #[cfg(unix)]
        let _lock = {
            use std::os::fd::AsRawFd;
            let file = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(path.with_extension("lock"))?;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            file
        };
        // Do not overwrite an unreadable or corrupt file.
        let mut merged = self.merged(Self::read(path)?);
        write_atomic(path, &serde_json::to_string_pretty(&merged)?)?;
        merged.base = merged.entries.clone();
        *self = merged;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track {
            id: id.into(),
            title: id.into(),
            artist: "Artist".into(),
            artists: Vec::new(),
            album: None,
            duration: Some(180),
        }
    }

    #[test]
    fn repeated_plays_update_counts_metadata_and_order() {
        let mut history = History::default();
        let mut a = track("aaaaaaaaaaa");
        let b = track("bbbbbbbbbbb");
        history.record_at(&a, 1);
        history.record_at(&a, 2);
        history.record_at(&b, 3);
        assert_eq!(history.sorted(false)[0].track, b);
        assert_eq!(history.sorted(true)[0].play_count, 2);
        a.title = "Updated title".into();
        history.record_at(&a, 4);
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.sorted(false)[0].track.title, "Updated title");
        assert_eq!(history.sorted(false)[0].play_count, 3);
    }

    #[test]
    fn ties_use_recency_then_id() {
        let mut history = History::default();
        history.record_at(&track("bbbbbbbbbbb"), 1);
        history.record_at(&track("aaaaaaaaaaa"), 1);
        assert_eq!(history.sorted(true)[0].track.id, "aaaaaaaaaaa");
        history.record_at(&track("ccccccccccc"), 2);
        assert_eq!(history.sorted(true)[0].track.id, "ccccccccccc");
    }

    #[test]
    fn merge_preserves_other_instances_counts() {
        let mut disk = History::default();
        let a = track("aaaaaaaaaaa");
        disk.record_at(&a, 1);
        disk.base = disk.entries.clone();
        let mut mine: History =
            serde_json::from_str(&serde_json::to_string(&disk).unwrap()).unwrap();
        mine.base = mine.entries.clone();
        mine.record_at(&a, 2);
        disk.record_at(&a, 3);
        disk.record_at(&track("bbbbbbbbbbb"), 4);
        let merged = mine.merged(disk);
        assert_eq!(merged.entries[&a.id].play_count, 3);
        assert_eq!(merged.entries[&a.id].last_played, 3);
        assert_eq!(merged.entries.len(), 2);
    }

    #[test]
    fn persistence_and_repeated_save_do_not_double_count() {
        let dir = std::env::temp_dir().join(format!("drift-history-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");
        let mut history = History::default();
        let a = track("aaaaaaaaaaa");
        history.record_at(&a, 42);
        history.save_to(&path).unwrap();
        history.save_to(&path).unwrap();
        let loaded = History::read(&path).unwrap();
        assert_eq!(loaded.entries[&a.id].play_count, 1);
        assert_eq!(loaded.entries[&a.id].last_played, 42);
        fs::write(&path, "invalid JSON").unwrap();
        history.record_at(&a, 43);
        assert!(history.save_to(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "invalid JSON");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_ids_are_not_recorded() {
        let mut history = History::default();
        history.record_at(&track("bad"), 1);
        assert!(history.entries.is_empty());
    }
}
