use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use crate::model::{is_valid_id, parse_clock, Collection, Track};

const ENDPOINT: &str = "https://music.youtube.com/youtubei/v1/search?prettyPrint=false";
const BROWSE_ENDPOINT: &str = "https://music.youtube.com/youtubei/v1/browse?prettyPrint=false";
const NEXT_ENDPOINT: &str = "https://music.youtube.com/youtubei/v1/next?prettyPrint=false";
const ALBUMS_FILTER: &str = "EgWKAQIYAWoMEA4QChADEAQQCRAF";
const PLAYLISTS_FILTER: &str = "Eg-KAQwIABAAGAAgACgBMABqChAEEAMQCRAFEAo%3D";
const RADIO_TARGET: usize = 50;
const RADIO_PAGES: usize = 4;
const SONGS_FILTER: &str = "EgWKAQIIAWoMEA4QChADEAQQCRAF";
const ARTISTS_FILTER: &str = "EgWKAQIgAWoMEA4QChADEAQQCRAF";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";
const FALLBACK_COUNT: usize = 25;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Album,
    Playlist,
}

pub struct Page {
    pub tracks: Vec<Track>,
    pub next: Option<String>,
}

impl From<Vec<Track>> for Page {
    fn from(tracks: Vec<Track>) -> Self {
        Page { tracks, next: None }
    }
}

pub fn search(query: &str) -> Result<Vec<Track>, String> {
    search_page(query).map(|p| p.tracks)
}

pub fn search_page(query: &str) -> Result<Page, String> {
    match search_innertube(query) {
        Ok(page) if !page.tracks.is_empty() => Ok(page),
        Ok(_) => ytdlp_search(query)
            .map(Page::from)
            .map_err(|e| format!("{e:#}")),
        Err(primary) => ytdlp_search(query).map(Page::from).map_err(|fallback| {
            format!(
                "YouTube Music search failed ({primary:#}); yt-dlp fallback failed ({fallback:#})"
            )
        }),
    }
}

pub fn search_more(token: &str) -> Result<Page, String> {
    let url = format!("{ENDPOINT}&ctoken={token}&continuation={token}");
    post(&url, json!({}))
        .map(|v| page_from(&v))
        .map_err(|e| format!("{e:#}"))
}

pub fn search_collections(query: &str, kind: Kind) -> Result<Vec<Collection>, String> {
    let params = match kind {
        Kind::Album => ALBUMS_FILTER,
        Kind::Playlist => PLAYLISTS_FILTER,
    };
    post(ENDPOINT, json!({ "query": query, "params": params }))
        .map(|v| parse_collections(&v, kind))
        .map_err(|e| format!("{e:#}"))
}

/* browse_id: MPREb_... for an album, VL... for a playlist */
pub fn browse_tracks(browse_id: &str, album: &str, artist: &str) -> Result<Vec<Track>, String> {
    let playlist = browse_id.starts_with("VL");
    let v =
        post(BROWSE_ENDPOINT, json!({ "browseId": browse_id })).map_err(|e| format!("{e:#}"))?;
    let mut tracks = parse_browse(&v, playlist);
    let mut next = if playlist {
        find_continuation(&v)
    } else {
        None
    };
    let mut pages = 0;
    while let Some(tok) = next.take() {
        if pages >= 5 || tracks.len() >= 300 {
            break;
        }
        pages += 1;
        let url = format!("{BROWSE_ENDPOINT}&ctoken={tok}&continuation={tok}");
        let Ok(v) = post(&url, json!({})) else { break };
        let more = parse_browse(&v, true);
        if more.is_empty() {
            break;
        }
        let mut known: std::collections::HashSet<String> =
            tracks.iter().map(|t| t.id.clone()).collect();
        tracks.extend(more.into_iter().filter(|t| known.insert(t.id.clone())));
        next = find_continuation(&v);
    }
    if tracks.is_empty() {
        return Err("nothing playable found".into());
    }
    if !playlist {
        for t in &mut tracks {
            if !album.is_empty() && t.album.is_none() {
                t.album = Some(album.to_string());
            }
            if !artist.is_empty() && t.artist == "Unknown artist" {
                t.artist = artist.to_string();
            }
        }
    }
    Ok(tracks)
}

pub fn radio(seed: &str) -> Result<Vec<Track>, String> {
    fetch_radio(seed).map_err(|e| format!("{e:#}"))
}

