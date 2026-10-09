use std::collections::HashSet;
use std::sync::mpsc::Sender;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::TableState;

use crate::command::{self, Amount, Command, COMMAND_NAMES};
use crate::config::{self, Config};
use crate::model::{split_artists, RepeatMode, Track};
use crate::player::{Player, PlayerEvent};
use crate::session::Session;
use crate::store::Store;
use crate::ytmusic;

pub enum AppEvent {
    Player(PlayerEvent),
    Results {
        id: u64,
        title: String,
        result: Result<Vec<Track>, String>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Results,
    Queue,
    Playlists,
}

impl View {
    pub fn idx(self) -> usize {
        match self {
            View::Results => 0,
            View::Queue => 1,
            View::Playlists => 2,
        }
    }
    fn from_idx(i: usize) -> View {
        [View::Results, View::Queue, View::Playlists][i % 3]
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Command,
    ArtistPick,
    Help,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    Loading,
    Playing,
    Paused,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Error,
}

pub struct Status {
    pub text: String,
    pub level: Level,
    pub at: Instant,
}

pub struct App {
    pub view: View,
    pub mode: Mode,
    pub results: Vec<Track>,
    pub results_title: String,
    pub queue: Vec<Track>,
    pub current: Option<usize>,
    pub store: Store,
    pub states: [TableState; 3],
    pub open: Option<String>,
    pub pl_state: TableState,

    pub cmd_buf: String,
    pub artist_opts: Vec<String>,
    pub artist_sel: usize,
    pub help_scroll: usize,
    history: Vec<String>,
    hist_pos: Option<usize>,
    pub count: Option<usize>,
    pub pending: Option<char>,

    pub play_state: PlayState,
    pub position: f64,
    pub duration: f64,
    pub volume: u8,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    played: HashSet<usize>,
    fail_streak: u8,
    autoplay: bool,
    seek_step: f64,
    volume_step: f64,
    resume: Option<(String, f64)>,
    saved: String,
    save_failed: bool,

    pub status: Option<Status>,
    pub busy: Option<String>,
    pub tick: usize,
    pub should_quit: bool,

    search_id: u64,
    player: Option<Player>,
    tx: Sender<AppEvent>,
    rng: u64,
}

impl App {
    pub fn new(tx: Sender<AppEvent>) -> App {
        let (store, store_warning) = Store::load();
        config::migrate_legacy();
        let (cfg, cfg_warning) = Config::load();
        let p = cfg.playback;
        let session = if p.restore_session { Session::load() } else { Session::default() };
        let warning = cfg_warning.or(store_warning);
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1;
        let mut app = App {
            view: View::Results,
            mode: Mode::Normal,
            results: Vec::new(),
            results_title: "Results".into(),
            queue: session.queue,
            current: session.current,
            store,
            states: Default::default(),
            open: None,
            pl_state: TableState::default(),
            cmd_buf: String::new(),
            artist_opts: Vec::new(),
            artist_sel: 0,
            help_scroll: 0,
            history: Vec::new(),
            hist_pos: None,
            count: None,
            pending: None,
            play_state: PlayState::Stopped,
            position: 0.0,
            duration: 0.0,
            volume: p.volume,
            shuffle: p.shuffle,
            repeat: p.repeat,
            played: HashSet::new(),
            fail_streak: 0,
            autoplay: p.autoplay,
            seek_step: p.seek_step,
            volume_step: p.volume_step,
            resume: None,
            saved: String::new(),
            save_failed: false,
            status: None,
            busy: None,
            tick: 0,
            should_quit: false,
            search_id: 0,
            player: None,
            tx,
            rng: seed,
        };
        match warning {
            Some(w) => app.error(w),
            None => app.info("Welcome! Press / to search, ? for help."),
        }
        if let Some(t) = app.now_track().cloned() {
            app.position = session.position;
            app.duration = t.duration.map_or(0.0, f64::from);
            app.resume = Some((t.id, session.position));
            app.states[View::Queue.idx()].select(app.current);
            if app.autoplay {
                app.play_default();
            }
        }
        app
    }

    pub fn shutdown(&mut self) {
        if let Err(e) = self.save_session(true) {
            eprintln!("drift: could not save session: {e:#}");
        }
        self.player = None;
    }

    fn save_session(&mut self, force: bool) -> Result<()> {
        let session = Session {
            queue: self.queue.clone(),
            current: self.current,
            position: self.position,
        };
        let text = session.render()?;
        if !force && text == self.saved {
            return Ok(());
        }
        Session::write(&text)?;
        self.saved = text;
        Ok(())
    }

    pub fn info(&mut self, msg: impl Into<String>) {
        self.set_status(msg.into(), Level::Info);
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        self.set_status(msg.into(), Level::Error);
    }

    fn set_status(&mut self, text: String, level: Level) {
        self.status = Some(Status { text, level, at: Instant::now() });
    }

    pub fn visible_status(&self) -> Option<&Status> {
        self.status.as_ref().filter(|s| {
            s.at.elapsed().as_secs() < if s.level == Level::Error { 12 } else { 6 }
        })
    }

    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if self.tick % 50 == 0 {
            match self.save_session(false) {
                Ok(()) => self.save_failed = false,
                Err(e) if !self.save_failed => {
                    self.save_failed = true;
                    self.error(format!("Could not save session: {e:#}"));
                }
                Err(_) => {}
            }
        }
    }

