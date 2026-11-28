use std::fs;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Result};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::config::{Artist, Audio, Config, Discord, Lastfm, Playback, Ui};
use crate::lastfm::{self, LoginTicket};
use crate::store::{write_atomic, Store};
use crate::sys;

const DEFAULT_VOLUME: u8 = 80;
const DEFAULT_DIR: &str = "~/Music/drift";
const CARD_WIDTH: u16 = 72;
const TALL_TERM: u16 = 26;
const LASTFM_SIGNUP: &str = "https://www.last.fm/api/account/create";
const DISCORD_APPLICATIONS_DEV: &str = "https://discord.com/developers/applications";

const DRIFT_ASCII: [&str; 5] = [
    r"     _      _  __ _   ",
    r"  __| |_ __(_)/ _| |_ ",
    r" / _` | '__| | |_| __|",
    r"| (_| | |  | |  _| |_ ",
    r" \__,_|_|  |_|_|  \__|",
];

const ACCENTS: [(&str, Color); 8] = [
    ("Cyan", Color::Cyan),
    ("Light blue", Color::LightBlue),
    ("Magenta", Color::Magenta),
    ("Light magenta", Color::LightMagenta),
    ("Red", Color::Red),
    ("Yellow", Color::Yellow),
    ("Green", Color::Green),
    ("White", Color::White),
];

const BROWSERS: [(&str, &str); 6] = [
    ("No, not needed", ""),
    ("Firefox", "firefox"),
    ("Chrome", "chrome"),
    ("Chromium", "chromium"),
    ("Brave", "brave"),
    ("Edge", "edge"),
];

const SECTIONS: [&str; 8] = [
    "Welcome",
    "Requirements",
    "Appearance",
    "Playback",
    "Audio",
    "Last.fm",
    "Discord",
    "Review",
];

pub enum Outcome {
    Done,
    Skipped,
    Quit,
}

fn config_path() -> Option<PathBuf> {
    Store::dir().ok().map(|dir| dir.join("config.toml"))
}

fn forced() -> bool {
    cfg!(debug_assertions) && std::env::var("FIRST_RUN").is_ok_and(|v| v == "1")
}

pub fn needed() -> bool {
    if !(io::stdin().is_terminal() && io::stdout().is_terminal()) {
        return false;
    }
    if forced() {
        return true;
    }
    config_path().is_some_and(|path| !path.exists())
}

