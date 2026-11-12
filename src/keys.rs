use std::collections::HashMap;
use std::fmt;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    MoveDown,
    MoveUp,
    HalfPageDown,
    HalfPageUp,
    PageDown,
    PageUp,
    GoTop,
    GoBottom,
    Menu,
    Delete,
    NextView,
    PrevView,
    ViewResults,
    ViewQueue,
    ViewPlaylists,
    ViewRecent,
    ViewMostPlayed,
    ToggleQueueSidebar,
    Activate,
    Pause,
    Next,
    Prev,
    SeekBack,
    SeekForward,
    VolumeUp,
    VolumeDown,
    Add,
    AddAll,
    PlayAll,
    MoveItemDown,
    MoveItemUp,
    Shuffle,
    Repeat,
    Search,
    Command,
    LyricsPanel,
    Lyrics,
    GoArtist,
    GoSimilar,
    GoAlbum,
    SaveAlbum,
    Download,
    DownloadSelected,
    DownloadAll,
    Help,
    Quit,
    Back,
}

const DEFAULTS: &[(&str, Action, &[&str])] = &[
    ("move_down", Action::MoveDown, &["j", "down"]),
    ("move_up", Action::MoveUp, &["k", "up"]),
    ("half_page_down", Action::HalfPageDown, &["ctrl+d"]),
    ("half_page_up", Action::HalfPageUp, &["ctrl+u"]),
    ("page_down", Action::PageDown, &["ctrl+f", "pagedown"]),
    ("page_up", Action::PageUp, &["ctrl+b", "pageup"]),
    ("go_top", Action::GoTop, &["gg", "home"]),
    ("go_bottom", Action::GoBottom, &["ge", "end"]),
    ("menu", Action::Menu, &["G", "menu"]),
    ("delete", Action::Delete, &["x", "delete", "dd"]),
    ("next_view", Action::NextView, &["tab"]),
    ("prev_view", Action::PrevView, &["backtab"]),
    ("view_results", Action::ViewResults, &["gr"]),
    ("view_queue", Action::ViewQueue, &["gq"]),
    ("view_playlists", Action::ViewPlaylists, &["gp"]),
    ("view_recent", Action::ViewRecent, &["gh"]),
    ("view_most_played", Action::ViewMostPlayed, &["gm"]),
    ("toggle_queue_sidebar", Action::ToggleQueueSidebar, &["Q"]),
    ("activate", Action::Activate, &["enter"]),
    ("pause", Action::Pause, &["space"]),
    ("next", Action::Next, &["n", ">"]),
    ("prev", Action::Prev, &["N", "<"]),
    ("seek_back", Action::SeekBack, &["h", "left"]),
    ("seek_forward", Action::SeekForward, &["l", "right"]),
    ("volume_up", Action::VolumeUp, &["+", "="]),
    ("volume_down", Action::VolumeDown, &["-", "_"]),
    ("add", Action::Add, &["a"]),
    ("add_all", Action::AddAll, &["A"]),
    ("play_all", Action::PlayAll, &["P"]),
    ("move_item_down", Action::MoveItemDown, &["J"]),
    ("move_item_up", Action::MoveItemUp, &["K"]),
    ("shuffle", Action::Shuffle, &["s"]),
    ("repeat", Action::Repeat, &["r"]),
    ("search", Action::Search, &["/"]),
    ("command", Action::Command, &[":"]),
    ("lyrics_panel", Action::LyricsPanel, &["L"]),
    ("lyrics", Action::Lyrics, &["F"]),
    ("go_artist", Action::GoArtist, &["ga"]),
    ("go_similar", Action::GoSimilar, &["gs"]),
    ("go_album", Action::GoAlbum, &["gb"]),
    ("save_album", Action::SaveAlbum, &["gB"]),
    ("download", Action::Download, &["D"]),
    ("download_selected", Action::DownloadSelected, &["gd"]),
    ("download_all", Action::DownloadAll, &["gD"]),
    ("help", Action::Help, &["?"]),
    ("quit", Action::Quit, &["q"]),
    ("back", Action::Back, &["backspace", "esc"]),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    code: KeyCode,
    ctrl: bool,
}

impl Key {
    pub fn digit(&self) -> Option<usize> {
        match self.code {
            KeyCode::Char(c @ '0'..='9') if !self.ctrl => Some(c as usize - '0' as usize),
            _ => None,
        }
    }
}

