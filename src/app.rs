use std::collections::HashSet;
use std::sync::mpsc::Sender;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::TableState;

use crate::command::{self, Amount, Command, COMMAND_NAMES};
use crate::config::{self, Config};
use crate::lyrics::{self, Lyrics};
use crate::model::{split_artists, Collection, RepeatMode, Track};
use crate::discord::Discord;
use crate::mpris::{Cmd, Mpris};
use crate::player::{self, Player, PlayerEvent};
use crate::session::Session;
use crate::store::Store;
use crate::ytmusic::{self, Kind, Page};

pub enum AppEvent {
    Player(u64, PlayerEvent),
    Results {
        id: u64,
        title: String,
        result: Result<Page, String>,
    },
    More {
        generation: u64,
        result: Result<Page, String>,
    },
    Lyrics {
        id: String,
        result: Result<Option<Lyrics>, String>,
    },
    Collections {
        id: u64,
        title: String,
        result: Result<Vec<Collection>, String>,
    },
    SavePlaylist {
        name: String,
        result: Result<Vec<Track>, String>,
    },
    Media(Cmd),
    Audio {
        id: String,
        result: Result<std::path::PathBuf, String>,
    },
}

const FIRST_PAGE: usize = 20;
const NEXT_PAGE: usize = 10;
const LOAD_AHEAD: usize = 3;

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
    CollectionPick,
    ConfirmDelete,
    Help,
    Menu,
    Info,
    Lyrics,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    Loading,
    Playing,
    Paused,
}

pub enum LyricsState {
    Idle,
    Loading,
    Ready(Lyrics),
    Missing,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Error,
}

/// Hotkey value meaning "this menu entry has no hotkey".
pub const NO_KEY: char = '\0';

#[derive(Clone, PartialEq, Eq)]
pub enum MenuAction {
    Play,
    PlayNext,
    Queue,
    Artist,
    Similar,
    Album,
    SaveTrackAlbum,
    SaveAlbum,
    SaveResults,
    PlaylistMenu,
    PickPlaylist(String),
    NewPlaylist,
    Remove,
    Info,
    QueueAll,
    PlayAll,
    ClearQueue,
    SaveQueue,
    OpenPlaylist,
    AppendPlaylist,
    DeletePlaylist,
}

#[derive(Clone)]
pub struct MenuItem {
    pub key: char,
    pub label: String,
    pub detail: String,
    pub action: MenuAction,
}

/// Ctrl held without Alt. Windows reports AltGr (needed for `@ \\ { [ ~` and
/// friends on many layouts) as Ctrl+Alt, which must still count as typing.
fn is_ctrl(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT)
}

fn menu_item(key: char, label: &str, action: MenuAction) -> MenuItem {
    MenuItem {
        key,
        label: label.to_string(),
        detail: String::new(),
        action,
    }
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
    pub cmd_search: bool,
    pub menu_items: Vec<MenuItem>,
    pub menu_sel: usize,
    pub menu_title: String,
    pub menu_sub: bool,
    pub menu_track: Option<Track>,
    pub cmd_sel: usize,
    cmd_cycle: Option<String>,
    pub artist_opts: Vec<String>,
    pub info_track: Option<Track>,
    pub artist_sel: usize,
    pub picks: Vec<Collection>,
    pub pick_sel: usize,
    pub pick_title: String,
    pub pending_delete: Option<String>,
    pub help_scroll: usize,
    pub lyrics: LyricsState,
    pub lyrics_panel: bool,
    pub lyrics_scroll: usize,
    pub lyrics_id: Option<String>,
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
    pub accent: ratatui::style::Color,
    played: HashSet<usize>,
    radio_played: HashSet<usize>,
    resume_radio: bool,
    fail_streak: u8,
    want: Option<String>,
    fetching: Option<String>,
    result_cursor: Option<usize>,
    buffered: Vec<Track>,
    next_page: Option<String>,
    loading_more: bool,
    results_gen: u64,
    radio: Vec<Track>,
    on_radio: bool,
    similar_load: Option<u64>,
    mpris: Option<Mpris>,
    discord: Option<Discord>,
    owns_session: bool,
    seek_step: f64,
    volume_step: f64,
    autoplay: bool,
    popular_first: bool,
    resume: Option<(String, f64)>,
    saved: String,
    save_failed: bool,

    pub status: Option<Status>,
    pub busy: Option<String>,
    pub tick: usize,
    pub should_quit: bool,

    search_id: u64,
    player: Option<Player>,
    player_gen: u64,
    tx: Sender<AppEvent>,
    rng: u64,
}

impl App {
    pub fn new(tx: Sender<AppEvent>) -> App {
        let (store, store_warning) = Store::load();
        config::migrate_legacy();
        let (cfg, cfg_warning) = Config::load();
        let p = cfg.playback;
        let warning = cfg_warning.or(store_warning);
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1;
        let mpris = {
            let t = tx.clone();
            Mpris::new(move |c| {
                let _ = t.send(AppEvent::Media(c));
            })
        };
        let discord = Discord::new(&cfg.discord.client_id);
        let owns_session = Session::claim();
        let mut app = App {
            view: View::Results,
            mode: Mode::Normal,
            results: Vec::new(),
            results_title: "Results".into(),
            queue: Vec::new(),
            current: None,
            store,
            states: Default::default(),
            open: None,
            pl_state: TableState::default(),
            cmd_buf: String::new(),
            cmd_search: false,
            menu_items: Vec::new(),
            menu_sel: 0,
            menu_title: String::new(),
            menu_sub: false,
            menu_track: None,
            cmd_sel: 0,
            cmd_cycle: None,
            artist_opts: Vec::new(),
            info_track: None,
            artist_sel: 0,
            picks: Vec::new(),
            pick_sel: 0,
            pick_title: String::new(),
            pending_delete: None,
            help_scroll: 0,
            lyrics: LyricsState::Idle,
            lyrics_panel: false,
            lyrics_scroll: 0,
            lyrics_id: None,
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
            accent: cfg.ui.accent,
            played: HashSet::new(),
            radio_played: HashSet::new(),
            resume_radio: false,
            fail_streak: 0,
            want: None,
            fetching: None,
            result_cursor: None,
            buffered: Vec::new(),
            next_page: None,
            loading_more: false,
            results_gen: 0,
            radio: Vec::new(),
            on_radio: false,
            similar_load: None,
            mpris,
            discord,
            owns_session,
            seek_step: p.seek_step,
            volume_step: p.volume_step,
            autoplay: p.autoplay,
            popular_first: cfg.artist.popular_first,
            resume: None,
            saved: String::new(),
            save_failed: false,
            status: None,
            busy: None,
            tick: 0,
            should_quit: false,
            search_id: 0,
            player: None,
            player_gen: 0,
            tx,
            rng: seed,
        };
        if p.restore_session && owns_session {
            let s = Session::load();
            app.resume = s
                .current
                .and_then(|c| s.queue.get(c))
                .map(|t| (t.id.clone(), s.position));
            app.current = s.current;
            app.queue = s.queue;
            if !app.queue.is_empty() {
                app.states[View::Queue.idx()].select(Some(app.current.unwrap_or(0)));
            }
        }
        match warning {
            Some(w) => app.error(w),
            None => app.info("Hello! Press / to search, and ? for help"),
        }
        app
    }