pub fn run(tools: &[(&'static str, Option<String>)]) -> Result<Outcome> {
    let mut terminal = ratatui::try_init()?;
    let result = event_loop(&mut terminal, &mut Wizard::new(tools));
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, wizard: &mut Wizard) -> Result<Outcome> {
    loop {
        terminal.draw(|f| wizard.draw(f))?;
        if let Some(task) = wizard.pending.take() {
            wizard.run_task(task);
            while event::poll(Duration::ZERO)? {
                event::read()?;
            }
            continue;
        }
        if let Event::Key(key) = event::read()? {
            if let Some(outcome) = wizard.on_key(key) {
                return Ok(outcome);
            }
        }
    }
}

fn save(answers: &Answers) -> Result<PathBuf> {
    let path = config_path().ok_or_else(|| anyhow!("Cannot demertine config directory"))?;
    if path.exists() {
        let _ = fs::copy(&path, path.with_extension("toml.bak"));
    }
    write_atomic(&path, &answers.render())?;
    Ok(path)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Welcome,
    Requirements,
    Accent,
    Volume,
    Session,
    AudioMode,
    DownloadDir,
    Cookies,
    Lastfm,
    LastfmKey,
    LastfmSecret,
    LastfmLogin,
    LastfmAuth,
    Discord,
    DiscordId,
    Review,
}

impl Step {
    fn section(self) -> usize {
        match self {
            Step::Welcome => 0,
            Step::Requirements => 1,
            Step::Accent => 2,
            Step::Volume | Step::Session => 3,
            Step::AudioMode | Step::DownloadDir | Step::Cookies => 4,
            Step::Lastfm
            | Step::LastfmKey
            | Step::LastfmSecret
            | Step::LastfmLogin
            | Step::LastfmAuth => 5,
            Step::Discord | Step::DiscordId => 6,
            Step::Review => 7,
        }
    }

    fn is_text(self) -> bool {
        matches!(
            self,
            Step::Volume
                | Step::DownloadDir
                | Step::LastfmKey
                | Step::LastfmSecret
                | Step::DiscordId
        )
    }
}

struct Choice {
    label: &'static str,
    hint: &'static str,
    color: Option<Color>,
}

fn choice(label: &'static str, hint: &'static str) -> Choice {
    Choice {
        label,
        hint,
        color: None,
    }
}

fn choices(step: Step) -> Vec<Choice> {
    match step {
        Step::Welcome => vec![
            choice("Start setup", "about a minute"),
            choice("Skip", "use the defaults, edit config.toml later"),
        ],
        Step::Accent => ACCENTS
            .iter()
            .map(|&(label, color)| Choice {
                label,
                hint: "",
                color: Some(color),
            })
            .collect(),
        Step::Session => vec![
            choice("Yes", "bring back your queue and position"),
            choice("No", "start empty every time"),
        ],
        Step::AudioMode => vec![
            choice("Stream", "starts instantly, even for hour-long tracks"),
            choice("Download first", "cache each song, prefetch the next"),
        ],
        Step::Cookies => BROWSERS
            .iter()
            .enumerate()
            .map(|(i, &(label, _))| Choice {
                label,
                hint: if i == 0 {
                    "only if YouTube blocks playback"
                } else {
                    ""
                },
                color: None,
            })
            .collect(),
        Step::Lastfm => vec![
            choice("Yes", "scrobble your plays to Last.fm"),
            choice("No", "you can turn it on later in config.toml"),
        ],
        Step::LastfmLogin => vec![
            choice("Sign in now", "opens Last.fm in your browser"),
            choice("Later", "run drift --lastfm-login when ready"),
        ],
        Step::Discord => vec![
            choice("Yes", "show what you're playing in your status"),
            choice("No", "you can turn it on later in config.toml"),
        ],
        _ => Vec::new(),
    }
}

fn copy(step: Step) -> (&'static str, String) {
    match step {
        Step::Welcome => ("", String::new()),
        Step::Requirements => (
            "Requirements",
            "drift plays audio with mpv and fetches streams with yt-dlp.".into(),
        ),
        Step::Accent => (
            "Pick an accent color",
            "Used for borders, the cursor and highlights. Move the cursor to preview it right here."
                .into(),
        ),
        Step::Volume => (
            "Starting volume",
            "A number from 0 to 100. Press enter to keep the default.".into(),
        ),
        Step::Session => (
            "Restore your session?",
            "Pick up your queue and position the next time you start drift.".into(),
        ),
        Step::AudioMode => (
            "How should songs play?",
            "Streaming is the default and works well for almost everyone.".into(),
        ),
        Step::DownloadDir => (
            "Where should downloads go?",
            "Songs you save with D, gd and gD land here. ~ is your home folder. Press enter to keep the default."
                .into(),
        ),
        Step::Cookies => (
            "Use your browser's cookies?",
            "If YouTube ever says \"Sign in to confirm you're not a bot\", drift can borrow your browser's login. Skip this unless that happens."
                .into(),
        ),
        Step::Lastfm => (
            "Scrobble to Last.fm?",
            "Send what you play to your Last.fm profile. You will need a free API key.".into(),
        ),
        Step::LastfmKey => (
            "Last.fm API key",
            format!("Create an API account at {LASTFM_SIGNUP} (any name works, leave the callback URL empty), then paste its API key here."),
        ),
        Step::LastfmSecret => (
            "Last.fm shared secret",
            "Paste the shared secret from the same page. It is stored in config.toml, so keep that file private."
                .into(),
        ),
        Step::LastfmLogin => (
            "Sign in to Last.fm",
            "drift needs your permission once. Your password is never seen by drift.".into(),
        ),
        Step::LastfmAuth => ("", String::new()),
        Step::Discord => (
            "Discord rich presence?",
            "Show the song you're playing, with cover art and a progress bar, in your Discord status."
                .into(),
        ),
        Step::DiscordId => (
            "Discord Application ID",
            format!("Create an application at {DISCORD_APPLICATIONS_DEV}. Its name is shown after \"Listening to\". Copy its Application ID and paste it here."),
        ),
        Step::Review => (
            "Ready to go",
            "This is what'll be saved. Everything can be changed later in config.toml!".into(),
        ),
    }
}

fn placeholder(step: Step) -> &'static str {
    match step {
        Step::Volume => "80",
        Step::DownloadDir => DEFAULT_DIR,
        Step::LastfmKey => "paste your API key",
        Step::LastfmSecret => "paste your shared secret",
        Step::DiscordId => "paste your Application ID",
        _ => "",
    }
}

fn install_hint(tool: &str) -> &'static str {
    match (tool, cfg!(target_os = "macos"), cfg!(windows)) {
        ("mpv", true, _) => "brew install mpv",
        ("mpv", _, true) => "winget install mpv.net  (or: scoop install mpv)",
        ("mpv", _, _) => "your package manager, e.g. sudo apt install mpv",
        (_, true, _) => "brew install yt-dlp",
        (_, _, true) => "winget install yt-dlp.yt-dlp  (or: scoop install yt-dlp)",
        _ => "pipx install yt-dlp  (or your package manager)",
    }
}

