use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Cell, Clear, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

use crate::app::{App, Level, LyricsState, MenuAction, Mode, PlayState, View, NO_KEY};
use crate::lyrics::Lyrics;
use crate::model::{fmt_time, Track};

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/* Cursor row: accent-colored bold text plus a bar, no filled background. */
const CURSOR: &str = "▎";

fn selected_style(accent: Color) -> Style {
    Style::new().fg(accent).add_modifier(Modifier::BOLD)
}

/* Swap the leading space of a menu line for the cursor bar. */
fn cursor_text(text: &str) -> String {
    match text.strip_prefix(' ') {
        Some(rest) => format!("{CURSOR}{rest}"),
        None => text.to_string(),
    }
}

fn header_style() -> Style {
    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
}

fn dim() -> Style {
    Style::new().fg(Color::DarkGray)
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let [top, body, playing] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(4),
    ])
    .areas(area);

    if app.mode == Mode::Lyrics {
        draw_lyrics_header(f, app, top);
        draw_lyrics(f, app, body, true);
    } else {
        draw_tabs(f, app, top);
        let list = if app.lyrics_panel && body.width >= 70 {
            let [list, side] =
                Layout::horizontal([Constraint::Min(40), Constraint::Percentage(40)]).areas(body);
            draw_lyrics(f, app, side, false);
            list
        } else {
            body
        };
        match app.view {
            View::Results => draw_tracks(f, app, list, View::Results),
            View::Queue => draw_tracks(f, app, list, View::Queue),
            View::Playlists => draw_playlists(f, app, list),
        }
    }
    draw_now_playing(f, app, playing);
    if app.mode == Mode::Command {
        draw_palette(f, app, area);
    }
    if app.mode == Mode::ConfirmDelete {
        draw_confirm(f, app, area);
    }
    if app.mode == Mode::Menu {
        draw_menu(f, app, area);
    }
    if app.mode == Mode::Help {
        draw_help(f, app, area);
    }
    if app.mode == Mode::ArtistPick {
        draw_artist_picker(f, app, area);
    }
    if app.mode == Mode::CollectionPick {
        draw_collection_picker(f, app, area);
    }
    if app.mode == Mode::Info {
        draw_info(f, app, area);
    }
    draw_toast(f, app, body);
}

