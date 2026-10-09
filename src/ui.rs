use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Table, Tabs};
use ratatui::Frame;

use crate::app::{App, Level, Mode, PlayState, View};
use crate::model::{fmt_time, Track};

const ACCENT: Color = Color::Cyan;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn selected_style() -> Style {
    Style::new().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)
}

fn header_style() -> Style {
    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
}

fn dim() -> Style {
    Style::new().fg(Color::DarkGray)
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let [top, body, playing, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(4),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_tabs(f, app, top);
    match app.view {
        View::Results => draw_tracks(f, app, body, View::Results),
        View::Queue => draw_tracks(f, app, body, View::Queue),
        View::Playlists => draw_playlists(f, app, body),
    }
    draw_now_playing(f, app, playing);
    draw_cmdline(f, app, bottom);
    if app.mode == Mode::Help {
        draw_help(f, app, area);
    }
    if app.mode == Mode::ArtistPick {
        draw_artist_picker(f, app, area);
    }
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let [name, tabs, right] = Layout::horizontal([
        Constraint::Length(9),
        Constraint::Min(10),
        Constraint::Length(34),
    ])
    .areas(area);

    f.render_widget(Paragraph::new(" ♪ Drift").style(Style::new().fg(ACCENT).bold()), name);

    let titles = [
        format!(" Results {} ", app.results.len()),
        format!(" Queue {} ", app.queue.len()),
        format!(" Playlists {} ", app.store.playlists.len()),
    ];
    f.render_widget(
        Tabs::new(titles)
            .select(app.view.idx())
            .style(Style::new().fg(Color::Gray))
            .highlight_style(Style::new().fg(Color::Black).bg(ACCENT).bold())
            .divider(" "),
        tabs,
    );

    if let Some(busy) = &app.busy {
        let spin = SPINNER[app.tick % SPINNER.len()];
        f.render_widget(
            Paragraph::new(format!("{spin} {busy}… "))
                .alignment(Alignment::Right)
                .style(Style::new().fg(Color::Yellow)),
            right,
        );
    }
}

fn track_rows<'a>(
    tracks: &'a [Track],
    playing: Option<(&str, &'a str)>,
    show_album: bool,
) -> Vec<Row<'a>> {
    tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let is_playing = playing.is_some_and(|(id, _)| id == t.id);
            let marker = if is_playing { playing.map_or("", |p| p.1) } else { "" };
            let mut cells = vec![
                Cell::from(marker),
                Cell::from(format!("{:>3}", i + 1)).style(dim()),
                Cell::from(t.title.as_str()),
                Cell::from(t.artist.as_str()),
            ];
            if show_album {
                cells.push(Cell::from(t.album.as_deref().unwrap_or("")));
            }
            cells.push(
                Cell::from(t.duration.map_or(String::new(), |d| fmt_time(d as f64)))
                    .style(dim()),
            );
            let row = Row::new(cells);
            if is_playing {
                row.style(Style::new().fg(Color::Green).add_modifier(Modifier::BOLD))
            } else {
                row
            }
        })
        .collect()
}

fn track_table<'a>(rows: Vec<Row<'a>>, show_album: bool, block: Block<'a>) -> Table<'a> {
    let mut header = vec!["", "  #", "Title", "Artist"];
    let mut widths = vec![
        Constraint::Length(2),
        Constraint::Length(4),
        Constraint::Fill(3),
        Constraint::Fill(2),
    ];
    if show_album {
        header.push("Album");
        widths.push(Constraint::Fill(2));
    }
    header.push("Time");
    widths.push(Constraint::Length(7));

    Table::new(rows, widths)
        .header(Row::new(header).style(header_style()))
        .block(block)
        .column_spacing(1)
        .row_highlight_style(selected_style())
}