struct Answers {
    accent: usize,
    volume: u8,
    restore: bool,
    download: bool,
    download_dir: String,
    cookies: usize,
    lastfm: bool,
    api_key: String,
    api_secret: String,
    lastfm_user: Option<String>,
    discord: bool,
    client_id: String,
}

impl Default for Answers {
    fn default() -> Self {
        Answers {
            accent: 0,
            volume: DEFAULT_VOLUME,
            restore: true,
            download: false,
            download_dir: String::new(),
            cookies: 0,
            lastfm: false,
            api_key: String::new(),
            api_secret: String::new(),
            lastfm_user: None,
            discord: false,
            client_id: String::new(),
        }
    }
}

impl Answers {
    fn dir(&self) -> String {
        if self.download_dir.is_empty() {
            DEFAULT_DIR.to_string()
        } else {
            self.download_dir.clone()
        }
    }

    fn lastfm_config(&self) -> Lastfm {
        Lastfm {
            api_key: self.api_key.trim().to_string(),
            api_secret: self.api_secret.trim().to_string(),
        }
    }

    fn render(&self) -> String {
        let playback = Playback {
            volume: self.volume,
            restore_session: self.restore,
            ..Playback::default()
        };
        let ui = Ui {
            accent: ACCENTS[self.accent].1,
        };
        let discord = Discord {
            client_id: if self.discord {
                self.client_id.trim().to_string()
            } else {
                String::new()
            },
        };
        let lastfm = if self.lastfm {
            self.lastfm_config()
        } else {
            Lastfm::default()
        };
        let audio = Audio {
            download: self.download,
            download_dir: self.dir(),
            cookies_from_browser: BROWSERS[self.cookies].1.to_string(),
            ..Audio::default()
        };
        Config::render(
            &playback,
            &Artist::default(),
            &ui,
            &discord,
            &lastfm,
            &audio,
        )
    }
}

#[derive(Default)]
struct Input {
    chars: Vec<char>,
    cursor: usize,
}

impl Input {
    fn new(text: &str) -> Input {
        let chars: Vec<char> = text.chars().collect();
        let cursor = chars.len();
        Input { chars, cursor }
    }

    fn value(&self) -> String {
        self.chars.iter().collect()
    }

    fn insert(&mut self, c: char) {
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.chars.len());
    }

    fn home(&mut self) {
        self.cursor = 0;
    }

    fn end(&mut self) {
        self.cursor = self.chars.len();
    }

    fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
    }
}

enum Auth {
    Idle,
    Waiting {
        ticket: LoginTicket,
        note: Option<String>,
    },
    Failed(String),
}

#[derive(Clone, Copy)]
enum Task {
    Begin,
    Finish,
}