fn hints(items: &[(&str, &str)], width: u16) -> Option<Line<'static>> {
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    let mut len = 2usize;
    for (i, (key, label)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", dim()));
            len += 2;
        }
        spans.push(Span::styled(key.to_string(), Style::new().fg(Color::Yellow)));
        spans.push(Span::styled(format!(" {label}"), dim()));
        len += key.chars().count() + 1 + label.chars().count();
    }
    spans.push(Span::raw(" "));
    (len + 4 <= width as usize).then(|| Line::from(spans).right_aligned())
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let [name, tabs, right] = Layout::horizontal([
        Constraint::Length(10),
        Constraint::Min(10),
        Constraint::Length(app.busy.as_ref().map_or(0, |b| b.chars().count() as u16 + 5)),
    ])
    .areas(area);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ♪ ", Style::new().fg(app.accent)),
            Span::styled("Drift", Style::new().fg(app.accent).bold()),
        ])),
        name,
    );

    let titles = [
        ("Results", app.results.len()),
        ("Queue", app.queue.len()),
        ("Playlists", app.store.playlists.len()),
    ];
    let active = Style::new()
        .fg(app.accent)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let mut spans: Vec<Span> = Vec::new();
    for (i, (title, count)) in titles.into_iter().enumerate() {
        spans.push(Span::raw("  "));
        if i == app.view.idx() {
            spans.push(Span::styled(format!("{title} {count}"), active));
        } else {
            spans.push(Span::styled(title, Style::new().fg(Color::Gray)));
            spans.push(Span::styled(format!(" {count}"), dim()));
        }
        spans.push(Span::raw("  "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), tabs);

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

fn playing_marker(app: &App) -> Option<(String, &'static str)> {
    let id = app
        .now_track()
        .filter(|_| app.play_state != PlayState::Stopped)?
        .id
        .clone();
    let marker = match app.play_state {
        PlayState::Paused => "⏸",
        PlayState::Loading => "…",
        _ => "▶",
    };
    Some((id, marker))
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
            let marker = if is_playing {
                playing.map_or("", |p| p.1)
            } else {
                ""
            };
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
                Cell::from(t.duration.map_or(String::new(), |d| fmt_time(d as f64))).style(dim()),
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

fn track_table<'a>(rows: Vec<Row<'a>>, show_album: bool, block: Block<'a>, accent: Color) -> Table<'a> {
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
        .row_highlight_style(selected_style(accent))
        .highlight_symbol(CURSOR)
}

fn empty_hint(f: &mut Frame, block: Block, area: Rect, lines: &[&str]) {
    let text: Vec<Line> = lines.iter().map(|l| Line::from(*l).style(dim())).collect();
    f.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .block(block),
        area,
    );
}

fn draw_tracks(f: &mut Frame, app: &mut App, area: Rect, view: View) {
    let show_album = area.width >= 76 && view == View::Results;
    let now = playing_marker(app);
    let playing = now.as_ref().map(|(id, marker)| (id.as_str(), *marker));

    let (tracks, title) = match view {
        View::Queue => (&app.queue, " Queue ".to_string()),
        _ => (&app.results, format!(" {} ", app.results_title)),
    };
    let mut block = Block::bordered()
        .title(format!("{title}({}) ", tracks.len()))
        .border_style(Style::new().fg(app.accent));
    if !tracks.is_empty() {
        let keys: &[(&str, &str)] = if view == View::Queue {
            &[("⏎", "play"), ("x", "remove"), ("J/K", "move"), ("G", "menu"), ("?", "help")]
        } else {
            &[("⏎", "play"), ("a", "queue"), ("G", "menu"), ("/", "search"), ("?", "help")]
        };
        if let Some(h) = hints(keys, area.width) {
            block = block.title_bottom(h);
        }
    }

    if tracks.is_empty() {
        let hint: &[&str] = if view == View::Queue {
            &[
                "",
                "The queue is empty.",
                "Press `a` on a search result to queue it.",
            ]
        } else {
            &[
                "",
                "No results yet.",
                "Press / and type a query, or use  :search \"daft punk\"",
            ]
        };
        return empty_hint(f, block, area, hint);
    }
    let rows = track_rows(tracks, playing, show_album);
    let table = track_table(rows, show_album, block, app.accent);
    let idx = view.idx();
    f.render_stateful_widget(table, area, &mut app.states[idx]);
}

fn draw_open_playlist(f: &mut Frame, app: &mut App, area: Rect, name: &str) {
    let now = playing_marker(app);
    let playing = now.as_ref().map(|(id, marker)| (id.as_str(), *marker));

    let Some(tracks) = app.store.playlists.get(name) else {
        return;
    };
    let mut block = Block::bordered()
        .title(format!(" {name} ({}) ", tracks.len()))
        .border_style(Style::new().fg(app.accent));
    if !tracks.is_empty() {
        let keys = [("⏎", "play"), ("a", "queue"), ("x", "remove"), ("G", "menu"), ("←", "back")];
        if let Some(h) = hints(&keys, area.width) {
            block = block.title_bottom(h);
        }
    }
    if tracks.is_empty() {
        return empty_hint(
            f,
            block,
            area,
            &[
                "",
                "This playlist is empty.",
                "Add songs with  :addto \"name\"  from Results or Queue.",
            ],
        );
    }
    let show_album = area.width >= 76;
    let rows = track_rows(tracks, playing, show_album);
    let table = track_table(rows, show_album, block, app.accent);
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

    let mut block = Block::bordered()
        .title(format!(" Playlists ({}) ", app.store.playlists.len()))
        .border_style(Style::new().fg(app.accent));
    if !app.store.playlists.is_empty() {
        if let Some(h) = hints(&[("⏎", "load"), ("→", "open"), ("G", "menu")], left.width) {
            block = block.title_bottom(h);
        }
    }

    if app.store.playlists.is_empty() {
        empty_hint(
            f,
            block,
            left,
            &[
                "",
                "No playlists yet.",
                "Queue some songs, then",
                ":save \"name\"",
            ],
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
        [
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Length(5),
        ],
    )
    .header(Row::new(vec!["", "Name", "Songs"]).style(header_style()))
    .block(block)
    .row_highlight_style(selected_style(app.accent))
    .highlight_symbol(CURSOR);
    f.render_stateful_widget(table, left, &mut app.states[View::Playlists.idx()]);

    let sel = app.selected();
    let preview_block = Block::bordered().title(" Preview ").border_style(dim());
    match app
        .playlist_name(sel)
        .and_then(|n| app.store.playlists.get(&n))
    {
        Some(tracks) if !tracks.is_empty() => {
            let show_album = right.width >= 70;
            let rows = track_rows(tracks, None, show_album);
            f.render_widget(track_table(rows, show_album, preview_block, app.accent), right);
        }
        _ => f.render_widget(preview_block, right),
    }
}

const BARS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

fn gradient(_t: f64, accent: Color) -> Color {
    accent
}

fn equalizer(app: &App, n: usize) -> Vec<Span<'static>> {
    let t = app.tick as f64;
    (0..n)
        .map(|i| {
            let x = i as f64;
            let level = match app.play_state {
                PlayState::Playing => {
                    let v = ((t * 0.45 + x * 1.3).sin() + (t * 0.23 + x * 2.1).sin()) / 4.0 + 0.5;
                    (v.clamp(0.0, 1.0) * 7.0).round() as usize
                }
                _ => 0,
            };
            let color = if app.play_state == PlayState::Playing {
                gradient(x / n as f64, app.accent)
            } else {
                Color::DarkGray
            };
            Span::styled(BARS[level], Style::new().fg(color))
        })
        .collect()
}

fn draw_now_playing(f: &mut Frame, app: &App, area: Rect) {
    let active = app.play_state != PlayState::Stopped;
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if active { app.accent } else { Color::DarkGray }))
        .title(Line::from(vec![
            Span::styled(" ♪ ", Style::new().fg(app.accent).bold()),
            Span::styled("Now Playing ", Style::new().fg(app.accent).bold()),
        ]));

    let mut pending = String::new();
    if let Some(c) = app.count {
        pending.push_str(&c.to_string());
    }
    if let Some(p) = app.pending {
        pending.push(p);
    }
    if !pending.is_empty() {
        block = block.title_bottom(
            Line::styled(format!(" {pending} "), Style::new().fg(Color::Yellow).bold())
                .right_aligned(),
        );
    }

    let inner = block.inner(area);
    f.render_widget(block, area);
    let [l1, l2] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);

    let on = Style::new().fg(app.accent).bold();
    let vol_cells = 5usize;
    let vol_fill = ((app.volume as usize * vol_cells) + 50) / 100;
    let mut flags = vec![
        Span::styled("⇄", if app.shuffle { on } else { dim() }),
        Span::raw("  "),
        Span::styled(
            match app.repeat {
                crate::model::RepeatMode::Off => "↻".to_string(),
                m => format!("↻ {}", m.label()),
            },
            if app.repeat != crate::model::RepeatMode::Off {
                on
            } else {
                dim()
            },
        ),
        Span::raw("   "),
        Span::styled("vol ", dim()),
    ];
    for i in 0..vol_cells {
        flags.push(if i < vol_fill.min(vol_cells) {
            Span::styled("━", Style::new().fg(gradient(i as f64 / vol_cells as f64, app.accent)))
        } else {
            Span::styled("─", dim())
        });
    }
    flags.push(Span::styled(format!(" {:>3}%", app.volume), Style::new().fg(Color::Gray)));
    flags.push(Span::raw(" "));
    let flags_w: usize = flags.iter().map(|s| s.content.chars().count()).sum();
    let [title_area, flags_area] =
        Layout::horizontal([Constraint::Min(1), Constraint::Length(flags_w as u16)]).areas(l1);
    f.render_widget(Paragraph::new(Line::from(flags)), flags_area);

    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    match app.play_state {
        PlayState::Loading => spans.push(Span::styled(
            format!("{}      ", SPINNER[app.tick % SPINNER.len()]),
            Style::new().fg(Color::Yellow),
        )),
        PlayState::Playing => {
            spans.extend(equalizer(app, 6));
            spans.push(Span::raw(" "));
        }
        PlayState::Paused => {
            spans.push(Span::styled("⏸ ", Style::new().fg(Color::Yellow)));
            spans.push(Span::raw("     "));
        }
        _ => {
        }
    }
    spans.push(Span::raw(" "));
    match (app.now_track(), active) {
        (Some(t), true) => {
            spans.push(Span::styled(t.title.clone(), Style::new().fg(Color::White).bold()));
            spans.push(Span::styled("  ·  ", dim()));
            spans.push(Span::styled(t.artist.clone(), Style::new().fg(app.accent)));
        }
        (Some(t), false) => spans.push(Span::styled(
            format!("Last played: {} — {}", t.title, t.artist),
            dim(),
        )),
        _ => spans.push(Span::styled("Nothing playing  ·  press / to search", dim())),
    }
    f.render_widget(Paragraph::new(Line::from(spans)), title_area);

    let left = format!(" {} ", fmt_time(app.position));
    let right = format!(
        " {} ",
        if app.duration > 0.0 {
            fmt_time(app.duration)
        } else {
            "--:--".into()
        }
    );
    let bar_w = (l2.width as usize).saturating_sub(left.chars().count() + right.chars().count());
    let ratio = if app.duration > 0.0 {
        (app.position / app.duration).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let head = ((ratio * bar_w as f64) as usize).min(bar_w.saturating_sub(1));
    let mut line = vec![Span::styled(left, Style::new().fg(Color::Gray))];
    for i in 0..bar_w {
        line.push(if !active || app.duration <= 0.0 {
            Span::styled("─", dim())
        } else if i < head {
            Span::styled("━", Style::new().fg(gradient(i as f64 / bar_w.max(1) as f64, app.accent)))
        } else if i == head {
            Span::styled("●", Style::new().fg(Color::White).bold())
        } else {
            Span::styled("─", dim())
        });
    }
    line.push(Span::styled(right, Style::new().fg(Color::Gray)));
    f.render_widget(Paragraph::new(Line::from(line)), l2);
}

