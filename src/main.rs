mod app;
mod command;
mod config;
mod model;
mod player;
mod session;
mod store;
mod ui;
mod ytmusic;

use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event};
use ratatui::DefaultTerminal;

use app::{App, AppEvent};

const USAGE: &str = "\
drift — Vim-style terminal player for YouTube Music (no login needed)

USAGE:
    drift [QUERY...]      start the player, optionally running a search first
    drift --check         verify that mpv and yt-dlp are installed
    drift --help          show this help
    drift --version

Inside the player press ? for key bindings and :help for commands.";

fn tool_version(name: &str) -> Option<String> {
    let out = Command::new(name)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
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
        ("mpv", "sudo apt install mpv | brew install mpv | sudo pacman -S mpv"),
        ("yt-dlp", "pipx install yt-dlp | brew install yt-dlp | sudo pacman -S yt-dlp"),
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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-h") | Some("--help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some("-V") | Some("--version") => {
            println!("drift {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Some("--check") => {
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
        _ => {}
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

    let initial = (!args.is_empty()).then(|| args.join(" "));
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

    let mut terminal = ratatui::init();
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