struct Wizard {
    step: Step,
    history: Vec<Step>,
    cursor: usize,
    input: Input,
    error: Option<String>,
    answers: Answers,
    tools: Vec<(&'static str, Option<String>)>,
    auth: Auth,
    busy: Option<&'static str>,
    pending: Option<Task>,
}

impl Wizard {
    fn new(tools: &[(&'static str, Option<String>)]) -> Wizard {
        let mut wizard = Wizard {
            step: Step::Welcome,
            history: Vec::new(),
            cursor: 0,
            input: Input::default(),
            error: None,
            answers: Answers::default(),
            tools: tools.to_vec(),
            auth: Auth::Idle,
            busy: None,
            pending: None,
        };
        wizard.enter(Step::Welcome);
        wizard
    }

    fn accent(&self) -> Color {
        let index = if self.step == Step::Accent {
            self.cursor
        } else {
            self.answers.accent
        };
        ACCENTS[index.min(ACCENTS.len() - 1)].1
    }

    fn cursor_for(&self, step: Step) -> usize {
        match step {
            Step::Accent => self.answers.accent,
            Step::Session => usize::from(!self.answers.restore),
            Step::AudioMode => usize::from(self.answers.download),
            Step::Cookies => self.answers.cookies,
            Step::Lastfm => usize::from(!self.answers.lastfm),
            Step::Discord => usize::from(!self.answers.discord),
            _ => 0,
        }
    }

    fn load_input(&mut self) {
        let text = match self.step {
            Step::Volume if self.answers.volume != DEFAULT_VOLUME => {
                self.answers.volume.to_string()
            }
            Step::DownloadDir => self.answers.download_dir.clone(),
            Step::LastfmKey => self.answers.api_key.clone(),
            Step::LastfmSecret => self.answers.api_secret.clone(),
            Step::DiscordId => self.answers.client_id.clone(),
            _ => String::new(),
        };
        self.input = Input::new(&text);
    }

    fn enter(&mut self, step: Step) {
        self.step = step;
        self.error = None;
        self.cursor = self.cursor_for(step);
        if step.is_text() {
            self.load_input();
        }
        if step == Step::LastfmAuth {
            self.start(Task::Begin);
        }
    }

    fn start(&mut self, task: Task) {
        self.busy = Some(match task {
            Task::Begin => "Contacting Last.fm...",
            Task::Finish => "Finishing the sign-in...",
        });
        self.pending = Some(task);
    }

    fn go(&mut self, step: Step) {
        self.history.push(self.step);
        self.enter(step);
    }

    fn replace(&mut self, step: Step) {
        self.enter(step);
    }

    fn back(&mut self) {
        if let Some(previous) = self.history.pop() {
            self.enter(previous);
        }
    }

    fn shift(&mut self, by: isize) {
        let count = choices(self.step).len();
        if count == 0 {
            return;
        }
        self.cursor = (self.cursor as isize + by).rem_euclid(count as isize) as usize;
    }

    fn open_link(&self) {
        if let Auth::Waiting { ticket, .. } = &self.auth {
            let _ = sys::open_url(ticket.url());
        }
    }

    fn run_task(&mut self, task: Task) {
        let config = self.answers.lastfm_config();
        match task {
            Task::Begin => {
                self.auth = match lastfm::begin_login(&config) {
                    Ok(ticket) => Auth::Waiting { ticket, note: None },
                    Err(e) => Auth::Failed(format!("{e:#}")),
                };
            }
            Task::Finish => {
                if let Auth::Waiting { ticket, .. } = std::mem::replace(&mut self.auth, Auth::Idle)
                {
                    match lastfm::finish_login(&config, &ticket) {
                        Ok(user) => {
                            self.answers.lastfm_user = Some(user);
                            self.replace(Step::Discord);
                        }
                        Err(e) => {
                            self.auth = Auth::Waiting {
                                ticket,
                                note: Some(format!("{e:#}")),
                            };
                        }
                    }
                }
            }
        }
        self.busy = None;
    }

    fn on_key(&mut self, key: KeyEvent) -> Option<Outcome> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return Some(Outcome::Quit);
        }
        if self.busy.is_some() {
            return None;
        }
        self.error = None;
        match key.code {
            KeyCode::Esc => {
                if self.history.is_empty() {
                    return Some(Outcome::Skipped);
                }
                self.back();
                return None;
            }
            KeyCode::Enter => return self.confirm(),
            _ => {}
        }
        if self.step.is_text() {
            self.on_text_key(key);
            return None;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => self.shift(1),
            KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => self.shift(-1),
            KeyCode::Char('g') | KeyCode::Home => self.cursor = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.cursor = choices(self.step).len().saturating_sub(1)
            }
            KeyCode::Char('h') | KeyCode::Left => self.back(),
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Char(' ') => return self.confirm(),
            KeyCode::Char('o') => {
                if self.step == Step::LastfmAuth {
                    self.open_link();
                }
            }
            _ => {}
        }
        None
    }