impl From<KeyEvent> for Key {
    fn from(e: KeyEvent) -> Key {
        Key {
            code: e.code,
            ctrl: e.modifiers.contains(KeyModifiers::CONTROL)
                && !e.modifiers.contains(KeyModifiers::ALT),
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.ctrl {
            f.write_str("^")?;
        }
        match self.code {
            KeyCode::Char(c) => write!(f, "{c}"),
            other => write!(f, "{other}"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Keys {
    One(String),
    Many(Vec<String>),
}

impl Keys {
    fn as_slice(&self) -> &[String] {
        match self {
            Keys::One(s) => std::slice::from_ref(s),
            Keys::Many(v) => v,
        }
    }
}

pub enum Lookup {
    Run(Action),
    Pending,
    Nothing,
}

#[derive(Debug, Clone)]
pub struct Keymap(Vec<(Vec<Key>, Action)>);

impl Default for Keymap {
    fn default() -> Keymap {
        Keymap::build(&HashMap::new()).expect("default keys are valid")
    }
}

impl Keymap {
    pub fn build(custom: &HashMap<String, Keys>) -> Result<Keymap, String> {
        if let Some(name) = custom
            .keys()
            .find(|n| !DEFAULTS.iter().any(|d| d.0 == n.as_str()))
        {
            return Err(format!("unknown action \"{name}\""));
        }

        let mut map: Vec<(Vec<Key>, Action)> = Vec::new();
        for (name, action, _) in DEFAULTS {
            let Some(keys) = custom.get(*name) else {
                continue;
            };
            for text in keys.as_slice() {
                let seq = parse_sequence(text).map_err(|e| format!("{name}: {e}"))?;
                if let Some((_, other)) = map.iter().find(|(s, _)| *s == seq) {
                    if other != action {
                        return Err(format!("\"{text}\" is bound to two actions"));
                    }
                    continue;
                }
                map.push((seq, *action));
            }
        }

        for (name, action, keys) in DEFAULTS {
            if custom.contains_key(*name) {
                continue;
            }
            for text in *keys {
                let seq = parse_sequence(text).expect("default keys are valid");
                if !map.iter().any(|(s, _)| *s == seq) {
                    map.push((seq, *action));
                }
            }
        }
        for (i, (short, _)) in map.iter().enumerate() {
            if map.iter().skip(i + 1).any(|(long, _)| {
                (short.len() < long.len() && long.starts_with(short))
                    || (long.len() < short.len() && short.starts_with(long))
            }) {
                return Err("key sequences cannot be prefixes of one another".into());
            }
        }
        Ok(Keymap(map))
    }

    pub fn lookup(&self, seq: &[Key]) -> Lookup {
        if let Some((_, action)) = self.0.iter().find(|(s, _)| s == seq) {
            Lookup::Run(*action)
        } else if self.0.iter().any(|(s, _)| s.starts_with(seq)) {
            Lookup::Pending
        } else {
            Lookup::Nothing
        }
    }
}

fn parse_sequence(text: &str) -> Result<Vec<Key>, String> {
    let mut keys = Vec::new();
    for token in text.split_whitespace() {
        match parse_key(token) {
            Some(key) => keys.push(key),
            None if !token.contains('+') => keys.extend(token.chars().map(|c| Key {
                code: KeyCode::Char(c),
                ctrl: false,
            })),
            None => return Err(format!("unknown key \"{token}\"")),
        }
    }
    if keys.is_empty() {
        return Err("empty key".into());
    }
    Ok(keys)
}

fn parse_key(token: &str) -> Option<Key> {
    let (ctrl, name) = match token.get(..5) {
        Some(p) if p.eq_ignore_ascii_case("ctrl+") => (true, &token[5..]),
        _ => (false, token),
    };
    let mut chars = name.chars();
    let code = match (chars.next()?, chars.next()) {
        (c, None) if ctrl => KeyCode::Char(c.to_ascii_lowercase()),
        (c, None) => KeyCode::Char(c),
        _ => match name.to_ascii_lowercase().as_str() {
            "space" => KeyCode::Char(' '),
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" => KeyCode::Insert,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "menu" => KeyCode::Menu,
            f => match f.strip_prefix('f')?.parse::<u8>() {
                Ok(n @ 1..=12) => KeyCode::F(n),
                _ => return None,
            },
        },
    };
    Some(Key { code, ctrl })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(entries: &[(&str, &[&str])]) -> HashMap<String, Keys> {
        entries
            .iter()
            .map(|(name, keys)| {
                let keys = keys.iter().map(|k| k.to_string()).collect();
                (name.to_string(), Keys::Many(keys))
            })
            .collect()
    }

    fn seq(text: &str) -> Vec<Key> {
        parse_sequence(text).unwrap()
    }

    #[test]
    fn download_bindings() {
        let map = Keymap::default();
        for (keys, expected) in [
            ("D", Action::Download),
            ("gd", Action::DownloadSelected),
            ("gD", Action::DownloadAll),
            ("dd", Action::Delete),
            ("ctrl+d", Action::HalfPageDown),
        ] {
            assert!(matches!(map.lookup(&seq(keys)), Lookup::Run(action) if action == expected));
        }
        let map = Keymap::build(&custom(&[("download", &["ctrl+s"])])).unwrap();
        assert!(matches!(
            map.lookup(&seq("ctrl+s")),
            Lookup::Run(Action::Download)
        ));
        assert!(matches!(map.lookup(&seq("D")), Lookup::Nothing));
    }

    fn run(map: &Keymap, text: &str) -> Option<Action> {
        match map.lookup(&seq(text)) {
            Lookup::Run(a) => Some(a),
            _ => None,
        }
    }

    #[test]
    fn defaults_are_vim() {
        let map = Keymap::default();
        assert_eq!(run(&map, "j"), Some(Action::MoveDown));
        assert_eq!(run(&map, "down"), Some(Action::MoveDown));
        assert_eq!(run(&map, "gg"), Some(Action::GoTop));
        assert_eq!(run(&map, "dd"), Some(Action::Delete));
        assert_eq!(run(&map, "ctrl+d"), Some(Action::HalfPageDown));
        assert_eq!(run(&map, "G"), Some(Action::Menu));
        assert_eq!(run(&map, "Q"), Some(Action::ToggleQueueSidebar));
        assert_eq!(run(&map, "space"), Some(Action::Pause));
        assert!(matches!(map.lookup(&seq("g")), Lookup::Pending));
        assert!(matches!(map.lookup(&seq("gz")), Lookup::Nothing));
    }

    #[test]
    fn parsing() {
        assert_eq!(seq("ctrl+D"), seq("ctrl+d"));
        assert_eq!(seq("gg"), seq("g g"));
        assert_eq!(seq("+").len(), 1);
        assert_eq!(seq("ctrl+x g").len(), 2);
        assert_eq!(seq("f5")[0].code, KeyCode::F(5));
        assert!(parse_sequence("").is_err());
        assert!(parse_sequence("ctrl+nope").is_err());
        assert!(parse_key("f13").is_none());
    }

    #[test]
    fn custom_replaces_default_keys() {
        let map = Keymap::build(&custom(&[("quit", &["Q"]), ("move_down", &["e"])])).unwrap();
        assert_eq!(run(&map, "Q"), Some(Action::Quit));
        assert_eq!(run(&map, "q"), None);
        assert_eq!(run(&map, "e"), Some(Action::MoveDown));
        assert_eq!(run(&map, "j"), None);
        assert_eq!(run(&map, "k"), Some(Action::MoveUp));
    }

    #[test]
    fn queue_sidebar_can_be_rebound_or_unbound() {
        let map = Keymap::build(&custom(&[("toggle_queue_sidebar", &["ctrl+q"])])).unwrap();
        assert_eq!(run(&map, "ctrl+q"), Some(Action::ToggleQueueSidebar));
        assert_eq!(run(&map, "Q"), None);

        let map = Keymap::build(&custom(&[("toggle_queue_sidebar", &[])])).unwrap();
        assert_eq!(run(&map, "Q"), None);
    }

    #[test]
    fn custom_wins_over_other_defaults() {
        let map = Keymap::build(&custom(&[("quit", &["x"])])).unwrap();
        assert_eq!(run(&map, "x"), Some(Action::Quit));
        assert_eq!(run(&map, "dd"), Some(Action::Delete));
    }

    #[test]
    fn single_string_and_disable() {
        let toml = "quit = \"Q\"\nshuffle = []";
        let keys: HashMap<String, Keys> = toml::from_str(toml).unwrap();
        let map = Keymap::build(&keys).unwrap();
        assert_eq!(run(&map, "Q"), Some(Action::Quit));
        assert_eq!(run(&map, "s"), None);
    }

    #[test]
    fn bad_config_is_rejected() {
        assert!(Keymap::build(&custom(&[("quit", &["g"]), ("add", &["gg"])])).is_err());
        assert!(Keymap::build(&custom(&[("fly", &["f"])])).is_err());
        assert!(Keymap::build(&custom(&[("quit", &["ctrl+nope"])])).is_err());
        assert!(Keymap::build(&custom(&[("quit", &["z"]), ("add", &["z"])])).is_err());
        assert!(Keymap::build(&custom(&[("quit", &["z", "z"])])).is_ok());
    }
}