fn draw_lyrics_header(f: &mut Frame, app: &App, area: Rect) {
    let unsynced = matches!(&app.lyrics, LyricsState::Ready(l) if !l.synced);
    let mut title = vec![
        Span::styled(" Drift", Style::new().fg(app.accent).bold()),
        Span::styled("  ·  Lyrics", dim()),
    ];
    if unsynced {
        title.push(Span::styled("  ·  not synced", dim()));
    }
    f.render_widget(Paragraph::new(Line::from(title)), area);

    let mut keys = vec![("space", "pause"), ("h l", "seek"), ("n N", "skip")];
    if unsynced {
        keys.insert(0, ("j k", "scroll"));
    }
    keys.push(("Esc", "close"));
    if let Some(h) = hints(&keys, area.width.saturating_sub(30)) {
        f.render_widget(Paragraph::new(h), area);
    }
}

fn lyric_rows(
    lyrics: &Lyrics,
    position: f64,
    width: usize,
    roomy: bool,
    accent: Color,
) -> (Vec<Line<'static>>, Option<usize>) {
    let active = lyrics.active(position);
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut focus = None;
    for (i, line) in lyrics.lines.iter().enumerate() {
        let parts = wrap_words(&line.text, width);
        let is_active = active == Some(i);
        if is_active {
            focus = Some(rows.len() + parts.len().max(1) / 2);
        }
        if parts.is_empty() {
            rows.push(if is_active {
                Line::styled("♪", Style::new().fg(accent).bold())
            } else {
                Line::raw("")
            });
            continue;
        }
        if is_active {
            let total: usize = parts.iter().map(|p| p.chars().count()).sum();
            let mut fill = (lyrics.progress(i, position) * total as f64).round() as usize;
            for part in parts {
                let done = fill.min(part.chars().count());
                fill -= done;
                let sung: String = part.chars().take(done).collect();
                let rest: String = part.chars().skip(done).collect();
                rows.push(Line::from(vec![
                    Span::styled(sung, Style::new().fg(accent).bold()),
                    Span::styled(rest, Style::new().fg(Color::White).bold()),
                ]));
            }
        } else {
            let ahead = active.map_or(i as isize + 1, |a| i as isize - a as isize);
            let color = match (lyrics.synced, ahead) {
                (false, _) | (_, 1..=2) => Color::Gray,
                _ => Color::DarkGray,
            };
            rows.extend(parts.into_iter().map(|p| Line::styled(p, Style::new().fg(color))));
        }
        if roomy {
            rows.push(Line::raw(""));
        }
    }
    (rows, focus)
}

