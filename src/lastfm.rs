//! Last.fm scrobbling.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::PlayState;
use crate::config::Lastfm as LastfmConfig;
use crate::model::Track;
use crate::store::{state_base, write_atomic, Store};

const API_URL: &str = "https://ws.audioscrobbler.com/2.0/";
const AUTH_URL: &str = "https://www.last.fm/api/auth/";
const SIGNUP_URL: &str = "https://www.last.fm/api/account/create";
const UA: &str = concat!("drift/", env!("CARGO_PKG_VERSION"));

const TIMEOUT: Duration = Duration::from_secs(10);
const EXIT_TIMEOUT: Duration = Duration::from_secs(4);
const IDLE: Duration = Duration::from_secs(3600);
const BACKOFF_START: Duration = Duration::from_secs(30);
const BACKOFF_MAX: Duration = Duration::from_secs(15 * 60);

const MIN_TRACK_SECS: f64 = 30.0;
const MAX_LISTEN_SECS: f64 = 240.0;
const MAX_STEP_SECS: f64 = 2.0;

const BATCH_SIZE: usize = 50;
/// Last.fm refuses scrobbles older than 14 days, so there is no point in
/// keeping them around
const MAX_AGE_SECS: u64 = 14 * 24 * 60 * 60;
const MAX_QUEUE: usize = 2000;

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

const MD5_SHIFTS: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
]; // Fucking cancer shit bro
fn md5_hex(data: &[u8]) -> String {
    let mut state: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];

    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&(data.len() as u64).wrapping_mul(8).to_le_bytes());

    let (blocks, remainder) = msg.as_chunks::<64>();
    debug_assert!(remainder.is_empty());

    for block in blocks {
        let mut words = [0u32; 16];
        let (word_bytes, _) = block.as_chunks::<4>();
        for (word, bytes) in words.iter_mut().zip(word_bytes) {
            *word = u32::from_le_bytes(*bytes);
        }

        let [mut a, mut b, mut c, mut d] = state;
        for (i, (&k, &shift)) in MD5_K.iter().zip(MD5_SHIFTS.iter()).enumerate() {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k).wrapping_add(words[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(shift));
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }

    state
        .iter()
        .map(|w| format!("{:08x}", w.swap_bytes()))
        .collect()
}

fn sign(params: &BTreeMap<String, String>, secret: &str) -> String {
    let mut payload = String::new();
    for (name, value) in params {
        if name == "format" || name == "callback" {
            continue;
        }
        payload.push_str(name);
        payload.push_str(value);
    }
    payload.push_str(secret);
    md5_hex(payload.as_bytes())
}

#[derive(Debug)]
enum ApiError {
    /// Network trouble or Last.fm having a bad moment, we will retry
    Temporary(String),
    /// Credentials invalid or something
    Auth(String),
    /// Last.fm refused this particular request.
    Rejected(String),
}

impl ApiError {
    fn from_code(code: i64, message: &str) -> ApiError {
        let text = format!("{message} (error {code})");
        match code {
            8 | 11 | 16 | 29 => ApiError::Temporary(text),
            4 | 9 | 10 | 13 | 14 | 15 | 26 => ApiError::Auth(text),
            _ => ApiError::Rejected(text),
        }
    }