    pub fn shutdown(&mut self) {
        if let Err(e) = self.save_session(true) {
            eprintln!("drift: could not save session: {e:#}");
        }
        self.player = None;
        self.mpris = None;
        self.discord = None;
    }

    fn save_session(&mut self, force: bool) -> Result<()> {
        if !self.owns_session {
            return Ok(());
        }
        let session = Session {
            queue: self.queue.clone(),
            current: self.current,
            position: if self.on_radio { 0.0 } else { self.position },
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
        self.status = Some(Status {
            text,
            level,
            at: Instant::now(),
        });
    }

    pub fn visible_status(&self) -> Option<&Status> {
        self.status
            .as_ref()
            .filter(|s| s.at.elapsed().as_secs() < if s.level == Level::Error { 12 } else { 6 })
    }

    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.load_more();
        self.sync_lyrics();
        self.sync_mpris();
        self.sync_discord();
        /* open the picker once nothing else is being typed, so keys aren't hijacked */
        if self.mode == Mode::Normal && !self.picks.is_empty() {
            self.mode = Mode::CollectionPick;
        }
        if self.tick.is_multiple_of(50) {
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
            && self
                .open
                .as_ref()
                .is_some_and(|n| self.store.playlists.contains_key(n))
    }

    pub fn open_len(&self) -> usize {
        self.open
            .as_ref()
            .and_then(|n| self.store.playlists.get(n))
            .map_or(0, |t| t.len())
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
        if self.on_radio {
            return self.result_cursor.and_then(|i| self.radio.get(i));
        }
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
            Mode::CollectionPick => self.key_collection_pick(key),
            Mode::ConfirmDelete => self.key_confirm_delete(key),
            Mode::Menu => self.key_menu(key),
            Mode::Info => self.key_info(key),
            Mode::Lyrics => self.key_lyrics(key),
            Mode::Normal => self.key_normal(key),
        }
    }