fn draw_lyrics(f: &mut Frame, app: &mut App, area: Rect, full: bool) {
    let inner = if full {
        let w = area.width.saturating_sub(4).min(76);
        Rect {
            x: area.x + (area.width - w) / 2,
            width: w,
            ..area
        }
    } else {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(dim())
            .title(Span::styled(" ♪ Lyrics ", Style::new().fg(app.accent).bold()));
        let inner = block.inner(area);
        f.render_widget(block, area);
        inner.inner(Margin::new(1, 0))
    };
    if inner.width < 4 || inner.height == 0 {
        return;
    }

    let now = app.now_track().map(|t| t.id.clone());
    let playing = now.is_some() && app.play_state != PlayState::Stopped;
    let fresh = playing && app.lyrics_id == now;
    let notice: Option<Vec<Line>> = match &app.lyrics {
        _ if !playing => Some(vec![Line::styled("Nothing playing", dim())]),
        LyricsState::Ready(_) if fresh => None,
        LyricsState::Missing if fresh => Some(vec![
            Line::styled("No lyrics found", Style::new().fg(Color::Gray)),
            Line::styled("not every song has them", dim()),
        ]),
        LyricsState::Failed(e) if fresh => Some(vec![
            Line::styled("Couldn't load lyrics", Style::new().fg(Color::Red)),
            Line::styled(e.clone(), dim()),
        ]),
        _ => Some(vec![Line::styled(
            format!("{} Searching lyrics…", SPINNER[app.tick % SPINNER.len()]),
            Style::new().fg(Color::Yellow),
        )]),
    };
    if let Some(lines) = notice {
        let h = (lines.len() as u16 + 1).min(inner.height);
        let rect = Rect {
            y: inner.y + (inner.height - h) / 2,
            height: h,
            ..inner
        };
        f.render_widget(
            Paragraph::new(lines)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            rect,
        );
        return;
    }

    let LyricsState::Ready(lyrics) = &app.lyrics else {
        return;
    };
    let (rows, focus) = lyric_rows(lyrics, app.position, inner.width as usize, full, app.accent);
    let height = inner.height as usize;
    let max_start = rows.len().saturating_sub(height);
    let start = if lyrics.synced {
        focus.map_or(0, |r| r.saturating_sub(height / 2))
    } else {
        app.lyrics_scroll
    }
    .min(max_start);
    app.lyrics_scroll = start;
    let visible: Vec<Line> = rows.into_iter().skip(start).take(height).collect();
    let align = if full { Alignment::Center } else { Alignment::Left };
    f.render_widget(Paragraph::new(visible).alignment(align), inner);
}

