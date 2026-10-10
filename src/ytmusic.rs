use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use crate::model::{is_valid_id, parse_clock, Track};

const ENDPOINT: &str = "https://music.youtube.com/youtubei/v1/search?prettyPrint=false";
const SONGS_FILTER: &str = "EgWKAQIIAWoMEA4QChADEAQQCRAF";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";
const FALLBACK_COUNT: usize = 25;

pub fn search(query: &str) -> Result<Vec<Track>, String> {
    match search_innertube(query) {
        Ok(tracks) if !tracks.is_empty() => Ok(tracks),
        Ok(_) => ytdlp_search(query).map_err(|e| format!("{e:#}")),
        Err(primary) => ytdlp_search(query).map_err(|fallback| {
            format!("YouTube Music search failed ({primary:#}); yt-dlp fallback failed ({fallback:#})")
        }),
    }
}

pub fn import(url: &str) -> Result<Vec<Track>, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("import expects a full http(s) URL".into());
    }
    ytdlp_entries(url).map_err(|e| format!("{e:#}"))
}

fn client_version() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1_750_000_000);
    let days = (secs / 86_400) as i64 - 1;
    let (y, m, d) = civil_from_days(days);
    format!("1.{y:04}{m:02}{d:02}.01.00")
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn search_innertube(query: &str) -> Result<Vec<Track>> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(12))
        .user_agent(UA)
        .build();
    let body = json!({
        "context": { "client": {
            "clientName": "WEB_REMIX",
            "clientVersion": client_version(),
            "hl": "en",
            "gl": "US",
        }},
        "query": query,
        "params": SONGS_FILTER,
    });
    let resp = agent
        .post(ENDPOINT)
        .set("Origin", "https://music.youtube.com")
        .set("Referer", "https://music.youtube.com/")
        .set("Cookie", "SOCS=CAI")
        .send_json(body)
        .map_err(|e| anyhow!("request failed: {e}"))?;
    let v: Value = resp.into_json().context("invalid JSON from YouTube Music")?;
    Ok(parse_search_response(&v))
}

fn collect_items<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(map) => {
            if let Some(item) = map.get("musicResponsiveListItemRenderer") {
                out.push(item);
            } else {
                for child in map.values() {
                    collect_items(child, out);
                }
            }
        }
        Value::Array(arr) => arr.iter().for_each(|c| collect_items(c, out)),
        _ => {}
    }
}

fn find_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    match v {
        Value::Object(map) => {
            if let Some(Value::String(s)) = map.get(key) {
                return Some(s);
            }
            map.values().find_map(|c| find_str(c, key))
        }
        Value::Array(arr) => arr.iter().find_map(|c| find_str(c, key)),
        _ => None,
    }
}

fn column_segments(item: &Value, col: usize) -> Vec<String> {
    let runs = item
        .get("flexColumns")
        .and_then(|c| c.get(col))
        .and_then(|c| c.pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs"))
        .and_then(Value::as_array);
    let mut segments = Vec::new();
    let mut cur = String::new();
    for run in runs.into_iter().flatten() {
        let Some(text) = run.get("text").and_then(Value::as_str) else { continue };
        if text.trim() == "•" {
            segments.push(std::mem::take(&mut cur));
        } else {
            cur.push_str(text);
        }
    }
    segments.push(cur);
    segments
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn looks_like_duration(s: &str) -> bool {
    s.contains(':') && s.chars().all(|c| c.is_ascii_digit() || c == ':')
}

fn parse_item(item: &Value) -> Option<Track> {
    let id = find_str(item, "videoId")?;
    if !is_valid_id(id) {
        return None;
    }
    let title = column_segments(item, 0).into_iter().next()?;

    let mut meta = column_segments(item, 1);
    if matches!(meta.first().map(String::as_str), Some("Song" | "Video" | "Episode")) {
        meta.remove(0);
    }
    let duration = match meta.last() {
        Some(last) if looks_like_duration(last) => {
            let d = parse_clock(last).map(|s| s as u32);
            meta.pop();
            d
        }
        _ => None,
    };
    let artist = meta.first().cloned().unwrap_or_else(|| "Unknown artist".into());
    let album = meta
        .get(1)
        .filter(|a| !a.contains(" views") && !a.contains(" plays"))
        .cloned();

    Some(Track {
        id: id.to_string(),
        title,
        artist,
        album,
        duration,
    })
}

pub fn parse_search_response(v: &Value) -> Vec<Track> {
    let mut items = Vec::new();
    collect_items(v, &mut items);
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter_map(parse_item)
        .filter(|t| seen.insert(t.id.clone()))
        .collect()
}

fn ytdlp_search(query: &str) -> Result<Vec<Track>> {
    ytdlp_entries(&format!("ytsearch{FALLBACK_COUNT}:{query}"))
}

fn ytdlp_entries(target: &str) -> Result<Vec<Track>> {
    let out = Command::new("yt-dlp")
        .args([
            "--flat-playlist",
            "--dump-json",
            "--no-warnings",
            "--ignore-errors",
            "--socket-timeout",
            "15",
        ])
        .arg(target)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| anyhow!("could not run yt-dlp ({e}); is it installed and on PATH?"))?;

    let tracks = parse_ytdlp_lines(&String::from_utf8_lossy(&out.stdout));
    if tracks.is_empty() && !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let last = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("unknown error");
        bail!("yt-dlp failed: {}", last.trim());
    }
    Ok(tracks)
}

