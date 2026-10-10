use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use serde::Deserialize;

use crate::model::{RepeatMode, Track};
use crate::session::Session;
use crate::store::{write_atomic, Store};

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct Playback {
    pub volume: u8,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub autoplay: bool,
    pub restore_session: bool,
    pub seek_step: f64,
    pub volume_step: f64,
}

impl Default for Playback {
    fn default() -> Self {
        Playback {
            volume: 80,
            shuffle: false,
            repeat: RepeatMode::Off,
            autoplay: false,
            restore_session: true,
            seek_step: 5.0,
            volume_step: 5.0,
        }
    }
}

impl Playback {
    fn sanitize(&mut self) {
        self.volume = self.volume.min(100);
        let ok = |v: f64| v.is_finite() && v > 0.0;
        if !ok(self.seek_step) {
            self.seek_step = 5.0;
        }
        if !ok(self.volume_step) {
            self.volume_step = 5.0;
        }
        self.volume_step = self.volume_step.min(100.0);
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub playback: Playback,
}

impl Config {
    fn path() -> Result<PathBuf> {
        Ok(Store::dir()?.join("config.toml"))
    }

    pub fn render(p: &Playback) -> String {
        format!(
            "\
# Drift settings. Drift only reads this file and never rewrites it, so your
# comments are safe. Restart drift after editing.

[playback]
volume = {volume}
shuffle = {shuffle}
repeat = \"{repeat}\"
autoplay = {autoplay}
restore_session = {restore}
seek_step = {seek:?}
volume_step = {vstep:?}
",
            volume = p.volume,
            shuffle = p.shuffle,
            repeat = p.repeat.label(),
            autoplay = p.autoplay,
            restore = p.restore_session,
            seek = p.seek_step,
            vstep = p.volume_step,
        )
    }

    fn write(text: &str) -> Result<()> {
        write_atomic(&Self::path()?, text)
    }

    pub fn load() -> (Config, Option<String>) {
        let Ok(path) = Self::path() else {
            return (Config::default(), None);
        };
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let _ = Self::write(&Self::render(&Playback::default()));
                return (Config::default(), None);
            }
            Err(e) => {
                return (Config::default(), Some(format!("cannot read {}: {e}", path.display())))
            }
        };
        match toml::from_str::<Config>(&text) {
            Ok(mut c) => {
                c.playback.sanitize();
                (c, None)
            }
            Err(e) => {
                let bak = path.with_extension("toml.bak");
                let _ = fs::rename(&path, &bak);
                (
                    Config::default(),
                    Some(format!(
                        "config.toml was invalid ({}); moved to {}",
                        e.message(),
                        bak.display()
                    )),
                )
            }
        }
    }
}

pub fn migrate_legacy() {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Legacy {
        volume: Option<u8>,
        shuffle: Option<bool>,
        repeat: Option<RepeatMode>,
        autoplay: Option<bool>,
        current: Option<usize>,
        position: f64,
        queue: Vec<Track>,
    }

    let Ok(path) = Config::path() else { return };
    let Ok(text) = fs::read_to_string(&path) else { return };
    let Ok(table) = text.parse::<toml::Table>() else { return };
    if table.contains_key("playback") || !(table.contains_key("queue") || table.contains_key("position")) {
        return;
    }
    let Ok(old) = toml::from_str::<Legacy>(&text) else { return };

    let d = Playback::default();
    let mut p = Playback {
        volume: old.volume.unwrap_or(d.volume),
        shuffle: old.shuffle.unwrap_or(d.shuffle),
        repeat: old.repeat.unwrap_or(d.repeat),
        autoplay: old.autoplay.unwrap_or(d.autoplay),
        ..d
    };
    p.sanitize();
    let session = Session { queue: old.queue, current: old.current, position: old.position };
    if let Ok(s) = session.render() {
        if !Session::exists() {
            let _ = Session::write(&s);
        }
    }
    let _ = Config::write(&Config::render(&p));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_parses_back() {
        let text = Config::render(&Playback::default());
        let c: Config = toml::from_str(&text).unwrap();
        let d = Playback::default();
        assert_eq!(c.playback.volume, d.volume);
        assert_eq!(c.playback.repeat, d.repeat);
        assert_eq!(c.playback.seek_step, 5.0);
        assert!(c.playback.restore_session);
    }

    #[test]
    fn partial_and_empty_files() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.playback.volume, 80);
        let c: Config =
            toml::from_str("[playback]\nvolume = 10\nrepeat = \"one\"\nseek_step = 10").unwrap();
        assert_eq!((c.playback.volume, c.playback.repeat), (10, RepeatMode::One));
        assert_eq!(c.playback.seek_step, 10.0);
        assert!(toml::from_str::<Config>("[playback]\nrepeat = \"sometimes\"").is_err());
    }

    #[test]
    fn sanitize_bad_values() {
        let mut p = Playback { volume: 250, seek_step: -3.0, volume_step: 900.0, ..Playback::default() };
        p.sanitize();
        assert_eq!((p.volume, p.seek_step, p.volume_step), (100, 5.0, 100.0));
    }
}
