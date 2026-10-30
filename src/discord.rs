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
const DRIFT_TOLERANCE_MS: i64 = 2500;

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
    paused: bool,
    start: i64,
    end: Option<i64>,
}

pub struct Discord {
    tx: Sender<Msg>,
    track: Option<String>,
    state: PlayState,
    start: i64,
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
        Some(Discord {
            tx,
            track: None,
            state: PlayState::Stopped,
            start: 0,
        })
    }

    pub fn update(&mut self, track: Option<&Track>, state: PlayState, position: f64) {
        let Some(track) = track.filter(|_| state != PlayState::Stopped) else {
            if self.state != PlayState::Stopped {
                self.state = PlayState::Stopped;
                self.track = None;
                let _ = self.tx.send(Msg::Clear);
            }
            return;
        };

        let position = if position.is_finite() { position.max(0.0) } else { 0.0 };
        let start = now_ms() - (position * 1000.0) as i64;
        let paused = state == PlayState::Paused;
        let same_track = self.track.as_deref() == Some(track.id.as_str());
        let was_paused = self.state == PlayState::Paused;
        let on_time = paused || (start - self.start).abs() < DRIFT_TOLERANCE_MS;
        if same_track && paused == was_paused && self.state != PlayState::Stopped && on_time {
            self.state = state;
            return;
        }

        self.track = Some(track.id.clone());
        self.state = state;
        self.start = start;
        let end = track
            .duration
            .map(|d| start + i64::from(d) * 1000)
            .filter(|_| !paused);
        let _ = self.tx.send(Msg::Show(Presence {
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            cover: format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", track.id),
            url: track.url(),
            paused,
            start,
            end,
        }));
    }
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
            Ok(msg) => pending = Some(msg),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        while let Ok(msg) = rx.try_recv() {
            pending = Some(msg);
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
    match msg {
        Msg::Clear => client.clear_activity().is_ok(),
        Msg::Show(p) => {
            let mut assets = Assets::new().large_image(&p.cover);
            if let Some(album) = &p.album {
                assets = assets.large_text(album);
            }
            let mut timestamps = Timestamps::new().start(p.start);
            if let Some(end) = p.end {
                timestamps = timestamps.end(end);
            }
            let artist = if p.paused {
                format!("{} (paused)", p.artist)
            } else {
                p.artist.clone()
            };
            let mut activity = Activity::new()
                .activity_type(ActivityType::Listening)
                .status_display_type(StatusDisplayType::Details)
                .details(&p.title)
                .state(&artist)
                .assets(assets)
                .buttons(vec![Button::new("Listen Along", &p.url)]);
            if !p.paused {
                activity = activity.timestamps(timestamps);
            }
            client.set_activity(activity).is_ok()
        }
    }
}