    fn key_normal(&mut self, key: KeyEvent) {
        let ctrl = is_ctrl(&key);

        if let Some(p) = self.pending.take() {
            let n = self.count.take();
            if let KeyCode::Char(c) = key.code {
                match (p, c) {
                    ('g', 'g') => self.set_selected(n.map_or(0, |n| n.saturating_sub(1))),
                    ('g', 'e') => {
                        self.set_selected(n.map_or(usize::MAX, |n| n.saturating_sub(1)))
                    }
                    ('g', 'r') => self.set_view(View::Results),
                    ('g', 'q') => self.set_view(View::Queue),
                    ('g', 'p') => self.set_view(View::Playlists),
                    ('d', 'd') => self.delete_selected(),
                    ('g', 'a') => self.go_to_artist(),
                    ('g', 's') => self.go_similar(),
                    ('g', 'b') => self.go_album(),
                    ('g', 'B') => self.save_album(None),
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
            KeyCode::Char('G') | KeyCode::Menu => self.open_menu(),
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
            KeyCode::Char('h') | KeyCode::Left => {
                self.seek(Amount::Delta(-self.seek_step * times as f64))
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.seek(Amount::Delta(self.seek_step * times as f64))
            }
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
                self.cmd_buf.clear();
                self.cmd_search = true;
                self.enter_command_mode();
            }
            KeyCode::Char(':') => {
                self.cmd_buf.clear();
                self.cmd_search = false;
                self.enter_command_mode();
            }
            KeyCode::Char('L') => self.lyrics_panel = !self.lyrics_panel,
            KeyCode::Char('F') => self.open_lyrics(),
            KeyCode::Char('?') => self.open_help(),
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => self.status = None,
            _ => {}
        }
    }

    /// The song under the cursor (no fallback to the playing song).
    fn selected_track(&self) -> Option<Track> {
        let sel = self.selected();
        match self.view {
            View::Results => self.results.get(sel).cloned(),
            View::Queue => self.queue.get(sel).cloned(),
            View::Playlists if self.in_playlist() => self
                .open
                .as_ref()
                .and_then(|n| self.store.playlists.get(n))
                .and_then(|t| t.get(sel))
                .cloned(),
            View::Playlists => None,
        }
    }

    /// `G`: open the context menu for whatever is highlighted.
    fn open_menu(&mut self) {
        let mut items: Vec<MenuItem> = Vec::new();
        if self.view == View::Playlists && !self.in_playlist() {
            let Some(name) = self.playlist_name(self.selected()) else {
                return self.error("No playlist selected");
            };
            let n = self.store.playlists.get(&name).map_or(0, Vec::len);
            items.push(menu_item('o', "Open", MenuAction::OpenPlaylist));
            items.push(menu_item('p', "Play", MenuAction::Play));
            items.push(menu_item('a', "Append to queue", MenuAction::AppendPlaylist));
            items.push(menu_item('x', "Delete playlist…", MenuAction::DeletePlaylist));
            self.menu_title = format!("{name} ({n})");
            self.menu_track = None;
        } else {
            let Some(track) = self.selected_track() else {
                return self.error("No song selected");
            };
            let in_queue = self.view == View::Queue;
            items.push(menu_item('p', "Play now", MenuAction::Play));
            if !in_queue {
                items.push(menu_item('n', "Play next", MenuAction::PlayNext));
                items.push(menu_item('a', "Add to queue", MenuAction::Queue));
            }
            items.push(menu_item('g', "Show artist", MenuAction::Artist));
            items.push(menu_item('s', "Similar songs", MenuAction::Similar));
            if track.album.is_some() {
                items.push(menu_item('b', "Show album", MenuAction::Album));
                items.push(menu_item('B', "Save album to playlist", MenuAction::SaveTrackAlbum));
            }
            items.push(menu_item('l', "Add to playlist…", MenuAction::PlaylistMenu));
            if in_queue {
                items.push(menu_item('x', "Remove from queue", MenuAction::Remove));
            } else if self.in_playlist() {
                items.push(menu_item('x', "Remove from playlist", MenuAction::Remove));
            }
            items.push(menu_item('i', "Song info", MenuAction::Info));
            if self.view == View::Results {
                items.push(menu_item('Q', "Queue all results", MenuAction::QueueAll));
                items.push(menu_item('P', "Play all results", MenuAction::PlayAll));
                if self.results_title.starts_with("Album: ") {
                    items.push(menu_item('S', "Save album as playlist…", MenuAction::SaveAlbum));
                } else {
                    items.push(menu_item('S', "Save results as playlist…", MenuAction::SaveResults));
                }
            }
            if in_queue {
                items.push(menu_item('c', "Clear queue", MenuAction::ClearQueue));
                items.push(menu_item('S', "Save queue as playlist…", MenuAction::SaveQueue));
            }
            self.menu_title = track.title.clone();
            self.menu_track = Some(track);
        }
        self.menu_items = items;
        self.menu_sel = 0;
        self.menu_sub = false;
        self.mode = Mode::Menu;
    }

    fn open_playlist_menu(&mut self) {
        let track = self.menu_track.clone();
        let mut items: Vec<MenuItem> = self
            .store
            .playlists
            .iter()
            .enumerate()
            .map(|(i, (name, list))| {
                let has = track
                    .as_ref()
                    .is_some_and(|t| list.iter().any(|x| x.id == t.id));
                MenuItem {
                    key: if i < 9 {
                        char::from_digit(i as u32 + 1, 10).unwrap_or(NO_KEY)
                    } else {
                        NO_KEY
                    },
                    label: name.clone(),
                    detail: if has {
                        "✓ already in it".to_string()
                    } else {
                        format!("{} songs", list.len())
                    },
                    action: MenuAction::PickPlaylist(name.clone()),
                }
            })
            .collect();
        items.push(menu_item('+', "New playlist…", MenuAction::NewPlaylist));
        self.menu_items = items;
        self.menu_title = "Add to playlist".into();
        self.menu_sel = 0;
        self.menu_sub = true;
    }

    fn close_menu(&mut self) {
        self.mode = Mode::Normal;
        self.menu_items.clear();
        self.menu_sel = 0;
        self.menu_sub = false;
        self.menu_track = None;
    }

    fn key_menu(&mut self, key: KeyEvent) {
        let ctrl = is_ctrl(&key);
        let len = self.menu_items.len();
        if len == 0 {
            return self.close_menu();
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('G') | KeyCode::Menu => {
                if self.menu_sub {
                    self.open_menu();
                } else {
                    self.close_menu();
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.menu_sel = (self.menu_sel + 1) % len,
            KeyCode::Char('k') | KeyCode::Up => self.menu_sel = (self.menu_sel + len - 1) % len,
            KeyCode::Char('n') if ctrl => self.menu_sel = (self.menu_sel + 1) % len,
            KeyCode::Char('p') if ctrl => self.menu_sel = (self.menu_sel + len - 1) % len,
            KeyCode::Home => self.menu_sel = 0,
            KeyCode::End => self.menu_sel = len - 1,
            KeyCode::Enter => self.run_menu(self.menu_sel),
            KeyCode::Char(c) if !ctrl && c != NO_KEY => {
                if let Some(i) = self.menu_items.iter().position(|it| it.key == c) {
                    self.menu_sel = i;
                    self.run_menu(i);
                }
            }
            _ => {}
        }
    }

    fn run_menu(&mut self, i: usize) {
        let Some(item) = self.menu_items.get(i).cloned() else {
            return;
        };
        if item.action == MenuAction::PlaylistMenu {
            return self.open_playlist_menu();
        }
        let track = self.menu_track.clone();
        self.close_menu();
        match item.action {
            MenuAction::Play => self.activate(),
            MenuAction::PlayNext => {
                if let Some(t) = track {
                    self.play_next(t);
                }
            }
            MenuAction::Queue | MenuAction::AppendPlaylist => self.add_selected(),
            MenuAction::Artist => self.go_to_artist(),
            MenuAction::Similar => self.go_similar(),
            MenuAction::Album => self.go_album(),
            MenuAction::SaveTrackAlbum => {
                if let Some(t) = track {
                    let album_title = t.album.clone().unwrap_or_else(|| "Album".to_string());
                    self.save_album_background(t, album_title);
                }
            }
            MenuAction::SaveAlbum => {
                let default_name = self.results_title.strip_prefix("Album: ").unwrap_or("").trim();
                if !default_name.is_empty() {
                    self.prefill_command(&format!("savealbum \"{default_name}\""));
                } else {
                    self.prefill_command("savealbum ");
                }
            }
            MenuAction::SaveResults => self.prefill_command("saveresults "),
            MenuAction::PlaylistMenu => {}
            MenuAction::PickPlaylist(name) => self.add_to_playlist(&name),
            MenuAction::NewPlaylist => self.prefill_command("addto "),
            MenuAction::Remove | MenuAction::DeletePlaylist => self.delete_selected(),
            MenuAction::Info => {
                if let Some(t) = track {
                    self.info_track = Some(t);
                    self.mode = Mode::Info;
                }
            }
            MenuAction::QueueAll => self.add_all_results(),
            MenuAction::PlayAll => self.play_all_results(),
            MenuAction::ClearQueue => self.clear_queue(),
            MenuAction::SaveQueue => self.prefill_command("save "),
            MenuAction::OpenPlaylist => self.open_selected_playlist(),
        }
    }

    /// Open the `:` popup with some text already typed (e.g. `save `).
    fn prefill_command(&mut self, text: &str) {
        self.cmd_buf = text.to_string();
        self.cmd_search = false;
        self.enter_command_mode();
    }

    fn play_next(&mut self, t: Track) {
        let label = t.label();
        let len = self.queue.len();
        let at = self.current.map_or(0, |c| (c + 1).min(len));
        self.queue.insert(at, t);
        self.played = self
            .played
            .iter()
            .map(|&i| if i >= at { i + 1 } else { i })
            .collect();
        self.info(format!("Playing next: {label}"));
    }

    fn key_confirm_delete(&mut self, key: KeyEvent) {
        self.mode = Mode::Normal;
        let name = self.pending_delete.take();
        if let (KeyCode::Char('y'), Some(name)) = (key.code, name) {
            self.delete_playlist(&name);
        }
    }

    fn open_help(&mut self) {
        self.help_scroll = 0;
        self.mode = Mode::Help;
    }

    fn key_info(&mut self, _key: KeyEvent) {
        self.mode = Mode::Normal;
        self.info_track = None;
    }

    fn open_lyrics(&mut self) {
        self.mode = Mode::Lyrics;
    }

    fn key_lyrics(&mut self, key: KeyEvent) {
        let scroll = self.lyrics_scroll;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'F' | 'L') => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => self.lyrics_scroll = scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => self.lyrics_scroll = scroll.saturating_sub(1),
            KeyCode::Char(' ') => self.toggle_pause(),
            KeyCode::Char('n') | KeyCode::Char('>') => self.next(),
            KeyCode::Char('N') | KeyCode::Char('<') => self.prev(),
            KeyCode::Char('h') | KeyCode::Left => self.seek(Amount::Delta(-self.seek_step)),
            KeyCode::Char('l') | KeyCode::Right => self.seek(Amount::Delta(self.seek_step)),
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.set_volume(Amount::Delta(self.volume_step))
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                self.set_volume(Amount::Delta(-self.volume_step))
            }
            _ => {}
        }
    }