fn fetch_radio(seed: &str) -> Result<Vec<Track>> {
    let body = json!({
        "videoId": seed,
        "playlistId": format!("RDAMVM{seed}"),
        "params": "wAEB",
        "isAudioOnly": true,
        "enablePersistentPlaylistPanel": true,
        "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
    });
    let mut url = NEXT_ENDPOINT.to_string();
    let mut tracks: Vec<Track> = Vec::new();
    for _ in 0..RADIO_PAGES {
        let v = post(&url, body.clone())?;
        let before = tracks.len();
        for t in parse_radio_response(&v) {
            if !tracks.iter().any(|x| x.id == t.id) {
                tracks.push(t);
            }
        }
        match find_continuation(&v) {
            Some(tok) if tracks.len() < RADIO_TARGET && tracks.len() > before => {
                url = format!("{NEXT_ENDPOINT}&ctoken={tok}&continuation={tok}");
            }
            _ => break,
        }
    }
    if tracks.is_empty() {
        bail!("no similar songs found");
    }
    Ok(tracks)
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

fn search_innertube(query: &str) -> Result<Page> {
    let v = post(ENDPOINT, json!({ "query": query, "params": SONGS_FILTER }))?;
    Ok(page_from(&v))
}

fn page_from(v: &Value) -> Page {
    Page {
        tracks: parse_search_response(v),
        next: find_continuation(v),
    }
}

fn find_continuation(v: &Value) -> Option<String> {
    match v {
        Value::Object(map) => {
            let token = map
                .get("nextContinuationData")
                .or_else(|| map.get("nextRadioContinuationData"))
                .and_then(|c| c.get("continuation"))
                .or_else(|| map.get("continuationCommand").and_then(|c| c.get("token")))
                .and_then(Value::as_str);
            match token {
                Some(t) => Some(t.to_string()),
                None => map.values().find_map(find_continuation),
            }
        }
        Value::Array(arr) => arr.iter().find_map(find_continuation),
        _ => None,
    }
}

/* one shared agent so paging and radio reuse the same connection */
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(12))
            .user_agent(UA)
            .build()
    })
}

fn post(url: &str, mut body: Value) -> Result<Value> {
    body["context"] = json!({ "client": {
        "clientName": "WEB_REMIX",
        "clientVersion": client_version(),
        "hl": "en",
        "gl": "US",
    }});
    let resp = agent()
        .post(url)
        .set("Origin", "https://music.youtube.com")
        .set("Referer", "https://music.youtube.com/")
        .set("Cookie", "SOCS=CAI")
        .send_json(body)
        .map_err(|e| anyhow!("request failed: {e}"))?;
    resp.into_json::<Value>()
        .context("invalid JSON from YouTube Music")
}

pub fn artist_top_songs(name: &str) -> Result<Vec<Track>, String> {
    let fetch = || -> Result<Vec<Track>> {
        let found = post(ENDPOINT, json!({ "query": name, "params": ARTISTS_FILTER }))?;
        let Some(id) = pick_artist_id(&found, name) else {
            return Ok(Vec::new());
        };
        let page = post(BROWSE_ENDPOINT, json!({ "browseId": id }))?;
        Ok(parse_search_response(&page))
    };
    fetch().map_err(|e| format!("{e:#}"))
}

