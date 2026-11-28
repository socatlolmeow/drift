use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use discord_rich_presence::activity::{
    Activity, ActivityType, Assets, Button, StatusDisplayType, Timestamps,
};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};

use crate::app::PlayState;
use crate::model::Track;

const RETRY: Duration = Duration::from_secs(5);
/// Quiet period before sending, so seeking or skipping doesn't flood Discord
const SETTLE: Duration = Duration::from_millis(750);
const DRIFT_TOLERANCE_MS: u64 = 2500;

/// Discord rejects activity text outside this range (in characters)
const FIELD_MIN_LEN: usize = 2;
const FIELD_MAX_LEN: usize = 128;

enum Msg {
    Show(Presence),
    Clear,
}

struct Presence {
    title: String,
    artist: String,
    album: Option<String>,
    cover: String,
    url: String,
    start: i64,
    end: Option<i64>,
}

struct Shown {
    track_id: String,
    start: i64,
}

pub struct Discord {
    tx: Sender<Msg>,
    shown: Option<Shown>,
}

impl Discord {
    pub fn new(client_id: &str) -> Option<Discord> {
        let id = client_id.trim();
        if id.is_empty() {
            return None;
        }
        let id = id.to_string();
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("discord".into())
            .spawn(move || run(&id, rx))
            .ok()?;
        Some(Discord { tx, shown: None })
    }

    /// Shows the track while it plays and clears the presence when playback
    /// is paused or stopped
    pub fn update(&mut self, track: Option<&Track>, state: PlayState, position: f64) {
        let playing = !matches!(state, PlayState::Stopped | PlayState::Paused);
        let Some(track) = track.filter(|_| playing) else {
            if self.shown.take().is_some() {
                let _ = self.tx.send(Msg::Clear);
            }
            return;
        };

        let position = if position.is_finite() {
            position.max(0.0)
        } else {
            0.0
        };
        let start = now_ms() - (position * 1000.0) as i64;

        if let Some(shown) = &self.shown {
            let on_time = start.abs_diff(shown.start) < DRIFT_TOLERANCE_MS;
            if shown.track_id == track.id && on_time {
                return;
            }
        }

        self.shown = Some(Shown {
            track_id: track.id.clone(),
            start,
        });
        let end = track.duration.map(|d| start + i64::from(d) * 1000);
        let _ = self.tx.send(Msg::Show(Presence {
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            cover: cover_url(&track.id),
            url: track.url(),
            start,
            end,
        }));
    }
}

fn cover_url(video_id: &str) -> String {
    format!("https://i.ytimg.com/vi/{video_id}/mqdefault.jpg")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn run(client_id: &str, rx: mpsc::Receiver<Msg>) {
    let mut client: Option<DiscordIpcClient> = None;
    let mut pending: Option<Msg> = None;
    loop {
        match rx.recv_timeout(RETRY) {
            Ok(msg) => {
                pending = Some(msg);
                while let Ok(msg) = rx.recv_timeout(SETTLE) {
                    pending = Some(msg);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let Some(msg) = pending.take() else { continue };

        if client.is_none() {
            if matches!(msg, Msg::Clear) {
                continue;
            }
            let mut c = DiscordIpcClient::new(client_id);
            if c.connect().is_ok() {
                client = Some(c);
            }
        }
        let sent = client.as_mut().is_some_and(|c| send(c, &msg));
        if !sent {
            client = None;
            if matches!(msg, Msg::Show(_)) {
                pending = Some(msg);
            }
        }
    }
    if let Some(mut c) = client {
        let _ = c.clear_activity();
        let _ = c.close();
    }
}

fn send(client: &mut DiscordIpcClient, msg: &Msg) -> bool {
    let p = match msg {
        Msg::Clear => return client.clear_activity().is_ok(),
        Msg::Show(p) => p,
    };

    let title = fit_field(&p.title);
    let artist = fit_field(&p.artist);
    let album = p.album.as_deref().map(fit_field);

    let mut assets = Assets::new().large_image(&p.cover);
    if let Some(album) = &album {
        assets = assets.large_text(album);
    }

    let mut timestamps = Timestamps::new().start(p.start);
    if let Some(end) = p.end {
        timestamps = timestamps.end(end);
    }

    let activity = Activity::new()
        .activity_type(ActivityType::Listening)
        .status_display_type(StatusDisplayType::Name)
        .details(&title)
        .state(&artist)
        .assets(assets)
        .timestamps(timestamps)
        .buttons(vec![Button::new("Listen Along", &p.url)]);
    client.set_activity(activity).is_ok()
}

fn fit_field(text: &str) -> String {
    let len = text.chars().count();
    if len > FIELD_MAX_LEN {
        let mut fitted: String = text.chars().take(FIELD_MAX_LEN - 1).collect();
        fitted.push('…');
        return fitted;
    }
    let mut fitted = text.to_string();
    fitted.extend(std::iter::repeat('\u{2800}').take(FIELD_MIN_LEN.saturating_sub(len)));
    fitted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_padded_to_minimum() {
        assert_eq!(fit_field("A").chars().count(), FIELD_MIN_LEN);
        assert_eq!(fit_field("").chars().count(), FIELD_MIN_LEN);
    }

    #[test]
    fn normal_text_is_unchanged() {
        assert_eq!(fit_field("Intro"), "Intro");
    }

    #[test]
    fn long_text_is_truncated_with_ellipsis() {
        let fitted = fit_field(&"x".repeat(300));
        assert_eq!(fitted.chars().count(), FIELD_MAX_LEN);
        assert!(fitted.ends_with('…'));
    }

    #[test]
    fn truncation_respects_multibyte_characters() {
        let fitted = fit_field(&"é".repeat(200));
        assert_eq!(fitted.chars().count(), FIELD_MAX_LEN);
    }
}