    fn from_body(body: &Value) -> Option<ApiError> {
        let code = body.get("error")?.as_i64()?;
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        Some(ApiError::from_code(code, message))
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Temporary(m) | ApiError::Auth(m) | ApiError::Rejected(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ApiError {}

struct Client {
    agent: ureq::Agent,
    api_key: String,
    api_secret: String,
    session_key: Option<String>,
}

fn build_agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(timeout)
        .user_agent(UA)
        .build()
}

impl Client {
    fn new(api_key: &str, api_secret: &str, session_key: Option<String>) -> Client {
        Client {
            agent: build_agent(TIMEOUT),
            api_key: api_key.to_string(),
            api_secret: api_secret.to_string(),
            session_key,
        }
    }

    fn set_timeout(&mut self, timeout: Duration) {
        self.agent = build_agent(timeout);
    }

    fn call(&self, method: &str, mut params: BTreeMap<String, String>) -> Result<Value, ApiError> {
        params.insert("method".into(), method.into());
        params.insert("api_key".into(), self.api_key.clone());
        if let Some(sk) = &self.session_key {
            params.insert("sk".into(), sk.clone());
        }
        let signature = sign(&params, &self.api_secret);
        params.insert("api_sig".into(), signature);
        params.insert("format".into(), "json".into());

        let form: Vec<(&str, &str)> = params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        match self.agent.post(API_URL).send_form(&form) {
            Ok(response) => {
                let body: Value = response
                    .into_json()
                    .map_err(|e| ApiError::Temporary(format!("bad response: {e}")))?;
                match ApiError::from_body(&body) {
                    Some(err) => Err(err),
                    None => Ok(body),
                }
            }
            Err(ureq::Error::Status(status, response)) => {
                if let Some(err) = response
                    .into_json::<Value>()
                    .ok()
                    .and_then(|body| ApiError::from_body(&body))
                {
                    return Err(err);
                }
                let text = format!("HTTP {status}");
                Err(if status >= 500 {
                    ApiError::Temporary(text)
                } else {
                    ApiError::Rejected(text)
                })
            }
            Err(e) => Err(ApiError::Temporary(e.to_string())),
        }
    }

    fn now_playing(&self, play: &Scrobble) -> Result<(), ApiError> {
        let mut params = BTreeMap::new();
        play.insert_fields(&mut params, "");
        self.call("track.updateNowPlaying", params).map(|_| ())
    }

    fn scrobble(&self, batch: &[Scrobble]) -> Result<(), ApiError> {
        let mut params = BTreeMap::new();
        for (i, play) in batch.iter().enumerate() {
            let suffix = format!("[{i}]");
            play.insert_fields(&mut params, &suffix);
            params.insert(format!("timestamp{suffix}"), play.timestamp.to_string());
        }
        self.call("track.scrobble", params).map(|_| ())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scrobble {
    artist: String,
    track: String,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    duration: Option<u32>,
    /// When the track started playing (Unix seconds / UTC)
    timestamp: u64,
}

impl Scrobble {
    fn from_track(track: &Track, timestamp: u64) -> Option<Scrobble> {
        let artist = track
            .artist_names()
            .into_iter()
            .next()
            .unwrap_or_else(|| track.artist.clone());
        let artist = artist.trim();
        let title = track.title.trim();
        if artist.is_empty() || title.is_empty() {
            return None;
        }
        Some(Scrobble {
            artist: artist.to_string(),
            track: title.to_string(),
            album: track
                .album
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
                .map(str::to_string),
            duration: track.duration.filter(|&d| d > 0),
            timestamp,
        })
    }

    fn insert_fields(&self, params: &mut BTreeMap<String, String>, suffix: &str) {
        params.insert(format!("artist{suffix}"), self.artist.clone());
        params.insert(format!("track{suffix}"), self.track.clone());
        if let Some(album) = &self.album {
            params.insert(format!("album{suffix}"), album.clone());
        }
        if let Some(duration) = self.duration {
            params.insert(format!("duration{suffix}"), duration.to_string());
        }
    }
}

/// These are last.fm rules, it checks if song is longer than 30s and
/// listened to more than half of the song / 4 mins
fn qualifies(length: Option<f64>, listened: f64) -> bool {
    match length {
        Some(len) if len <= MIN_TRACK_SECS => false,
        Some(len) => listened >= (len / 2.0).min(MAX_LISTEN_SECS),
        None => listened >= MAX_LISTEN_SECS,
    }
}

struct Play {
    scrobble: Scrobble,
    listened: f64,
    last_position: Option<f64>,
    submitted: bool,
}

/// Decides when the current play has been listened to for long enough
#[derive(Default)]
struct Tracker {
    current: Option<Play>,
}

impl Tracker {
    fn start(&mut self, scrobble: Scrobble) {
        self.current = Some(Play {
            scrobble,
            listened: 0.0,
            last_position: None,
            submitted: false,
        });
    }

    fn clear(&mut self) {
        self.current = None;
    }

    fn tick(&mut self, state: PlayState, position: f64, duration: f64) -> Option<Scrobble> {
        let play = self.current.as_mut()?;
        if state != PlayState::Playing || !position.is_finite() {
            // Forget the baseline so time spent paused or loading is not
            // counted when playback continues
            play.last_position = None;
            return None;
        }
        // Measuring the position instead of wallclock time keeps playback
        // speed right, and lets seeks be told apart from listening
        if let Some(previous) = play.last_position.replace(position) {
            let step = position - previous;
            if (0.0..=MAX_STEP_SECS).contains(&step) {
                play.listened += step;
            }
        }
        if play.submitted {
            return None;
        }
        let length = play.scrobble.duration.map(f64::from).or_else(|| {
            let known = duration.is_finite() && duration > 0.0;
            known.then_some(duration)
        });
        if !qualifies(length, play.listened) {
            return None;
        }
        play.submitted = true;
        Some(play.scrobble.clone())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Account {
    username: String,
    session_key: String,
}

impl Account {
    fn path() -> Result<PathBuf> {
        Ok(Store::dir()?.join("lastfm.json"))
    }

    fn load() -> Result<Option<Account>> {
        let path = Self::path()?;
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map(Some).with_context(|| {
                format!(
                    "{} is invalid; run 'drift --lastfm-login' again",
                    path.display()
                )
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = path.with_extension("tmp");
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        file.write_all(serde_json::to_string_pretty(self)?.as_bytes())
            .with_context(|| format!("writing {}", tmp.display()))?;
        drop(file);
        fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))
    }
}

/// Scrobbles that are waiting to be sent
struct Queue {
    items: VecDeque<Scrobble>,
    path: Option<PathBuf>,
    dirty: bool,
}

fn queue_path() -> Option<PathBuf> {
    Some(state_base()?.join("drift").join("lastfm_queue.json"))
}

impl Queue {
    fn load(path: Option<PathBuf>) -> Queue {
        let items = path
            .as_deref()
            .and_then(|p| {
                let text = fs::read_to_string(p).ok()?;
                match serde_json::from_str::<VecDeque<Scrobble>>(&text) {
                    Ok(items) => Some(items),
                    Err(_) => {
                        let _ = fs::rename(p, p.with_extension("json.bak"));
                        None
                    }
                }
            })
            .unwrap_or_default();
        Queue {
            items,
            path,
            dirty: false,
        }
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn push(&mut self, scrobble: Scrobble) {
        self.items.push_back(scrobble);
        while self.items.len() > MAX_QUEUE {
            self.items.pop_front();
        }
        self.dirty = true;
    }

    fn drop_stale(&mut self, now: u64) {
        let before = self.items.len();
        self.items
            .retain(|s| now.saturating_sub(s.timestamp) <= MAX_AGE_SECS);
        self.dirty |= self.items.len() != before;
    }

    fn front(&self, count: usize) -> Vec<Scrobble> {
        self.items.iter().take(count).cloned().collect()
    }

    fn remove_front(&mut self, count: usize) {
        let count = count.min(self.items.len());
        self.items.drain(..count);
        self.dirty |= count > 0;
    }

    fn persist(&mut self) -> Result<()> {
        let Some(path) = self.path.as_deref().filter(|_| self.dirty) else {
            return Ok(());
        };
        write_atomic(path, &serde_json::to_string(&self.items)?)?;
        self.dirty = false;
        Ok(())
    }
}

enum Job {
    NowPlaying(Scrobble),
    Scrobble(Scrobble),
}

struct Worker {
    client: Client,
    queue: Queue,
    notify: Box<dyn Fn(String) + Send>,
    retry_at: Option<Instant>,
    backoff: Duration,
    disabled: bool,
    isolate: bool,
    outage_reported: bool,
    save_failed: bool,
}

impl Worker {
    fn in_backoff(&self) -> bool {
        self.retry_at.is_some_and(|at| Instant::now() < at)
    }

    fn wait(&self) -> Duration {
        match self.retry_at {
            Some(at) if !self.disabled && !self.queue.is_empty() => {
                at.saturating_duration_since(Instant::now())
            }
            _ => IDLE,
        }
    }

    fn run(mut self, rx: Receiver<Job>) {
        self.flush();
        loop {
            match rx.recv_timeout(self.wait()) {
                Ok(Job::NowPlaying(play)) => self.now_playing(&play),
                Ok(Job::Scrobble(play)) => {
                    self.queue.push(play);
                    self.persist();
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.flush();
        }
        self.client.set_timeout(EXIT_TIMEOUT);
        self.flush();
    }

    fn now_playing(&mut self, play: &Scrobble) {
        if self.disabled || self.in_backoff() {
            return;
        }
        if let Err(ApiError::Auth(msg)) = self.client.now_playing(play) {
            self.disable(&msg);
        }
    }

    fn flush(&mut self) {
        if self.disabled || self.in_backoff() {
            return;
        }
        self.queue.drop_stale(unix_now());
        while !self.queue.is_empty() {
            let size = if self.isolate { 1 } else { BATCH_SIZE };
            let batch = self.queue.front(size);
            match self.client.scrobble(&batch) {
                Ok(()) => {
                    self.queue.remove_front(batch.len());
                    self.retry_at = None;
                    self.backoff = BACKOFF_START;
                    self.outage_reported = false;
                }
                Err(ApiError::Temporary(msg)) => {
                    self.back_off(&msg);
                    break;
                }
                Err(ApiError::Auth(msg)) => {
                    self.disable(&msg);
                    break;
                }
                Err(ApiError::Rejected(_)) if batch.len() > 1 => self.isolate = true,
                Err(ApiError::Rejected(msg)) => {
                    self.queue.remove_front(1);
                    (self.notify)(format!("Last.fm rejected a scrobble: {msg}"));
                }
            }
        }
        if self.queue.is_empty() {
            self.isolate = false;
        }
        self.persist();
    }

    fn back_off(&mut self, msg: &str) {
        self.retry_at = Some(Instant::now() + self.backoff);
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
        if !self.outage_reported {
            self.outage_reported = true;
            (self.notify)(format!(
                "Last.fm unreachable ({msg}); scrobbles are kept and retried"
            ));
        }
    }

    fn disable(&mut self, msg: &str) {
        self.disabled = true;
        (self.notify)(format!(
            "Last.fm scrobbling stopped: {msg}. Check [lastfm] in config.toml or run 'drift --lastfm-login'"
        ));
    }

    fn persist(&mut self) {
        match self.queue.persist() {
            Ok(()) => self.save_failed = false,
            Err(e) if !self.save_failed => {
                self.save_failed = true;
                (self.notify)(format!("Could not save the last.fm queue: {e:#}"));
            }
            Err(_) => {}
        }
    }
}

pub struct Scrobbler {
    tx: Option<Sender<Job>>,
    worker: Option<JoinHandle<()>>,
    tracker: Tracker,
}

impl Scrobbler {
    /// Returns `None` when Last.fm is not configured or not signed in.
    /// `owns_state` says whether this drift may keep the on-disk queue so
    /// extra windows queue in memory only so they cannot clobber each other
    /// `notify` receives messages worth showing to the user, from any thread
    pub fn new(
        config: &LastfmConfig,
        owns_state: bool,
        notify: impl Fn(String) + Send + 'static,
    ) -> Option<Scrobbler> {
        let (api_key, api_secret) = config.credentials()?;
        let account = match Account::load() {
            Ok(Some(account)) => account,
            Ok(None) => {
                notify("Last.fm: not signed in, run `drift --lastfm-login`".into());
                return None;
            }
            Err(e) => {
                notify(format!("Last.fm: {e:#}"));
                return None;
            }
        };
        let path = if owns_state { queue_path() } else { None };
        let worker = Worker {
            client: Client::new(api_key, api_secret, Some(account.session_key)),
            queue: Queue::load(path),
            notify: Box::new(notify),
            retry_at: None,
            backoff: BACKOFF_START,
            disabled: false,
            isolate: false,
            outage_reported: false,
            save_failed: false,
        };
        let (tx, rx) = mpsc::channel();
        let handle = thread::Builder::new()
            .name("lastfm".into())
            .spawn(move || worker.run(rx))
            .ok()?;
        Some(Scrobbler {
            tx: Some(tx),
            worker: Some(handle),
            tracker: Tracker::default(),
        })
    }

    /// A track began playing
    pub fn start(&mut self, track: &Track) {
        let Some(scrobble) = Scrobble::from_track(track, unix_now()) else {
            self.tracker.clear();
            return;
        };
        self.send(Job::NowPlaying(scrobble.clone()));
        self.tracker.start(scrobble);
    }

    /// Call regularly with the player's state and args
    pub fn tick(&mut self, state: PlayState, position: f64, duration: f64) {
        if let Some(scrobble) = self.tracker.tick(state, position, duration) {
            self.send(Job::Scrobble(scrobble));
        }
    }

    fn send(&self, job: Job) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(job);
        }
    }
}

impl Drop for Scrobbler {
    fn drop(&mut self) {
        // Closing the channel tells the worker to do its final flush
        self.tx = None;
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

pub fn login(config: &LastfmConfig) -> Result<()> {
    let Some((api_key, api_secret)) = config.credentials() else {
        bail!(
            "set api_key and api_secret under [lastfm] in config.toml first \
             (create them at {SIGNUP_URL})"
        );
    };
    let client = Client::new(api_key, api_secret, None);

    let body = client
        .call("auth.getToken", BTreeMap::new())
        .context("requesting a login token")?;
    let token = body["token"]
        .as_str()
        .ok_or_else(|| anyhow!("Last.fm sent no token"))?
        .to_string();

    println!("Open this link, sign in and allow drift access:\n");
    println!("  {AUTH_URL}?api_key={api_key}&token={token}\n");
    print!("Press Enter here once you have allowed it.... ");
    io::stdout().flush()?;
    io::stdin().lock().read_line(&mut String::new())?;

    let mut params = BTreeMap::new();
    params.insert("token".to_string(), token);
    let body = client
        .call("auth.getSession", params)
        .context("Finishing the login (did you allow Access?)")?;
    let session = &body["session"];
    let (Some(username), Some(session_key)) = (session["name"].as_str(), session["key"].as_str())
    else {
        bail!("Last.fm sent no session");
    };
    Account {
        username: username.to_string(),
        session_key: session_key.to_string(),
    }
    .save()?;
    println!("Sucessfully signed in to Last.fm as {username}. ");
    Ok(())
}

pub fn logout() -> Result<()> {
    let path = Account::path()?;
    match fs::remove_file(&path) {
        Ok(()) => println!("Signed out of Last.fm."),
        Err(e) if e.kind() == io::ErrorKind::NotFound => println!("Not signed in to Last.fm."),
        Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn track(artist: &str, title: &str) -> Track {
        Track {
            id: "dQw4w9WgXcQ".into(),
            title: title.into(),
            artist: artist.into(),
            artists: Vec::new(),
            album: None,
            duration: Some(200),
        }
    }

    fn scrobble(duration: Option<u32>) -> Scrobble {
        Scrobble {
            artist: "A".into(),
            track: "T".into(),
            album: None,
            duration,
            timestamp: 1_700_000_000,
        }
    }

    fn listen(tracker: &mut Tracker, from: f64, seconds: f64) -> usize {
        let mut due = 0;
        let steps = (seconds * 10.0) as usize;
        for i in 1..=steps {
            let position = from + i as f64 / 10.0;
            if tracker.tick(PlayState::Playing, position, 0.0).is_some() {
                due += 1;
            }
        }
        due
    }

    #[test]
    fn md5_known_vectors() {
        let cases = [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(md5_hex(input.as_bytes()), expected, "{input:?}");
        }
    }

    #[test]
    fn md5_padding_boundaries() {
        assert_eq!(md5_hex(&[b'a'; 55]), "ef1772b6dff9a122358552954ad0df65");
        assert_eq!(md5_hex(&[b'a'; 56]), "3b0c8ac703f828b04c6c197006d17218");
        assert_eq!(md5_hex(&[b'a'; 64]), "014842d480b571495a4a0363793f7367");
    }

    #[test]
    fn signature_of_a_batch() {
        let mut params = BTreeMap::new();
        params.insert("method".to_string(), "track.scrobble".to_string());
        params.insert(
            "api_key".to_string(),
            "0123456789abcdef0123456789abcdef".to_string(),
        );
        params.insert("sk".to_string(), "sessionkey".to_string());
        let entries = [
            ("Daft Punk", "Around the World", "1700000000"),
            ("Sigur Rós", "Hoppípolla", "1700000300"),
        ];
        for (i, (artist, title, ts)) in entries.iter().enumerate() {
            params.insert(format!("artist[{i}]"), artist.to_string());
            params.insert(format!("track[{i}]"), title.to_string());
            params.insert(format!("timestamp[{i}]"), ts.to_string());
        }
        assert_eq!(sign(&params, "secret"), "759802ec79f8bad7c85da5ff432c9c65");
        params.insert("format".to_string(), "json".to_string());
        params.insert("callback".to_string(), "cb".to_string());
        assert_eq!(sign(&params, "secret"), "759802ec79f8bad7c85da5ff432c9c65");
    }

    #[test]
    fn error_classification() {
        let kind = |body: Value| match ApiError::from_body(&body).unwrap() {
            ApiError::Temporary(_) => "temporary",
            ApiError::Auth(_) => "auth",
            ApiError::Rejected(_) => "rejected",
        };
        assert_eq!(
            kind(json!({"error": 16, "message": "try later"})),
            "temporary"
        );
        assert_eq!(kind(json!({"error": 29, "message": "rate"})), "temporary");
        assert_eq!(
            kind(json!({"error": 9, "message": "Invalid session key"})),
            "auth"
        );
        assert_eq!(
            kind(json!({"error": 13, "message": "Invalid signature"})),
            "auth"
        );
        assert_eq!(
            kind(json!({"error": 6, "message": "Invalid parameters"})),
            "rejected"
        );
        assert!(ApiError::from_body(&json!({"scrobbles": {}})).is_none());
    }

    #[test]
    fn scrobble_uses_primary_artist_and_trims() {
        let mut t = track("Daft Punk & Pharrell", "  Get Lucky ");
        t.album = Some("  ".into());
        let s = Scrobble::from_track(&t, 5).unwrap();
        assert_eq!(
            (s.artist.as_str(), s.track.as_str()),
            ("Daft Punk", "Get Lucky")
        );
        assert_eq!(s.album, None);
        assert_eq!(s.timestamp, 5);

        t.artists = vec!["Hall & Oates".into(), "X".into()];
        t.album = Some("Album".into());
        let s = Scrobble::from_track(&t, 5).unwrap();
        assert_eq!(s.artist, "Hall & Oates");
        assert_eq!(s.album.as_deref(), Some("Album"));

        assert!(Scrobble::from_track(&track("", "Song"), 5).is_none());
        assert!(Scrobble::from_track(&track("Artist", "  "), 5).is_none());
    }

    #[test]
    fn batch_fields_are_indexed() {
        let mut params = BTreeMap::new();
        let mut s = scrobble(Some(180));
        s.album = Some("Alb".into());
        s.insert_fields(&mut params, "[3]");
        let keys: Vec<&str> = params.keys().map(String::as_str).collect();
        assert_eq!(keys, ["album[3]", "artist[3]", "duration[3]", "track[3]"]);
        assert_eq!(params["duration[3]"], "180");
    }

    #[test]
    fn listening_threshold() {
        assert!(!qualifies(Some(30.0), 30.0), "30s tracks do not count");
        assert!(!qualifies(Some(20.0), 20.0));
        assert!(!qualifies(Some(100.0), 49.9));
        assert!(qualifies(Some(100.0), 50.0));
        assert!(!qualifies(Some(1200.0), 239.0));
        assert!(qualifies(Some(1200.0), 240.0));
        assert!(!qualifies(None, 239.0));
        assert!(qualifies(None, 240.0));
    }

    #[test]
    fn scrobbles_once_at_half_the_track() {
        let mut tracker = Tracker::default();
        tracker.start(scrobble(Some(100)));
        assert_eq!(listen(&mut tracker, 0.0, 49.0), 0);
        assert_eq!(listen(&mut tracker, 49.0, 60.0), 1);
    }

    #[test]
    fn seeking_is_not_listening() {
        let mut tracker = Tracker::default();
        tracker.start(scrobble(Some(100)));
        assert_eq!(listen(&mut tracker, 0.0, 20.0), 0);
        assert_eq!(listen(&mut tracker, 80.0, 20.0), 0);
        assert_eq!(listen(&mut tracker, 10.0, 15.0), 1);
    }

    #[test]
    fn pause_and_loading_are_not_counted() {
        let mut tracker = Tracker::default();
        tracker.start(scrobble(Some(100)));
        assert_eq!(listen(&mut tracker, 0.0, 40.0), 0);
        for _ in 0..100 {
            assert!(tracker.tick(PlayState::Paused, 40.0, 0.0).is_none());
            assert!(tracker.tick(PlayState::Loading, 40.0, 0.0).is_none());
        }
        assert_eq!(listen(&mut tracker, 40.0, 9.0), 0);
        assert_eq!(listen(&mut tracker, 49.0, 2.0), 1);
    }

    #[test]
    fn restart_allows_another_scrobble() {
        let mut tracker = Tracker::default();
        let first = scrobble(Some(60));
        tracker.start(first.clone());
        assert_eq!(listen(&mut tracker, 0.0, 40.0), 1);
        tracker.start(first);
        assert_eq!(listen(&mut tracker, 0.0, 40.0), 1);
    }

    #[test]
    fn short_tracks_never_scrobble() {
        let mut tracker = Tracker::default();
        tracker.start(scrobble(Some(25)));
        assert_eq!(listen(&mut tracker, 0.0, 25.0), 0);
    }

    #[test]
    fn falls_back_to_player_duration() {
        let mut tracker = Tracker::default();
        tracker.start(scrobble(None));
        let mut due = 0;
        for i in 1..=700 {
            if tracker
                .tick(PlayState::Playing, f64::from(i) / 10.0, 100.0)
                .is_some()
            {
                due += 1;
            }
        }
        assert_eq!(due, 1);
    }

    #[test]
    fn idle_tracker_does_nothing() {
        let mut tracker = Tracker::default();
        assert_eq!(listen(&mut tracker, 0.0, 300.0), 0);
        tracker.start(scrobble(Some(100)));
        tracker.clear();
        assert_eq!(listen(&mut tracker, 0.0, 300.0), 0);
    }

    #[test]
    fn queue_limits_and_staleness() {
        let mut queue = Queue {
            items: VecDeque::new(),
            path: None,
            dirty: false,
        };
        for i in 0..(MAX_QUEUE as u64 + 5) {
            let mut s = scrobble(Some(100));
            s.timestamp = 1_000_000 + i;
            queue.push(s);
        }
        assert_eq!(queue.items.len(), MAX_QUEUE);
        assert_eq!(queue.items[0].timestamp, 1_000_005);

        let newest = queue.items.back().unwrap().timestamp;
        queue.drop_stale(newest + MAX_AGE_SECS - 100);
        assert_eq!(queue.items.len(), 101);
        assert_eq!(queue.front(3).len(), 3);
        queue.remove_front(1000);
        assert!(queue.is_empty());
    }

    #[test]
    fn queue_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("drift-lastfm-test-{}", std::process::id()));
        let path = dir.join("queue.json");
        let mut queue = Queue::load(Some(path.clone()));
        assert!(queue.is_empty());
        queue.push(scrobble(Some(100)));
        queue.persist().unwrap();

        let reloaded = Queue::load(Some(path.clone()));
        assert_eq!(reloaded.items, queue.items);

        fs::write(&path, "not json").unwrap();
        assert!(Queue::load(Some(path)).is_empty());
        let _ = fs::remove_dir_all(dir);
    }
}