    fn on_text_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Char('a') if ctrl && !alt => self.input.home(),
            KeyCode::Char('e') if ctrl && !alt => self.input.end(),
            KeyCode::Char('u') if ctrl && !alt => self.input.clear(),
            KeyCode::Char(c) if !ctrl || alt => self.input.insert(c),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            _ => {}
        }
    }

    fn required(&mut self, message: &str) -> Option<String> {
        let text = self.input.value().trim().to_string();
        if text.is_empty() {
            self.error = Some(message.to_string());
            None
        } else {
            Some(text)
        }
    }

    fn confirm(&mut self) -> Option<Outcome> {
        match self.step {
            Step::Welcome => {
                if self.cursor == 0 {
                    self.go(Step::Requirements);
                } else {
                    return Some(Outcome::Skipped);
                }
            }
            Step::Requirements => self.go(Step::Accent),
            Step::Accent => {
                self.answers.accent = self.cursor;
                self.go(Step::Volume);
            }
            Step::Volume => {
                let raw = self.input.value();
                let text = raw.trim();
                if text.is_empty() {
                    self.answers.volume = DEFAULT_VOLUME;
                    self.go(Step::Session);
                } else {
                    match text.parse::<u8>() {
                        Ok(volume) if volume <= 100 => {
                            self.answers.volume = volume;
                            self.go(Step::Session);
                        }
                        _ => self.error = Some("Enter a number from 0 to 100.".into()),
                    }
                }
            }
            Step::Session => {
                self.answers.restore = self.cursor == 0;
                self.go(Step::AudioMode);
            }
            Step::AudioMode => {
                self.answers.download = self.cursor == 1;
                self.go(Step::DownloadDir);
            }
            Step::DownloadDir => {
                let text = self.input.value().trim().to_string();
                let probe = Audio {
                    download_dir: if text.is_empty() {
                        DEFAULT_DIR.to_string()
                    } else {
                        text.clone()
                    },
                    ..Audio::default()
                };
                match probe.download_directory() {
                    Ok(_) => {
                        self.answers.download_dir = text;
                        self.go(Step::Cookies);
                    }
                    Err(message) => self.error = Some(message),
                }
            }
            Step::Cookies => {
                self.answers.cookies = self.cursor;
                self.go(Step::Lastfm);
            }
            Step::Lastfm => {
                self.answers.lastfm = self.cursor == 0;
                self.go(if self.answers.lastfm {
                    Step::LastfmKey
                } else {
                    Step::Discord
                });
            }
            Step::LastfmKey => {
                if let Some(text) = self.required("Paste the API key from Last.fm.") {
                    self.answers.api_key = text;
                    self.go(Step::LastfmSecret);
                }
            }
            Step::LastfmSecret => {
                if let Some(text) = self.required("Paste the shared secret from Last.fm.") {
                    self.answers.api_secret = text;
                    self.go(Step::LastfmLogin);
                }
            }
            Step::LastfmLogin => {
                if self.cursor == 0 {
                    self.go(Step::LastfmAuth);
                } else {
                    self.go(Step::Discord);
                }
            }
            Step::LastfmAuth => {
                if matches!(self.auth, Auth::Waiting { .. }) {
                    self.start(Task::Finish);
                } else if matches!(self.auth, Auth::Failed(_)) {
                    self.auth = Auth::Idle;
                    self.start(Task::Begin);
                }
            }
            Step::Discord => {
                self.answers.discord = self.cursor == 0;
                self.go(if self.answers.discord {
                    Step::DiscordId
                } else {
                    Step::Review
                });
            }
            Step::DiscordId => {
                let text = self.input.value().trim().to_string();
                if text.len() >= 10 && text.chars().all(|c| c.is_ascii_digit()) {
                    self.answers.client_id = text;
                    self.go(Step::Review);
                } else {
                    self.error = Some("The Application ID is a long number, digits only.".into());
                }
            }
            Step::Review => match save(&self.answers) {
                Ok(_) => return Some(Outcome::Done),
                Err(e) => self.error = Some(format!("Couldn't save: {e:#}")),
            },
        }
        None
    }

    fn draw(&self, f: &mut Frame) {
        let accent = self.accent();
        let area = f.area();
        let width = CARD_WIDTH.min(area.width);
        let inner = usize::from(width.saturating_sub(4)).max(1);
        let lines = self.lines(accent, inner, area.height >= TALL_TERM);
        let height = u16::try_from(lines.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2)
            .min(area.height);
        let card = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        );
        let left = Line::from(vec![
            Span::styled(" ♪ ", tint(accent)),
            Span::styled("drift ", bold(accent)),
        ]);
        let right = Line::from(Span::styled(" setup ", dim())).right_aligned();
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(tint(accent))
            .title(left)
            .title(right)
            .padding(Padding::horizontal(1));
        f.render_widget(Paragraph::new(lines).block(block), card);
    }

    fn lines(&self, accent: Color, w: usize, tall: bool) -> Vec<Line<'static>> {
        let mut out = vec![self.progress(accent), Line::from("")];
        match self.step {
            Step::Welcome => self.welcome(&mut out, accent, w, tall),
            Step::Requirements => self.requirements(&mut out, accent, w),
            Step::LastfmAuth => self.auth_lines(&mut out, accent, w),
            Step::Review => self.review(&mut out, accent, w),
            step => {
                let (title, text) = copy(step);
                heading(&mut out, title, &text, w);
                if step.is_text() {
                    out.extend(self.field(accent, w));
                } else {
                    out.extend(self.options(accent));
                }
            }
        }
        if let Some(message) = &self.error {
            out.push(Line::from(""));
            for row in wrap(message, w) {
                out.push(Line::styled(row, tint(Color::Red)));
            }
        }
        out.push(Line::from(""));
        out.push(self.footer());
        out
    }

    fn progress(&self, accent: Color) -> Line<'static> {
        let at = self.step.section();
        let mut spans = Vec::new();
        for i in 0..SECTIONS.len() {
            let style = if i <= at { tint(accent) } else { dim() };
            spans.push(Span::styled("━━━", style));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(format!(" {}", SECTIONS[at]), plain()));
        Line::from(spans)
    }

    fn welcome(&self, out: &mut Vec<Line<'static>>, accent: Color, w: usize, tall: bool) {
        if tall {
            for row in DRIFT_ASCII {
                out.push(Line::styled(row, bold(accent)).alignment(Alignment::Center));
            }
            out.push(Line::from(""));
        }
        out.push(
            Line::styled("A YouTube Music player for your terminal.", strong())
                .alignment(Alignment::Center),
        );
        out.push(
            Line::styled("No account. No login. No browser.", dim()).alignment(Alignment::Center),
        );
        out.push(Line::from(""));
        for row in wrap(
            "Let's get you set up. Every question is optional and every default is fine.",
            w,
        ) {
            out.push(Line::styled(row, plain()));
        }
        out.push(Line::from(""));
        out.extend(self.options(accent));
    }

    fn requirements(&self, out: &mut Vec<Line<'static>>, accent: Color, w: usize) {
        let (title, text) = copy(Step::Requirements);
        heading(out, title, &text, w);
        for (name, version) in &self.tools {
            let mut spans = Vec::new();
            match version {
                Some(v) => {
                    spans.push(Span::styled("✓ ", tint(Color::Green)));
                    spans.push(Span::styled(format!("{name:<8}"), strong()));
                    let shown: String = v.chars().take(w.saturating_sub(12)).collect();
                    spans.push(Span::styled(shown, dim()));
                }
                None => {
                    spans.push(Span::styled("✗ ", tint(Color::Red)));
                    spans.push(Span::styled(format!("{name:<8}"), strong()));
                    spans.push(Span::styled("not found", tint(Color::Red)));
                }
            }
            out.push(Line::from(spans));
        }
        let missing: Vec<&&'static str> = self
            .tools
            .iter()
            .filter(|(_, version)| version.is_none())
            .map(|(name, _)| name)
            .collect();
        if missing.is_empty() {
            out.push(Line::from(""));
            out.push(Line::styled(
                "Everything drift needs is installed.",
                plain(),
            ));
        } else {
            out.push(Line::from(""));
            out.push(Line::styled(
                "Install what's missing, then start drift again:",
                plain(),
            ));
            for name in missing {
                out.push(Line::from(vec![
                    Span::styled(format!("  {name:<8}"), strong()),
                    Span::styled(install_hint(name), tint(accent)),
                ]));
            }
        }
    }

    fn auth_lines(&self, out: &mut Vec<Line<'static>>, accent: Color, w: usize) {
        if let Some(message) = self.busy {
            heading(out, "Last.fm", "", w);
            out.push(Line::from(vec![
                Span::styled("⠋ ", tint(accent)),
                Span::styled(message, plain()),
            ]));
            return;
        }
        match &self.auth {
            Auth::Idle => {}
            Auth::Waiting { ticket, note } => {
                heading(
                    out,
                    "Allow drift on Last.fm",
                    "Open this link, sign in and press Allow. Then come back here and press enter.",
                    w,
                );
                for row in wrap(ticket.url(), w) {
                    out.push(Line::styled(
                        row,
                        tint(accent).add_modifier(Modifier::UNDERLINED),
                    ));
                }
                if let Some(note) = note {
                    out.push(Line::from(""));
                    for row in wrap(note, w) {
                        out.push(Line::styled(row, tint(Color::Red)));
                    }
                }
            }
            Auth::Failed(message) => {
                heading(
                    out,
                    "Couldn't start the sign-in",
                    "Check your API key, shared secret and connection.",
                    w,
                );
                for row in wrap(message, w) {
                    out.push(Line::styled(row, tint(Color::Red)));
                }
            }
        }
    }

    fn review(&self, out: &mut Vec<Line<'static>>, accent: Color, w: usize) {
        let (title, text) = copy(Step::Review);
        heading(out, title, &text, w);
        for (label, value) in self.summary(accent) {
            let mut spans = vec![Span::styled(format!("{label:<14}"), dim())];
            spans.extend(value);
            out.push(Line::from(spans));
        }
        if let Some(path) = config_path() {
            out.push(Line::from(""));
            for row in wrap(&format!("Saving to {}", path.display()), w) {
                out.push(Line::styled(row, dim()));
            }
        }
    }

    fn summary(&self, accent: Color) -> Vec<(&'static str, Vec<Span<'static>>)> {
        let a = &self.answers;
        let (accent_name, accent_color) = ACCENTS[a.accent];
        let lastfm = match (a.lastfm, &a.lastfm_user) {
            (false, _) => "off".to_string(),
            (true, Some(user)) => format!("on, signed in as {user}"),
            (true, None) => "on, run drift --lastfm-login to sign in".to_string(),
        };
        let discord = if a.discord {
            format!("on, application {}", a.client_id)
        } else {
            "off".to_string()
        };
        let cookies = match BROWSERS[a.cookies].1 {
            "" => "off".to_string(),
            name => name.to_string(),
        };
        vec![
            (
                "Accent",
                vec![
                    Span::styled("● ", tint(accent_color)),
                    Span::styled(accent_name, strong()),
                ],
            ),
            (
                "Volume",
                vec![Span::styled(format!("{}%", a.volume), strong())],
            ),
            (
                "Session",
                vec![Span::styled(
                    if a.restore {
                        "restore queue and position"
                    } else {
                        "start fresh every time"
                    },
                    strong(),
                )],
            ),
            (
                "Playback",
                vec![Span::styled(
                    if a.download {
                        "download first"
                    } else {
                        "stream"
                    },
                    strong(),
                )],
            ),
            ("Downloads", vec![Span::styled(a.dir(), strong())]),
            ("Cookies", vec![Span::styled(cookies, strong())]),
            (
                "Last.fm",
                vec![Span::styled(
                    lastfm,
                    if a.lastfm_user.is_some() {
                        tint(Color::Green)
                    } else {
                        strong()
                    },
                )],
            ),
            ("Discord", vec![Span::styled(discord, tint(accent))]),
        ]
    }

    fn options(&self, accent: Color) -> Vec<Line<'static>> {
        choices(self.step)
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let on = i == self.cursor;
                let mut spans = vec![Span::styled(if on { "› " } else { "  " }, bold(accent))];
                if let Some(color) = c.color {
                    spans.push(Span::styled("● ", tint(color)));
                }
                spans.push(Span::styled(
                    c.label,
                    if on { bold(accent) } else { plain() },
                ));
                if !c.hint.is_empty() {
                    spans.push(Span::styled(format!("  {}", c.hint), dim()));
                }
                Line::from(spans)
            })
            .collect()
    }

    fn field(&self, accent: Color, w: usize) -> Vec<Line<'static>> {
        let masked = self.step == Step::LastfmSecret;
        let avail = w.saturating_sub(2).max(1);
        let mut shown: Vec<char> = self
            .input
            .chars
            .iter()
            .map(|&c| if masked { '•' } else { c })
            .collect();
        shown.push(' ');
        let cursor = self.input.cursor;
        let start = (cursor + 1).saturating_sub(avail);
        let end = (start + avail).min(shown.len());
        let slice = |from: usize, to: usize| -> String { shown[from..to].iter().collect() };

        let mut spans = vec![
            Span::styled("› ", bold(accent)),
            Span::styled(slice(start, cursor), strong()),
            Span::styled(
                slice(cursor, cursor + 1),
                Style::new().add_modifier(Modifier::REVERSED),
            ),
            Span::styled(slice(cursor + 1, end), strong()),
        ];
        if self.input.chars.is_empty() {
            let hint: String = placeholder(self.step)
                .chars()
                .take(avail.saturating_sub(1))
                .collect();
            spans.push(Span::styled(hint, dim()));
        }
        vec![Line::from(spans), Line::styled("─".repeat(w), tint(accent))]
    }

    fn footer(&self) -> Line<'static> {
        let keys: &[(&str, &str)] = match self.step {
            Step::Welcome => &[("j/k", "move"), ("enter", "select"), ("esc", "skip")],
            Step::Requirements => &[("enter", "continue"), ("esc", "back")],
            Step::Review => &[("enter", "save and start"), ("esc", "back")],
            Step::LastfmAuth => match self.auth {
                _ if self.busy.is_some() => &[],
                Auth::Waiting { .. } => &[
                    ("o", "open browser"),
                    ("enter", "I've allowed it"),
                    ("esc", "back"),
                ],
                Auth::Failed(_) => &[("enter", "retry"), ("esc", "back")],
                Auth::Idle => &[],
            },
            step if step.is_text() => &[("enter", "confirm"), ("esc", "back")],
            _ => &[("j/k", "move"), ("enter", "select"), ("esc", "back")],
        };
        let mut spans = Vec::new();
        for (key, action) in keys.iter().chain(&[("ctrl+c", "quit")]) {
            if !spans.is_empty() {
                spans.push(Span::styled(" · ", dim()));
            }
            spans.push(Span::styled(*key, plain()));
            spans.push(Span::styled(format!(" {action}"), dim()));
        }
        Line::from(spans)
    }
}