    pub fn len_of(&self, v: View) -> usize {
        match v {
            View::Results => self.results.len(),
            View::Queue => self.queue.len(),
            View::Playlists => self.store.playlists.len(),
        }
    }

    pub fn in_playlist(&self) -> bool {
        self.view == View::Playlists
            && self.open.as_ref().is_some_and(|n| self.store.playlists.contains_key(n))
    }

    pub fn open_len(&self) -> usize {
        self.open.as_ref().and_then(|n| self.store.playlists.get(n)).map_or(0, |t| t.len())
    }

    fn cur_len(&self) -> usize {
        if self.in_playlist() {
            self.open_len()
        } else {
            self.len_of(self.view)
        }
    }

    pub fn selected(&self) -> usize {
        if self.in_playlist() {
            return self.pl_state.selected().unwrap_or(0);
        }
        self.states[self.view.idx()].selected().unwrap_or(0)
    }

    fn set_selected(&mut self, i: usize) {
        let len = self.cur_len();
        let new = if len == 0 { None } else { Some(i.min(len - 1)) };
        if self.in_playlist() {
            self.pl_state.select(new);
        } else {
            let idx = self.view.idx();
            self.states[idx].select(new);
        }
    }

    fn move_sel(&mut self, delta: isize) {
        let len = self.cur_len();
        if len == 0 {
            return;
        }
        let new = (self.selected() as isize + delta).clamp(0, len as isize - 1);
        self.set_selected(new as usize);
    }

    fn set_view(&mut self, v: View) {
        if v != View::Playlists {
            self.open = None;
        }
        self.view = v;
        let sel = self.selected();
        self.set_selected(sel);
    }

    pub fn playlist_name(&self, i: usize) -> Option<String> {
        self.store.playlists.keys().nth(i).cloned()
    }

    pub fn now_track(&self) -> Option<&Track> {
        self.current.and_then(|i| self.queue.get(i))
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        match self.mode {
            Mode::Help => self.key_help(key),
            Mode::Command => self.key_command(key),
            Mode::ArtistPick => self.key_artist_pick(key),
            Mode::Normal => self.key_normal(key),
        }
    }

    fn key_normal(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if let Some(p) = self.pending.take() {
            let n = self.count.take();
            if let KeyCode::Char(c) = key.code {
                match (p, c) {
                    ('g', 'g') => self.set_selected(n.map_or(0, |n| n.saturating_sub(1))),
                    ('g', 'r') => self.set_view(View::Results),
                    ('g', 'q') => self.set_view(View::Queue),
                    ('g', 'p') => self.set_view(View::Playlists),
                    ('d', 'd') => self.delete_selected(),
                    ('g', 'a') => self.go_to_artist(),
                    _ => {}
                }
            }
            return;
        }

        if let KeyCode::Char(c @ '0'..='9') = key.code {
            if !ctrl && !(c == '0' && self.count.is_none()) {
                let d = c as usize - '0' as usize;
                self.count = Some((self.count.unwrap_or(0) * 10 + d).min(9999));
                return;
            }
        }
        let n = self.count.take();
        let times = n.unwrap_or(1);

        if self.view == View::Playlists {
            match key.code {
                KeyCode::Right | KeyCode::Char('l') if !self.in_playlist() && !ctrl => {
                    return self.open_selected_playlist();
                }
                KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace | KeyCode::Esc
                    if self.in_playlist() && !ctrl =>
                {
                    return self.close_playlist();
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Char('d') if ctrl => self.move_sel(10),
            KeyCode::Char('u') if ctrl => self.move_sel(-10),
            KeyCode::Char('f') if ctrl => self.move_sel(20),
            KeyCode::Char('b') if ctrl => self.move_sel(-20),
            KeyCode::PageDown => self.move_sel(20),
            KeyCode::PageUp => self.move_sel(-20),
            KeyCode::Home => self.set_selected(0),
            KeyCode::End => self.set_selected(usize::MAX),

            KeyCode::Char('j') | KeyCode::Down => self.move_sel(times as isize),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-(times as isize)),
            KeyCode::Char('g') => {
                self.pending = Some('g');
                self.count = n;
            }
            KeyCode::Char('G') => self.set_selected(n.map_or(usize::MAX, |n| n.saturating_sub(1))),
            KeyCode::Char('d') => {
                if matches!(self.view, View::Queue | View::Playlists) {
                    self.pending = Some('d');
                }
            }
            KeyCode::Char('x') | KeyCode::Delete => self.delete_selected(),

            KeyCode::Tab => self.set_view(View::from_idx(self.view.idx() + 1)),
            KeyCode::BackTab => self.set_view(View::from_idx(self.view.idx() + 2)),

            KeyCode::Enter => self.activate(),
            KeyCode::Char(' ') => self.toggle_pause(),
            KeyCode::Char('n') | KeyCode::Char('>') => self.next(),
            KeyCode::Char('N') | KeyCode::Char('<') => self.prev(),
            KeyCode::Char('h') | KeyCode::Left => self.seek(Amount::Delta(-self.seek_step * times as f64)),
            KeyCode::Char('l') | KeyCode::Right => self.seek(Amount::Delta(self.seek_step * times as f64)),
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.set_volume(Amount::Delta(self.volume_step * times as f64))
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                self.set_volume(Amount::Delta(-self.volume_step * times as f64))
            }

            KeyCode::Char('a') => self.add_selected(),
            KeyCode::Char('A') => self.add_all_results(),
            KeyCode::Char('P') => self.play_all_results(),
            KeyCode::Char('J') => self.move_item(1),
            KeyCode::Char('K') => self.move_item(-1),
            KeyCode::Char('s') => self.toggle_shuffle(),
            KeyCode::Char('r') => self.cycle_repeat(),

            KeyCode::Char('/') => {
                self.cmd_buf = "search ".into();
                self.enter_command_mode();
            }
            KeyCode::Char(':') => {
                self.cmd_buf.clear();
                self.enter_command_mode();
            }
            KeyCode::Char('?') => self.open_help(),
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => self.status = None,
            _ => {}
        }
    }