    fn sync_lyrics(&mut self) {
        if !self.lyrics_panel && self.mode != Mode::Lyrics {
            return;
        }
        let Some(track) = self
            .now_track()
            .filter(|_| self.play_state != PlayState::Stopped)
            .cloned()
        else {
            return;
        };
        if self.lyrics_id.as_deref() == Some(track.id.as_str()) {
            return;
        }
        self.lyrics_id = Some(track.id.clone());
        self.lyrics = LyricsState::Loading;
        self.lyrics_scroll = 0;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = lyrics::fetch(&track);
            let _ = tx.send(AppEvent::Lyrics {
                id: track.id,
                result,
            });
        });
    }

    fn key_help(&mut self, key: KeyEvent) {
        let ctrl = is_ctrl(&key);
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
        self.cmd_cycle = None;
        self.cmd_sel = 0;
    }

    /// Commands matching what is typed so far (only while typing the command name).
    pub fn command_matches(&self) -> Vec<&'static str> {
        if self.cmd_search || self.cmd_buf.contains(' ') {
            return Vec::new();
        }
        let prefix = self.cmd_cycle.as_deref().unwrap_or(self.cmd_buf.as_str());
        COMMAND_NAMES
            .iter()
            .copied()
            .filter(|c| c.starts_with(prefix))
            .collect()
    }

    /// True while Tab / Shift-Tab is stepping through the suggestion list.
    pub fn cycling(&self) -> bool {
        self.cmd_cycle.is_some()
    }

    /// Most recent searches, newest first (shown in the `/` popup).
    pub fn recent_searches(&self, n: usize) -> Vec<&str> {
        self.history
            .iter()
            .rev()
            .filter_map(|h| h.strip_prefix("search "))
            .take(n)
            .collect()
    }

    fn key_command(&mut self, key: KeyEvent) {
        let ctrl = is_ctrl(&key);
        let cycling = matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
            || (ctrl && matches!(key.code, KeyCode::Char('n') | KeyCode::Char('p')));
        if !cycling {
            self.cmd_cycle = None;
        }
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.cmd_buf.clear();
            }
            KeyCode::Enter => {
                let line = std::mem::take(&mut self.cmd_buf);
                self.mode = Mode::Normal;
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return;
                }
                let full = if self.cmd_search {
                    format!("search {trimmed}")
                } else {
                    trimmed.to_string()
                };
                if self.history.last() != Some(&full) {
                    self.history.push(full.clone());
                }
                self.run_command(&full);
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
            KeyCode::Tab => self.complete(1),
            KeyCode::BackTab => self.complete(-1),
            KeyCode::Char('n') if ctrl => self.complete(1),
            KeyCode::Char('p') if ctrl => self.complete(-1),
            KeyCode::Char(c) if !ctrl => self.cmd_buf.push(c),
            _ => {}
        }
    }

    fn history_step(&mut self, dir: isize) {
        let search = self.cmd_search;
        let entries: Vec<String> = self
            .history
            .iter()
            .filter_map(|h| match h.strip_prefix("search ") {
                Some(q) if search => Some(q.to_string()),
                _ if search => None,
                _ => Some(h.clone()),
            })
            .collect();
        if entries.is_empty() {
            return;
        }
        let len = entries.len() as isize;
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
            self.cmd_buf = entries[pos].clone();
        }
    }

    /// Tab / Shift-Tab: cycle through the commands matching the typed prefix.
    fn complete(&mut self, dir: isize) {
        if self.cmd_search || self.cmd_buf.contains(' ') {
            return;
        }
        if self.cmd_cycle.is_none() {
            self.cmd_cycle = Some(self.cmd_buf.clone());
            self.cmd_sel = if dir < 0 { usize::MAX } else { 0 };
        } else {
            let n = self.command_matches().len().max(1) as isize;
            self.cmd_sel = (self.cmd_sel as isize + dir).rem_euclid(n) as usize;
        }
        let matches = self.command_matches();
        if matches.is_empty() {
            self.cmd_cycle = None;
            return;
        }
        if self.cmd_sel >= matches.len() {
            self.cmd_sel = matches.len() - 1;
        }
        let pick = matches[self.cmd_sel];
        if matches.len() == 1 {
            self.cmd_buf = format!("{pick} ");
            self.cmd_cycle = None;
        } else {
            self.cmd_buf = pick.to_string();
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
                let len = self.cur_len();
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
            Command::Remove(Some(n)) => self.remove_from_queue(n - 1),
            Command::Remove(None) if self.view == View::Queue => {
                let i = self.selected();
                self.remove_from_queue(i);
            }
            Command::Remove(None) => self.error("Open the queue or give a row number: :remove N"),
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
            Command::Save(name) => self.save_playlist(&name, false),
            Command::SaveForce(name) => self.save_playlist(&name, true),
            Command::SaveAlbum(name) => self.save_album(name),
            Command::SaveResults(name) => self.save_results_as_playlist(name),
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
            Command::Similar => self.go_similar(),
            Command::Album(Some(q)) => self.start_collection_search(q, Kind::Album),
            Command::Album(None) => self.go_album(),
            Command::PlaylistSearch(q) => self.start_collection_search(q, Kind::Playlist),
            Command::Lyrics => self.lyrics_panel = !self.lyrics_panel,
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
        let names = track.artist_names();
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
            KeyCode::Char('j') | KeyCode::Down => {
                self.artist_sel = (self.artist_sel + 1).min(len - 1)
            }
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
        let popular_first = self.popular_first;
        self.spawn_loader(title, busy, move || {
            let wanted = name.to_lowercase();
            let mut tracks = if popular_first {
                ytmusic::artist_top_songs(&name).unwrap_or_default()
            } else {
                Vec::new()
            };
            let found = match ytmusic::search(&name) {
                Ok(found) => found,
                Err(e) if tracks.is_empty() => return Err(e),
                Err(_) => Vec::new(),
            };
            let known: HashSet<String> = tracks.iter().map(|t| t.id.clone()).collect();
            tracks.extend(found.into_iter().filter(|t| {
                !known.contains(&t.id)
                    && (t.artist.to_lowercase() == wanted
                        || t.artist_names()
                            .iter()
                            .any(|a| a.to_lowercase() == wanted))
            }));
            Ok(tracks.into())
        });
    }

    fn go_album(&mut self) {
        let Some(track) = self.artist_target() else {
            return self.error("No song selected");
        };
        let Some(album) = track.album.clone() else {
            return self.error("No album known for this song");
        };
        let artist = track
            .artist_names()
            .into_iter()
            .next()
            .unwrap_or_default();
        let busy = format!("Loading album \u{201c}{album}\u{201d}");
        let title = format!("Album: {album}");
        self.spawn_loader(title, busy, move || {
            let found = ytmusic::search_collections(&format!("{album} {artist}"), Kind::Album)?;
            let want = album.to_lowercase();
            let pick = found
                .iter()
                .find(|c| c.title.to_lowercase() == want)
                .or(found.first())
                .ok_or_else(|| "album not found".to_string())?;
            ytmusic::browse_tracks(&pick.browse_id, &pick.title, &pick.artist).map(Page::from)
        });
    }

    fn save_album(&mut self, name: Option<String>) {
        if self.view == View::Results
            && self.results_title.starts_with("Album: ")
            && !self.results.is_empty()
        {
            return self.save_results_as_playlist(name);
        }

        if let Some(track) = self.artist_target() {
            if let Some(album) = track.album.clone() {
                let target_name = name.unwrap_or(album);
                return self.save_album_background(track, target_name);
            }
        }

        if self.view == View::Results && !self.results.is_empty() {
            return self.save_results_as_playlist(name);
        }

        self.error("No album to save — open an album or select a song with an album");
    }

    fn save_album_background(&mut self, track: Track, target_name: String) {
        let Some(album) = track.album else {
            return self.error("No album known for this song");
        };
        let artist = split_artists(&track.artist)
            .into_iter()
            .next()
            .unwrap_or_default();
        self.busy = Some(format!("Saving album \u{201c}{album}\u{201d}"));
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<Vec<Track>, String> {
                let found = ytmusic::search_collections(&format!("{album} {artist}"), Kind::Album)?;
                let want = album.to_lowercase();
                let pick = found
                    .iter()
                    .find(|c| c.title.to_lowercase() == want)
                    .or(found.first())
                    .ok_or_else(|| "album not found".to_string())?;
                ytmusic::browse_tracks(&pick.browse_id, &pick.title, &pick.artist)
            })();
            let _ = tx.send(AppEvent::SavePlaylist {
                name: target_name,
                result,
            });
        });
    }

    fn start_collection_search(&mut self, query: String, kind: Kind) {
        let (what, title) = match kind {
            Kind::Album => ("albums", format!("Albums: {query}")),
            Kind::Playlist => ("playlists", format!("Playlists: {query}")),
        };
        self.search_id += 1;
        let id = self.search_id;
        self.busy = Some(format!("Searching {what} for \u{201c}{query}\u{201d}"));
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = ytmusic::search_collections(&query, kind);
            let _ = tx.send(AppEvent::Collections { id, title, result });
        });
    }

    fn key_collection_pick(&mut self, key: KeyEvent) {
        let len = self.picks.len();
        if len == 0 {
            self.mode = Mode::Normal;
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.close_collection_pick(),
            KeyCode::Char('s') | KeyCode::Char('S') => self.save_picked_collection(),
            KeyCode::Char('j') | KeyCode::Down => self.pick_sel = (self.pick_sel + 1).min(len - 1),
            KeyCode::Char('k') | KeyCode::Up => self.pick_sel = self.pick_sel.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => self.pick_sel = 0,
            KeyCode::Char('G') | KeyCode::End => self.pick_sel = len - 1,
            KeyCode::Char(c @ '1'..='9') => {
                let i = c as usize - '1' as usize;
                if i < len {
                    self.pick_sel = i;
                    self.confirm_collection();
                }
            }
            KeyCode::Enter => self.confirm_collection(),
            _ => {}
        }
    }

    fn close_collection_pick(&mut self) {
        self.mode = Mode::Normal;
        self.picks.clear();
        self.pick_sel = 0;
    }

    fn confirm_collection(&mut self) {
        let c = self.picks.get(self.pick_sel).cloned();
        self.close_collection_pick();
        let Some(c) = c else { return };
        let kind = if c.browse_id.starts_with("VL") { "Playlist" } else { "Album" };
        let busy = format!("Loading \u{201c}{}\u{201d}", c.title);
        let title = format!("{kind}: {}", c.title);
        self.spawn_loader(title, busy, move || {
            ytmusic::browse_tracks(&c.browse_id, &c.title, &c.artist).map(Page::from)
        });
    }

    fn save_picked_collection(&mut self) {
        let c = self.picks.get(self.pick_sel).cloned();
        self.close_collection_pick();
        let Some(c) = c else { return };
        let kind = if c.browse_id.starts_with("VL") { "Playlist" } else { "Album" };
        let target_name = c.title.clone();
        self.busy = Some(format!("Saving {kind} \u{201c}{target_name}\u{201d}"));
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = ytmusic::browse_tracks(&c.browse_id, &c.title, &c.artist);
            let _ = tx.send(AppEvent::SavePlaylist {
                name: target_name,
                result,
            });
        });
    }

    fn on_media(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Play => match self.play_state {
                PlayState::Stopped => self.play_default(),
                _ => self.set_pause(false),
            },
            Cmd::Pause => {
                if self.play_state != PlayState::Stopped {
                    self.set_pause(true);
                }
            }
            Cmd::Toggle => self.toggle_pause(),
            Cmd::Stop => self.stop_playback(),
            Cmd::Next => self.next(),
            Cmd::Prev => self.prev(),
            Cmd::SeekBy(d) => self.seek(Amount::Delta(d)),
            Cmd::SeekTo(p) => self.seek(Amount::Abs(p)),
            Cmd::Quit => self.should_quit = true,
        }
    }

    fn sync_mpris(&mut self) {
        if let Some(mut m) = self.mpris.take() {
            m.update(self.now_track(), self.play_state, self.position);
            self.mpris = Some(m);
        }
    }

    fn sync_discord(&mut self) {
        if let Some(mut d) = self.discord.take() {
            d.update(self.now_track(), self.play_state, self.position);
            self.discord = Some(d);
        }
    }

    fn go_similar(&mut self) {
        let Some(track) = self.artist_target() else {
            return self.error("No song selected");
        };
        let busy = format!("Finding songs like \u{201c}{}\u{201d}", track.title);
        let title = format!("Similar to {}", track.title);
        let id = track.id;
        self.spawn_loader(title, busy, move || ytmusic::radio(&id).map(Page::from));
        self.similar_load = Some(self.search_id);
    }

    fn start_search(&mut self, query: String) {
        self.spawn_loader(
            format!("Search: {query}"),
            format!("Searching \u{201c}{query}\u{201d}"),
            move || ytmusic::search_page(&query),
        );
    }

    fn start_import(&mut self, url: String) {
        self.spawn_loader(
            "Imported playlist".into(),
            "Importing playlist".into(),
            move || ytmusic::import(&url).map(Page::from),
        );
    }

    fn spawn_loader<F>(&mut self, title: String, busy: String, work: F)
    where
        F: FnOnce() -> Result<Page, String> + Send + 'static,
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
                let autoplay = self.similar_load.take() == Some(id);
                match result {
                    Ok(page) if page.tracks.is_empty() => self.info("No results found."),
                    Ok(page) => {
                        let mut tracks = page.tracks;
                        self.buffered = match page.next {
                            Some(_) if tracks.len() > FIRST_PAGE => tracks.split_off(FIRST_PAGE),
                            _ => Vec::new(),
                        };
                        self.next_page = page.next;
                        self.loading_more = false;
                        self.results_gen += 1;
                        let n = tracks.len();
                        self.results = tracks;
                        self.results_title = title;
                        self.states[View::Results.idx()].select(Some(0));
                        self.set_view(View::Results);
                        self.info(format!("{n} results — Enter plays, a queues, ? for help"));
                        if autoplay {
                            self.play_result(0);
                        }
                    }
                    Err(e) => self.error(e),
                }
            }
            AppEvent::More { generation, result } => {
                if generation != self.results_gen {
                    return;
                }
                self.loading_more = false;
                match result {
                    Ok(page) => {
                        let known: HashSet<&str> = self
                            .results
                            .iter()
                            .chain(&self.buffered)
                            .map(|t| t.id.as_str())
                            .collect();
                        let fresh: Vec<Track> = page
                            .tracks
                            .into_iter()
                            .filter(|t| !known.contains(t.id.as_str()))
                            .collect();
                        self.buffered.extend(fresh);
                        self.next_page = page.next;
                        self.reveal_buffered();
                    }
                    Err(e) => {
                        self.next_page = None;
                        self.error(format!("Could not load more results: {e}"));
                    }
                }
            }
            AppEvent::Collections { id, title, result } => {
                if id != self.search_id {
                    return;
                }
                self.busy = None;
                match result {
                    Ok(list) if list.is_empty() => self.info("Nothing found."),
                    Ok(list) => {
                        self.picks = list;
                        self.pick_sel = 0;
                        self.pick_title = title;
                    }
                    Err(e) => self.error(e),
                }
            }
            AppEvent::Media(cmd) => self.on_media(cmd),
            AppEvent::SavePlaylist { name, result } => {
                self.busy = None;
                match result {
                    Ok(tracks) if tracks.is_empty() => {
                        self.error(format!("Album \u{201c}{name}\u{201d} contains no tracks"));
                    }
                    Ok(tracks) => {
                        let n = tracks.len();
                        let key = self.store.resolve_exact(&name).unwrap_or(name);
                        self.store.playlists.insert(key.clone(), tracks);
                        if self.persist() {
                            self.info(format!(
                                "Saved {n} tracks to playlist \u{201c}{key}\u{201d}"
                            ));
                            let sel = self.selected();
                            self.set_selected(sel);
                        }
                    }
                    Err(e) => {
                        self.error(format!("Could not save album \u{201c}{name}\u{201d}: {e}"))
                    }
                }
            }
            AppEvent::Lyrics { id, result } => {
                if self.lyrics_id.as_deref() != Some(id.as_str()) {
                    return;
                }
                self.lyrics = match result {
                    Ok(Some(l)) => LyricsState::Ready(l),
                    Ok(None) => LyricsState::Missing,
                    Err(e) => LyricsState::Failed(e),
                };
            }
            AppEvent::Audio { id, result } => self.on_audio(id, result),
            AppEvent::Player(generation, pe) => {
                if generation == self.player_gen {
                    self.on_player(pe);
                }
            }
        }
    }

    fn reveal_buffered(&mut self) {
        let n = NEXT_PAGE.min(self.buffered.len());
        self.results.extend(self.buffered.drain(..n));
    }

    fn load_more(&mut self) {
        if self.view != View::Results
            || self.results.is_empty()
            || self.selected() + LOAD_AHEAD < self.results.len()
        {
            return;
        }
        if !self.buffered.is_empty() {
            return self.reveal_buffered();
        }
        let Some(token) = self.next_page.clone() else {
            return;
        };
        if self.loading_more {
            return;
        }
        self.loading_more = true;
        let generation = self.results_gen;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = ytmusic::search_more(&token);
            let _ = tx.send(AppEvent::More { generation, result });
        });
    }

    fn ensure_player(&mut self) -> Result<&mut Player> {
        if let Some(p) = self.player.as_mut() {
            if !p.is_alive() {
                self.player = None;
            }
        }
        if self.player.is_none() {
            self.player_gen += 1;
            let generation = self.player_gen;
            let tx = self.tx.clone();
            let p = Player::spawn(self.volume, move |ev| {
                let _ = tx.send(AppEvent::Player(generation, ev));
            })?;
            self.player = Some(p);
        }
        Ok(self.player.as_mut().expect("player was just created"))
    }

    fn with_player(&mut self, f: impl Fn(&mut Player) -> Result<()>) -> Result<()> {
        match self.ensure_player().and_then(&f) {
            Ok(()) => Ok(()),
            Err(_) => {
                self.player = None;
                self.ensure_player().and_then(f)
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
                self.prefetch_next();
                if let Some((id, pos)) = self.resume.take() {
                    let ok = self.now_track().is_some_and(|t| {
                        t.id == id && t.duration.is_none_or(|d| pos < f64::from(d) - 1.0)
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
                        "Playback failed 3 times in a row: {msg}"
                    ));
                } else {
                    self.error(format!(
                        "Can't play \u{201c}{name}\u{201d} ({msg}) — skipping"
                    ));
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

    fn fetch_audio(&mut self, id: &str) {
        if self.fetching.as_deref() == Some(id) {
            return;
        }
        self.fetching = Some(id.to_string());
        let tx = self.tx.clone();
        let id = id.to_string();
        std::thread::spawn(move || {
            let result = player::fetch_audio(&id);
            let _ = tx.send(AppEvent::Audio { id, result });
        });
    }

    fn load_track(&mut self, id: &str) {
        self.want = Some(id.to_string());
        match player::cached_audio(id) {
            Some(path) => self.on_audio(id.to_string(), Ok(path)),
            None => self.fetch_audio(id),
        }
    }

    fn on_audio(&mut self, id: String, result: Result<std::path::PathBuf, String>) {
        if self.fetching.as_deref() == Some(id.as_str()) {
            self.fetching = None;
        }
        if self.want.as_deref() != Some(id.as_str()) {
            return;
        }
        self.want = None;
        match result {
            Ok(path) => {
                let path = path.to_string_lossy().into_owned();
                if let Err(e) = self.with_player(|p| p.load(&path)) {
                    self.play_state = PlayState::Stopped;
                    self.error(format!("Playback failed: {e:#}"));
                }
            }
            Err(msg) => self.on_player(PlayerEvent::LoadError(msg)),
        }
    }

    fn prefetch_next(&mut self) {
        if self.shuffle || self.on_radio || self.fetching.is_some() {
            return;
        }
        let Some(next) = self.current.and_then(|c| self.queue.get(c + 1)) else {
            return;
        };
        let id = next.id.clone();
        if player::cached_audio(&id).is_none() {
            self.fetch_audio(&id);
        }
    }

    fn play_index(&mut self, i: usize) {
        self.start_track(i, false);
    }

    fn start_track(&mut self, i: usize, keep_resume: bool) {
        let Some(track) = self.queue.get(i).cloned() else {
            return;
        };
        if !keep_resume {
            self.resume = None;
        }
        self.on_radio = false;
        self.current = Some(i);
        self.played.insert(i);
        self.play_state = PlayState::Loading;
        self.position = match &self.resume {
            Some((id, pos)) if *id == track.id => *pos,
            _ => 0.0,
        };
        self.duration = track.duration.map_or(0.0, f64::from);
        self.info(format!("♪ {}", track.label()));
        self.load_track(&track.id);
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
                    let i = self.selected().min(self.results.len() - 1);
                    self.play_result(i);
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
        self.resume_radio = false;
        self.start_track(i.min(self.queue.len() - 1), true);
    }

    fn activate(&mut self) {
        let (v, i) = (self.view, self.selected());
        self.activate_at(v, i);
    }

    fn activate_at(&mut self, view: View, i: usize) {
        match view {
            View::Results => {
                if i < self.results.len() {
                    self.play_result(i);
                }
            }
            View::Queue => {
                if i < self.queue.len() {
                    self.resume_radio = false;
                    self.play_index(i);
                }
            }
            View::Playlists => {
                if self.in_playlist() {
                    if let Some(name) = self.open.clone() {
                        self.play_playlist_from(&name, i);
                    }
                } else if let Some(name) = self.playlist_name(i) {
                    self.play_playlist_from(&name, 0);
                }
            }
        }
    }

    fn play_result(&mut self, i: usize) {
        if i >= self.results.len() {
            return;
        }
        self.radio = self.results.clone();
        self.radio_played.clear();
        self.play_radio(i);
    }

    fn play_radio(&mut self, i: usize) {
        let Some(track) = self.radio.get(i).cloned() else {
            return;
        };
        if !self.on_radio && self.current.is_some() {
            /* leaving the queue for a list: what is queued now counts as done,
            only songs added from here on play after this one */
            self.current = self.queue.len().checked_sub(1);
            self.played = (0..self.queue.len()).collect();
        }
        self.on_radio = true;
        self.resume_radio = true;
        self.result_cursor = Some(i);
        self.radio_played.insert(i);
        self.resume = None;
        self.play_state = PlayState::Loading;
        self.position = 0.0;
        self.duration = track.duration.map_or(0.0, f64::from);
        self.info(format!("♪ {}", track.label()));
        self.load_track(&track.id);
    }

    fn set_pause(&mut self, pause: bool) {
        match self.play_state {
            PlayState::Stopped => return self.error("Nothing is playing"),
            PlayState::Loading => return,
            _ => {}
        }
        match self.with_player(|p| p.set_pause(pause)) {
            Ok(()) => {
                self.play_state = if pause {
                    PlayState::Paused
                } else {
                    PlayState::Playing
                };
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
        self.want = None;
        if let Some(p) = self.player.as_mut() {
            let _ = p.stop();
        }
        self.play_state = PlayState::Stopped;
        self.position = 0.0;
    }

    fn next(&mut self) {
        if self.queue.is_empty() && self.result_cursor.is_none() {
            return self.error("Queue is empty");
        }
        self.advance(false);
    }

    fn prev(&mut self) {
        if let Some(cursor) = self.result_cursor.filter(|_| self.on_radio) {
            if self.position > 3.0 {
                return self.seek(Amount::Abs(0.0));
            }
            if cursor > 0 {
                self.play_radio(cursor - 1);
            } else {
                self.seek(Amount::Abs(0.0));
            }
            return;
        }
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
        if auto && self.repeat == RepeatMode::One {
            if self.on_radio {
                if let Some(c) = self.result_cursor {
                    return self.play_radio(c);
                }
            } else if let Some(c) = self.current {
                return self.play_index(c);
            }
        }

        let len = self.queue.len();
        let wrap = self.repeat == RepeatMode::All && !(self.on_radio || self.resume_radio);
        let next = if len == 0 {
            None
        } else if self.shuffle {
            let mut pool: Vec<usize> = (0..len).filter(|i| !self.played.contains(i)).collect();
            if pool.is_empty() && wrap {
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
                Some(_) if wrap => Some(0),
                Some(_) => None,
                None => Some(0),
            }
        };
        match next {
            Some(i) => self.play_index(i),
            None => self.advance_radio(auto),
        }
    }

    fn advance_radio(&mut self, auto: bool) {
        let cursor = self
            .result_cursor
            .filter(|_| self.on_radio || self.resume_radio || self.autoplay);
        match cursor.and_then(|c| self.next_in_radio(c)) {
            Some(i) => self.play_radio(i),
            None if auto => {
                self.stop_playback();
                self.info("Finished playing");
            }
            None => self.info("End of the list"),
        }
    }

    fn next_in_radio(&mut self, cursor: usize) -> Option<usize> {
        let len = self.radio.len();
        if !self.shuffle {
            return match cursor + 1 {
                n if n < len => Some(n),
                _ if self.repeat == RepeatMode::All && len > 0 => Some(0),
                _ => None,
            };
        }
        let mut pool: Vec<usize> = (0..len)
            .filter(|i| !self.radio_played.contains(i))
            .collect();
        if pool.is_empty() && self.repeat == RepeatMode::All {
            self.radio_played.clear();
            pool = (0..len).filter(|&i| i != cursor).collect();
            if pool.is_empty() {
                pool = vec![cursor];
            }
        }
        if pool.is_empty() {
            return None;
        }
        let k = self.rand_below(pool.len());
        Some(pool[k])
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
        self.radio_played.clear();
        if let Some(c) = self.result_cursor {
            self.radio_played.insert(c);
        }
        self.info(format!(
            "Shuffle: {}",
            if self.shuffle { "on" } else { "off" }
        ));
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
        let start = if self.view == View::Results {
            self.selected()
        } else {
            0
        };
        self.play_result(start.min(self.results.len() - 1));
    }

    fn clear_queue(&mut self) {
        self.queue.clear();
        self.current = None;
        self.played.clear();
        if !self.on_radio {
            self.stop_playback();
        }
        self.states[View::Queue.idx()].select(None);
        self.info("Cleared queue");
    }

    fn remove_from_queue(&mut self, i: usize) {
        if i >= self.queue.len() {
            return self.error("No such queue entry");
        }
        let removed = self.queue.remove(i);
        self.played.clear();
        let active = self.play_state != PlayState::Stopped;
        match self.current {
            Some(c) if c == i && self.on_radio => self.current = i.checked_sub(1),
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
        self.states[View::Queue.idx()].select(if len == 0 {
            None
        } else {
            Some(sel.min(len - 1))
        });
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
            View::Playlists => {
                if let Some(name) = self.playlist_name(i) {
                    self.pending_delete = Some(name);
                    self.mode = Mode::ConfirmDelete;
                }
            }
            View::Results => {
                self.info("Results can't be deleted — they are replaced by the next search")
            }
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

    fn save_playlist(&mut self, name: &str, force: bool) {
        if self.queue.is_empty() {
            return self.error("Queue is empty — nothing to save");
        }
        let key = self
            .store
            .resolve_exact(name)
            .unwrap_or_else(|| name.to_string());
        let exists = self.store.playlists.get(&key).is_some_and(|l| !l.is_empty());
        if exists && !force {
            return self.error(format!(
                "Playlist \u{201c}{key}\u{201d} already exists. :save! {key} replaces it, :append {key} adds the queue to the end"
            ));
        }
        let n = self.queue.len();
        self.store.playlists.insert(key.clone(), self.queue.clone());
        if self.persist() {
            self.info(format!(
                "Saved {n} tracks to playlist \u{201c}{key}\u{201d}"
            ));
        }
    }

    fn save_results_as_playlist(&mut self, name: Option<String>) {
        if self.results.is_empty() {
            return self.error("No results to save — search or open an album first");
        }
        let target_name = match name {
            Some(n) if !n.trim().is_empty() => n.trim().to_string(),
            _ => {
                if let Some(rest) = self.results_title.strip_prefix("Album: ") {
                    rest.trim().to_string()
                } else if let Some(rest) = self.results_title.strip_prefix("Playlist: ") {
                    rest.trim().to_string()
                } else if let Some(album) = self.results.iter().find_map(|t| t.album.as_deref()) {
                    album.to_string()
                } else {
                    return self.error(
                        "No album name found; usage: :savealbum <name> or :saveresults <name>",
                    );
                }
            }
        };
        let n = self.results.len();
        let key = self.store.resolve_exact(&target_name).unwrap_or(target_name);
        self.store
            .playlists
            .insert(key.clone(), self.results.clone());
        if self.persist() {
            self.info(format!(
                "Saved {n} tracks to playlist \u{201c}{key}\u{201d}"
            ));
            let sel = self.selected();
            self.set_selected(sel);
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
        let key = self
            .store
            .resolve_exact(name)
            .unwrap_or_else(|| name.to_string());
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
            self.result_cursor = None;
            self.resume_radio = false;
            self.states[View::Queue.idx()].select(Some(0));
            self.play_index(0);
            self.info(format!(
                "Playing playlist \u{201c}{name}\u{201d} ({n} tracks)"
            ));
        } else {
            self.queue.extend(tracks);
            self.info(format!(
                "Appended \u{201c}{name}\u{201d} ({n} tracks) to the queue"
            ));
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
        self.radio = tracks;
        self.radio_played.clear();
        self.play_radio(i);
    }

    fn remove_playlist_track(&mut self, i: usize) {
        let Some(name) = self.open.clone() else {
            return;
        };
        let removed = {
            let Some(list) = self.store.playlists.get_mut(&name) else {
                return;
            };
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
        self.pl_state
            .select(if len == 0 { None } else { Some(i.min(len - 1)) });
        if self.persist() {
            self.info(format!(
                "Removed from \u{201c}{name}\u{201d}: {}",
                track.label()
            ));
        }
    }

    fn move_item(&mut self, dir: isize) {
        if self.in_playlist() {
            return self.move_playlist_track(dir);
        }
        self.move_queue_item(dir);
    }

    fn move_playlist_track(&mut self, dir: isize) {
        let Some(name) = self.open.clone() else {
            return;
        };
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
            artists: Vec::new(),
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

    fn check_paging(app: &mut App) {
        let tracks: Vec<Track> = (0..25)
            .map(|i| track(&format!("{i:011}"), &format!("Song {i}")))
            .collect();
        let page = Page {
            tracks,
            next: Some("token".into()),
        };
        app.search_id = 7;
        app.on_event(AppEvent::Results {
            id: 7,
            title: "Search: x".into(),
            result: Ok(page),
        });
        assert_eq!((app.results.len(), app.buffered.len()), (20, 5));

        app.on_tick();
        assert_eq!(app.results.len(), 20);
        app.states[View::Results.idx()].select(Some(18));
        app.load_more();
        assert_eq!((app.results.len(), app.buffered.len()), (25, 0));

        app.next_page = None;
        app.load_more();
        assert_eq!(app.results.len(), 25);
    }

    fn now_title(app: &App) -> Option<&str> {
        app.now_track().map(|t| t.title.as_str())
    }

    fn check_playlist_playback(app: &mut App) {
        app.queue.clear();
        app.current = None;
        app.played.clear();
        app.store.playlists.insert(
            "run".into(),
            vec![
                track("rrrrrrrrrr1", "R1"),
                track("rrrrrrrrrr2", "R2"),
                track("rrrrrrrrrr3", "R3"),
            ],
        );

        app.set_view(View::Playlists);
        app.open = Some("run".into());
        app.pl_state.select(Some(0));
        press(app, KeyCode::Enter);
        assert!(app.queue.is_empty());
        assert_eq!(now_title(app), Some("R1"));

        app.enqueue(track("qqqqqqqqqq1", "Q1"));
        app.advance(true);
        assert_eq!(now_title(app), Some("Q1"));
        app.advance(true);
        assert_eq!(now_title(app), Some("R2"));
        app.advance(true);
        assert_eq!(now_title(app), Some("R3"));
        assert_eq!(app.queue.len(), 1);

        app.repeat = RepeatMode::All;
        app.advance(true);
        assert_eq!(now_title(app), Some("R1"));
        app.repeat = RepeatMode::Off;

        app.open = None;
        let i = app.store.playlists.keys().position(|k| k == "run").unwrap();
        app.activate_at(View::Playlists, i);
        assert_eq!(now_title(app), Some("R1"));
        assert_eq!(app.queue.len(), 1);

        app.shuffle = true;
        app.advance(true);
        let first = app.result_cursor.unwrap();
        app.advance(true);
        let second = app.result_cursor.unwrap();
        assert!(first != 0 && second != 0 && first != second);
        app.advance(true);
        assert_eq!(app.result_cursor, Some(second));
        app.shuffle = false;
    }

    #[test]
    fn playlist_drill_in() {
        let dir = std::env::temp_dir().join(format!("drift-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("DRIFT_CONFIG_DIR", &dir);
        std::env::set_var("DRIFT_STATE_DIR", &dir);

        let (tx, _rx) = mpsc::channel();
        let mut app = App::new(tx);
        app.store.playlists.insert(
            "mix".into(),
            vec![
                track("aaaaaaaaaaa", "A"),
                track("bbbbbbbbbbb", "B"),
                track("ccccccccccc", "C"),
            ],
        );
        app.store
            .playlists
            .insert("other".into(), vec![track("ddddddddddd", "D")]);

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
        let ids: Vec<&str> = app.store.playlists["mix"]
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(ids, ["aaaaaaaaaaa", "ccccccccccc", "bbbbbbbbbbb"]);
        assert_eq!(app.selected(), 1);

        press(&mut app, KeyCode::Char('x'));
        let ids: Vec<&str> = app.store.playlists["mix"]
            .iter()
            .map(|t| t.id.as_str())
            .collect();
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

        app.set_view(View::Playlists);
        press(&mut app, KeyCode::Char('x'));
        assert!(app.mode == Mode::ConfirmDelete);
        press(&mut app, KeyCode::Char('n'));
        assert!(app.mode == Mode::Normal);
        assert_eq!(app.store.playlists.len(), 2);
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.store.playlists.len(), 1);

        check_artist_popup(&mut app);
        check_paging(&mut app);

        app.run_command("remove");
        assert_eq!(app.queue.len(), 1);

        check_playlist_playback(&mut app);

        // Test savealbum and saveresults
        let mut t1 = track("song1111111", "Dreams");
        t1.album = Some("Rumours".into());
        let mut t2 = track("song2222222", "Go Your Own Way");
        t2.album = Some("Rumours".into());
        app.results = vec![t1.clone(), t2.clone()];
        app.results_title = "Album: Rumours".into();
        app.view = View::Results;

        // :savealbum with no args uses album name from results_title
        app.run_command("savealbum");
        assert_eq!(app.store.playlists.get("Rumours").map(Vec::len), Some(2));

        // :savealbum with custom name
        app.run_command(r#"savealbum "Fleetwood Mac - Rumours""#);
        assert_eq!(
            app.store
                .playlists
                .get("Fleetwood Mac - Rumours")
                .map(Vec::len),
            Some(2)
        );

        // :saveresults with custom name
        app.run_command(r#"saveresults "Classic Hits""#);
        assert_eq!(
            app.store.playlists.get("Classic Hits").map(Vec::len),
            Some(2)
        );

        // Test AppEvent::SavePlaylist background event
        app.on_event(AppEvent::SavePlaylist {
            name: "Background Album".into(),
            result: Ok(vec![track("bg111111111", "BG Track")]),
        });
        assert_eq!(
            app.store.playlists.get("Background Album").map(Vec::len),
            Some(1)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