fn empty_hint(f: &mut Frame, block: Block, area: Rect, lines: &[&str]) {
    let text: Vec<Line> = lines.iter().map(|l| Line::from(*l).style(dim())).collect();
    f.render_widget(
        Paragraph::new(text).alignment(Alignment::Center).block(block),
        area,
    );
}

fn draw_tracks(f: &mut Frame, app: &mut App, area: Rect, view: View) {
    let show_album = area.width >= 76 && view == View::Results;
    let marker = match app.play_state {
        PlayState::Paused => "⏸",
        PlayState::Loading => "…",
        _ => "▶",
    };
    let now_id = app
        .now_track()
        .filter(|_| app.play_state != PlayState::Stopped)
        .map(|t| t.id.clone());
    let playing = now_id.as_deref().map(|id| (id, marker));

    let (tracks, title) = match view {
        View::Queue => (&app.queue, " Queue ".to_string()),
        _ => (&app.results, format!(" {} ", app.results_title)),
    };
    let block = Block::bordered()
        .title(format!("{title}({}) ", tracks.len()))
        .border_style(Style::new().fg(ACCENT));

    if tracks.is_empty() {
        let hint: &[&str] = if view == View::Queue {
            &["", "The queue is empty.", "Press Enter on a search result, or `a` to add it."]
        } else {
            &["", "No results yet.", "Press / and type a query, or use  :search \"daft punk\""]
        };
        return empty_hint(f, block, area, hint);
    }
    let rows = track_rows(tracks, playing, show_album);
    let table = track_table(rows, show_album, block);
    let idx = view.idx();
    f.render_stateful_widget(table, area, &mut app.states[idx]);
}

fn draw_open_playlist(f: &mut Frame, app: &mut App, area: Rect, name: &str) {
    let marker = match app.play_state {
        PlayState::Paused => "⏸",
        PlayState::Loading => "…",
        _ => "▶",
    };
    let now_id = app
        .now_track()
        .filter(|_| app.play_state != PlayState::Stopped)
        .map(|t| t.id.clone());
    let playing = now_id.as_deref().map(|id| (id, marker));

    let Some(tracks) = app.store.playlists.get(name) else { return };
    let block = Block::bordered()
        .title(format!(" {name} ({}) ", tracks.len()))
        .border_style(Style::new().fg(ACCENT));
    if tracks.is_empty() {
        return empty_hint(
            f,
            block,
            area,
            &["", "This playlist is empty.", "Add songs with  :addto \"name\"  from Results or Queue."],
        );
    }
    let show_album = area.width >= 76;
    let rows = track_rows(tracks, playing, show_album);
    let table = track_table(rows, show_album, block);
    f.render_stateful_widget(table, area, &mut app.pl_state);
}