fn draw_toast(f: &mut Frame, app: &App, body: Rect) {
    let Some(s) = app.visible_status() else {
        return;
    };
    if body.width < 16 || body.height < 5 {
        return;
    }
    let (icon, color) = match s.level {
        Level::Error => ("✖", Color::Red),
        Level::Info => ("●", Color::Green),
    };
    let max_text = (body.width as usize).saturating_sub(8).min(120);
    let mut text: String = s.text.lines().next().unwrap_or("").to_string();
    if text.chars().count() > max_text {
        text = text.chars().take(max_text.saturating_sub(1)).collect::<String>() + "…";
    }
    let w = (text.chars().count() as u16 + 6).min(body.width);
    let rect = Rect::new(body.right().saturating_sub(w + 1).max(body.x), body.bottom().saturating_sub(4), w, 3);
    f.render_widget(Clear, rect);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!(" {icon} "), Style::new().fg(color).bold()),
            Span::styled(text, Style::new().fg(Color::White)),
        ]))
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(color)),
        ),
        rect,
    );
}

fn draw_info(f: &mut Frame, app: &App, area: Rect) {
    let Some(t) = &app.info_track else {
        return;
    };
    let rows: Vec<(&str, String)> = vec![
        ("Title", t.title.clone()),
        ("Artist", t.artist.clone()),
        ("Album", t.album.clone().unwrap_or_else(|| "—".into())),
        ("Length", t.duration.map_or("—".into(), |d| fmt_time(d as f64))),
        ("ID", t.id.clone()),
    ];
    let label_w = 8usize;
    let widest = rows.iter().map(|(_, v)| v.chars().count()).max().unwrap_or(0);
    let w = ((label_w + widest + 6).max(40) as u16).min(area.width.min(80));
    let value_w = (w as usize).saturating_sub(label_w + 4).max(1);
    let mut lines: Vec<Line> = Vec::new();
    for (k, v) in &rows {
        let mut parts = wrap_words(v, value_w).into_iter();
        let first = parts.next().unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled(format!(" {k:<label_w$}"), header_style()),
            Span::styled(first, Style::new().fg(Color::White)),
        ]));
        for rest in parts {
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(label_w + 1)),
                Span::styled(rest, Style::new().fg(Color::White)),
            ]));
        }
    }
    let popup = popup_rect(area, w, lines.len() as u16 + 2, false);
    if popup.width < 12 || popup.height < 3 {
        return;
    }
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(app.accent))
                .title(Span::styled(" ♪ Song info ", Style::new().fg(app.accent).bold()))
                .title_bottom(Line::styled(" any key to close ", dim()).right_aligned()),
        ),
        popup,
    );
}

fn popup_rect(area: Rect, want_w: u16, want_h: u16, top_third: bool) -> Rect {
    let w = want_w.min(area.width);
    let x = area.x + (area.width - w) / 2;
    let y = if top_third {
        area.y + area.height / 5
    } else {
        area.y + area.height.saturating_sub(want_h) / 2
    };
    let h = want_h.min(area.bottom().saturating_sub(y));
    Rect::new(x, y, w, h)
}

const CMD_INFO: &[(&str, &str)] = &[
    ("search", "search YouTube Music"),
    ("play", "play / resume  ·  play N plays row N"),
    ("pause", "pause playback"),
    ("resume", "resume playback"),
    ("toggle", "toggle pause"),
    ("stop", "stop playback"),
    ("next", "skip to the next track"),
    ("prev", "go back to the previous track"),
    ("quit", "exit drift"),
    ("add", "queue the selected result  ·  add N"),
    ("addall", "queue all results"),
    ("playall", "play all results"),
    ("remove", "remove from the queue  ·  remove N"),
    ("clear", "empty the queue"),
    ("results", "show the Results tab"),
    ("queue", "show the Queue tab"),
    ("playlists", "show the Playlists tab"),
    ("volume", "set volume  ·  50 | +5 | -5"),
    ("seek", "seek  ·  90 | 1:30 | +10 | -10"),
    ("shuffle", "toggle shuffle"),
    ("repeat", "repeat  ·  off | all | one"),
    ("save", "save the queue as a playlist"),
    ("savealbum", "save album as a playlist  ·  savealbum [name]"),
    ("saveresults", "save results as a playlist  ·  saveresults [name]"),
    ("load", "load a playlist into the queue"),
    ("append", "append a playlist to the queue"),
    ("delpl", "delete a playlist"),
    ("addto", "add the selected track to a playlist"),
    ("import", "import a YouTube / YT Music playlist URL"),
    ("artist", "songs by the selected artist"),
    ("similar", "radio: songs similar to the selected one"),
    ("album", "open the selected song's album, or :album <query> to search"),
    ("playlist", "search YouTube Music playlists"),
    ("lyrics", "show or hide the lyrics panel"),
    ("help", "show key bindings"),
];

fn cmd_info(name: &str) -> &'static str {
    CMD_INFO
        .iter()
        .find(|(n, _)| *n == name)
        .map_or("", |(_, d)| *d)
}

