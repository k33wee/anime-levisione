use crate::{
    anilist::{MediaEntry, TrackEvent},
    events::Message,
    input::Input,
    model::{Anime, Episode, ProviderId, normalize, sort_by_rank},
    playback::{Media, OfflineEpisode},
    store::{Config, Library, Progress},
};
use crossterm::event::{KeyCode as K, KeyEvent, KeyModifiers as M};
use ratatui::widgets::ListState;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Discover,
    Search,
    Continue,
    Watchlist,
    Offline,
}
impl View {
    pub fn label(self) -> &'static str {
        match self {
            Self::Discover => "Discover",
            Self::Search => "Search",
            Self::Continue => "Continue",
            Self::Watchlist => "Watchlist",
            Self::Offline => "Offline",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Search,
    Titles,
    Episodes,
    Details,
    EpisodeFilter,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audio {
    All,
    Sub,
    Dub,
}
impl Audio {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All audio",
            Self::Sub => "Sub ITA",
            Self::Dub => "Dub ITA",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modal {
    Help,
    Commands,
    Settings,
    Diagnostics,
    Quit,
}
#[derive(Debug, Clone)]
pub struct SourceStatus {
    pub provider: ProviderId,
    pub pending: bool,
    pub count: usize,
    pub error: Option<String>,
}
#[derive(Clone)]
pub enum Action {
    Catalog {
        generation: u64,
        query: Option<String>,
    },
    Episodes(Anime),
    Play {
        anime: Anime,
        episode: Episode,
        local: Option<Media>,
        resume: f64,
    },
    Download {
        anime: Anime,
        episode: Episode,
    },
    /// Fetch AniList metadata to enrich a title's details.
    Lookup(Anime),
    /// Mirror a local watchlist/progress change to the user's AniList list.
    Track {
        anime: Anime,
        event: TrackEvent,
    },
    Save,
    /// Library save for high-frequency player progress updates; debounced by the
    /// effects layer instead of fsync-ing on every ~2 s report.
    SaveProgress,
    CancelPlayback,
    CancelDownload,
    Rescan,
    Quit,
}

pub const COMMANDS: &[(&str, &str)] = &[
    ("Search anime", "/"),
    ("Discover latest releases", "2"),
    ("Continue watching", "3"),
    ("Open watchlist", "4"),
    ("Open offline library", "5"),
    ("Play selected episode", "Enter"),
    ("Toggle watchlist", "w"),
    ("Mark episode watched / unwatched", "m"),
    ("Download selected episode", "d"),
    ("Retry / refresh", "r"),
    ("Toggle private mode", "P"),
    ("Settings & paths", ","),
    ("Provider diagnostics", "!"),
    ("AniList login & sync status", "!"),
    ("Keyboard help & about", "?"),
    ("Stop playback", "Ctrl+x"),
    ("Cancel download", "Ctrl+d"),
    ("Quit", "q"),
];

pub struct App {
    pub config: Config,
    pub library: Library,
    pub data_path: String,
    pub demo: bool,
    pub view: View,
    pub focus: Focus,
    pub query: Input,
    pub episode_filter: Input,
    pub catalog: Vec<Anime>,
    pub discovered: Vec<Anime>,
    pub searched: Vec<Anime>,
    pub offline: Vec<OfflineEpisode>,
    pub titles: ListState,
    pub episodes: ListState,
    pub provider_filter: Option<ProviderId>,
    pub audio: Audio,
    pub details: HashMap<String, (Anime, Vec<Episode>)>,
    pub detail_error: Option<String>,
    pub detail_loading: bool,
    pub detail_scroll: u16,
    pub statuses: Vec<SourceStatus>,
    pub generation: u64,
    pub modal: Option<Modal>,
    pub command_input: Input,
    pub command_selected: usize,
    pub notice: String,
    pub notice_error: bool,
    pub playing: bool,
    pub downloading: bool,
    pub playback_label: String,
    pub download_label: String,
    pub tick: usize,
    pub search_pending: Option<Instant>,
    detail_pending: Option<Instant>,
    /// AniList metadata per title key; `None` caches a lookup miss so the
    /// same title is not searched again while scrolling.
    pub meta: HashMap<String, Option<MediaEntry>>,
    pub meta_pending: Option<Instant>,
    /// AniList account name once logged in; drives sync and UI hints.
    pub anilist_user: Option<String>,
    pub anilist_error: Option<String>,
    catalog_view: View,
}
impl App {
    pub fn new(config: Config, library: Library, data_path: String, demo: bool) -> Self {
        Self {
            config,
            library,
            data_path,
            demo,
            view: View::Discover,
            focus: Focus::Titles,
            query: Input::default(),
            episode_filter: Input::default(),
            catalog: vec![],
            discovered: vec![],
            searched: vec![],
            offline: vec![],
            titles: ListState::default(),
            episodes: ListState::default(),
            provider_filter: None,
            audio: Audio::All,
            details: HashMap::new(),
            detail_error: None,
            detail_loading: false,
            detail_scroll: 0,
            statuses: vec![],
            generation: 0,
            modal: None,
            command_input: Input::default(),
            command_selected: 0,
            notice: "Find your next episode. Press / to search, ? for help.".into(),
            notice_error: false,
            playing: false,
            downloading: false,
            playback_label: String::new(),
            download_label: String::new(),
            tick: 0,
            search_pending: None,
            detail_pending: None,
            meta: HashMap::new(),
            meta_pending: None,
            anilist_user: None,
            anilist_error: None,
            catalog_view: View::Discover,
        }
    }
    pub fn visible(&self) -> Vec<&Anime> {
        self.catalog
            .iter()
            .filter(|a| self.provider_filter.is_none_or(|p| a.provider == p))
            .filter(|a| match self.audio {
                Audio::All => true,
                Audio::Sub => !a.dub,
                Audio::Dub => a.dub,
            })
            .collect()
    }
    pub fn selected(&self) -> Option<&Anime> {
        self.visible().get(self.titles.selected()?).copied()
    }
    pub fn detail(&self) -> Option<&(Anime, Vec<Episode>)> {
        self.details.get(&self.selected()?.key())
    }
    pub fn visible_episodes(&self) -> Vec<&Episode> {
        let query = normalize(&self.episode_filter.value);
        self.detail()
            .map(|(_, eps)| {
                eps.iter()
                    .filter(|e| query.is_empty() || normalize(&e.label()).contains(&query))
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn selected_episode(&self) -> Option<&Episode> {
        self.visible_episodes()
            .get(self.episodes.selected()?)
            .copied()
    }
    pub fn commands(&self) -> Vec<usize> {
        let query = normalize(&self.command_input.value);
        COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, (name, _))| normalize(name).contains(&query))
            .map(|(i, _)| i)
            .collect()
    }
    pub fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.notice = text.into();
        self.notice_error = error;
    }
    pub fn start_catalog(&mut self, query: Option<String>) -> Vec<Action> {
        self.generation += 1;
        self.catalog_view = if query.is_some() {
            View::Search
        } else {
            View::Discover
        };
        self.statuses = ProviderId::ALL
            .into_iter()
            .map(|provider| SourceStatus {
                provider,
                pending: true,
                count: 0,
                error: None,
            })
            .collect();
        if query.is_some() {
            self.searched.clear();
        } else {
            self.discovered.clear();
        }
        self.rebuild();
        self.search_pending = None;
        vec![Action::Catalog {
            generation: self.generation,
            query,
        }]
    }
    pub fn switch_view(&mut self, view: View) -> Vec<Action> {
        if self.view != view && (self.view == View::Offline || view == View::Offline) {
            self.details.clear();
        }
        self.search_pending = None;
        self.view = view;
        self.focus = if view == View::Search {
            Focus::Search
        } else {
            Focus::Titles
        };
        self.episode_filter = Input::default();
        self.titles.select(Some(0));
        self.rebuild();
        match view {
            View::Discover if self.discovered.is_empty() => self.start_catalog(None),
            View::Search if !self.query.value.trim().is_empty() && self.searched.is_empty() => {
                self.start_catalog(Some(self.query.value.trim().to_owned()))
            }
            View::Offline => vec![Action::Rescan],
            _ => vec![],
        }
    }
    pub fn rebuild(&mut self) {
        self.catalog = match self.view {
            View::Discover => self.discovered.clone(),
            View::Search => self.searched.clone(),
            View::Continue => self
                .library
                .history
                .iter()
                .map(|h| h.anime.clone())
                .collect(),
            View::Watchlist => self.library.watchlist.clone(),
            View::Offline => {
                let mut titles: Vec<Anime> = vec![];
                for entry in &self.offline {
                    if !titles.iter().any(|a| a.key() == entry.anime.key()) {
                        titles.push(entry.anime.clone());
                    }
                }
                titles
            }
        };
        let count = self.visible().len();
        self.titles.select(if count == 0 {
            None
        } else {
            Some(self.titles.selected().unwrap_or(0).min(count - 1))
        });
        self.selection_changed();
    }
    fn selection_changed(&mut self) {
        self.detail_error = None;
        self.detail_scroll = 0;
        self.episodes.select(Some(0));
        self.detail_loading = self.selected().is_some() && self.detail().is_none();
        self.detail_pending = self
            .detail_loading
            .then(|| Instant::now() + Duration::from_millis(180));
        // AniList enrichment trails the selection with its own debounce.
        self.meta_pending = None;
        if self.anilist_enabled()
            && self
                .selected()
                .is_some_and(|a| !a.title.trim().is_empty() && !self.meta.contains_key(&a.key()))
        {
            self.meta_pending = Some(Instant::now() + Duration::from_millis(450));
        }
        self.select_resume();
    }
    /// AniList is consulted only when it cannot leak anything: never in demo
    /// or private mode.
    fn anilist_enabled(&self) -> bool {
        !self.demo && !self.config.private
    }
    fn anilist_track(&self, anime: &Anime, event: TrackEvent) -> Option<Action> {
        (self.anilist_enabled() && self.anilist_user.is_some()).then(|| Action::Track {
            anime: anime.clone(),
            event,
        })
    }
    fn select_resume(&mut self) {
        let Some(anime) = self.selected() else {
            return;
        };
        let Some(history) = self
            .library
            .history
            .iter()
            .find(|h| h.anime.key() == anime.key())
        else {
            return;
        };
        let eps = self.visible_episodes();
        if let Some(i) = eps.iter().position(|e| e.number == history.last_episode) {
            let completed = history
                .progress
                .get(&history.last_episode)
                .is_some_and(|p| p.completed);
            self.episodes.select(Some(if completed {
                (i + 1).min(eps.len().saturating_sub(1))
            } else {
                i
            }));
        }
    }
    pub fn on_tick(&mut self) -> Vec<Action> {
        self.tick = self.tick.wrapping_add(1);
        let mut actions = vec![];
        if self.search_pending.is_some_and(|t| Instant::now() >= t) {
            let query = self.query.value.trim().to_owned();
            if !query.is_empty() {
                actions.extend(self.start_catalog(Some(query)));
            } else {
                self.search_pending = None;
            }
        }
        if self.detail_pending.is_some_and(|t| Instant::now() >= t) {
            self.detail_pending = None;
            if let Some(a) = self.selected().cloned() {
                if self.view == View::Offline {
                    let eps = self
                        .offline
                        .iter()
                        .filter(|e| e.anime.key() == a.key())
                        .map(|e| e.episode.clone())
                        .collect();
                    self.details.insert(a.key(), (a, eps));
                    self.detail_loading = false;
                    self.select_resume();
                } else {
                    actions.push(Action::Episodes(a));
                }
            }
        }
        if self.meta_pending.is_some_and(|t| Instant::now() >= t) {
            self.meta_pending = None;
            if self.anilist_enabled()
                && let Some(anime) = self.selected().cloned()
                && !self.meta.contains_key(&anime.key())
            {
                actions.push(Action::Lookup(anime));
            }
        }
        actions
    }
    pub fn search_changed(&mut self) {
        // Invalidate immediately, not after the debounce, so stale requests cannot flash old results.
        self.generation += 1;
        self.statuses.clear();
        self.searched.clear();
        self.view = View::Search;
        self.catalog.clear();
        self.titles.select(None);
        self.detail_pending = None;
        self.detail_loading = false;
        self.search_pending = (!self.query.value.trim().is_empty())
            .then(|| Instant::now() + Duration::from_millis(350));
    }
    pub fn message(&mut self, message: Message) -> Vec<Action> {
        match message {
            Message::DomainMoved { provider, url } => {
                match provider {
                    ProviderId::AnimeWorld => self.config.animeworld_url = url.clone(),
                    ProviderId::AnimeUnity => self.config.animeunity_url = url.clone(),
                    ProviderId::StreamingCommunity => {
                        self.config.streamingcommunity_url = url.clone()
                    }
                }
                self.notify(format!("{provider} moved to {url}; retrying"), false);
            }
            Message::Meta { key, result } => {
                match result {
                    Ok(entry) => {
                        self.anilist_error = None;
                        self.meta.insert(key, entry);
                    }
                    // Errors are cached too, so a failing API is not retried
                    // on every selection; diagnostics shows the reason.
                    Err(error) => {
                        self.anilist_error = Some(error);
                        self.meta.insert(key, None);
                    }
                }
            }
            Message::Tracked(result) => match result {
                Ok(text) => self.notify(text, false),
                Err(error) => self.notify(error, true),
            },
            Message::Catalog {
                generation,
                provider,
                result,
            } => {
                if generation != self.generation {
                    return vec![];
                }
                let old_key = self.selected().map(Anime::key);
                let status = self.statuses.iter_mut().find(|s| s.provider == provider);
                if let Some(status) = status {
                    status.pending = false;
                    match result {
                        Ok(mut results) => {
                            status.count = results.len();
                            let target = if self.catalog_view == View::Search {
                                &mut self.searched
                            } else {
                                &mut self.discovered
                            };
                            target.append(&mut results);
                            if self.catalog_view == View::Search {
                                sort_by_rank(target, &self.query.value);
                            }
                            let mut seen = std::collections::HashSet::new();
                            target.retain(|a| seen.insert(a.key()));
                        }
                        Err(error) => status.error = Some(error),
                    }
                }
                if self.view != self.catalog_view {
                    return vec![];
                }
                self.catalog = if self.view == View::Search {
                    self.searched.clone()
                } else {
                    self.discovered.clone()
                };
                let selected = old_key
                    .as_ref()
                    .and_then(|key| self.visible().iter().position(|a| a.key() == *key));
                self.titles
                    .select(selected.or_else(|| (!self.visible().is_empty()).then_some(0)));
                if self.selected().map(Anime::key) != old_key {
                    self.selection_changed();
                }
            }
            Message::Episodes { key, result } => {
                // A queued online response must not replace the offline-only episode list.
                if self.view == View::Offline {
                    return vec![];
                }
                let selected = self.selected().is_some_and(|a| a.key() == key);
                match result {
                    Ok(detail) => {
                        self.details.insert(key, detail);
                        if selected {
                            self.detail_loading = false;
                            self.detail_error = None;
                            self.select_resume();
                        }
                    }
                    Err(error) if selected => {
                        self.detail_loading = false;
                        self.detail_error = Some(error);
                    }
                    _ => {}
                }
            }
            Message::PlaybackStarted => self.notify(
                "Playing in your player · resume is saved automatically with mpv · Ctrl+x to stop",
                false,
            ),
            Message::Progress { anime, progress } => {
                if !self.config.private {
                    let was_completed = self
                        .library
                        .progress(&anime, &progress.episode)
                        .is_some_and(|p| p.completed);
                    let key = anime.key();
                    let total = self
                        .details
                        .get(&key)
                        .map(|(a, _)| a.episodes)
                        .unwrap_or(anime.episodes);
                    let completed_now = progress.completed && !was_completed;
                    let number = progress.episode.number.clone();
                    self.library.record(anime.clone(), progress);
                    let mut actions = vec![Action::SaveProgress];
                    if completed_now
                        && let Some(track) =
                            self.anilist_track(&anime, TrackEvent::Watched { number, total })
                    {
                        actions.push(track);
                    }
                    return actions;
                }
            }
            Message::PlaybackEnded(result) => {
                self.playing = false;
                match result {
                    Ok(()) => self.notify(
                        "Player closed. Enter to watch, n for the next episode.",
                        false,
                    ),
                    Err(e) => self.notify(e, true),
                }
            }
            Message::DownloadStarted => self.notify(
                "Downloading with ffmpeg · keep browsing · Ctrl+d to cancel",
                false,
            ),
            Message::DownloadEnded(result) => {
                self.downloading = false;
                match result {
                    Ok(path) => {
                        self.notify(format!("Saved {path}"), false);
                        return vec![Action::Rescan];
                    }
                    Err(e) => self.notify(e, true),
                }
            }
        }
        vec![]
    }
    pub fn paste(&mut self, text: &str) {
        if self.modal == Some(Modal::Commands) {
            self.command_input.insert(text);
            self.command_selected = 0;
        } else if self.focus == Focus::Search {
            self.query.insert(text);
            self.search_changed();
        } else if self.focus == Focus::EpisodeFilter {
            self.episode_filter.insert(text);
            self.episodes.select(Some(0));
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> Vec<Action> {
        if key.modifiers.contains(M::CONTROL) {
            match key.code {
                K::Char('c') => return self.quit(),
                K::Char('p') => {
                    self.open_commands();
                    return vec![];
                }
                K::Char('x') => {
                    self.playing = false;
                    self.notify("Playback stopped", false);
                    return vec![Action::CancelPlayback];
                }
                K::Char('d') => {
                    self.downloading = false;
                    self.notify("Download cancelled; partial file removed", false);
                    return vec![Action::CancelDownload];
                }
                _ => {}
            }
        }
        if let Some(modal) = self.modal {
            if key.code == K::Esc
                || (modal != Modal::Commands && matches!(key.code, K::Char('q') | K::Char('?')))
            {
                self.modal = None;
                return vec![];
            }
            if modal == Modal::Quit {
                if matches!(key.code, K::Char('y') | K::Char('Y')) {
                    return vec![Action::Quit];
                }
                if matches!(key.code, K::Char('n') | K::Char('N')) {
                    self.modal = None;
                }
            } else if modal == Modal::Commands {
                let commands = self.commands();
                match key.code {
                    K::Down => {
                        self.command_selected =
                            (self.command_selected + 1).min(commands.len().saturating_sub(1))
                    }
                    K::Up => self.command_selected = self.command_selected.saturating_sub(1),
                    K::Enter => {
                        if let Some(index) = commands.get(self.command_selected).copied() {
                            self.modal = None;
                            return self.execute_command(index);
                        }
                    }
                    _ => {
                        if self.command_input.key(key) {
                            self.command_selected = 0;
                        }
                    }
                }
            }
            return vec![];
        }
        if self.focus == Focus::Search {
            match key.code {
                K::Esc => {
                    self.focus = Focus::Titles;
                    return vec![];
                }
                K::Enter => {
                    self.focus = Focus::Titles;
                    let query = self.query.value.trim().to_owned();
                    if query.is_empty() {
                        return vec![];
                    }
                    let mut actions = if self.search_pending.is_none()
                        && self.catalog_view == View::Search
                        && !self.statuses.is_empty()
                    {
                        vec![]
                    } else {
                        self.start_catalog(Some(query.clone()))
                    };
                    if !self.config.private {
                        self.library.remember_search(&query);
                        actions.push(Action::Save);
                    }
                    return actions;
                }
                K::Down | K::Tab => {
                    self.focus = Focus::Titles;
                    return vec![];
                }
                K::Up if !self.library.recent_searches.is_empty() => {
                    self.query = Input::new(self.library.recent_searches[0].clone());
                    self.search_changed();
                    return vec![];
                }
                _ => {
                    if self.query.key(key) {
                        self.search_changed();
                    }
                    return vec![];
                }
            }
        }
        if self.focus == Focus::EpisodeFilter {
            match key.code {
                K::Esc | K::Enter | K::Tab => self.focus = Focus::Episodes,
                _ => {
                    if self.episode_filter.key(key) {
                        self.episodes.select(Some(0));
                    }
                }
            }
            return vec![];
        }
        match key.code {
            K::Char('/') => {
                self.view = View::Search;
                self.focus = Focus::Search;
                self.rebuild();
            }
            K::Char('q') => return self.quit(),
            K::Char('?') => self.modal = Some(Modal::Help),
            K::Char(':') => self.open_commands(),
            K::Char(',') => self.modal = Some(Modal::Settings),
            K::Char('!') => self.modal = Some(Modal::Diagnostics),
            K::Char('1') => {
                self.view = View::Search;
                self.focus = Focus::Search;
                self.rebuild();
            }
            K::Char('2') => {
                let view = if self.query.value.trim().is_empty() {
                    View::Discover
                } else {
                    View::Search
                };
                let actions = self.switch_view(view);
                self.focus = Focus::Titles;
                return actions;
            }
            K::Char('3') => return self.switch_view(View::Continue),
            K::Char('4') => return self.switch_view(View::Watchlist),
            K::Char('5') => return self.switch_view(View::Offline),
            K::Char('0') => self.focus = Focus::Details,
            K::Tab | K::Char('l') | K::Right => {
                self.focus = match self.focus {
                    Focus::Titles => Focus::Episodes,
                    Focus::Episodes => Focus::Details,
                    _ => Focus::Titles,
                }
            }
            K::BackTab | K::Char('h') | K::Left => {
                self.focus = match self.focus {
                    Focus::Details => Focus::Episodes,
                    Focus::Episodes => Focus::Titles,
                    _ => Focus::Details,
                }
            }
            K::Esc => {
                self.focus = Focus::Titles;
                self.episode_filter = Input::default();
            }
            K::Char('j') | K::Down => self.move_selection(1),
            K::Char('k') | K::Up => self.move_selection(-1),
            K::PageDown => self.move_selection(10),
            K::PageUp => self.move_selection(-10),
            K::Char('g') | K::Home => self.move_selection(-100_000),
            K::Char('G') | K::End => self.move_selection(100_000),
            K::Enter | K::Char(' ') => {
                if self.focus == Focus::Titles {
                    self.focus = Focus::Episodes;
                } else if self.focus == Focus::Episodes {
                    return self.play(false);
                }
            }
            K::Char('s') => return self.play(true),
            K::Char('n') => {
                if self.playing {
                    self.notify(
                        "Stop the current player with Ctrl+x before playing the next episode",
                        true,
                    );
                    return vec![];
                }
                if self.episodes.selected().unwrap_or(0) + 1 >= self.visible_episodes().len() {
                    self.notify("You are at the last available episode", false);
                    return vec![];
                }
                self.focus = Focus::Episodes;
                self.move_selection(1);
                return self.play(false);
            }
            K::Char('f') => {
                self.focus = Focus::EpisodeFilter;
            }
            K::Char('w') => {
                if let Some(a) = self.selected().cloned() {
                    self.library.toggle_bookmark(a.clone());
                    let added = self.library.bookmarked(&a);
                    self.notify("Watchlist updated", false);
                    if self.view == View::Watchlist {
                        self.rebuild();
                    }
                    let mut actions = vec![Action::Save];
                    if let Some(track) = self.anilist_track(&a, TrackEvent::Watchlist { added }) {
                        actions.push(track);
                    }
                    return actions;
                }
            }
            K::Char('m') => {
                if let (Some(anime), Some(episode)) =
                    (self.selected().cloned(), self.selected_episode().cloned())
                {
                    let completed = !self
                        .library
                        .progress(&anime, &episode)
                        .is_some_and(|p| p.completed);
                    self.library.record(
                        anime.clone(),
                        Progress {
                            episode,
                            seconds: 0.0,
                            duration: 0.0,
                            completed,
                        },
                    );
                    self.notify(
                        if completed {
                            "Episode marked watched"
                        } else {
                            "Episode marked unwatched"
                        },
                        false,
                    );
                    let mut actions = vec![Action::Save];
                    if completed {
                        let total = self
                            .details
                            .get(&anime.key())
                            .map(|(a, _)| a.episodes)
                            .unwrap_or(anime.episodes);
                        if let Some(track) = self.anilist_track(
                            &anime,
                            TrackEvent::Watched {
                                number: self
                                    .selected_episode()
                                    .map(|e| e.number.clone())
                                    .unwrap_or_default(),
                                total,
                            },
                        ) {
                            actions.push(track);
                        }
                    }
                    return actions;
                }
            }
            K::Char('d') => {
                if self.demo {
                    self.notify("Demo mode: downloads disabled", false);
                } else if self.downloading {
                    self.notify("A download is already running · Ctrl+d to cancel it", true);
                } else if let (Some(anime), Some(episode)) =
                    (self.selected().cloned(), self.selected_episode().cloned())
                {
                    self.downloading = true;
                    self.download_label = format!("{} · Ep {}", anime.title, episode.number);
                    self.notify("Resolving download…", false);
                    return vec![Action::Download { anime, episode }];
                }
            }
            K::Char('r') => return self.refresh(),
            K::Char('p') => {
                self.provider_filter = match self.provider_filter {
                    None => Some(ProviderId::AnimeWorld),
                    Some(ProviderId::AnimeWorld) => Some(ProviderId::AnimeUnity),
                    Some(ProviderId::AnimeUnity) => Some(ProviderId::StreamingCommunity),
                    _ => None,
                };
                self.titles.select(Some(0));
                self.selection_changed();
            }
            K::Char('a') => {
                self.audio = match self.audio {
                    Audio::All => Audio::Sub,
                    Audio::Sub => Audio::Dub,
                    Audio::Dub => Audio::All,
                };
                self.titles.select(Some(0));
                self.selection_changed();
            }
            K::Char('P') => {
                self.config.private = !self.config.private;
                if self.config.private {
                    // Nothing leaves the machine in private mode.
                    self.meta_pending = None;
                }
                self.notify(
                    if self.config.private {
                        "Private mode on · playback and searches will not be saved"
                    } else {
                        "Private mode off"
                    },
                    false,
                );
            }
            _ => {}
        }
        vec![]
    }
    fn open_commands(&mut self) {
        self.modal = Some(Modal::Commands);
        self.command_input = Input::default();
        self.command_selected = 0;
    }
    fn execute_command(&mut self, index: usize) -> Vec<Action> {
        let key = match index {
            0 => K::Char('/'),
            1 => {
                self.query = Input::default();
                K::Char('2')
            }
            2 => K::Char('3'),
            3 => K::Char('4'),
            4 => K::Char('5'),
            5 => {
                self.focus = Focus::Episodes;
                K::Enter
            }
            6 => K::Char('w'),
            7 => K::Char('m'),
            8 => K::Char('d'),
            9 => K::Char('r'),
            10 => K::Char('P'),
            11 => K::Char(','),
            12 => K::Char('!'),
            13 => K::Char('?'),
            14 => return self.key(KeyEvent::new(K::Char('x'), M::CONTROL)),
            15 => return self.key(KeyEvent::new(K::Char('d'), M::CONTROL)),
            16 => {
                self.notify(
                    if self.anilist_user.is_some() {
                        format!(
                            "AniList: syncing as {} · log out with `anime-levisione logout`",
                            self.anilist_user.clone().unwrap_or_default()
                        )
                    } else {
                        "AniList: run `anime-levisione login` in your shell to sync your list"
                            .into()
                    },
                    false,
                );
                return vec![];
            }
            _ => K::Char('q'),
        };
        self.key(key.into())
    }
    fn quit(&mut self) -> Vec<Action> {
        if self.playing || self.downloading {
            self.modal = Some(Modal::Quit);
            vec![]
        } else {
            vec![Action::Quit]
        }
    }
    fn refresh(&mut self) -> Vec<Action> {
        if self.view == View::Offline {
            return vec![Action::Rescan];
        }
        if self.focus == Focus::Episodes
            && let Some(a) = self.selected().cloned()
        {
            self.details.remove(&a.key());
            self.detail_loading = true;
            self.detail_error = None;
            return vec![Action::Episodes(a)];
        }
        match self.view {
            View::Discover => self.start_catalog(None),
            View::Search if !self.query.value.trim().is_empty() => {
                self.start_catalog(Some(self.query.value.trim().to_owned()))
            }
            View::Offline => vec![Action::Rescan],
            _ => vec![],
        }
    }
    pub fn move_selection(&mut self, delta: i32) {
        if self.focus == Focus::Details {
            self.detail_scroll =
                (self.detail_scroll as i32 + delta).clamp(0, u16::MAX as i32) as u16;
            return;
        }
        let count = if self.focus == Focus::Episodes {
            self.visible_episodes().len()
        } else {
            self.visible().len()
        };
        let state = if self.focus == Focus::Episodes {
            &mut self.episodes
        } else {
            &mut self.titles
        };
        let old = state.selected();
        let index =
            (old.unwrap_or(0) as i32 + delta).clamp(0, count.saturating_sub(1) as i32) as usize;
        state.select((count > 0).then_some(index));
        if self.focus == Focus::Titles && old != state.selected() {
            self.selection_changed();
        }
    }
    fn play(&mut self, restart: bool) -> Vec<Action> {
        if self.demo {
            self.notify("Demo mode: use real providers to play an episode", false);
            return vec![];
        }
        if self.playing {
            self.notify("A player is already open · Ctrl+x to stop it first", true);
            return vec![];
        }
        if let (Some(anime), Some(episode)) =
            (self.selected().cloned(), self.selected_episode().cloned())
        {
            let resume = if restart {
                0.0
            } else {
                self.library
                    .progress(&anime, &episode)
                    .filter(|p| !p.completed)
                    .map_or(0.0, |p| p.seconds)
            };
            let local = self
                .offline
                .iter()
                .find(|e| e.anime.key() == anime.key() && e.episode.number == episode.number)
                .map(|e| Media::Local(e.path.clone()));
            self.playing = true;
            self.playback_label = format!("{} · Ep {}", anime.title, episode.number);
            self.notify("Resolving stream…", false);
            return vec![Action::Play {
                anime,
                episode,
                local,
                resume,
            }];
        }
        self.notify(
            "Select an available episode first · r retries a failed episode list",
            true,
        );
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_search_results_cannot_replace_new_query() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        app.query = Input::new("old");
        app.start_catalog(Some("old".into()));
        let generation = app.generation;
        app.query = Input::new("new");
        app.search_changed();
        app.message(Message::Catalog {
            generation,
            provider: ProviderId::AnimeWorld,
            result: Err("old error".into()),
        });
        assert!(app.statuses.is_empty());
        assert!(app.catalog.is_empty());
    }
    #[test]
    fn typing_shortcuts_in_search_does_not_quit_or_change_tabs() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        app.focus = Focus::Search;
        assert!(app.key(K::Char('q').into()).is_empty());
        assert_eq!(app.query.value, "q");
        app.key(K::Char('3').into());
        assert_eq!(app.view, View::Search);
        assert_eq!(app.query.value, "q3");
    }
    #[test]
    fn empty_navigation_and_modal_quit_are_safe() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        app.move_selection(-10);
        assert_eq!(app.titles.selected(), None);
        app.playing = true;
        assert!(app.key(K::Char('q').into()).is_empty());
        assert_eq!(app.modal, Some(Modal::Quit));
        assert!(matches!(
            app.key(K::Char('y').into()).as_slice(),
            [Action::Quit]
        ));
    }
    fn sample(provider: ProviderId, id: &str) -> Anime {
        Anime {
            provider,
            id: id.into(),
            title: format!("Title {id}"),
            aliases: vec![],
            synopsis: String::new(),
            year: "2024".into(),
            kind: "TV".into(),
            status: "Finished".into(),
            score: String::new(),
            episodes: 2,
            dub: false,
            genres: vec![],
            page_url: String::new(),
        }
    }
    #[test]
    fn late_catalog_result_updates_storage_but_not_the_active_tab() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        let actions = app.start_catalog(None);
        let generation = actions
            .iter()
            .find_map(|a| match a {
                Action::Catalog { generation, .. } => Some(*generation),
                _ => None,
            })
            .expect("catalog action");
        app.message(Message::Catalog {
            generation,
            provider: ProviderId::AnimeWorld,
            result: Ok(vec![sample(ProviderId::AnimeWorld, "1")]),
        });
        assert_eq!(app.catalog.len(), 1);
        app.switch_view(View::Continue);
        app.message(Message::Catalog {
            generation,
            provider: ProviderId::AnimeUnity,
            result: Ok(vec![sample(ProviderId::AnimeUnity, "2")]),
        });
        assert!(
            app.catalog.is_empty(),
            "continue tab must not show search rows"
        );
        assert_eq!(
            app.discovered.len(),
            2,
            "result is stored for when the user returns"
        );
    }
    #[test]
    fn offline_episode_list_survives_a_stale_online_response() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        let anime = sample(ProviderId::AnimeUnity, "42");
        let local = Episode {
            id: "local".into(),
            number: "1".into(),
            title: String::new(),
        };
        app.offline = vec![crate::playback::OfflineEpisode {
            anime: anime.clone(),
            episode: local.clone(),
            path: "/tmp/demo.mkv".into(),
        }];
        app.view = View::Offline;
        app.details
            .insert(anime.key(), (anime.clone(), vec![local.clone()]));
        app.rebuild();
        app.message(Message::Episodes {
            key: anime.key(),
            result: Ok((
                anime,
                vec![Episode {
                    id: "web".into(),
                    number: "9".into(),
                    title: String::new(),
                }],
            )),
        });
        let (_, episodes) = app
            .details
            .get(&app.offline[0].anime.key())
            .expect("offline detail");
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].id, "local");
    }
    #[test]
    fn submitting_the_search_box_does_not_duplicate_a_live_query() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        app.query = Input::new("frieren");
        app.start_catalog(Some("frieren".into()));
        app.focus = Focus::Search;
        let actions = app.key(K::Enter.into());
        assert!(
            !actions.iter().any(|a| matches!(a, Action::Catalog { .. })),
            "a search is already in flight"
        );
        app.statuses.clear();
        app.focus = Focus::Search;
        assert!(
            app.key(K::Enter.into())
                .iter()
                .any(|a| matches!(a, Action::Catalog { .. }))
        );
    }
    #[test]
    fn next_episode_is_refused_at_the_end_and_while_playing() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        let anime = sample(ProviderId::AnimeWorld, "7");
        let episodes = (1..=2)
            .map(|n| Episode {
                id: format!("e{n}"),
                number: n.to_string(),
                title: String::new(),
            })
            .collect::<Vec<_>>();
        app.details.insert(anime.key(), (anime.clone(), episodes));
        app.discovered = vec![anime];
        app.rebuild();
        app.focus = Focus::Episodes;
        app.playing = true;
        assert!(app.key(K::Char('n').into()).is_empty());
        app.playing = false;
        app.episodes.select(Some(1));
        assert!(app.key(K::Char('n').into()).is_empty(), "last episode");
    }
    #[test]
    fn anilist_tracking_follows_login_and_private_mode() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), false);
        app.discovered = vec![sample(ProviderId::AnimeUnity, "7")];
        app.rebuild();
        app.titles.select(Some(0));
        // Logged out: purely local watchlist toggle.
        let actions = app.key(K::Char('w').into());
        assert!(!actions.iter().any(|a| matches!(a, Action::Track { .. })));
        app.anilist_user = Some("viewer".into());
        let actions = app.key(K::Char('w').into());
        assert!(
            actions.iter().any(|a| matches!(a, Action::Track { .. })),
            "logged-in watchlist toggles mirror to AniList"
        );
        // Private mode is a hard off switch for anything leaving the machine.
        app.config.private = true;
        let actions = app.key(K::Char('w').into());
        assert!(!actions.iter().any(|a| matches!(a, Action::Track { .. })));
    }
    #[test]
    fn anilist_lookup_debounces_and_caches_misses() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), false);
        let anime = sample(ProviderId::AnimeUnity, "7");
        app.details.insert(anime.key(), (anime.clone(), vec![]));
        app.discovered = vec![anime.clone()];
        app.rebuild();
        app.titles.select(Some(0));
        app.selection_changed();
        assert!(app.meta_pending.is_some(), "a selection schedules a lookup");
        assert!(app.on_tick().is_empty(), "before the debounce elapses");
        app.meta_pending = Some(Instant::now() - Duration::from_secs(1));
        let actions = app.on_tick();
        assert!(
            actions.iter().any(|a| matches!(a, Action::Lookup(_))),
            "due debounce issues the lookup"
        );
        // A cached miss must not re-query while scrolling back to the title.
        app.message(Message::Meta {
            key: anime.key(),
            result: Ok(None),
        });
        app.selection_changed();
        app.meta_pending = Some(Instant::now() - Duration::from_secs(1));
        assert!(app.on_tick().is_empty(), "cached miss skips the request");
        // Errors surface for diagnostics without failing the panel.
        app.message(Message::Meta {
            key: "AU:other".into(),
            result: Err("rate limited".into()),
        });
        assert_eq!(app.anilist_error.as_deref(), Some("rate limited"));
    }
    #[test]
    fn completed_playback_mirrors_to_anilist_once() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), false);
        let anime = sample(ProviderId::AnimeUnity, "7");
        app.anilist_user = Some("viewer".into());
        app.details.insert(anime.key(), (anime.clone(), vec![]));
        let episode = Episode {
            id: "e1".into(),
            number: "1".into(),
            title: String::new(),
        };
        let progress = |completed| crate::store::Progress {
            episode: episode.clone(),
            seconds: 100.0,
            duration: 100.0,
            completed,
        };
        let actions = app.message(Message::Progress {
            anime: anime.clone(),
            progress: progress(true),
        });
        assert!(
            actions.iter().any(|a| matches!(
                a,
                Action::Track {
                    event: TrackEvent::Watched { .. },
                    ..
                }
            )),
            "first completion syncs to AniList"
        );
        let actions = app.message(Message::Progress {
            anime,
            progress: progress(true),
        });
        assert!(
            !actions.iter().any(|a| matches!(a, Action::Track { .. })),
            "repeat reports do not re-sync"
        );
    }
}