fn heading(out: &mut Vec<Line<'static>>, title: &str, text: &str, w: usize) {
    out.push(Line::styled(title.to_string(), strong()));
    if !text.is_empty() {
        out.push(Line::from(""));
        for row in wrap(text, w) {
            out.push(Line::styled(row, plain()));
        }
    }
    out.push(Line::from(""));
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut rest = word;
        while rest.chars().count() > width {
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            let cut = rest
                .char_indices()
                .nth(width)
                .map_or(rest.len(), |(i, _)| i);
            out.push(rest[..cut].to_string());
            rest = &rest[cut..];
        }
        let need = rest.chars().count() + usize::from(!line.is_empty());
        if line.chars().count() + need > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(rest);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

fn tint(color: Color) -> Style {
    Style::new().fg(color)
}

fn bold(color: Color) -> Style {
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

fn strong() -> Style {
    bold(Color::White)
}

fn plain() -> Style {
    tint(Color::Gray)
}

fn dim() -> Style {
    tint(Color::DarkGray)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_long_words() {
        assert_eq!(wrap("aaaaaaaaaa bb", 4), vec!["aaaa", "aaaa", "aa", "bb"]);
    }

    #[test]
    fn answers_render_into_a_valid_config() {
        let answers = Answers {
            accent: 2,
            volume: 35,
            restore: false,
            download: true,
            download_dir: "~/Songs".into(),
            cookies: 1,
            lastfm: true,
            api_key: " key ".into(),
            api_secret: "secret".into(),
            discord: true,
            client_id: "123456789012345678".into(),
            ..Answers::default()
        };
        let c: Config = toml::from_str(&answers.render()).unwrap();
        assert_eq!(c.ui.accent, Color::Magenta);
        assert_eq!(c.playback.volume, 35);
        assert!(!c.playback.restore_session);
        assert!(c.audio.download);
        assert_eq!(c.audio.download_dir, "~/Songs");
        assert_eq!(c.audio.cookies(), Some("firefox"));
        assert_eq!(c.lastfm.credentials(), Some(("key", "secret")));
        assert_eq!(c.discord.client_id, "123456789012345678");
    }

    #[test]
    fn default_answers_match_default_config() {
        let c: Config = toml::from_str(&Answers::default().render()).unwrap();
        assert_eq!(c.ui.accent, Color::Cyan);
        assert_eq!(c.playback.volume, DEFAULT_VOLUME);
        assert!(c.playback.restore_session);
        assert!(!c.audio.download);
        assert_eq!(c.audio.download_dir, DEFAULT_DIR);
        assert_eq!(c.lastfm.credentials(), None);
        assert!(c.discord.client_id.is_empty());
    }
}