fn draw_palette(f: &mut Frame, app: &App, area: Rect) {
    const MAX_ROWS: usize = 8;
    let search = app.cmd_search;
    let matches = app.command_matches();
    let recent = if search && app.cmd_buf.is_empty() {
        app.recent_searches(5)
    } else {
        Vec::new()
    };
    let first_word = app.cmd_buf.split_whitespace().next().unwrap_or("");
    let typing_args = !search && app.cmd_buf.contains(' ');

    let list_rows = if search {
        recent.len().max(1)
    } else if typing_args {
        1
    } else {
        matches.len().clamp(1, MAX_ROWS)
    };
    let want_h = 2 + 1 + 1 + list_rows as u16;
    let popup = popup_rect(area, 64, want_h, true);
    if popup.width < 12 || popup.height < 3 {
        return;
    }

    let (title, prompt) = if search {
        (" Search YouTube Music ", " / ")
    } else {
        (" Command ", " : ")
    };
    let footer = if search {
        " ⏎ search · ↑↓ history · Esc close "
    } else {
        " ⏎ run · Tab complete · Esc close "
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if search { app.accent } else { app.accent }))
        .title(Span::styled(
            title,
            Style::new()
                .fg(if search { app.accent } else { app.accent })
                .bold(),
        ))
        .title_bottom(Line::styled(footer, dim()).right_aligned());
    let inner = block.inner(popup);
    f.render_widget(Clear, popup);
    f.render_widget(block, popup);

    let [input, sep, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);

    let prompt_w = prompt.chars().count();
    let avail = (input.width as usize).saturating_sub(prompt_w + 1);
    let chars: Vec<char> = app.cmd_buf.chars().collect();
    let shown: String = chars[chars.len().saturating_sub(avail)..].iter().collect();
    let shown_w = shown.chars().count();
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                prompt,
                Style::new().fg(Color::Black).bg(if search { app.accent } else { app.accent }).bold(),
            ),
            Span::raw(" "),
            Span::styled(shown, Style::new().fg(Color::White)),
        ])),
        input,
    );
    if input.width as usize > prompt_w + 1 {
        f.set_cursor_position((input.x + (prompt_w + 1 + shown_w) as u16, input.y));
    }
    f.render_widget(
        Paragraph::new("─".repeat(sep.width as usize)).style(dim()),
        sep,
    );

    let mut lines: Vec<Line> = Vec::new();
    if search {
        if recent.is_empty() {
            lines.push(Line::styled(" type what you want to hear, then press Enter", dim()));
        } else {
            for r in recent {
                lines.push(Line::from(vec![
                    Span::styled(" ↺ ", dim()),
                    Span::styled(r.to_string(), Style::new().fg(Color::Gray)),
                ]));
            }
        }
    } else if typing_args {
        let d = cmd_info(first_word);
        lines.push(if d.is_empty() {
            Line::styled(" ⏎ run", dim())
        } else {
            Line::from(vec![
                Span::styled(format!(" {first_word} "), Style::new().fg(app.accent).bold()),
                Span::styled(d, dim()),
            ])
        });
    } else if matches.is_empty() {
        lines.push(Line::styled(" no matching command", Style::new().fg(Color::Red)));
    } else {
        let sel = app.cycling().then_some(app.cmd_sel);
        let start = sel.map_or(0, |s| (s + 1).saturating_sub(MAX_ROWS));
        let width = list.width as usize;
        for (i, name) in matches.iter().enumerate().skip(start).take(MAX_ROWS) {
            let text = format!(" {name:<10}{}", cmd_info(name));
            let text: String = text.chars().take(width).collect();
            lines.push(if Some(i) == sel {
                let pad = width.saturating_sub(text.chars().count());
                Line::styled(format!("{}{}", cursor_text(&text), " ".repeat(pad)), selected_style(app.accent))
            } else {
                Line::from(vec![
                    Span::styled(
                        text.chars().take(11).collect::<String>(),
                        Style::new().fg(app.accent),
                    ),
                    Span::styled(text.chars().skip(11).collect::<String>(), dim()),
                ])
            });
        }
    }
    f.render_widget(Paragraph::new(lines), list);
}