fn draw_playlists(f: &mut Frame, app: &mut App, area: Rect) {
    if app.in_playlist() {
        if let Some(name) = app.open.clone() {
            return draw_open_playlist(f, app, area, &name);
        }
    }
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)]).areas(area);

    let block = Block::bordered()
        .title(format!(" Playlists ({}) ", app.store.playlists.len()))
        .border_style(Style::new().fg(ACCENT));

    if app.store.playlists.is_empty() {
        empty_hint(
            f,
            block,
            left,
            &["", "No playlists yet.", "Queue some songs, then", ":save \"name\""],
        );
        let b = Block::bordered().title(" Preview ").border_style(dim());
        f.render_widget(b, right);
        return;
    }

    let rows: Vec<Row> = app
        .store
        .playlists
        .iter()
        .enumerate()
        .map(|(i, (name, tracks))| {
            Row::new(vec![
                Cell::from(format!("{:>2}", i + 1)).style(dim()),
                Cell::from(name.as_str()),
                Cell::from(format!("{}", tracks.len())).style(dim()),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [Constraint::Length(3), Constraint::Fill(1), Constraint::Length(5)],
    )
    .header(Row::new(vec!["", "Name", "Songs"]).style(header_style()))
    .block(block)
    .row_highlight_style(selected_style());
    f.render_stateful_widget(table, left, &mut app.states[View::Playlists.idx()]);

    let sel = app.selected();
    let preview_block = Block::bordered().title(" Preview ").border_style(dim());
    match app.playlist_name(sel).and_then(|n| app.store.playlists.get(&n)) {
        Some(tracks) if !tracks.is_empty() => {
            let show_album = right.width >= 70;
            let rows = track_rows(tracks, None, show_album);
            f.render_widget(track_table(rows, show_album, preview_block), right);
        }
        _ => f.render_widget(preview_block, right),
    }
}

fn draw_now_playing(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered()
        .title(" Now Playing ")
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [l1, l2] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);

    let mut flags = vec![format!("vol {}%", app.volume)];
    if app.shuffle {
        flags.push("shuffle".into());
    }
    if app.repeat != crate::model::RepeatMode::Off {
        flags.push(format!("repeat:{}", app.repeat.label()));
    }
    let flags = flags.join(" · ");
    let flags_w = flags.chars().count() as u16 + 1;
    let [title_area, flags_area] =
        Layout::horizontal([Constraint::Min(1), Constraint::Length(flags_w)]).areas(l1);
    f.render_widget(Paragraph::new(flags).style(dim()).alignment(Alignment::Right), flags_area);

    let icon = match app.play_state {
        PlayState::Playing => "▶".to_string(),
        PlayState::Paused => "⏸".to_string(),
        PlayState::Loading => SPINNER[app.tick % SPINNER.len()].to_string(),
        PlayState::Stopped => "■".to_string(),
    };
    let line = match (app.now_track(), app.play_state) {
        (Some(t), s) if s != PlayState::Stopped => Line::from(vec![
            Span::styled(format!("{icon} "), Style::new().fg(Color::Green)),
            Span::styled(t.title.clone(), Style::new().bold()),
            Span::styled(format!("  {}", t.artist), Style::new().fg(Color::Gray)),
        ]),
        (Some(t), _) => Line::from(vec![
            Span::styled(format!("{icon} "), dim()),
            Span::styled(format!("Last played: {} — {}", t.title, t.artist), dim()),
        ]),
        _ => Line::from(vec![
            Span::styled(format!("{icon} "), dim()),
            Span::styled("Nothing playing", dim()),
        ]),
    };
    f.render_widget(Paragraph::new(line), title_area);

    let left = format!(" {} ", fmt_time(app.position));
    let right = format!(" {} ", if app.duration > 0.0 { fmt_time(app.duration) } else { "--:--".into() });
    let bar_w = (l2.width as usize).saturating_sub(left.chars().count() + right.chars().count());
    let ratio = if app.duration > 0.0 {
        (app.position / app.duration).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let filled = (ratio * bar_w as f64).round() as usize;
    let line = Line::from(vec![
        Span::raw(left),
        Span::styled("━".repeat(filled), Style::new().fg(ACCENT)),
        Span::styled("─".repeat(bar_w.saturating_sub(filled)), dim()),
        Span::raw(right),
    ]);
    f.render_widget(Paragraph::new(line), l2);
}

fn draw_cmdline(f: &mut Frame, app: &App, area: Rect) {
    if app.mode == Mode::Command {
        let text = format!(":{}", app.cmd_buf);
        let x = area.x + text.chars().count() as u16;
        f.render_widget(Paragraph::new(text), area);
        f.set_cursor_position((x.min(area.right().saturating_sub(1)), area.y));
        return;
    }

    let line = match app.visible_status() {
        Some(s) => {
            let style = match s.level {
                Level::Error => Style::new().fg(Color::Red).bold(),
                Level::Info => Style::new().fg(Color::Green),
            };
            Line::styled(format!(" {}", s.text), style)
        }
        None => Line::default(),
    };
    f.render_widget(Paragraph::new(line), area);

    let mut pending = String::new();
    if let Some(c) = app.count {
        pending.push_str(&c.to_string());
    }
    if let Some(p) = app.pending {
        pending.push(p);
    }
    if !pending.is_empty() {
        f.render_widget(
            Paragraph::new(format!("{pending} ")).alignment(Alignment::Right).style(Style::new().fg(Color::Yellow)),
            area,
        );
    }
}

const HELP: &[(&str, &str)] = &[
    ("THIS WINDOW", "#"),
    ("j k  PgUp PgDn", "scroll"),
    ("q  Esc  ?", "close"),
    ("NAVIGATION", "#"),
    ("j k / ↓ ↑", "move (counts work: 5j)"),
    ("gg  G", "top / bottom (5G → row 5)"),
    ("Ctrl-d  Ctrl-u", "half page down / up"),
    ("Tab  S-Tab", "cycle Results / Queue / Playlists"),
    ("gr  gq  gp", "jump to Results / Queue / Playlists"),
    ("PLAYBACK", "#"),
    ("Enter", "play selected (on a playlist: load it; inside one: play from there)"),
    ("Space", "pause / resume"),
    ("n  N  (or > <)", "next / previous track"),
    ("h l  / ← →", "seek -5s / +5s (counts: 6l)"),
    ("+  -", "volume up / down"),
    ("s  r", "toggle shuffle / cycle repeat"),
    ("ga  or  :artist", "songs by the artist of the selected song; a popup asks which one when there are several"),
    ("QUEUE", "#"),
    ("a  A  P", "add selected / add all / play all results"),
    ("dd  x", "remove from queue / delete playlist / delete track in a playlist"),
    ("J K", "move queue or playlist item down / up"),
    ("PLAYLISTS", "#"),
    ("→ or l", "open the highlighted playlist"),
    ("← h Esc Backspace", "back to the playlist list"),
    ("COMMANDS  ( : or / )", "#"),
    (":search \"q\"", "search YouTube Music (/ prefills it)"),
    (":play [N]", "play or resume; N plays row N"),
    (":pause :resume :stop", "playback state"),
    (":next  :prev", "skip"),
    (":seek 90 | 1:30 | +10", "absolute or relative seek"),
    (":vol 50 | +5", "set or change the volume"),
    (":shuffle  :repeat [m]", "modes: off, all or one"),
    (":add [N]  :addall", "queue a result / all results"),
    (":playall  :remove [N]", "play all results / remove from queue"),
    (":clear", "empty the queue"),
    (":save n  :load n", "save the queue as a playlist / load one"),
    (":append n  :delpl n", "append a playlist to the queue / delete it"),
    (":addto n", "add the selected track to playlist n"),
    (":import URL", "load a YouTube / YT Music playlist"),
    (":artist", "same as ga"),
    (":quit  (or q, Ctrl-c)", ""),
];

const HELP_KEY_W: usize = 24;

fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let cur_len = cur.chars().count();
        if !cur.is_empty() && cur_len + 1 + word.chars().count() > width {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn help_lines(inner_w: usize) -> Vec<Line<'static>> {
    let key_w = HELP_KEY_W.min(inner_w / 2).max(1);
    let desc_w = inner_w.saturating_sub(key_w);
    let key_style = Style::new().fg(ACCENT);
    let mut lines: Vec<Line<'static>> = Vec::new();

    for (k, d) in HELP {
        if *d == "#" {
            if !lines.is_empty() {
                lines.push(Line::raw(""));
            }
            lines.push(Line::styled(k.to_string(), header_style()));
            continue;
        }
        let mut desc = wrap_words(d, desc_w).into_iter();
        if k.chars().count() < key_w {
            let first = desc.next().unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(format!("{k:<key_w$}"), key_style),
                Span::raw(first),
            ]));
        } else {
            for part in wrap_words(k, inner_w) {
                lines.push(Line::styled(part, key_style));
            }
        }
        for rest in desc {
            lines.push(Line::from(vec![Span::raw(" ".repeat(key_w)), Span::raw(rest)]));
        }
    }
    lines
}

