use crate::{
    app::{App, COMMANDS, Focus, Modal, View},
    model::{Anime, ProviderId},
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint as C, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

const ACTIVE: Color = Color::Green;
const MUTED: Color = Color::DarkGray;
const ACCENT: Color = Color::Cyan;
const SELECTED: Color = Color::Rgb(20, 25, 155);
const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];
fn dim(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(MUTED))
}
fn accent(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().fg(ACCENT))
}
fn panel(title: impl Into<String>, active: bool) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title.into())
        .border_style(Style::default().fg(if active { ACTIVE } else { Color::Gray }))
        .title_style(
            Style::default()
                .fg(if active { ACTIVE } else { Color::Reset })
                .add_modifier(if active {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        )
}
fn count_footer(index: Option<usize>, count: usize) -> Line<'static> {
    Line::from(format!(
        "{} of {}",
        index.map_or(0, |i| (i + 1).min(count)),
        count
    ))
    .alignment(Alignment::Right)
}
fn selected_style(active: bool) -> Style {
    Style::default()
        .bg(if active { SELECTED } else { Color::DarkGray })
        .fg(Color::White)
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 60 || area.height < 18 {
        frame.render_widget(
            Paragraph::new("anime-levisione\n\nResize to at least 60 × 18.\nPress q to quit.")
                .block(panel(" anime-levisione ", true))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let root = Layout::vertical([C::Min(1), C::Length(1)]).split(area);
    let columns = Layout::horizontal([C::Percentage(35), C::Percentage(65)])
        .spacing(1)
        .split(root[0]);
    if area.width >= 100 && area.height >= 30 {
        let left = Layout::vertical([
            C::Length(5),
            C::Fill(4),
            C::Fill(2),
            C::Fill(2),
            C::Length(4),
        ])
        .spacing(1)
        .split(columns[0]);
        search(frame, app, left[0]);
        results(frame, app, left[1]);
        library_panel(frame, app, View::Continue, left[2]);
        library_panel(frame, app, View::Watchlist, left[3]);
        library_panel(frame, app, View::Offline, left[4]);
    } else {
        let left = Layout::vertical([C::Length(5), C::Min(1)]).split(columns[0]);
        search(frame, app, left[0]);
        if matches!(app.view, View::Discover | View::Search) {
            results(frame, app, left[1]);
        } else {
            library_panel(frame, app, app.view, left[1]);
        }
    }
    let right = Layout::vertical([
        C::Fill(5),
        C::Fill(4),
        C::Length(if app.playing || app.downloading { 4 } else { 3 }),
    ])
    .spacing(1)
    .split(columns[1]);
    details(frame, app, right[0]);
    episodes(frame, app, right[1]);
    log(frame, app, right[2]);
    footer(frame, app, root[1]);
    if let Some(modal) = app.modal {
        popup(frame, app, modal);
    }
}
fn search(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Search;
    let block = panel(
        format!(
            "[1]-Search{}",
            if app.demo {
                " · DEMO"
            } else if app.config.private {
                " · PRIVATE"
            } else {
                ""
            }
        ),
        focused,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let query = if app.query.value.is_empty() {
        dim("/ to search anime…")
    } else {
        Span::raw(app.query.value.clone())
    };
    let available = inner.width.saturating_sub(2) as usize;
    let scroll = app
        .query
        .column()
        .saturating_sub(available.saturating_sub(1));
    if inner.height > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(vec![accent("> "), query]))
                .scroll((0, scroll.min(u16::MAX as usize) as u16)),
            Rect { height: 1, ..inner },
        );
    }
    if inner.height > 1 {
        let mut sources = vec![];
        for id in ProviderId::ALL {
            let status = app.statuses.iter().find(|s| s.provider == id);
            let (label, color) = match status {
                Some(s) if s.pending => (
                    format!("{} {}  ", id.short(), SPINNER[app.tick / 3 % 4]),
                    Color::Yellow,
                ),
                Some(s) if s.error.is_some() => (format!("{} !  ", id.short()), Color::Red),
                Some(s) => (format!("{} {}  ", id.short(), s.count), ACTIVE),
                None => (format!("{}  ", id.short()), MUTED),
            };
            sources.push(Span::styled(label, Style::default().fg(color)));
        }
        if app.search_pending.is_some() {
            sources.push(dim("typing…"));
        }
        frame.render_widget(
            Paragraph::new(Line::from(sources)),
            Rect {
                y: inner.y + 1,
                height: 1,
                ..inner
            },
        );
    }
    if inner.height > 2 {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                dim("p: "),
                accent(
                    app.provider_filter
                        .map_or("All sources".to_string(), |p| p.to_string()),
                ),
                dim("  a: "),
                accent(app.audio.label()),
            ])),
            Rect {
                y: inner.y + 2,
                height: 1,
                ..inner
            },
        );
    }
    if focused && app.modal.is_none() && inner.width > 2 {
        frame.set_cursor_position((
            inner.x
                + 2
                + app
                    .query
                    .column()
                    .saturating_sub(scroll)
                    .min(available.saturating_sub(1)) as u16,
            inner.y,
        ));
    }
}
fn anime_item(app: &App, anime: &Anime) -> ListItem<'static> {
    let saved = app.library.bookmarked(anime);
    ListItem::new(Line::from(vec![
        Span::styled(
            if saved { "* " } else { "  " },
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            format!("{} ", anime.provider.short()),
            Style::default().fg(if anime.provider == ProviderId::AnimeWorld {
                ACCENT
            } else {
                Color::Magenta
            }),
        ),
        Span::raw(anime.title.clone()),
        dim(if anime.dub { " [ITA]" } else { "" }),
    ]))
}
fn results(frame: &mut Frame, app: &mut App, area: Rect) {
    let active_view = matches!(app.view, View::Discover | View::Search);
    let active = active_view && app.focus == Focus::Titles;
    let visible: Vec<Anime> = if active_view {
        app.visible().into_iter().cloned().collect()
    } else {
        if app.query.value.trim().is_empty() {
            &app.discovered
        } else {
            &app.searched
        }
        .clone()
    };
    let label = if app.query.value.trim().is_empty() {
        "Discover - Search"
    } else {
        "Search - Discover"
    };
    let block = panel(format!("[2]-{label}"), active).title_bottom(count_footer(
        if active_view {
            app.titles.selected()
        } else {
            None
        },
        visible.len(),
    ));
    let inner = block.inner(area);
    if visible.is_empty() {
        frame.render_widget(block, area);
        let text = if app.search_pending.is_some() || app.statuses.iter().any(|s| s.pending) {
            "Searching sources…\nYou can keep typing."
        } else if app.statuses.iter().any(|s| s.error.is_some()) {
            "A source is unavailable.\n! diagnostics · r retry\np switch provider"
        } else if app.view == View::Search && app.query.value.trim().is_empty() {
            "What are we watching?\n\nType an anime title above.\nTry English, Japanese,\nor Italian titles.\n\n↑ recalls your last search."
        } else {
            "No matching anime.\n\nTry a shorter title or\nan alternative spelling.\np / a clear source/audio\nfilters."
        };
        frame.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(MUTED))
                .wrap(Wrap { trim: false }),
            inner,
        );
    } else {
        let items = visible
            .iter()
            .map(|a| anime_item(app, a))
            .collect::<Vec<_>>();
        let list = List::new(items)
            .block(block)
            .highlight_style(selected_style(active))
            .highlight_symbol("");
        if active_view {
            frame.render_stateful_widget(list, area, &mut app.titles);
        } else {
            frame.render_widget(list, area);
        }
    }
}
fn library_panel(frame: &mut Frame, app: &mut App, view: View, area: Rect) {
    let number = match view {
        View::Continue => 3,
        View::Watchlist => 4,
        _ => 5,
    };
    let is_view = app.view == view;
    let active = is_view && app.focus == Focus::Titles;
    let data: Vec<Anime> = if is_view {
        app.visible().into_iter().cloned().collect()
    } else {
        match view {
            View::Continue => app
                .library
                .history
                .iter()
                .map(|h| h.anime.clone())
                .collect(),
            View::Watchlist => app.library.watchlist.clone(),
            _ => {
                let mut titles: Vec<Anime> = vec![];
                for e in &app.offline {
                    if !titles.iter().any(|a| a.key() == e.anime.key()) {
                        titles.push(e.anime.clone());
                    }
                }
                titles
            }
        }
    };
    let block = panel(format!("[{number}]-{}", view.label()), active).title_bottom(count_footer(
        if is_view { app.titles.selected() } else { None },
        data.len(),
    ));
    if data.is_empty() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let message = match view {
            View::Continue => "Your next episode appears here.",
            View::Watchlist => "w to save an anime for later.",
            _ => "d to download an episode.",
        };
        frame.render_widget(
            Paragraph::new(message)
                .style(Style::default().fg(MUTED))
                .wrap(Wrap { trim: false }),
            inner,
        );
    } else {
        let items = data
            .iter()
            .map(|a| {
                if view == View::Continue {
                    let last = app
                        .library
                        .history
                        .iter()
                        .find(|h| h.anime.key() == a.key())
                        .map(|h| h.last_episode.as_str())
                        .unwrap_or("?");
                    ListItem::new(Line::from(vec![
                        accent(format!(" {last} ")),
                        Span::raw(a.title.clone()),
                    ]))
                } else {
                    anime_item(app, a)
                }
            })
            .collect::<Vec<_>>();
        let list = List::new(items)
            .block(block)
            .highlight_style(selected_style(active));
        if is_view {
            frame.render_stateful_widget(list, area, &mut app.titles);
        } else {
            frame.render_widget(list, area);
        }
    }
}
fn details(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = panel("[0]-Anime details", app.focus == Focus::Details);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(selected) = app.selected() else {
        let mut lines = vec![
            Line::from(Span::styled(
                "anime-levisione",
                Style::default().fg(ACTIVE).add_modifier(Modifier::BOLD),
            )),
            Line::from("Anime, without leaving your terminal."),
            Line::from(""),
            Line::from(vec![
                accent("/ "),
                Span::raw("Search across AnimeWorld and AnimeUnity"),
            ]),
            Line::from(vec![
                accent("j/k "),
                Span::raw("Browse titles · Enter opens episodes"),
            ]),
            Line::from(vec![
                accent("w "),
                Span::raw("Keep a watchlist · 3 continue watching"),
            ]),
            Line::from(vec![
                accent(": "),
                Span::raw("Command palette · ? all keybindings"),
            ]),
            Line::from(""),
        ];
        if !app.library.recent_searches.is_empty() {
            lines.push(Line::from(dim(format!(
                "Recent: {}",
                app.library
                    .recent_searches
                    .iter()
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" · ")
            ))));
        }
        lines.push(Line::from(dim(
            "Sources offer Italian subtitles/dubs. Use content you are authorized to access.",
        )));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        return;
    };
    let anime = app.detail().map_or(selected, |(a, _)| a);
    let entry = app.meta.get(&anime.key()).and_then(|m| m.as_ref());
    let mut lines = vec![
        Line::from(Span::styled(
            anime.title.clone(),
            Style::default().fg(ACTIVE).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    let meta = [&anime.kind, &anime.year, &anime.status]
        .into_iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" · ");
    if !meta.is_empty() {
        lines.push(Line::from(meta));
    }
    lines.push(Line::from(vec![
        accent(anime.provider.to_string()),
        dim("  |  "),
        Span::raw(anime.audio()),
        dim("  |  "),
        Span::raw(if anime.episodes == 0 {
            "Episodes: unknown".into()
        } else {
            format!("{} episodes", anime.episodes)
        }),
        Span::styled(
            if anime.score.is_empty() {
                String::new()
            } else {
                format!("  |  Score {}", anime.score)
            },
            Style::default().fg(Color::Yellow),
        ),
    ]));
    if let Some(entry) = entry {
        // Score and episode count fill in what the streaming source omits.
        let episodes = if anime.episodes == 0 && entry.episodes.is_some() {
            format!("{} episodes", entry.episodes.unwrap_or_default())
        } else {
            String::new()
        };
        let bits = [
            (!entry.display_score().is_empty()).then(|| format!("★ {}", entry.display_score())),
            (!entry.display_status().is_empty()).then(|| entry.display_status()),
            (!episodes.is_empty()).then_some(episodes),
            (!entry.genres.is_empty()).then(|| {
                entry
                    .genres
                    .iter()
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("  ·  ");
        if !bits.is_empty() {
            lines.push(Line::from(vec![
                accent("AniList"),
                dim("  |  "),
                Span::raw(bits),
            ]));
        }
    } else if app.meta_pending.is_some() {
        lines.push(Line::from(dim("AniList: looking up…")));
    }
    if !anime.genres.is_empty() {
        lines.push(Line::from(dim(anime.genres.join(" · "))));
    } else if let Some(entry) = entry.filter(|m| !m.genres.is_empty()) {
        lines.push(Line::from(dim(entry.genres.join(" · "))));
    }
    if app.library.bookmarked(anime) {
        lines.push(Line::from(Span::styled(
            "* In your watchlist",
            Style::default().fg(Color::Yellow),
        )));
    }
    lines.push(Line::from(""));
    if app.detail_loading {
        lines.push(Line::from(dim("Loading episode list…")));
    }
    if let Some(error) = &app.detail_error {
        lines.push(Line::from(Span::styled(
            error.clone(),
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from(accent("Focus episodes, then r to retry.")));
    }
    lines.push(Line::from(if !anime.synopsis.is_empty() {
        Span::raw(anime.synopsis.clone())
    } else if let Some(entry) = entry.filter(|m| !m.plain_description().is_empty()) {
        Span::raw(entry.plain_description())
    } else {
        dim("No synopsis supplied by this source.")
    }));
    lines.push(Line::from(""));
    if let Some(episode) = app.selected_episode()
        && let Some(progress) = app.library.progress(anime, episode)
    {
        lines.push(Line::from(accent(if progress.completed {
            format!("Episode {} · watched", episode.number)
        } else {
            format!(
                "Resume episode {} at {}",
                episode.number,
                time(progress.seconds)
            )
        })));
    }
    lines.push(Line::from(dim(
        "Enter: episodes / play   w: save   d: download   m: watched",
    )));
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0)),
        inner,
    );
}
fn episodes(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = matches!(app.focus, Focus::Episodes | Focus::EpisodeFilter);
    let eps = app.visible_episodes();
    let title = if app.episode_filter.value.is_empty() {
        "Episodes · Enter: play · f: filter".into()
    } else {
        format!("Episodes · filter: {}", app.episode_filter.value)
    };
    let block =
        panel(title, focused).title_bottom(count_footer(app.episodes.selected(), eps.len()));
    let inner = block.inner(area);
    if eps.is_empty() {
        let message = if let Some(e) = &app.detail_error {
            format!("{e}\n\nr retry · p switch source")
        } else if app.detail_loading {
            "Loading episodes…".into()
        } else if !app.episode_filter.value.is_empty() {
            "No matching episodes. Esc clears the filter.".into()
        } else {
            "Select an anime to see its episodes.\nTab changes pane · Enter opens this pane".into()
        };
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(message)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(MUTED)),
            inner,
        );
    } else {
        let anime = app.selected();
        let items = eps
            .iter()
            .map(|e| {
                let progress = anime.and_then(|a| app.library.progress(a, e));
                let (marker, color) = if progress.is_some_and(|p| p.completed) {
                    ("+", ACTIVE)
                } else if progress.is_some_and(|p| p.seconds > 0.0) {
                    (">", Color::Yellow)
                } else {
                    (" ", MUTED)
                };
                let local = anime.is_some_and(|a| {
                    app.offline
                        .iter()
                        .any(|o| o.anime.key() == a.key() && o.episode.number == e.number)
                });
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {marker} "), Style::default().fg(color)),
                    Span::raw(e.label()),
                    progress
                        .filter(|p| !p.completed && p.seconds > 0.0)
                        .map_or_else(
                            || dim(""),
                            |p| accent(format!("  resume {}", time(p.seconds))),
                        ),
                    Span::styled(
                        if local { "  [offline]" } else { "" },
                        Style::default().fg(ACTIVE),
                    ),
                ]))
            })
            .collect::<Vec<_>>();
        frame.render_stateful_widget(
            List::new(items)
                .block(block)
                .highlight_style(selected_style(focused)),
            area,
            &mut app.episodes,
        );
    }
}
fn log(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines = vec![Line::from(Span::styled(
        app.notice.clone(),
        Style::default().fg(if app.notice_error {
            Color::Red
        } else {
            Color::Reset
        }),
    ))];
    if app.playing {
        lines.push(Line::from(vec![
            accent("Playing: "),
            Span::raw(app.playback_label.clone()),
        ]));
    }
    if app.downloading {
        lines.push(Line::from(vec![
            accent(format!("{} Downloading: ", SPINNER[app.tick / 3 % 4])),
            Span::raw(app.download_label.clone()),
        ]));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel("Activity", false))
            .wrap(Wrap { trim: false }),
        area,
    );
}
fn footer(frame: &mut Frame, app: &App, area: Rect) {
    let help = if app.focus == Focus::Search {
        "Search: type | Submit: Enter | Browse: Esc/↓ | Clear: Ctrl+u | Paste supported"
    } else if app.focus == Focus::EpisodeFilter {
        "Filter episodes: type | Done: Enter/Esc | Clear: Ctrl+u"
    } else if app.focus == Focus::Episodes {
        "Play: Enter | Restart: s | Next: n | Watched: m | Download: d | Filter: f | Help: ?"
    } else {
        "Search: / | Episodes: Enter | Save: w | Panels: Tab/1-5 | Commands: : | Help: ? | Quit: q"
    };
    frame.render_widget(
        Paragraph::new(help).style(Style::default().fg(Color::Magenta)),
        area,
    );
}
fn time(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
fn popup(frame: &mut Frame, app: &mut App, modal: Modal) {
    let outer = frame.area();
    let width = outer.width.saturating_sub(6).min(92);
    let height = match modal {
        Modal::Quit => 7,
        Modal::Settings => 21,
        Modal::Commands => 24,
        Modal::Diagnostics => 18,
        Modal::Help => 30,
    }
    .min(outer.height.saturating_sub(2));
    let area = Rect::new(
        outer.x + (outer.width - width) / 2,
        outer.y + (outer.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    let title = match modal {
        Modal::Help => "Keybindings & about · Esc to close",
        Modal::Commands => "Commands · type to filter · Esc to close",
        Modal::Settings => "Settings & paths · Esc to close",
        Modal::Diagnostics => "Provider diagnostics · Esc to close",
        Modal::Quit => "Quit anime-levisione?",
    };
    let block = panel(title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if modal == Modal::Commands {
        let rows = Layout::vertical([C::Length(2), C::Min(1)]).split(inner);
        frame.render_widget(
            Paragraph::new(format!("> {}", app.command_input.value)),
            rows[0],
        );
        let commands = app.commands();
        let items = commands
            .iter()
            .map(|&i| {
                ListItem::new(Line::from(vec![
                    Span::raw(COMMANDS[i].0),
                    dim(format!("  {}", COMMANDS[i].1)),
                ]))
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default()
            .with_selected((!commands.is_empty()).then_some(app.command_selected));
        frame.render_stateful_widget(
            List::new(items).highlight_style(selected_style(true)),
            rows[1],
            &mut state,
        );
        let cursor = app
            .command_input
            .column()
            .min(inner.width.saturating_sub(3) as usize) as u16;
        frame.set_cursor_position((inner.x + 2 + cursor, inner.y));
        return;
    }
    let lines = match modal {
        Modal::Help => vec![
            "NAVIGATION",
            "  / or 1       Search anime (350 ms debounce, both sources)",
            "  2            Results / latest releases",
            "  3 / 4 / 5    Continue / watchlist / offline",
            "  0            Anime details",
            "  Tab / h / l  Switch pane       j / k / ↑ / ↓  Move",
            "  g / G        First / last      PgUp / PgDn    Page",
            "  Enter        Open episodes, then play",
            "",
            "ACTIONS",
            "  w            Toggle watchlist",
            "  m            Mark episode watched / unwatched",
            "  s / n        Restart selected / play next episode",
            "  d            Download via ffmpeg (direct files and HLS)",
            "  Ctrl+x / Ctrl+d   Stop player / cancel download",
            "  f            Filter episodes   Esc clears the filter",
            "  p / a        Cycle provider / subtitle-dub filter",
            "  r            Refresh results or retry focused episodes",
            "  P            Toggle private mode (no automatic history)",
            "  : / Ctrl+p   Command palette   , settings   ! diagnostics",
            "  q / Ctrl+c   Quit (asks before stopping active jobs)",
            "",
            "ABOUT",
            "  anime-levisione",
            "  GPL-3.0-only · No warranty · Redistribution under GPLv3.",
        ]
        .into_iter()
        .map(Line::from)
        .collect(),
        Modal::Settings => vec![
            Line::from(accent("Edit config.toml, then restart to apply changes.")),
            Line::from(""),
            Line::from(format!("Directory: {}", app.data_path)),
            Line::from(format!("Player: {}", app.config.player)),
            Line::from(format!("Player args: {:?}", app.config.player_args)),
            Line::from(format!("Downloads: {}", app.config.download_dir.display())),
            Line::from(format!("AnimeWorld: {}", app.config.animeworld_url)),
            Line::from(format!("AnimeUnity: {}", app.config.animeunity_url)),
            Line::from(format!(
                "StreamingCommunity: {}",
                app.config.streamingcommunity_url
            )),
            Line::from(format!(
                "AniList: {}",
                app.anilist_user
                    .clone()
                    .unwrap_or_else(|| "not logged in · `anime-levisione login` to sync".into())
            )),
            Line::from(format!("Private mode: {}", app.config.private)),
            Line::from(""),
            Line::from(dim(
                "Domains rotate often; the live one is auto-detected, followed through",
            )),
            Line::from(dim(
                "redirects, cached in domains.json, and retried after failures. Add",
            )),
            Line::from(dim("candidate mirrors with *_domains keys in config.toml.")),
            Line::from(""),
            Line::from("mpv: automatic resume and watched tracking (macOS/Linux/WSL)."),
            Line::from("VLC: playback/resume; use m to mark completion manually."),
            Line::from("ffmpeg: required only for downloads. No Python or fzf."),
            Line::from("Player args are passed literally, never through a shell."),
            Line::from(""),
            Line::from(dim(
                "CLI: anime-levisione doctor · anime-levisione search \"title\" --json",
            )),
        ],
        Modal::Diagnostics => {
            let mut lines = vec![
                Line::from("Searches run independently; one failed source never hides the other."),
                Line::from("Network errors retry twice. Requests time out after 30 seconds."),
                Line::from(""),
            ];
            for s in &app.statuses {
                lines.push(Line::from(Span::styled(
                    format!(
                        "{}: {}",
                        s.provider,
                        if s.pending {
                            "requesting…".into()
                        } else if let Some(e) = &s.error {
                            e.clone()
                        } else {
                            format!("{} results", s.count)
                        }
                    ),
                    Style::default().fg(if s.error.is_some() {
                        Color::Red
                    } else {
                        ACTIVE
                    }),
                )));
                lines.push(Line::from(""));
            }
            if app.statuses.is_empty() {
                lines.push(Line::from("No active search. Press / and enter a title."));
            }
            let (text, color) = if app.config.private {
                (
                    "AniList: disabled (private mode keeps everything local)".to_string(),
                    Color::Yellow,
                )
            } else if let Some(error) = &app.anilist_error {
                (format!("AniList: {error}"), Color::Red)
            } else if let Some(user) = &app.anilist_user {
                (
                    format!("AniList: syncing as {user} · watchlist and progress mirror outward"),
                    ACTIVE,
                )
            } else {
                (
                    "AniList: metadata only · run `anime-levisione login` to sync".to_string(),
                    ACTIVE,
                )
            };
            lines.push(Line::from(Span::styled(text, Style::default().fg(color))));
            lines.push(Line::from(
                "Use p to select another source, r to retry, , for provider URLs.",
            ));
            lines
        }
        Modal::Quit => vec![
            Line::from("A player or download is still running."),
            Line::from("Stop active jobs and exit?"),
            Line::from(""),
            Line::from(accent("y: stop and quit    n / Esc: keep watching")),
        ],
        Modal::Commands => unreachable!(),
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::Audio,
        store::{Config, Library},
    };
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn layout_matches_numbered_lazygit_panes() -> anyhow::Result<()> {
        let mut app = App::new(
            Config::default(),
            Library::default(),
            "/tmp/test".into(),
            true,
        );
        let mut terminal = Terminal::new(TestBackend::new(160, 50))?;
        terminal.draw(|f| draw(f, &mut app))?;
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        for label in [
            "[1]-Search",
            "[2]-Discover",
            "[3]-Continue",
            "[4]-Watchlist",
            "[5]-Offline",
            "[0]-Anime details",
            "Episodes",
            "Activity",
        ] {
            assert!(text.contains(label), "missing {label}");
        }
        Ok(())
    }
    #[test]
    fn anilist_metadata_enriches_the_details_panel() -> anyhow::Result<()> {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), false);
        let anime = crate::model::Anime {
            provider: crate::model::ProviderId::AnimeUnity,
            id: "7".into(),
            title: "Frieren".into(),
            aliases: vec![],
            synopsis: String::new(),
            year: "2023".into(),
            kind: "TV".into(),
            status: String::new(),
            score: String::new(),
            episodes: 0,
            dub: false,
            genres: vec![],
            page_url: String::new(),
        };
        app.discovered = vec![anime.clone()];
        app.details.insert(anime.key(), (anime.clone(), vec![]));
        app.rebuild();
        app.titles.select(Some(0));
        app.anilist_user = Some("viewer".into());
        app.meta.insert(
            anime.key(),
            Some(crate::anilist::MediaEntry {
                id: 21,
                average_score: Some(86),
                status: Some("FINISHED".into()),
                episodes: Some(28),
                genres: vec!["Adventure".into(), "Fantasy".into()],
                description: Some("<i>Elven</i> mage".into()),
                ..crate::anilist::MediaEntry::default()
            }),
        );
        let mut terminal = Terminal::new(TestBackend::new(160, 50))?;
        terminal.draw(|f| draw(f, &mut app))?;
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        for label in ["AniList", "★ 8.6", "Finished", "28 episodes", "Elven mage"] {
            assert!(text.contains(label), "missing {label}");
        }
        Ok(())
    }
    #[test]
    fn resize_and_all_popups_never_panic() -> anyhow::Result<()> {
        for (width, height) in [(1, 1), (40, 10), (60, 18), (80, 24), (120, 40), (220, 70)] {
            let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
            for modal in [
                None,
                Some(Modal::Help),
                Some(Modal::Commands),
                Some(Modal::Settings),
                Some(Modal::Diagnostics),
                Some(Modal::Quit),
            ] {
                app.modal = modal;
                let mut terminal = Terminal::new(TestBackend::new(width, height))?;
                terminal.draw(|f| draw(f, &mut app))?;
            }
        }
        Ok(())
    }
    #[test]
    fn audio_labels_are_explicit_about_language() {
        assert_eq!(Audio::Sub.label(), "Sub ITA");
    }
}