fn pick_artist_id(v: &Value, name: &str) -> Option<String> {
    let mut items = Vec::new();
    collect_items(v, &mut items);
    let artists: Vec<(String, &str)> = items
        .into_iter()
        .filter_map(|item| {
            let id = item
                .pointer("/navigationEndpoint/browseEndpoint/browseId")?
                .as_str()?;
            let title = column_segments(item, 0).into_iter().next()?;
            id.starts_with("UC").then_some((title, id))
        })
        .collect();
    let wanted = name.to_lowercase();
    artists
        .iter()
        .find(|(title, _)| title.to_lowercase() == wanted)
        .or(artists.first())
        .map(|(_, id)| id.to_string())
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

fn column_runs(item: &Value, col: usize) -> Option<&Vec<Value>> {
    item.get("flexColumns")
        .and_then(|c| c.get(col))
        .and_then(|c| c.pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs"))
        .and_then(Value::as_array)
}

fn column_segments(item: &Value, col: usize) -> Vec<String> {
    run_segments(column_runs(item, col))
}

/* Artists are the runs that link to an artist/channel page ("UC..." browse
 * ids); the "&" and "," between them are plain runs. Using the source's own
 * boundaries means names that contain a separator stay in one piece. Names
 * must also appear in `credit`, so a stray linked run can't leak in. Empty
 * when the response carries no links (the caller falls back to guessing). */
fn artist_names_from_runs(runs: Option<&Vec<Value>>, credit: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for run in runs.into_iter().flatten() {
        let id = run
            .pointer("/navigationEndpoint/browseEndpoint/browseId")
            .and_then(Value::as_str);
        let text = run.get("text").and_then(Value::as_str);
        let (Some(id), Some(text)) = (id, text) else {
            continue;
        };
        let text = text.trim();
        if id.starts_with("UC")
            && !text.is_empty()
            && credit.contains(text)
            && !names.iter().any(|n| n == text)
        {
            names.push(text.to_string());
        }
    }
    names
}

fn run_segments(runs: Option<&Vec<Value>>) -> Vec<String> {
    let mut segments = Vec::new();
    let mut cur = String::new();
    for run in runs.into_iter().flatten() {
        let Some(text) = run.get("text").and_then(Value::as_str) else {
            continue;
        };
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

fn fixed_duration(item: &Value) -> Option<u32> {
    let s = item
        .pointer("/fixedColumns/0/musicResponsiveListItemFixedColumnRenderer/text/runs/0/text")?
        .as_str()?;
    parse_clock(s).map(|s| s as u32)
}

fn parse_item(item: &Value) -> Option<Track> {
    let id = find_str(item, "videoId")?;
    if !is_valid_id(id) {
        return None;
    }
    let title = column_segments(item, 0).into_iter().next()?;

    let mut meta = column_segments(item, 1);
    if matches!(
        meta.first().map(String::as_str),
        Some("Song" | "Video" | "Episode")
    ) {
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
    let duration = duration.or_else(|| fixed_duration(item));
    let artist = meta
        .first()
        .cloned()
        .unwrap_or_else(|| "Unknown artist".into());
    let artists = artist_names_from_runs(column_runs(item, 1), &artist);
    let album = meta
        .get(1)
        .filter(|a| !a.contains(" views") && !a.contains(" plays"))
        .cloned();

    Some(Track {
        id: id.to_string(),
        title,
        artist,
        artists,
        album,
        duration,
    })
}

fn parse_browse(v: &Value, playlist: bool) -> Vec<Track> {
    let mut items = Vec::new();
    collect_items(v, &mut items);
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter_map(|item| {
            let mut t = parse_item(item)?;
            if playlist && t.album.is_none() {
                t.album = column_segments(item, 2).into_iter().next();
            }
            Some(t)
        })
        .filter(|t| seen.insert(t.id.clone()))
        .collect()
}

pub fn parse_collections(v: &Value, kind: Kind) -> Vec<Collection> {
    let mut items = Vec::new();
    collect_items(v, &mut items);
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter_map(|item| {
            let id = item
                .pointer("/navigationEndpoint/browseEndpoint/browseId")?
                .as_str()?;
            let ok = match kind {
                Kind::Album => id.starts_with("MPRE"),
                Kind::Playlist => id.starts_with("VL"),
            };
            if !ok {
                return None;
            }
            let title = column_segments(item, 0).into_iter().next()?;
            let mut meta = column_segments(item, 1);
            if matches!(
                meta.first().map(String::as_str),
                Some("Album" | "EP" | "Single" | "Playlist")
            ) {
                meta.remove(0);
            }
            let artist = match kind {
                Kind::Album => meta.first().cloned().unwrap_or_default(),
                Kind::Playlist => String::new(),
            };
            Some(Collection {
                browse_id: id.to_string(),
                title,
                subtitle: meta.join(" · "),
                artist,
            })
        })
        .filter(|c| seen.insert(c.browse_id.clone()))
        .collect()
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

fn collect_panel<'a>(v: &'a Value, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(map) => {
            if let Some(w) = map.get("playlistPanelVideoWrapperRenderer") {
                out.extend(w.pointer("/primaryRenderer/playlistPanelVideoRenderer"));
            } else if let Some(item) = map.get("playlistPanelVideoRenderer") {
                out.push(item);
            } else {
                map.values().for_each(|c| collect_panel(c, out));
            }
        }
        Value::Array(arr) => arr.iter().for_each(|c| collect_panel(c, out)),
        _ => {}
    }
}

fn parse_panel_item(item: &Value) -> Option<Track> {
    let id = item.get("videoId")?.as_str()?;
    if !is_valid_id(id) {
        return None;
    }
    let title: String = item
        .pointer("/title/runs")?
        .as_array()?
        .iter()
        .filter_map(|r| r.get("text")?.as_str())
        .collect();
    let title = title.trim().to_string();
    if title.is_empty() {
        return None;
    }
    let byline = item
        .pointer("/longBylineText/runs")
        .and_then(Value::as_array);
    let meta = run_segments(byline);
    let artist = meta
        .first()
        .cloned()
        .unwrap_or_else(|| "Unknown artist".into());
    let artists = artist_names_from_runs(byline, &artist);
    let album = meta
        .get(1)
        .filter(|a| {
            !a.contains(" views")
                && !a.contains(" plays")
                && !(a.len() == 4 && a.bytes().all(|b| b.is_ascii_digit()))
        })
        .cloned();
    let duration = item
        .pointer("/lengthText/runs/0/text")
        .or_else(|| item.pointer("/lengthText/simpleText"))
        .and_then(Value::as_str)
        .and_then(parse_clock)
        .map(|s| s as u32);
    Some(Track {
        id: id.to_string(),
        title,
        artist,
        artists,
        album,
        duration,
    })
}

pub fn parse_radio_response(v: &Value) -> Vec<Track> {
    let mut items = Vec::new();
    collect_panel(v, &mut items);
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter_map(parse_panel_item)
        .filter(|t| seen.insert(t.id.clone()))
        .collect()
}

fn ytdlp_search(query: &str) -> Result<Vec<Track>> {
    ytdlp_entries(&format!("ytsearch{FALLBACK_COUNT}:{query}"))
}

fn ytdlp_entries(target: &str) -> Result<Vec<Track>> {
    let mut cmd = Command::new("yt-dlp");
    cmd.args([
        "--flat-playlist",
        "--dump-json",
        "--no-warnings",
        "--ignore-errors",
        "--socket-timeout",
        "15",
    ])
    .arg(target)
    .stdin(Stdio::null());
    let out = crate::sys::no_window(&mut cmd)
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
            /* newer yt-dlp lists each credited artist separately */
            let artists: Vec<String> = v
                .get("artists")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(|n| n.trim().to_string())
                        .filter(|n| !n.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let duration = v.get("duration").and_then(Value::as_f64).map(|d| d as u32);
            Some(Track {
                id: id.to_string(),
                title,
                artist,
                artists,
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
    fn keeps_linked_artist_runs_whole() {
        let link = |text: &str, id: &str| json!({"text": text, "navigationEndpoint": {"browseEndpoint": {"browseId": id}}});
        let resp = json!([{"musicResponsiveListItemRenderer": {
            "overlay": {"watchEndpoint": {"videoId": "abcdefghijk"}},
            "flexColumns": [
                {"musicResponsiveListItemFlexColumnRenderer": run("Song")},
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                    link("Tyler, The Creator", "UCaaaaaaaa"), {"text": " & "},
                    link("Kali Uchis", "UCbbbbbbbb"), {"text": " • "},
                    link("Album", "MPREb_cccc"), {"text": " • "}, {"text": "3:21"}
                ]}}}
            ]
        }}]);
        let t = &parse_search_response(&resp)[0];
        assert_eq!(t.artist, "Tyler, The Creator & Kali Uchis");
        assert_eq!(t.artists, ["Tyler, The Creator", "Kali Uchis"]);
        assert_eq!(t.album.as_deref(), Some("Album"));
    }

    #[test]
    fn reads_ytdlp_artists_list() {
        let line =
            r#"{"id":"abcdefghijk","title":"T","artist":"A, B","artists":["A","B"],"duration":10}"#;
        let t = parse_ytdlp_lines(line);
        assert_eq!(t[0].artists, ["A", "B"]);
        assert_eq!(t[0].artist, "A, B");
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
    fn picks_artist_by_name() {
        let artist = |name: &str, id: &str| {
            json!({"musicResponsiveListItemRenderer": {
                "navigationEndpoint": {"browseEndpoint": {"browseId": id}},
                "flexColumns": [{"musicResponsiveListItemFlexColumnRenderer": run(name)}]
            }})
        };
        let resp = json!({"contents": [
            artist("Daft Punk Tribute", "UCaaa"),
            artist("Daft Punk", "UCbbb"),
        ]});
        assert_eq!(pick_artist_id(&resp, "daft punk").as_deref(), Some("UCbbb"));
        assert_eq!(pick_artist_id(&resp, "Other").as_deref(), Some("UCaaa"));
        assert_eq!(pick_artist_id(&json!([]), "x"), None);
    }

    #[test]
    fn finds_continuation_tokens() {
        let old = json!({"musicShelfRenderer": {"contents": [], "continuations": [
            {"nextContinuationData": {"continuation": "abc%3D", "clickTrackingParams": "x"}}
        ]}});
        assert_eq!(find_continuation(&old).as_deref(), Some("abc%3D"));
        let new = json!([{"continuationItemRenderer": {"continuationEndpoint": {
            "continuationCommand": {"token": "def", "request": "CONTINUATION_REQUEST_TYPE_SEARCH"}
        }}}]);
        assert_eq!(find_continuation(&new).as_deref(), Some("def"));
        assert_eq!(find_continuation(&json!({"a": [1, 2]})), None);
    }

    #[test]
    fn parses_radio_panel() {
        let item = |id: &str, title: &str, by: Value, len: &str| {
            json!({"playlistPanelVideoRenderer": {
                "videoId": id,
                "title": {"runs": [{"text": title}]},
                "longBylineText": {"runs": by},
                "lengthText": {"runs": [{"text": len}]},
            }})
        };
        let by = json!([{"text": "A"}, {"text": " & "}, {"text": "B"},
            {"text": " • "}, {"text": "Album"}, {"text": " • "}, {"text": "2019"}]);
        let resp = json!({"contents": [
            item("abcdefghijk", "One", by, "3:21"),
            {"playlistPanelVideoWrapperRenderer": {
                "primaryRenderer": item("bcdefghijkl", "Two", json!([{"text": "C"}]), "1:05"),
                "counterpart": [item("zzzzzzzzzzz", "Video", json!([{"text": "C"}]), "1:05")],
            }},
            item("abcdefghijk", "dup", json!([{"text": "A"}]), "3:21"),
        ]});
        let t = parse_radio_response(&resp);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].artist, "A & B");
        assert_eq!(t[0].album.as_deref(), Some("Album"));
        assert_eq!(t[0].duration, Some(201));
        assert_eq!(t[1].id, "bcdefghijkl");
        assert_eq!(t[1].album, None);
    }

    #[test]
    fn parses_album_and_playlist_results() {
        let row = |id: &str, title: &str, runs: Value| {
            json!({"musicResponsiveListItemRenderer": {
                "navigationEndpoint": {"browseEndpoint": {"browseId": id}},
                "flexColumns": [
                    {"musicResponsiveListItemFlexColumnRenderer": run(title)},
                    {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": runs}}},
                ]
            }})
        };
        let resp = json!([
            row(
                "MPREb_abc",
                "Discovery",
                json!([{"text": "Album"}, {"text": " • "},
                {"text": "Daft Punk"}, {"text": " • "}, {"text": "2001"}])
            ),
            row(
                "VLPL123",
                "Mix",
                json!([{"text": "Playlist"}, {"text": " • "}, {"text": "Someone"}])
            ),
            row("UCartist", "Artist", json!([{"text": "Artist"}])),
        ]);
        let a = parse_collections(&resp, Kind::Album);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].title, "Discovery");
        assert_eq!(a[0].artist, "Daft Punk");
        assert_eq!(a[0].subtitle, "Daft Punk · 2001");
        let p = parse_collections(&resp, Kind::Playlist);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].browse_id, "VLPL123");
        assert_eq!(p[0].artist, "");
    }

    #[test]
    fn parses_browse_tracks() {
        let resp = json!({"contents": [{"musicResponsiveListItemRenderer": {
            "playlistItemData": {"videoId": "abcdefghijk"},
            "flexColumns": [{"musicResponsiveListItemFlexColumnRenderer": run("One More Time")}],
            "fixedColumns": [{"musicResponsiveListItemFixedColumnRenderer": run("5:20")}],
        }}]});
        let t = parse_browse(&resp, false);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].duration, Some(320));
        assert_eq!(t[0].artist, "Unknown artist");
    }

    #[test]
    fn date_math() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }
}
