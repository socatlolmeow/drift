use std::time::Duration;

use serde_json::Value;

use crate::model::{parse_clock, Track};

const SEARCH: &str = "https://lrclib.net/api/search";
const UA: &str = concat!("drift/", env!("CARGO_PKG_VERSION"));
const LEAD: f64 = 0.25;
const MAX_DRIFT: f64 = 8.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub at: f64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lyrics {
    pub lines: Vec<Line>,
    pub synced: bool,
}

impl Lyrics {
    pub fn active(&self, position: f64) -> Option<usize> {
        if !self.synced {
            return None;
        }
        let n = self.lines.partition_point(|l| l.at <= position + LEAD);
        n.checked_sub(1)
    }

    pub fn progress(&self, i: usize, position: f64) -> f64 {
        let Some(line) = self.lines.get(i) else {
            return 0.0;
        };
        let sing = line.text.chars().count() as f64 * 0.1 + 1.0;
        let span = match self.lines.get(i + 1) {
            Some(next) => (next.at - line.at).min(sing),
            None => sing,
        };
        ((position - line.at) / span.max(0.1)).clamp(0.0, 1.0)
    }
}

pub fn parse_lrc(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some(tail) = rest.strip_prefix('[') {
            let Some((tag, after)) = tail.split_once(']') else {
                break;
            };
            let Some(t) = parse_clock(tag) else {
                break;
            };
            stamps.push(t);
            rest = after;
        }
        let text = rest.trim();
        for at in stamps {
            out.push(Line {
                at,
                text: text.to_string(),
            });
        }
    }
    out.sort_by(|a, b| a.at.total_cmp(&b.at));
    out
}

fn from_entry(entry: &Value) -> Option<Lyrics> {
    let text = |key| entry[key].as_str().filter(|s| !s.trim().is_empty());
    if let Some(s) = text("syncedLyrics") {
        let lines = parse_lrc(s);
        if !lines.is_empty() {
            return Some(Lyrics {
                lines,
                synced: true,
            });
        }
    }
    let plain = text("plainLyrics")?;
    let lines = plain
        .lines()
        .map(|l| Line {
            at: 0.0,
            text: l.trim().to_string(),
        })
        .collect();
    Some(Lyrics {
        lines,
        synced: false,
    })
}

fn pick(entries: &[Value], duration: Option<u32>) -> Option<Lyrics> {
    let drift = |e: &Value| match (duration, e["duration"].as_f64()) {
        (Some(want), Some(got)) => (got - f64::from(want)).abs(),
        _ => 0.0,
    };
    let mut found: Vec<(bool, f64, Lyrics)> = entries
        .iter()
        .filter(|e| drift(e) <= MAX_DRIFT)
        .filter_map(|e| from_entry(e).map(|l| (!l.synced, drift(e), l)))
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    found.into_iter().next().map(|(_, _, l)| l)
}

fn query(params: &[(&str, &str)]) -> Result<Vec<Value>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .user_agent(UA)
        .build();
    let mut req = agent.get(SEARCH);
    for (k, v) in params {
        req = req.query(k, v);
    }
    req.call()
        .map_err(|e| e.to_string())?
        .into_json::<Vec<Value>>()
        .map_err(|e| e.to_string())
}

pub fn fetch(track: &Track) -> Result<Option<Lyrics>, String> {
    let artist = track.artist_names().into_iter().next().unwrap_or_default();
    let exact = query(&[("track_name", &track.title), ("artist_name", &artist)])?;
    if let Some(l) = pick(&exact, track.duration) {
        return Ok(Some(l));
    }
    let loose = query(&[("q", &format!("{} {}", track.title, artist))])?;
    Ok(pick(&loose, track.duration))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_timestamps_and_skips_metadata() {
        let lines =
            parse_lrc("[ar:Someone]\n[00:12.50]first\n[01:02.00][00:30.25]shared\n[00:45.00]");
        let got: Vec<(f64, &str)> = lines.iter().map(|l| (l.at, l.text.as_str())).collect();
        assert_eq!(
            got,
            [
                (12.5, "first"),
                (30.25, "shared"),
                (45.0, ""),
                (62.0, "shared")
            ]
        );
    }

    #[test]
    fn finds_active_line() {
        let l = Lyrics {
            lines: parse_lrc("[00:10.00]a\n[00:20.00]b"),
            synced: true,
        };
        assert_eq!(l.active(5.0), None);
        assert_eq!(l.active(10.0), Some(0));
        assert_eq!(l.active(19.9), Some(1));
        assert_eq!(l.active(99.0), Some(1));
    }

    #[test]
    fn progress_stays_in_range() {
        let l = Lyrics {
            lines: parse_lrc("[00:10.00]abcde\n[00:40.00]b"),
            synced: true,
        };
        assert_eq!(l.progress(0, 9.0), 0.0);
        assert!(l.progress(0, 10.75) > 0.4 && l.progress(0, 10.75) < 0.6);
        assert_eq!(l.progress(0, 30.0), 1.0);
    }

    #[test]
    fn prefers_synced_and_close_duration() {
        let entries = [
            json!({"duration": 200.0, "plainLyrics": "plain"}),
            json!({"duration": 230.0, "syncedLyrics": "[00:01.00]far"}),
            json!({"duration": 203.0, "syncedLyrics": "[00:01.00]near"}),
            json!({"duration": 201.0, "syncedLyrics": "[00:01.00]nearest"}),
        ];
        let l = pick(&entries, Some(200)).unwrap();
        assert!(l.synced);
        assert_eq!(l.lines[0].text, "nearest");
    }

    #[test]
    fn falls_back_to_plain_lyrics() {
        let entries = [json!({"duration": 200.0, "syncedLyrics": null, "plainLyrics": "one\ntwo"})];
        let l = pick(&entries, Some(200)).unwrap();
        assert!(!l.synced);
        assert_eq!(l.lines.len(), 2);
    }
}