    fn open_help(&mut self) {
        self.help_scroll = 0;
        self.mode = Mode::Help;
    }

    fn key_help(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let s = self.help_scroll;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::Enter => {
                self.mode = Mode::Normal
            }
            KeyCode::Char('j') | KeyCode::Down => self.help_scroll = s.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => self.help_scroll = s.saturating_sub(1),
            KeyCode::Char('d') if ctrl => self.help_scroll = s.saturating_add(10),
            KeyCode::Char('u') if ctrl => self.help_scroll = s.saturating_sub(10),
            KeyCode::PageDown | KeyCode::Char(' ') => self.help_scroll = s.saturating_add(10),
            KeyCode::PageUp => self.help_scroll = s.saturating_sub(10),
            KeyCode::Char('g') | KeyCode::Home => self.help_scroll = 0,
            KeyCode::Char('G') | KeyCode::End => self.help_scroll = usize::MAX,
            _ => {}
        }
    }

    fn enter_command_mode(&mut self) {
        self.mode = Mode::Command;
        self.hist_pos = None;
    }

    fn key_command(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.cmd_buf.clear();
            }
            KeyCode::Enter => {
                let line = std::mem::take(&mut self.cmd_buf);
                self.mode = Mode::Normal;
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    if self.history.last().map(String::as_str) != Some(trimmed) {
                        self.history.push(trimmed.to_string());
                    }
                    self.run_command(trimmed);
                }
            }
            KeyCode::Backspace => {
                if self.cmd_buf.is_empty() {
                    self.mode = Mode::Normal;
                } else {
                    self.cmd_buf.pop();
                }
            }
            KeyCode::Char('u') if ctrl => self.cmd_buf.clear(),
            KeyCode::Char('w') if ctrl => {
                let trimmed = self.cmd_buf.trim_end().len();
                let cut = self.cmd_buf[..trimmed].rfind(' ').map_or(0, |i| i + 1);
                self.cmd_buf.truncate(cut);
            }
            KeyCode::Up => self.history_step(-1),
            KeyCode::Down => self.history_step(1),
            KeyCode::Tab => self.complete(),
            KeyCode::Char(c) if !ctrl => self.cmd_buf.push(c),
            _ => {}
        }
    }

    fn history_step(&mut self, dir: isize) {
        if self.history.is_empty() {
            return;
        }
        let len = self.history.len() as isize;
        let pos = match (self.hist_pos, dir) {
            (None, -1) => len - 1,
            (None, _) => return,
            (Some(p), d) => p as isize + d,
        };
        if pos >= len {
            self.hist_pos = None;
            self.cmd_buf.clear();
        } else {
            let pos = pos.max(0) as usize;
            self.hist_pos = Some(pos);
            self.cmd_buf = self.history[pos].clone();
        }
    }

    fn complete(&mut self) {
        if self.cmd_buf.contains(' ') {
            return;
        }
        let matches: Vec<&&str> = COMMAND_NAMES
            .iter()
            .filter(|c| c.starts_with(self.cmd_buf.as_str()))
            .collect();
        match matches.len() {
            0 => {}
            1 => self.cmd_buf = format!("{} ", matches[0]),
            _ => {
                let list = matches.iter().map(|s| **s).collect::<Vec<_>>().join(" ");
                self.info(list);
            }
        }
    }

    pub fn run_command(&mut self, line: &str) {
        match command::parse(line) {
            Ok(Some(cmd)) => self.exec(cmd),
            Ok(None) => {}
            Err(e) => self.error(e),
        }
    }

    fn exec(&mut self, cmd: Command) {
        match cmd {
            Command::Search(q) => self.start_search(q),
            Command::Play(None) => self.play_default(),
            Command::Play(Some(n)) => {
                let len = self.len_of(self.view);
                if n > len {
                    self.error(format!("no row {n} (only {len} here)"));
                } else {
                    let v = self.view;
                    self.activate_at(v, n - 1);
                }
            }
            Command::Pause => self.set_pause(true),
            Command::Resume => self.set_pause(false),
            Command::Toggle => self.toggle_pause(),
            Command::Stop => self.stop_playback(),
            Command::Next => self.next(),
            Command::Prev => self.prev(),
            Command::Quit => self.should_quit = true,
            Command::Add(None) => self.add_selected(),
            Command::Add(Some(n)) => match self.results.get(n - 1).cloned() {
                Some(t) => self.enqueue(t),
                None => self.error(format!("no result #{n}")),
            },
            Command::AddAll => self.add_all_results(),
            Command::PlayAll => self.play_all_results(),
            Command::Remove(n) => {
                let i = n.map_or(self.selected(), |n| n - 1);
                self.remove_from_queue(i);
            }
            Command::Clear => self.clear_queue(),
            Command::ShowResults => self.set_view(View::Results),
            Command::ShowQueue => self.set_view(View::Queue),
            Command::ShowPlaylists => self.set_view(View::Playlists),
            Command::Volume(a) => self.set_volume(a),
            Command::Seek(a) => self.seek(a),
            Command::Shuffle => self.toggle_shuffle(),
            Command::Repeat(m) => {
                self.repeat = m.unwrap_or_else(|| self.repeat.cycle());
                self.info(format!("Repeat: {}", self.repeat.label()));
            }
            Command::Save(name) => self.save_playlist(&name),
            Command::Load(name) => match self.store.resolve(&name) {
                Some(n) => self.load_playlist(&n, true),
                None => self.error(format!("no playlist named '{name}'")),
            },
            Command::Append(name) => match self.store.resolve(&name) {
                Some(n) => self.load_playlist(&n, false),
                None => self.error(format!("no playlist named '{name}'")),
            },
            Command::DeletePlaylist(name) => match self.store.resolve(&name) {
                Some(n) => self.delete_playlist(&n),
                None => self.error(format!("no playlist named '{name}'")),
            },
            Command::AddTo(name) => self.add_to_playlist(&name),
            Command::Import(url) => self.start_import(url),
            Command::Artist => self.go_to_artist(),
            Command::Help => self.open_help(),
        }
    }

    fn artist_target(&self) -> Option<Track> {
        let sel = self.selected();
        let highlighted = match self.view {
            View::Results => self.results.get(sel),
            View::Queue => self.queue.get(sel),
            View::Playlists if self.in_playlist() => self
                .open
                .as_ref()
                .and_then(|n| self.store.playlists.get(n))
                .and_then(|t| t.get(sel)),
            View::Playlists => None,
        };
        highlighted.or_else(|| self.now_track()).cloned()
    }

    fn go_to_artist(&mut self) {
        let Some(track) = self.artist_target() else {
            return self.error("No song selected");
        };
        let names = split_artists(&track.artist);
        if names.is_empty() || track.artist == "Unknown artist" {
            return self.error("No artist known for this song");
        }
        if names.len() == 1 {
            return self.search_artist(names[0].clone());
        }
        let mut opts = names;
        opts.push(track.artist.trim().to_string());
        self.artist_opts = opts;
        self.artist_sel = 0;
        self.mode = Mode::ArtistPick;
    }

    fn key_artist_pick(&mut self, key: KeyEvent) {
        let len = self.artist_opts.len();
        if len == 0 {
            self.mode = Mode::Normal;
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.close_artist_pick(),
            KeyCode::Char('j') | KeyCode::Down => self.artist_sel = (self.artist_sel + 1).min(len - 1),
            KeyCode::Char('k') | KeyCode::Up => self.artist_sel = self.artist_sel.saturating_sub(1),
            KeyCode::Char(c @ '1'..='9') => {
                let i = c as usize - '1' as usize;
                if i < len {
                    self.artist_sel = i;
                    self.confirm_artist();
                }
            }
            KeyCode::Enter => self.confirm_artist(),
            _ => {}
        }
    }

    fn close_artist_pick(&mut self) {
        self.mode = Mode::Normal;
        self.artist_opts.clear();
        self.artist_sel = 0;
    }

    fn confirm_artist(&mut self) {
        let name = self.artist_opts.get(self.artist_sel).cloned();
        self.close_artist_pick();
        if let Some(name) = name {
            self.search_artist(name);
        }
    }

    fn search_artist(&mut self, name: String) {
        let busy = format!("Loading songs by \u{201c}{name}\u{201d}");
        let title = format!("Artist: {name}");
        self.spawn_loader(title, busy, move || {
            let wanted = name.to_lowercase();
            let tracks = ytmusic::search(&name)?;
            Ok(tracks
                .into_iter()
                .filter(|t| {
                    t.artist.to_lowercase() == wanted
                        || split_artists(&t.artist).iter().any(|a| a.to_lowercase() == wanted)
                })
                .collect())
        });
    }

    fn start_search(&mut self, query: String) {
        self.spawn_loader(format!("Search: {query}"), format!("Searching \u{201c}{query}\u{201d}"), move || {
            ytmusic::search(&query)
        });
    }

    fn start_import(&mut self, url: String) {
        self.spawn_loader("Imported playlist".into(), "Importing playlist".into(), move || {
            ytmusic::import(&url)
        });
    }

    fn spawn_loader<F>(&mut self, title: String, busy: String, work: F)
    where
        F: FnOnce() -> Result<Vec<Track>, String> + Send + 'static,
    {
        self.search_id += 1;
        let id = self.search_id;
        self.busy = Some(busy);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = work();
            let _ = tx.send(AppEvent::Results { id, title, result });
        });
    }

    pub fn on_event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Results { id, title, result } => {
                if id != self.search_id {
                    return;
                }
                self.busy = None;
                match result {
                    Ok(tracks) if tracks.is_empty() => self.info("No results found."),
                    Ok(tracks) => {
                        let n = tracks.len();
                        self.results = tracks;
                        self.results_title = title;
                        self.states[View::Results.idx()].select(Some(0));
                        self.set_view(View::Results);
                        self.info(format!("{n} results — Enter plays, a queues, ? for help"));
                    }
                    Err(e) => self.error(e),
                }
            }
            AppEvent::Player(pe) => self.on_player(pe),
        }
    }

    fn ensure_player(&mut self) -> Result<&mut Player> {
        if let Some(p) = self.player.as_mut() {
            if !p.is_alive() {
                self.player = None;
            }
        }
        if self.player.is_none() {
            let tx = self.tx.clone();
            let p = Player::spawn(self.volume, move |ev| {
                let _ = tx.send(AppEvent::Player(ev));
            })?;
            self.player = Some(p);
        }
        Ok(self.player.as_mut().expect("player was just created"))
    }

    fn with_player(&mut self, f: impl Fn(&mut Player) -> Result<()>) -> Result<()> {
        match self.ensure_player().and_then(|p| f(p)) {
            Ok(()) => Ok(()),
            Err(_) => {
                self.player = None;
                self.ensure_player().and_then(|p| f(p))
            }
        }
    }

    fn on_player(&mut self, ev: PlayerEvent) {
        match ev {
            PlayerEvent::Position(Some(p)) => self.position = p,
            PlayerEvent::Position(None) => {}
            PlayerEvent::Duration(Some(d)) => self.duration = d,
            PlayerEvent::Duration(None) => {}
            PlayerEvent::Paused(p) => match (self.play_state, p) {
                (PlayState::Playing, true) => self.play_state = PlayState::Paused,
                (PlayState::Paused, false) => self.play_state = PlayState::Playing,
                _ => {}
            },
            PlayerEvent::Volume(v) => self.volume = v.round().clamp(0.0, 130.0) as u8,
            PlayerEvent::FileLoaded => {
                self.play_state = PlayState::Playing;
                self.fail_streak = 0;
                if let Some((id, pos)) = self.resume.take() {
                    let ok = self.now_track().is_some_and(|t| {
                        t.id == id && t.duration.map_or(true, |d| pos < f64::from(d) - 1.0)
                    });
                    if ok && pos > 1.0 {
                        let _ = self.with_player(|p| p.seek_absolute(pos));
                        self.position = pos;
                    }
                }
            }
            PlayerEvent::EndOfFile => {
                if self.play_state != PlayState::Stopped {
                    self.advance(true);
                }
            }
            PlayerEvent::LoadError(msg) => {
                let name = self.now_track().map(Track::label).unwrap_or_default();
                self.fail_streak += 1;
                if self.fail_streak >= 3 {
                    self.stop_playback();
                    self.error(format!(
                        "Playback failed 3 times in a row ({msg}). Update yt-dlp: yt-dlp -U"
                    ));
                } else {
                    self.error(format!("Can't play \u{201c}{name}\u{201d} ({msg}) — skipping"));
                    self.advance(true);
                }
            }
            PlayerEvent::Died => {
                self.player = None;
                if self.play_state != PlayState::Stopped {
                    self.play_state = PlayState::Stopped;
                    self.error("mpv exited unexpectedly; it will restart on next play");
                }
            }
        }
    }

    fn play_index(&mut self, i: usize) {
        self.start_track(i, false);
    }

    fn start_track(&mut self, i: usize, keep_resume: bool) {
        let Some(track) = self.queue.get(i).cloned() else { return };
        if !keep_resume {
            self.resume = None;
        }
        self.current = Some(i);
        self.played.insert(i);
        self.play_state = PlayState::Loading;
        self.position = match &self.resume {
            Some((id, pos)) if *id == track.id => *pos,
            _ => 0.0,
        };
        self.duration = track.duration.map_or(0.0, f64::from);
        let url = track.url();
        match self.with_player(|p| p.load(&url)) {
            Ok(()) => self.info(format!("♪ {}", track.label())),
            Err(e) => {
                self.play_state = PlayState::Stopped;
                self.error(format!("Playback failed: {e:#}"));
            }
        }
    }

    fn play_default(&mut self) {
        match self.play_state {
            PlayState::Paused => return self.set_pause(false),
            PlayState::Playing | PlayState::Loading => {
                return self.info("Already playing (use :next, :pause, or :play N)");
            }
            PlayState::Stopped => {}
        }
        if self.queue.is_empty() {
            match self.view {
                View::Results if !self.results.is_empty() => {
                    let t = self.results[self.selected().min(self.results.len() - 1)].clone();
                    self.play_now(t);
                }
                View::Playlists if !self.store.playlists.is_empty() => {
                    let i = self.selected();
                    self.activate_at(View::Playlists, i);
                }
                _ => self.error("Nothing to play — search first with :search \"query\""),
            }
            return;
        }
        let i = if self.view == View::Queue {
            self.selected()
        } else {
            self.current.unwrap_or(0)
        };
        self.start_track(i.min(self.queue.len() - 1), true);
    }

    fn activate(&mut self) {
        let (v, i) = (self.view, self.selected());
        self.activate_at(v, i);
    }

    fn activate_at(&mut self, view: View, i: usize) {
        match view {
            View::Results => match self.results.get(i).cloned() {
                Some(t) => self.play_now(t),
                None => {}
            },
            View::Queue => {
                if i < self.queue.len() {
                    self.play_index(i);
                }
            }
            View::Playlists => {
                if self.in_playlist() {
                    if let Some(name) = self.open.clone() {
                        self.play_playlist_from(&name, i);
                    }
                } else if let Some(name) = self.playlist_name(i) {
                    self.load_playlist(&name, true);
                }
            }
        }
    }

    fn play_now(&mut self, t: Track) {
        if let Some(k) = self.queue.iter().position(|q| q.id == t.id) {
            return self.play_index(k);
        }
        let at = self.current.map_or(self.queue.len(), |c| (c + 1).min(self.queue.len()));
        self.queue.insert(at, t);
        self.played.clear();
        self.play_index(at);
    }

    fn set_pause(&mut self, pause: bool) {
        match self.play_state {
            PlayState::Stopped => return self.error("Nothing is playing"),
            PlayState::Loading => return,
            _ => {}
        }
        match self.with_player(|p| p.set_pause(pause)) {
            Ok(()) => {
                self.play_state = if pause { PlayState::Paused } else { PlayState::Playing };
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn toggle_pause(&mut self) {
        match self.play_state {
            PlayState::Playing => self.set_pause(true),
            PlayState::Paused => self.set_pause(false),
            PlayState::Loading => {}
            PlayState::Stopped => self.play_default(),
        }
    }

    fn stop_playback(&mut self) {
        if let Some(p) = self.player.as_mut() {
            let _ = p.stop();
        }
        self.play_state = PlayState::Stopped;
        self.position = 0.0;
    }

    fn next(&mut self) {
        if self.queue.is_empty() {
            return self.error("Queue is empty");
        }
        self.advance(false);
    }

    fn prev(&mut self) {
        let Some(c) = self.current else {
            return self.error("Nothing is playing");
        };
        if self.position > 3.0 {
            return self.seek(Amount::Abs(0.0));
        }
        if c > 0 {
            self.play_index(c - 1);
        } else if self.repeat == RepeatMode::All && !self.queue.is_empty() {
            self.play_index(self.queue.len() - 1);
        } else {
            self.seek(Amount::Abs(0.0));
        }
    }

    fn rand_below(&mut self, n: usize) -> usize {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as usize % n.max(1)
    }

    fn advance(&mut self, auto: bool) {
        let len = self.queue.len();
        if len == 0 {
            return self.stop_playback();
        }
        if auto && self.repeat == RepeatMode::One {
            if let Some(c) = self.current {
                return self.play_index(c);
            }
        }
        let next = if self.shuffle {
            let mut pool: Vec<usize> = (0..len).filter(|i| !self.played.contains(i)).collect();
            if pool.is_empty() && self.repeat == RepeatMode::All {
                self.played.clear();
                pool = (0..len).filter(|i| Some(*i) != self.current).collect();
                if pool.is_empty() {
                    pool = vec![0];
                }
            }
            if pool.is_empty() {
                None
            } else {
                let k = self.rand_below(pool.len());
                Some(pool[k])
            }
        } else {
            match self.current {
                Some(c) if c + 1 < len => Some(c + 1),
                Some(_) if self.repeat == RepeatMode::All => Some(0),
                Some(_) => None,
                None => Some(0),
            }
        };
        let next = next.or_else(|| self.next_from_results());
        match next {
            Some(i) => self.play_index(i),
            None if auto => {
                self.stop_playback();
                self.info("Queue finished");
            }
            None => self.info("End of queue"),
        }
    }

    fn next_from_results(&mut self) -> Option<usize> {
        let cur_id = self.now_track()?.id.clone();
        let r = self.results.iter().position(|t| t.id == cur_id)?;
        let next = self.results[r + 1..]
            .iter()
            .find(|t| !self.queue.iter().any(|q| q.id == t.id))?
            .clone();
        self.queue.push(next);
        Some(self.queue.len() - 1)
    }

    fn seek(&mut self, amount: Amount) {
        if !matches!(self.play_state, PlayState::Playing | PlayState::Paused) {
            return self.error("Nothing is playing");
        }
        let r = match amount {
            Amount::Delta(d) => self.with_player(|p| p.seek_relative(d)),
            Amount::Abs(a) => self.with_player(|p| p.seek_absolute(a)),
        };
        match r {
            Ok(()) => {
                self.position = match amount {
                    Amount::Delta(d) => (self.position + d).max(0.0),
                    Amount::Abs(a) => a,
                };
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn set_volume(&mut self, amount: Amount) {
        let v = match amount {
            Amount::Abs(a) => a,
            Amount::Delta(d) => self.volume as f64 + d,
        }
        .round()
        .clamp(0.0, 100.0) as u8;
        self.volume = v;
        if let Some(p) = self.player.as_mut() {
            if let Err(e) = p.set_volume(v) {
                return self.error(format!("{e:#}"));
            }
        }
        self.info(format!("Volume: {v}%"));
    }

    fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        self.played.clear();
        if let Some(c) = self.current {
            self.played.insert(c);
        }
        self.info(format!("Shuffle: {}", if self.shuffle { "on" } else { "off" }));
    }

    fn cycle_repeat(&mut self) {
        self.repeat = self.repeat.cycle();
        self.info(format!("Repeat: {}", self.repeat.label()));
    }

    fn enqueue(&mut self, t: Track) {
        let label = t.label();
        self.queue.push(t);
        self.info(format!("Queued: {label}"));
    }

    fn add_selected(&mut self) {
        let i = self.selected();
        match self.view {
            View::Results => match self.results.get(i).cloned() {
                Some(t) => self.enqueue(t),
                None => self.error("No result selected"),
            },
            View::Playlists if self.in_playlist() => {
                let track = self
                    .open
                    .as_ref()
                    .and_then(|n| self.store.playlists.get(n))
                    .and_then(|l| l.get(i))
                    .cloned();
                match track {
                    Some(t) => self.enqueue(t),
                    None => self.error("No track selected"),
                }
            }
            View::Playlists => match self.playlist_name(i) {
                Some(n) => self.load_playlist(&n, false),
                None => self.error("No playlist selected"),
            },
            View::Queue => self.info("Already in the queue"),
        }
    }

    fn add_all_results(&mut self) {
        if self.results.is_empty() {
            return self.error("No results to add");
        }
        let n = self.results.len();
        self.queue.extend(self.results.iter().cloned());
        self.info(format!("Queued {n} tracks"));
    }

    fn play_all_results(&mut self) {
        if self.results.is_empty() {
            return self.error("No results to play");
        }
        let start = if self.view == View::Results { self.selected() } else { 0 };
        self.queue = self.results.clone();
        self.played.clear();
        self.current = None;
        self.play_index(start.min(self.queue.len() - 1));
    }

    fn clear_queue(&mut self) {
        self.queue.clear();
        self.current = None;
        self.played.clear();
        self.stop_playback();
        self.states[View::Queue.idx()].select(None);
        self.info("Queue cleared");
    }

    fn remove_from_queue(&mut self, i: usize) {
        if i >= self.queue.len() {
            return self.error("No such queue entry");
        }
        let removed = self.queue.remove(i);
        self.played.clear();
        let active = self.play_state != PlayState::Stopped;
        match self.current {
            Some(c) if c == i => {
                if self.queue.is_empty() {
                    self.current = None;
                    self.stop_playback();
                } else if active {
                    self.play_index(i.min(self.queue.len() - 1));
                } else {
                    self.current = Some(i.min(self.queue.len() - 1));
                }
            }
            Some(c) if c > i => self.current = Some(c - 1),
            _ => {}
        }
        let sel = self.states[View::Queue.idx()].selected().unwrap_or(0);
        let len = self.queue.len();
        self.states[View::Queue.idx()].select(if len == 0 { None } else { Some(sel.min(len - 1)) });
        self.info(format!("Removed: {}", removed.label()));
    }

    fn move_queue_item(&mut self, dir: isize) {
        if self.view != View::Queue {
            return;
        }
        let i = self.selected();
        let j = i as isize + dir;
        if i >= self.queue.len() || j < 0 || j as usize >= self.queue.len() {
            return;
        }
        let j = j as usize;
        self.queue.swap(i, j);
        self.played.clear();
        if self.current == Some(i) {
            self.current = Some(j);
        } else if self.current == Some(j) {
            self.current = Some(i);
        }
        self.set_selected(j);
    }

    fn delete_selected(&mut self) {
        let i = self.selected();
        match self.view {
            View::Queue => self.remove_from_queue(i),
            View::Playlists if self.in_playlist() => self.remove_playlist_track(i),
            View::Playlists => match self.playlist_name(i) {
                Some(n) => self.delete_playlist(&n),
                None => {}
            },
            View::Results => self.info("Results can't be deleted — they are replaced by the next search"),
        }
    }

    fn persist(&mut self) -> bool {
        match self.store.save() {
            Ok(()) => true,
            Err(e) => {
                self.error(format!("Could not save playlists: {e:#}"));
                false
            }
        }
    }

    fn save_playlist(&mut self, name: &str) {
        if self.queue.is_empty() {
            return self.error("Queue is empty — nothing to save");
        }
        let n = self.queue.len();
        self.store.playlists.insert(name.to_string(), self.queue.clone());
        if self.persist() {
            self.info(format!("Saved {n} tracks to playlist \u{201c}{name}\u{201d}"));
        }
    }

    fn add_to_playlist(&mut self, name: &str) {
        let i = self.selected();
        let track = match self.view {
            View::Results => self.results.get(i).cloned(),
            View::Queue => self.queue.get(i).cloned(),
            View::Playlists => self
                .open
                .as_ref()
                .and_then(|n| self.store.playlists.get(n))
                .and_then(|l| l.get(i))
                .cloned(),
        };
        let Some(track) = track else {
            return self.error("Select a track in Results or Queue first");
        };
        let key = self.store.resolve(name).unwrap_or_else(|| name.to_string());
        let list = self.store.playlists.entry(key.clone()).or_default();
        if list.iter().any(|t| t.id == track.id) {
            return self.info(format!("Already in \u{201c}{key}\u{201d}"));
        }
        let label = track.label();
        list.push(track);
        if self.persist() {
            self.info(format!("Added {label} to \u{201c}{key}\u{201d}"));
        }
    }

    fn load_playlist(&mut self, name: &str, replace: bool) {
        let Some(tracks) = self.store.playlists.get(name).cloned() else {
            return self.error(format!("no playlist named '{name}'"));
        };
        if tracks.is_empty() {
            return self.error(format!("Playlist \u{201c}{name}\u{201d} is empty"));
        }
        let n = tracks.len();
        if replace {
            self.queue = tracks;
            self.played.clear();
            self.current = None;
            self.states[View::Queue.idx()].select(Some(0));
            self.play_index(0);
            self.info(format!("Playing playlist \u{201c}{name}\u{201d} ({n} tracks)"));
        } else {
            self.queue.extend(tracks);
            self.info(format!("Appended \u{201c}{name}\u{201d} ({n} tracks) to the queue"));
        }
    }

    fn open_selected_playlist(&mut self) {
        let Some(name) = self.playlist_name(self.selected()) else {
            return;
        };
        let len = self.store.playlists.get(&name).map_or(0, |l| l.len());
        self.open = Some(name);
        self.pl_state.select(if len == 0 { None } else { Some(0) });
    }

    fn close_playlist(&mut self) {
        self.open = None;
    }

    fn play_playlist_from(&mut self, name: &str, i: usize) {
        let Some(tracks) = self.store.playlists.get(name).cloned() else {
            return self.error(format!("no playlist named '{name}'"));
        };
        if tracks.is_empty() {
            return self.error(format!("Playlist \u{201c}{name}\u{201d} is empty"));
        }
        let i = i.min(tracks.len() - 1);
        self.queue = tracks;
        self.played.clear();
        self.current = None;
        self.states[View::Queue.idx()].select(Some(i));
        self.play_index(i);
    }

    fn remove_playlist_track(&mut self, i: usize) {
        let Some(name) = self.open.clone() else { return };
        let removed = {
            let Some(list) = self.store.playlists.get_mut(&name) else { return };
            if i >= list.len() {
                None
            } else {
                let t = list.remove(i);
                Some((t, list.len()))
            }
        };
        let Some((track, len)) = removed else {
            return self.error("No track selected");
        };
        self.pl_state.select(if len == 0 { None } else { Some(i.min(len - 1)) });
        if self.persist() {
            self.info(format!("Removed from \u{201c}{name}\u{201d}: {}", track.label()));
        }
    }

    fn move_item(&mut self, dir: isize) {
        if self.in_playlist() {
            return self.move_playlist_track(dir);
        }
        self.move_queue_item(dir);
    }

    fn move_playlist_track(&mut self, dir: isize) {
        let Some(name) = self.open.clone() else { return };
        let i = self.selected();
        let j = i as isize + dir;
        let moved = match self.store.playlists.get_mut(&name) {
            Some(list) if j >= 0 && (j as usize) < list.len() && i < list.len() => {
                list.swap(i, j as usize);
                true
            }
            _ => false,
        };
        if moved {
            self.set_selected(j as usize);
            self.persist();
        }
    }

    fn delete_playlist(&mut self, name: &str) {
        if self.store.playlists.remove(name).is_some() && self.persist() {
            if self.open.as_deref() == Some(name) {
                self.open = None;
            }
            self.info(format!("Deleted playlist \u{201c}{name}\u{201d}"));
            let sel = self.selected();
            self.set_selected(sel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn track(id: &str, title: &str) -> Track {
        Track {
            id: id.into(),
            title: title.into(),
            artist: "Artist".into(),
            album: None,
            duration: Some(100),
        }
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn check_artist_popup(app: &mut App) {
        let mut t = track("zzzzzzzzzzz", "Duet");
        t.artist = "A & B".into();
        app.results = vec![t];
        app.states[View::Results.idx()].select(Some(0));
        app.view = View::Results;

        app.go_to_artist();
        assert!(app.mode == Mode::ArtistPick);
        assert_eq!(app.artist_opts, ["A", "B", "A & B"]);
        press(app, KeyCode::Down);
        assert_eq!(app.artist_sel, 1);
        press(app, KeyCode::Esc);
        assert!(app.mode == Mode::Normal);
        assert!(app.artist_opts.is_empty());
    }

    #[test]
    fn playlist_drill_in() {
        let dir = std::env::temp_dir().join(format!("drift-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        std::env::set_var("XDG_STATE_HOME", &dir);

        let (tx, _rx) = mpsc::channel();
        let mut app = App::new(tx);
        app.store.playlists.insert(
            "mix".into(),
            vec![track("aaaaaaaaaaa", "A"), track("bbbbbbbbbbb", "B"), track("ccccccccccc", "C")],
        );
        app.store.playlists.insert("other".into(), vec![track("ddddddddddd", "D")]);

        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.view, View::Playlists);
        assert!(!app.in_playlist());

        press(&mut app, KeyCode::Right);
        assert!(app.in_playlist());
        assert_eq!(app.open.as_deref(), Some("mix"));
        assert_eq!(app.selected(), 0);

        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected(), 2);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected(), 2);

        press(&mut app, KeyCode::Char('K'));
        let ids: Vec<&str> = app.store.playlists["mix"].iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["aaaaaaaaaaa", "ccccccccccc", "bbbbbbbbbbb"]);
        assert_eq!(app.selected(), 1);

        press(&mut app, KeyCode::Char('x'));
        let ids: Vec<&str> = app.store.playlists["mix"].iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["aaaaaaaaaaa", "bbbbbbbbbbb"]);
        assert_eq!(app.selected(), 1);

        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.store.playlists["mix"].len(), 1);
        assert_eq!(app.selected(), 0);

        assert_eq!(app.store.playlists.len(), 2);
        let saved = std::fs::read_to_string(dir.join("drift").join("playlists.json")).unwrap();
        assert!(saved.contains("aaaaaaaaaaa") && !saved.contains("bbbbbbbbbbb"));

        press(&mut app, KeyCode::Char('a'));
        assert_eq!(app.queue.len(), 1);
        press(&mut app, KeyCode::Left);
        assert!(!app.in_playlist());
        assert_eq!(app.open, None);
        assert_eq!(app.store.playlists.len(), 2);

        press(&mut app, KeyCode::Right);
        assert!(app.in_playlist());
        press(&mut app, KeyCode::Esc);
        assert!(!app.in_playlist());

        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.open, None);

        check_artist_popup(&mut app);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