fn draw_artist_picker(f: &mut Frame, app: &App, area: Rect) {
    let n = app.artist_opts.len() as u16;
    let widest = app.artist_opts.iter().map(|a| a.chars().count()).max().unwrap_or(0) as u16;
    let w = (widest + 10).max(34).min(area.width);
    let h = (n + 4).min(area.height);
    let popup = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    let mut lines: Vec<Line> = app
        .artist_opts
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let text = format!(" {} {name}", i + 1);
            if i == app.artist_sel {
                Line::styled(text, selected_style())
            } else {
                Line::raw(text)
            }
        })
        .collect();
    lines.push(Line::styled(" Enter/1-9 select · Esc cancel", dim()));
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" Go to which artist? ")
                .border_style(Style::new().fg(Color::Yellow)),
        ),
        popup,
    );
}

fn draw_help(f: &mut Frame, app: &mut App, area: Rect) {
    let w = area.width.min(80);
    let lines = help_lines(w.saturating_sub(2) as usize);
    let h = (lines.len() as u16).saturating_add(2).min(area.height);
    let popup = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };

    let visible = h.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(visible);
    app.help_scroll = app.help_scroll.min(max_scroll);
    let scroll = app.help_scroll;

    let mut block = Block::bordered()
        .title(" Help — q or Esc to close ")
        .border_style(Style::new().fg(Color::Yellow));
    if max_scroll > 0 {
        let end = (scroll + visible).min(lines.len());
        block = block.title_bottom(format!(" j/k scroll · {}-{} of {} ", scroll + 1, end, lines.len()));
    }

    let shown: Vec<Line> = lines.into_iter().skip(scroll).take(visible).collect();
    f.render_widget(Clear, popup);
    f.render_widget(Paragraph::new(shown).block(block), popup);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use std::sync::mpsc;

    fn text(term: &Terminal<TestBackend>) -> String {
        term.backend().buffer().content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn open_playlist_renders() {
        let dir = std::env::temp_dir().join(format!("drift-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &dir);

        let (tx, _rx) = mpsc::channel();
        let mut app = App::new(tx);
        let t = |id: &str, title: &str| Track {
            id: id.into(),
            title: title.into(),
            artist: "Someone".into(),
            album: None,
            duration: Some(90),
        };
        app.store.playlists.insert("empty".into(), vec![]);
        app.store.playlists.insert("mix".into(), vec![t("aaaaaaaaaaa", "First Song"), t("bbbbbbbbbbb", "Second Song")]);
        let key = |app: &mut App, c| app.on_key(KeyEvent::new(c, KeyModifiers::NONE));

        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Tab);
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(text(&term).contains("Playlists"));

        key(&mut app, KeyCode::Right);
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(text(&term).contains("This playlist is empty"));
        key(&mut app, KeyCode::Left);

        key(&mut app, KeyCode::Down);
        key(&mut app, KeyCode::Right);
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = text(&term);
        assert!(s.contains("First Song") && s.contains("Second Song") && s.contains("mix"));
        assert!(!s.contains("Preview"));

        let mut tiny = Terminal::new(TestBackend::new(20, 6)).unwrap();
        tiny.draw(|f| draw(f, &mut app)).unwrap();

        key(&mut app, KeyCode::Char('?'));
        term.draw(|f| draw(f, &mut app)).unwrap();
        let s = text(&term);
        assert!(s.contains("THIS WINDOW") && !s.contains(":quit"));
        key(&mut app, KeyCode::Char('G'));
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(text(&term).contains(":quit"));
        tiny.draw(|f| draw(f, &mut app)).unwrap();
        let mut narrow = Terminal::new(TestBackend::new(12, 5)).unwrap();
        narrow.draw(|f| draw(f, &mut app)).unwrap();
        key(&mut app, KeyCode::Char('q'));

    }
}