pub fn parse_ytdlp_lines(text: &str) -> Vec<Track> {
    let mut seen = std::collections::HashSet::new();
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|v| {
            let id = v.get("id")?.as_str()?;
            if !is_valid_id(id) {
                return None;
            }
            let title = v.get("title")?.as_str()?.trim().to_string();
            let artist = ["artist", "channel", "uploader"]
                .iter()
                .find_map(|k| v.get(*k).and_then(Value::as_str))
                .unwrap_or("Unknown artist")
                .trim_end_matches(" - Topic")
                .to_string();
            let duration = v.get("duration").and_then(Value::as_f64).map(|d| d as u32);
            Some(Track {
                id: id.to_string(),
                title,
                artist,
                album: None,
                duration,
            })
        })
        .filter(|t| seen.insert(t.id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> Value {
        json!({"text": {"runs": [{"text": text}]}})
    }

    #[test]
    fn parses_song_item() {
        let resp = json!({"contents": {"shelf": [{"musicResponsiveListItemRenderer": {
            "playlistItemData": {"videoId": "dQw4w9WgXcQ"},
            "flexColumns": [
                {"musicResponsiveListItemFlexColumnRenderer": run("Never Gonna Give You Up")},
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                    {"text": "Rick Astley"}, {"text": " • "},
                    {"text": "Whenever You Need Somebody"}, {"text": " • "},
                    {"text": "3:33"}
                ]}}}
            ]
        }}]}});
        let tracks = parse_search_response(&resp);
        assert_eq!(tracks.len(), 1);
        let t = &tracks[0];
        assert_eq!(t.id, "dQw4w9WgXcQ");
        assert_eq!(t.title, "Never Gonna Give You Up");
        assert_eq!(t.artist, "Rick Astley");
        assert_eq!(t.album.as_deref(), Some("Whenever You Need Somebody"));
        assert_eq!(t.duration, Some(213));
    }

    #[test]
    fn parses_multi_artist_and_type_prefix() {
        let resp = json!([{"musicResponsiveListItemRenderer": {
            "overlay": {"watchEndpoint": {"videoId": "abcdefghijk"}},
            "flexColumns": [
                {"musicResponsiveListItemFlexColumnRenderer": run("Song")},
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                    {"text": "Video"}, {"text": " • "},
                    {"text": "A"}, {"text": " & "}, {"text": "B"}, {"text": " • "},
                    {"text": "1.2M views"}, {"text": " • "}, {"text": "4:01"}
                ]}}}
            ]
        }}]);
        let t = &parse_search_response(&resp)[0];
        assert_eq!(t.artist, "A & B");
        assert_eq!(t.album, None);
        assert_eq!(t.duration, Some(241));
    }

    #[test]
    fn skips_items_without_video_id() {
        let resp = json!([{"musicResponsiveListItemRenderer": {
            "navigationEndpoint": {"browseId": "UC123"},
            "flexColumns": [{"musicResponsiveListItemFlexColumnRenderer": run("Artist")}]
        }}]);
        assert!(parse_search_response(&resp).is_empty());
    }

    #[test]
    fn parses_ytdlp_output() {
        let text = r#"{"id":"dQw4w9WgXcQ","title":"Song","channel":"Rick - Topic","duration":213.0}
not json
{"id":"UCxxxxxxxxxxxxxxxxxxxxxx","title":"A channel"}
{"id":"dQw4w9WgXcQ","title":"dup"}"#;
        let t = parse_ytdlp_lines(text);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].artist, "Rick");
        assert_eq!(t[0].duration, Some(213));
    }

    #[test]
    fn date_math() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }
}
