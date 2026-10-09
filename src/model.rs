use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub duration: Option<u32>,
}

impl Track {
    pub fn url(&self) -> String {
        format!("https://music.youtube.com/watch?v={}", self.id)
    }

    pub fn label(&self) -> String {
        format!("{} — {}", self.title, self.artist)
    }
}

pub fn split_artists(credit: &str) -> Vec<String> {
    const MARK: char = '\u{1}';
    let mut s = credit.to_string();
    for sep in [" & ", ", ", " feat. ", " Feat. ", " feat ", " ft. ", " Ft. ", " × "] {
        s = s.replace(sep, &MARK.to_string());
    }
    let mut out: Vec<String> = Vec::new();
    for part in s.split(MARK) {
        let name = part.trim();
        if !name.is_empty() && !out.iter().any(|o| o == name) {
            out.push(name.to_string());
        }
    }
    out
}

pub fn is_valid_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn parse_clock(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches('s');
    if s.is_empty() {
        return None;
    }
    let mut total = 0.0;
    for part in s.split(':') {
        let v: f64 = part.trim().parse().ok()?;
        if v < 0.0 {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

pub fn fmt_time(secs: f64) -> String {
    let s = if secs.is_finite() { secs.max(0.0) as u64 } else { 0 };
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepeatMode {
    Off,
    All,
    One,
}

impl RepeatMode {
    pub fn cycle(self) -> Self {
        match self {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RepeatMode::Off => "off",
            RepeatMode::All => "all",
            RepeatMode::One => "one",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_parsing() {
        assert_eq!(parse_clock("3:45"), Some(225.0));
        assert_eq!(parse_clock("1:02:03"), Some(3723.0));
        assert_eq!(parse_clock("90"), Some(5400.0 / 60.0));
        assert_eq!(parse_clock("abc"), None);
        assert_eq!(fmt_time(225.0), "3:45");
        assert_eq!(fmt_time(3723.0), "1:02:03");
    }

    #[test]
    fn artist_splitting() {
        assert_eq!(split_artists("Daft Punk"), ["Daft Punk"]);
        assert_eq!(split_artists("A & B"), ["A", "B"]);
        assert_eq!(split_artists("A, B & C"), ["A", "B", "C"]);
        assert_eq!(split_artists("A feat. B"), ["A", "B"]);
        assert_eq!(split_artists("A, A"), ["A"]);
        assert!(split_artists("  ").is_empty());
    }

    #[test]
    fn ids() {
        assert!(is_valid_id("dQw4w9WgXcQ"));
        assert!(!is_valid_id("short"));
        assert!(!is_valid_id("dQw4w9WgXc&"));
    }
}
