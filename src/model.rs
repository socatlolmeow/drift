use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    /* individual credited artists when the source gave them to us */
    #[serde(default)]
    pub artists: Vec<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub duration: Option<u32>,
}

impl Track {
    pub fn url(&self) -> String {
        format!("https://music.youtube.com/watch?v={}", self.id)
    }

    /* Individual artists: the structured list if we have one, else a guess
     * made by splitting the display string (old saves, plain yt-dlp). */
    pub fn artist_names(&self) -> Vec<String> {
        if self.artists.is_empty() {
            split_artists(&self.artist)
        } else {
            self.artists.clone()
        }
    }

    pub fn label(&self) -> String {
        format!("{} — {}", self.title, self.artist)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Collection {
    pub browse_id: String,
    pub title: String,
    pub subtitle: String,
    pub artist: String,
}

/* names that contain a separator and must stay in one piece */
const WHOLE_NAMES: &[&str] = &[
    "Tyler, The Creator",
    "Earth, Wind & Fire",
    "Emerson, Lake & Palmer",
    "Crosby, Stills, Nash & Young",
    "Crosby, Stills & Nash",
    "Simon & Garfunkel",
    "Hall & Oates",
    "Brooks & Dunn",
    "Mumford & Sons",
    "Belle & Sebastian",
    "Sly & The Family Stone",
    "Ike & Tina Turner",
    "Bob Seger & The Silver Bullet Band",
    "Hootie & the Blowfish",
    "Florence & The Machine",
    "Years & Years",
    "Kool & The Gang",
    "Gladys Knight & The Pips",
    "Diana Ross & The Supremes",
    "Smokey Robinson & The Miracles",
    "Martha Reeves & The Vandellas",
    "Huey Lewis & The News",
    "Echo & the Bunnymen",
    "Prince & The Revolution",
    "Bruce Springsteen & The E Street Band",
    "Tom Petty & The Heartbreakers",
];

/* credit separators, matched case-insensitively */
const SEPARATORS: &[&str] = &[" & ", ", ", " feat. ", " feat ", " ft. ", " × "];

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn push_name(out: &mut Vec<String>, raw: &str) {
    let name = raw.trim();
    if !name.is_empty() && !out.iter().any(|o| o == name) {
        out.push(name.to_string());
    }
}

/* Best-effort split of a flattened credit string. Only a fallback: prefer
 * Track::artists, which comes straight from the source's own boundaries. */
pub fn split_artists(credit: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < credit.len() {
        let rest = &credit[i..];
        let at_piece_start = credit[start..i].trim().is_empty();
        if at_piece_start {
            if let Some(name) = WHOLE_NAMES.iter().find(|n| starts_with_ci(rest, n)) {
                i += name.len();
                continue;
            }
        }
        if let Some(sep) = SEPARATORS.iter().find(|sep| starts_with_ci(rest, sep)) {
            push_name(&mut out, &credit[start..i]);
            i += sep.len();
            start = i;
            continue;
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    push_name(&mut out, &credit[start..]);
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
        if !v.is_finite() || v < 0.0 {
            return None;
        }
        total = total * 60.0 + v;
        if !total.is_finite() {
            return None;
        }
    }
    Some(total)
}

pub fn fmt_time(secs: f64) -> String {
    let s = if secs.is_finite() {
        secs.max(0.0) as u64
    } else {
        0
    };
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
        assert_eq!(parse_clock("90"), Some(90.0));
        assert_eq!(parse_clock("abc"), None);
        assert_eq!(parse_clock("NaN"), None);
        assert_eq!(parse_clock("inf"), None);
        assert_eq!(parse_clock("1e308:0"), None);
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
        assert_eq!(split_artists("Tyler, The Creator"), ["Tyler, The Creator"]);
        assert_eq!(
            split_artists("Tyler, The Creator & Kali Uchis"),
            ["Tyler, The Creator", "Kali Uchis"]
        );
        assert_eq!(
            split_artists("Earth, Wind & Fire, Someone"),
            ["Earth, Wind & Fire", "Someone"]
        );
        assert!(split_artists("  ").is_empty());
    }

    #[test]
    fn artist_splitting_edge_cases() {
        assert_eq!(split_artists("hall & oates"), ["hall & oates"]);
        assert_eq!(
            split_artists(" Tyler, The Creator "),
            ["Tyler, The Creator"]
        );
        assert_eq!(split_artists("A Feat. B"), ["A", "B"]);
        assert_eq!(
            split_artists("Drake, Hall & Oates"),
            ["Drake", "Hall & Oates"]
        );
        assert_eq!(split_artists("Beyoncé × Jay-Z"), ["Beyoncé", "Jay-Z"]);
    }

    #[test]
    fn artist_names_prefers_structured_list() {
        let mut t = Track {
            id: "abcdefghijk".into(),
            title: "T".into(),
            artist: "Blood, Sweat & Tears".into(),
            artists: vec!["Blood, Sweat & Tears".into()],
            album: None,
            duration: None,
        };
        assert_eq!(t.artist_names(), ["Blood, Sweat & Tears"]);
        t.artists.clear();
        assert_eq!(t.artist_names(), ["Blood", "Sweat", "Tears"]);
    }

    #[test]
    fn ids() {
        assert!(is_valid_id("dQw4w9WgXcQ"));
        assert!(!is_valid_id("short"));
        assert!(!is_valid_id("dQw4w9WgXc&"));
    }
}