fn draw_menu(f: &mut Frame, app: &App, area: Rect) {
    let items = &app.menu_items;
    if items.is_empty() {
        return;
    }
    let label_w = items.iter().map(|i| i.label.chars().count()).max().unwrap_or(0);
    let detail_w = items.iter().map(|i| i.detail.chars().count()).max().unwrap_or(0);
    let body_w = 5 + label_w + if detail_w > 0 { detail_w + 3 } else { 0 } + 2;
    let title_w = app.menu_title.chars().count() + 6;
    let w = body_w.max(title_w).clamp(34, 60) as u16;
    let popup = popup_rect(area, w, items.len() as u16 + 2, false);
    if popup.width < 12 || popup.height < 3 {
        return;
    }

    let max_title = (popup.width as usize).saturating_sub(8);
    let mut title: String = app.menu_title.chars().take(max_title).collect();
    if app.menu_title.chars().count() > max_title {
        title.push('…');
    }
    let title = if app.menu_sub {
        format!(" {title} ")
    } else {
        format!(" ♪ {title} ")
    };
    let footer = if app.menu_sub {
        " ⏎ add · Esc back "
    } else {
        " ⏎ select · Esc close "
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(app.accent))
        .title(Span::styled(title, Style::new().fg(app.accent).bold()))
        .title_bottom(Line::styled(footer, dim()).right_aligned());
    let inner = block.inner(popup);
    f.render_widget(Clear, popup);
    f.render_widget(block, popup);

    let width = inner.width as usize;
    let accent = app.accent;
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let key = if it.key == NO_KEY { ' ' } else { it.key };
            let label = format!("{:<label_w$}", it.label);
            let selected = i == app.menu_sel;
            if selected {
                let mut text = format!(" {key}  {label}");
                if !it.detail.is_empty() {
                    text.push_str(&format!("   {}", it.detail));
                }
                let text: String = text.chars().take(width).collect();
                let pad = width.saturating_sub(text.chars().count());
                return Line::styled(format!("{}{}", cursor_text(&text), " ".repeat(pad)), selected_style(accent));
            }
            let danger = matches!(
                it.action,
                MenuAction::Remove | MenuAction::DeletePlaylist | MenuAction::ClearQueue
            );
            let label_style = if danger {
                Style::new().fg(Color::Red)
            } else {
                Style::new().fg(Color::White)
            };
            let mut spans = vec![
                Span::raw(" "),
                Span::styled(key.to_string(), Style::new().fg(Color::Yellow).bold()),
                Span::raw("  "),
                Span::styled(label, label_style),
            ];
            if !it.detail.is_empty() {
                spans.push(Span::styled(format!("   {}", it.detail), dim()));
            }
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_confirm(f: &mut Frame, app: &App, area: Rect) {
    let name = app.pending_delete.as_deref().unwrap_or_default();
    let msg = format!(" Delete playlist \u{201c}{name}\u{201d}? ");
    let w = (msg.chars().count() as u16 + 4).max(34);
    let popup = popup_rect(area, w, 5, false);
    if popup.width < 12 || popup.height < 3 {
        return;
    }
    let lines = vec![
        Line::styled(msg, Style::new().fg(Color::White).bold()),
        Line::from(vec![
            Span::styled(" y ", Style::new().fg(Color::Black).bg(Color::Red).bold()),
            Span::styled(" delete   ", dim()),
            Span::styled(" any other key ", Style::new().fg(Color::Gray)),
            Span::styled(" cancel", dim()),
        ]),
    ];
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(" Confirm ")
                .border_style(Style::new().fg(Color::Yellow)),
        ),
        popup,
    );
}

const HELP: &[(&str, &str)] = &[
    ("THIS WINDOW", "#"),
    ("j k  PgUp PgDn", "scroll"),
    ("q  Esc  ?", "close"),
    ("NAVIGATION", "#"),
    ("j k / ↓ ↑", "move (counts work: 5j)"),
    ("gg  ge  Home  End", "top / bottom (5gg or 5ge → row 5)"),
    ("Ctrl-d  Ctrl-u", "half page down / up"),
    ("Tab  S-Tab", "cycle Results / Queue / Playlists"),
    ("gr  gq  gp", "jump to Results / Queue / Playlists"),
    ("PLAYBACK", "#"),
    (
        "Enter",
        "play selected (on a playlist: load it; inside one: play from there)",
    ),
    ("Space", "pause / resume"),
    ("n  N  (or > <)", "next / previous track"),
    ("h l  / ← →", "seek -5s / +5s (counts: 6l)"),
    ("+  -", "volume up / down"),
    ("s  r", "toggle shuffle / cycle repeat"),
    ("L  F", "lyrics side panel / fullscreen lyrics (Esc closes)"),
    (
        "ga  or  :artist",
        "songs by the artist of the selected song; a popup asks which one when there are several",
    ),
    (
        "gb  or  :album",
        "open the album of the selected song; :album <query> searches albums",
    ),
    (":playlist <query>", "search YouTube Music playlists (pick one, then ⏎)"),
    (
        "gs  or  :similar",
        "radio: load songs similar to the selected one and start playing them",
    ),
    ("SONG MENU  ( G )", "#"),
    ("G", "menu for the highlighted song or playlist"),
    ("j k  ⏎", "move / choose (the yellow letter is a shortcut)"),
    ("p n a g s b l", "play · next · queue · artist · similar · album · playlist"),
    ("x i", "remove · song info"),
    ("Esc  q  G", "close (Esc goes back from a sub-menu)"),
    ("QUEUE", "#"),
    ("a  A  P", "add selected / add all / play all results"),
    (
        "dd  x",
        "remove from queue / delete playlist / delete track in a playlist",
    ),
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
    (
        ":playall  :remove [N]",
        "play all results / remove from queue",
    ),
    (":clear", "empty the queue"),
    (
        ":save n  :load n",
        "save the queue as a playlist / load one (:save! n replaces an existing one)",
    ),
    (
        ":savealbum [n]",
        "save album or current album results as a playlist",
    ),
    (
        ":saveresults [n]",
        "save current results as a playlist",
    ),
    (
        ":append n  :delpl n",
        "append a playlist to the queue / delete it",
    ),
    (":addto n", "add the selected track to playlist n"),
    (":import URL", "load a YouTube / YT Music playlist"),
    (":artist", "same as ga"),
    (":similar", "same as gs (also :radio)"),
    (":album [q]", "same as gb, or search albums"),
    (":lyrics", "same as L"),
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

fn help_lines(inner_w: usize, accent: Color) -> Vec<Line<'static>> {
    let key_w = HELP_KEY_W.min(inner_w / 2).max(1);
    let desc_w = inner_w.saturating_sub(key_w);
    let key_style = Style::new().fg(accent);
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
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(key_w)),
                Span::raw(rest),
            ]));
        }
    }
    lines
}

