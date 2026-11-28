mod app;
mod command;
mod config;
mod cover;
mod discord;
mod download;
mod history;
mod keys;
mod lastfm;
mod lyrics;
mod model;
mod mpris;
mod player;
mod session;
mod setup;
mod store;
mod sys;
mod ui;
mod ytmusic;

use std::io;
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use clap::{CommandFactory, Parser};
use clap_complete::Shell;
use ratatui::crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use app::{App, AppEvent};

#[derive(Parser)]
#[command(
    version,
    about = "A vim-style terminal player for YouTube Music (no login needed)",
    after_help = "Inside the player press ? for key bindings and :help for commands."
)]
struct Cli {
    #[arg(
        value_name = "QUERY",
        conflicts_with = "action",
        help = "Start the player and run this search first"
    )]
    query: Vec<String>,

    #[arg(
        long,
        group = "action",
        help = "Verify that mpv and yt-dlp are installed"
    )]
    check: bool,

    #[arg(
        long,
        group = "action",
        help = "Sign in to Last.fm so plays are scrobbled"
    )]
    lastfm_login: bool,

    #[arg(long, group = "action", help = "Sign out of Last.fm")]
    lastfm_logout: bool,

    #[arg(
        long,
        group = "action",
        value_name = "SHELL",
        help = "Print a shell completion script"
    )]
    completions: Option<Shell>,
}

fn tool_version(name: &str) -> Option<String> {
    let mut cmd = Command::new(name);
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    let out = sys::no_window(&mut cmd).output().ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    })
}

fn missing_dependencies(verbose: bool) -> Vec<String> {
    let mut missing = Vec::new();
    for (tool, hint) in [
        (
            "mpv",
            "sudo apt install mpv | brew install mpv | sudo pacman -S mpv | winget install mpv.net / scoop install mpv",
        ),
        (
            "yt-dlp",
            "pipx install yt-dlp | brew install yt-dlp | sudo pacman -S yt-dlp | winget install yt-dlp.yt-dlp / scoop install yt-dlp",
        ),
    ] {
        match tool_version(tool) {
            Some(v) => {
                if verbose {
                    println!("ok       {tool}: {v}");
                }
            }
            None => {
                if verbose {
                    println!("MISSING  {tool}");
                }
                missing.push(format!("  {tool:<7} not found. Install: {hint}"));
            }
        }
    }
    missing
}

fn exit_code(result: Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if let Some(shell) = cli.completions {
        clap_complete::generate(shell, &mut Cli::command(), "drift", &mut io::stdout());
        return ExitCode::SUCCESS;
    }
    if cli.check {
        let missing = missing_dependencies(true);
        return if missing.is_empty() {
            println!("All dependencies found.");
            ExitCode::SUCCESS
        } else {
            for m in &missing {
                eprintln!("{m}");
            }
            ExitCode::FAILURE
        };
    }
    if cli.lastfm_login {
        let (cfg, warning) = config::Config::load();
        if let Some(w) = warning {
            eprintln!("warning: {w}");
        }
        return exit_code(lastfm::login(&cfg.lastfm));
    }
    if cli.lastfm_logout {
        return exit_code(lastfm::logout());
    }

    if setup::needed() {
        let tools = [
            ("mpv", tool_version("mpv")),
            ("yt-dlp", tool_version("yt-dlp")),
        ];
        match setup::run(&tools) {
            Ok(setup::Outcome::Quit) => return ExitCode::SUCCESS,
            Ok(_) => {}
            Err(e) => eprintln!("warning: setup failed: {e:#}"),
        }
    }

    let missing = missing_dependencies(false);
    if !missing.is_empty() {
        eprintln!("drift needs these programs to search and play music:\n");
        for m in &missing {
            eprintln!("{m}");
        }
        eprintln!("\nSee the README for details.");
        return ExitCode::FAILURE;
    }

    let initial = (!cli.query.is_empty()).then(|| cli.query.join(" "));
    match run(initial) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(initial_query: Option<String>) -> Result<()> {
    let (tx, rx) = mpsc::channel::<AppEvent>();
    let mut app = App::new(tx);
    if let Some(q) = initial_query {
        app.run_command(&format!("search {q}"));
    }

    let mut terminal = ratatui::try_init()?;
    if let Ok(picker) = ratatui_image::picker::Picker::from_query_stdio() {
        app.set_image_picker(picker);
    }
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("main") {
            hook(info);
        }
    }));
    let result = event_loop(&mut terminal, &mut app, &rx);
    ratatui::restore();
    app.shutdown();
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    rx: &mpsc::Receiver<AppEvent>,
) -> Result<()> {
    while !app.should_quit {
        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                app.on_key(key);
            }
        }
        while let Ok(ev) = rx.try_recv() {
            app.on_event(ev);
        }
        app.on_tick();
    }
    Ok(())
}