fn draw_artist_picker(f: &mut Frame, app: &App, area: Rect) {
    let n = app.artist_opts.len() as u16;
    let widest = app
        .artist_opts
        .iter()
        .map(|a| a.chars().count())
        .max()
        .unwrap_or(0) as u16;
    let w = (widest + 10).max(34).min(area.width);
    let h = (n + 3).min(area.height);
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
                Line::styled(cursor_text(&text), selected_style(app.accent))
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

fn draw_collection_picker(f: &mut Frame, app: &App, area: Rect) {
    let n = app.picks.len();
    let w = area.width.min(78);
    let h = (n as u16).saturating_add(3).min(area.height);
    let visible = h.saturating_sub(3) as usize;
    let start = if app.pick_sel < visible {
        0
    } else {
        app.pick_sel + 1 - visible
    };
    let popup = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    let inner = w.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = app
        .picks
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .map(|(i, c)| {
            let mut text = format!(" {:>2} {}", i + 1, c.title);
            if !c.subtitle.is_empty() {
                text.push_str(&format!("  ·  {}", c.subtitle));
            }
            let text: String = text.chars().take(inner).collect();
            if i == app.pick_sel {
                let pad = inner.saturating_sub(text.chars().count());
                Line::styled(format!("{}{}", cursor_text(&text), " ".repeat(pad)), selected_style(app.accent))
            } else {
                Line::raw(text)
            }
        })
        .collect();
    lines.push(Line::styled(
        " Enter select · s save playlist · j k move · Esc cancel",
        dim(),
    ));
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(format!(" {} ", app.pick_title))
                .border_style(Style::new().fg(Color::Yellow)),
        ),
        popup,
    );
}

fn draw_help(f: &mut Frame, app: &mut App, area: Rect) {
    let w = area.width.min(80);
    let lines = help_lines(w.saturating_sub(2) as usize, app.accent);
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
        block = block.title_bottom(format!(
            " j/k scroll · {}-{} of {} ",
            scroll + 1,
            end,
            lines.len()
        ));
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
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn open_playlist_renders() {
        let dir = std::env::temp_dir().join(format!("drift-ui-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("DRIFT_CONFIG_DIR", &dir);
        std::env::set_var("DRIFT_STATE_DIR", &dir);

        let (tx, _rx) = mpsc::channel();
        let mut app = App::new(tx);
        let t = |id: &str, title: &str| Track {
            id: id.into(),
            title: title.into(),
            artist: "Someone".into(),
            artists: Vec::new(),
            album: None,
            duration: Some(90),
        };
        app.store.playlists.insert("empty".into(), vec![]);
        app.store.playlists.insert(
            "mix".into(),
            vec![
                t("aaaaaaaaaaa", "First Song"),
                t("bbbbbbbbbbb", "Second Song"),
            ],
        );
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

        key(&mut app, KeyCode::Char(':'));
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(text(&term).contains("Command"));
        key(&mut app, KeyCode::Tab);
        term.draw(|f| draw(f, &mut app)).unwrap();
        tiny.draw(|f| draw(f, &mut app)).unwrap();
        key(&mut app, KeyCode::Esc);
        key(&mut app, KeyCode::Char('/'));
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(text(&term).contains("Search YouTube Music"));
        tiny.draw(|f| draw(f, &mut app)).unwrap();
        key(&mut app, KeyCode::Esc);

        key(&mut app, KeyCode::Char('G'));
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(text(&term).contains("Play now"));
        tiny.draw(|f| draw(f, &mut app)).unwrap();
        key(&mut app, KeyCode::Char('l'));
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(text(&term).contains("New playlist"));
        key(&mut app, KeyCode::Esc);
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(text(&term).contains("Play now"));
        key(&mut app, KeyCode::Esc);
        term.draw(|f| draw(f, &mut app)).unwrap();
        assert!(!text(&term).contains("Play now"));

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

        let _ = std::fs::remove_dir_all(&dir);
    }
}
