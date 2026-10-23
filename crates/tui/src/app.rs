use crate::{events::Message, input::Input, playback::OfflineEpisode};
use anime_levisione_core::{
    anilist::{MediaEntry, TrackEvent},
    audio::AudioTrack,
    model::{Anime, Episode, ProviderId, normalize, sort_by_rank},
    provider::Media,
    store::{Config, Library, Progress},
};
use crossterm::event::{
    KeyCode as K, KeyEvent, KeyModifiers as M, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    layout::{Position, Rect},
    widgets::ListState,
};
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
    Activity,
}

/// Screen panes are distinct even when several share the title-list controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Search,
    Results,
    Continue,
    Watchlist,
    Offline,
    Details,
    Episodes,
    Activity,
}
impl Pane {
    pub const ALL: [Self; 8] = [
        Self::Search,
        Self::Results,
        Self::Continue,
        Self::Watchlist,
        Self::Offline,
        Self::Details,
        Self::Episodes,
        Self::Activity,
    ];
    pub fn shortcut(self) -> char {
        match self {
            Self::Search => '1',
            Self::Results => '2',
            Self::Continue => '3',
            Self::Watchlist => '4',
            Self::Offline => '5',
            Self::Details => '0',
            Self::Episodes => '6',
            Self::Activity => '7',
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PaneRegion {
    pub pane: Pane,
    pub area: Rect,
    pub content: Rect,
    pub offset: usize,
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
    Audio,
    Login,
}

pub struct AudioPrompt {
    pub tracks: Vec<AudioTrack>,
    pub state: ListState,
    reply: tokio::sync::oneshot::Sender<Option<AudioTrack>>,
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
        request_id: u64,
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
    /// Verify and store an AniList token entered in the TUI login form.
    AniListLogin {
        client_id: u32,
        token: String,
    },
    /// Open the AniList authorization page for a client id from the form.
    OpenAniListAuth(u32),
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
    pub activity_scroll: u16,
    /// Hit regions from the latest frame, including the rendered list offsets.
    pub pane_regions: Vec<PaneRegion>,
    pub statuses: Vec<SourceStatus>,
    pub generation: u64,
    pub modal: Option<Modal>,
    pub audio_prompt: Option<AudioPrompt>,
    /// Visible language rows from the most recent modal frame.
    pub audio_rows: Vec<(Rect, usize)>,
    pub playback_id: u64,
    pub command_input: Input,
    pub command_selected: usize,
    /// AniList login form: client id + pasted token (Tab switches field).
    pub login_client_id: Input,
    pub login_token: Input,
    pub login_field: u8,
    pub login_busy: bool,
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
        let client_id = if config.anilist_client_id > 0 {
            config.anilist_client_id.to_string()
        } else {
            String::new()
        };
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
            activity_scroll: 0,
            pane_regions: vec![],
            statuses: vec![],
            generation: 0,
            modal: None,
            audio_prompt: None,
            audio_rows: vec![],
            playback_id: 0,
            command_input: Input::default(),
            command_selected: 0,
            login_client_id: Input::new(client_id),
            login_token: Input::with_capacity(2000),
            login_field: 1,
            login_busy: false,
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
    pub fn pane(&self) -> Pane {
        match self.focus {
            Focus::Search => Pane::Search,
            Focus::Titles => match self.view {
                View::Discover | View::Search => Pane::Results,
                View::Continue => Pane::Continue,
                View::Watchlist => Pane::Watchlist,
                View::Offline => Pane::Offline,
            },
            Focus::Details => Pane::Details,
            Focus::Episodes | Focus::EpisodeFilter => Pane::Episodes,
            Focus::Activity => Pane::Activity,
        }
    }
    pub fn focus_pane(&mut self, pane: Pane) -> Vec<Action> {
        let view = match pane {
            Pane::Search => Some(View::Search),
            Pane::Results => Some(if self.query.value.trim().is_empty() {
                View::Discover
            } else {
                View::Search
            }),
            Pane::Continue => Some(View::Continue),
            Pane::Watchlist => Some(View::Watchlist),
            Pane::Offline => Some(View::Offline),
            _ => None,
        };
        let actions = view
            .filter(|view| *view != self.view)
            .map(|view| self.switch_view(view))
            .unwrap_or_default();
        self.focus = match pane {
            Pane::Search => Focus::Search,
            Pane::Results | Pane::Continue | Pane::Watchlist | Pane::Offline => Focus::Titles,
            Pane::Details => Focus::Details,
            Pane::Episodes => Focus::Episodes,
            Pane::Activity => Focus::Activity,
        };
        actions
    }
    fn cycle_pane(&mut self, backwards: bool) -> Vec<Action> {
        let index = Pane::ALL
            .iter()
            .position(|p| *p == self.pane())
            .unwrap_or(0);
        let next = (index + if backwards { Pane::ALL.len() - 1 } else { 1 }) % Pane::ALL.len();
        self.focus_pane(Pane::ALL[next])
    }
    pub fn mouse(&mut self, event: MouseEvent) -> Vec<Action> {
        if self.modal == Some(Modal::Audio) {
            match event.kind {
                MouseEventKind::ScrollUp => self.move_audio(-1),
                MouseEventKind::ScrollDown => self.move_audio(1),
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some((_, index)) = self
                        .audio_rows
                        .iter()
                        .find(|(rect, _)| rect.contains(Position::new(event.column, event.row)))
                        && let Some(prompt) = self.audio_prompt.as_mut()
                    {
                        prompt.state.select(Some(*index));
                    }
                }
                _ => {}
            }
            return vec![];
        }
        if self.modal.is_some()
            || !matches!(
                event.kind,
                MouseEventKind::Down(MouseButton::Left)
                    | MouseEventKind::ScrollUp
                    | MouseEventKind::ScrollDown
            )
        {
            return vec![];
        }
        let position = Position::new(event.column, event.row);
        let Some(region) = self
            .pane_regions
            .iter()
            .find(|r| r.area.contains(position))
            .copied()
        else {
            return vec![];
        };
        let actions = self.focus_pane(region.pane);
        match event.kind {
            MouseEventKind::ScrollUp => self.move_selection(-3),
            MouseEventKind::ScrollDown => self.move_selection(3),
            MouseEventKind::Down(MouseButton::Left) if region.content.contains(position) => {
                let index = region.offset + usize::from(event.row - region.content.y);
                if self.focus == Focus::Titles && index < self.visible().len() {
                    if self.titles.selected() != Some(index) {
                        self.titles.select(Some(index));
                        self.selection_changed();
                    }
                } else if self.focus == Focus::Episodes && index < self.visible_episodes().len() {
                    self.episodes.select(Some(index));
                }
            }
            _ => {}
        }
        actions
    }
    pub fn needs_tick_redraw(&self) -> bool {
        self.downloading
            || self.statuses.iter().any(|s| s.pending)
            || self.search_pending.is_some()
            || self.detail_pending.is_some()
            || self.meta_pending.is_some()
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
        self.activity_scroll = 0;
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
            // Only offline titles change episode source. Keep unrelated online
            // details cached when traversing the Offline pane with Tab.
            for entry in &self.offline {
                self.details.remove(&entry.anime.key());
            }
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
    fn unfiltered_titles(&self, view: View) -> Vec<Anime> {
        match view {
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
        }
    }
    pub fn view_titles(&self, view: View) -> Vec<Anime> {
        self.unfiltered_titles(view)
            .into_iter()
            .filter(|a| {
                self.provider_filter.is_none_or(|p| a.provider == p)
                    && match self.audio {
                        Audio::All => true,
                        Audio::Sub => !a.dub,
                        Audio::Dub => a.dub,
                    }
            })
            .collect()
    }
    pub fn rebuild(&mut self) {
        // Keep the unfiltered catalogue: p/a can change filters without a refetch.
        self.catalog = self.unfiltered_titles(self.view);
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
            Message::Login(result) => {
                self.login_busy = false;
                match result {
                    Ok(user) => {
                        self.anilist_user = Some(user.clone());
                        self.anilist_error = None;
                        if let Ok(id) = self.login_client_id.value.trim().parse::<u32>() {
                            self.config.anilist_client_id = id;
                        }
                        self.modal = None;
                        self.notify(format!("AniList: logged in as {user}"), false);
                    }
                    Err(error) => {
                        self.notify(format!("AniList login failed: {error}"), true);
                        // Keep the form open so the token can be re-pasted.
                        self.login_token = Input::with_capacity(2000);
                    }
                }
            }
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
            Message::Offline(result) => match result {
                Ok(offline) => {
                    let selected = self.selected().map(Anime::key);
                    if self.view == View::Offline {
                        for entry in self.offline.iter().chain(offline.iter()) {
                            self.details.remove(&entry.anime.key());
                        }
                    }
                    self.offline = offline;
                    if self.view == View::Offline {
                        self.rebuild();
                        if let Some(index) = selected
                            .and_then(|key| self.visible().iter().position(|a| a.key() == key))
                        {
                            self.titles.select(Some(index));
                            self.selection_changed();
                        }
                    }
                }
                Err(error) => self.notify(format!("Cannot read offline library: {error}"), true),
            },
            Message::ChooseAudio {
                request_id,
                tracks,
                reply,
            } => {
                if request_id != self.playback_id || !self.playing || reply.is_closed() {
                    return vec![];
                }
                let selected = tracks.iter().position(|track| track.default).unwrap_or(0);
                self.audio_prompt = Some(AudioPrompt {
                    tracks,
                    state: ListState::default().with_selected(Some(selected)),
                    reply,
                });
                if self.modal != Some(Modal::Quit) {
                    self.modal = Some(Modal::Audio);
                }
                self.notify("Choose an audio language before starting the player", false);
            }
            Message::PlaybackStarted {
                request_id,
                audio,
                warning,
            } => {
                if request_id != self.playback_id {
                    return vec![];
                }
                let mut notice = audio.map_or_else(
                    || "Playing in your player".into(),
                    |label| format!("Playing · {label}"),
                );
                if let Some(warning) = warning {
                    notice.push_str(&format!(
                        " · Audio metadata unavailable ({warning}); using player default"
                    ));
                }
                notice.push_str(" · Ctrl+x to stop");
                self.notify(notice, false);
            }
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
            Message::PlaybackEnded { request_id, result } => {
                if request_id != self.playback_id {
                    return vec![];
                }
                self.playing = false;
                self.audio_prompt = None;
                if self.modal == Some(Modal::Audio) {
                    self.modal = None;
                }
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
        if self.modal.is_some()
            && self.modal != Some(Modal::Commands)
            && self.modal != Some(Modal::Login)
        {
            return;
        }
        if self.modal == Some(Modal::Login) {
            if self.login_busy {
                return;
            }
            let input = if self.login_field == 0 {
                &mut self.login_client_id
            } else {
                &mut self.login_token
            };
            input.insert(text);
            return;
        }
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
                K::Char('x') => return self.cancel_playback(),
                K::Char('d') => {
                    self.downloading = false;
                    self.notify("Download cancelled; partial file removed", false);
                    return vec![Action::CancelDownload];
                }
                _ => {}
            }
        }
        if self.modal == Some(Modal::Audio) {
            match key.code {
                K::Esc | K::Char('q') => return self.cancel_playback(),
                K::Down | K::Char('j') | K::Tab => self.move_audio(1),
                K::Up | K::Char('k') | K::BackTab => self.move_audio(-1),
                K::Home | K::Char('g') => self.move_audio(-1000),
                K::End | K::Char('G') => self.move_audio(1000),
                K::Enter => self.confirm_audio(),
                _ => {}
            }
            return vec![];
        }
        if self.modal == Some(Modal::Login) {
            if self.login_busy {
                return vec![];
            }
            match key.code {
                K::Esc => {
                    self.close_modal();
                    return vec![];
                }
                K::Tab | K::Down => {
                    self.login_field = 1 - self.login_field;
                    return vec![];
                }
                K::BackTab | K::Up => {
                    self.login_field = 1 - self.login_field;
                    return vec![];
                }
                K::Enter if self.login_field == 0 => {
                    self.login_field = 1;
                    return vec![];
                }
                K::Enter => {
                    if self.login_token.value.trim().is_empty() {
                        self.notify("Paste your AniList token first", true);
                        return vec![];
                    }
                    let client_id = self.login_client_id.value.trim().parse().unwrap_or(0);
                    if client_id == 0 {
                        self.notify(
                            "Enter your AniList client id (anilist.co/settings/developer)",
                            true,
                        );
                        self.login_field = 0;
                        return vec![];
                    }
                    self.login_busy = true;
                    return vec![Action::AniListLogin {
                        client_id,
                        token: self.login_token.value.clone(),
                    }];
                }
                K::Char('o') if self.login_field != 0 => {
                    if let Ok(id) = self.login_client_id.value.trim().parse::<u32>() {
                        return vec![Action::OpenAniListAuth(id)];
                    }
                }
                _ => {}
            }
            let input = if self.login_field == 0 {
                &mut self.login_client_id
            } else {
                &mut self.login_token
            };
            input.key(key);
            return vec![];
        }
        if let Some(modal) = self.modal {
            if key.code == K::Esc
                || (modal != Modal::Commands
                    && modal != Modal::Login
                    && matches!(key.code, K::Char('q') | K::Char('?')))
            {
                self.close_modal();
                return vec![];
            }
            if modal == Modal::Quit {
                if matches!(key.code, K::Char('y') | K::Char('Y')) {
                    return vec![Action::Quit];
                }
                if matches!(key.code, K::Char('n') | K::Char('N')) {
                    self.close_modal();
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
                            let actions = self.execute_command(index);
                            if self.modal.is_none() {
                                self.close_modal();
                            }
                            return actions;
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
        // Pane traversal must work inside text editors too (without inserting h/l).
        if matches!(key.code, K::Tab | K::BackTab) {
            return self.cycle_pane(key.code == K::BackTab || key.modifiers.contains(M::SHIFT));
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
                    if self.view != View::Search {
                        self.view = View::Search;
                        self.rebuild();
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
                K::Down => return self.focus_pane(Pane::Results),
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
                K::Esc | K::Enter => self.focus = Focus::Episodes,
                _ => {
                    if self.episode_filter.key(key) {
                        self.episodes.select(Some(0));
                    }
                }
            }
            return vec![];
        }
        match key.code {
            K::Char('/') | K::Char('1') => return self.focus_pane(Pane::Search),
            K::Char('q') => return self.quit(),
            K::Char('?') => self.modal = Some(Modal::Help),
            K::Char(':') => self.open_commands(),
            K::Char(',') => self.modal = Some(Modal::Settings),
            K::Char('!') => self.modal = Some(Modal::Diagnostics),
            K::Char('2') => return self.focus_pane(Pane::Results),
            K::Char('3') => return self.focus_pane(Pane::Continue),
            K::Char('4') => return self.focus_pane(Pane::Watchlist),
            K::Char('5') => return self.focus_pane(Pane::Offline),
            K::Char('0') => return self.focus_pane(Pane::Details),
            K::Char('6') => return self.focus_pane(Pane::Episodes),
            K::Char('7') => return self.focus_pane(Pane::Activity),
            K::Char('l') | K::Right => return self.cycle_pane(false),
            K::Char('h') | K::Left => return self.cycle_pane(true),
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
    fn close_modal(&mut self) {
        self.modal = self.audio_prompt.as_ref().map(|_| Modal::Audio);
    }
    fn move_audio(&mut self, delta: i32) {
        if let Some(prompt) = self.audio_prompt.as_mut() {
            let index = (prompt.state.selected().unwrap_or(0) as i32 + delta)
                .clamp(0, prompt.tracks.len().saturating_sub(1) as i32)
                as usize;
            prompt
                .state
                .select((!prompt.tracks.is_empty()).then_some(index));
        }
    }
    fn confirm_audio(&mut self) {
        if let Some(prompt) = self.audio_prompt.take() {
            let track = prompt
                .state
                .selected()
                .and_then(|index| prompt.tracks.get(index))
                .cloned();
            if prompt.reply.send(track).is_err() {
                self.playing = false;
                self.notify("Audio selection expired; retry the episode", true);
            } else {
                self.notify("Starting player with the selected audio…", false);
            }
        }
        self.modal = None;
    }
    fn cancel_playback(&mut self) -> Vec<Action> {
        self.playback_id = self.playback_id.wrapping_add(1);
        self.playing = false;
        self.audio_prompt = None;
        if self.modal == Some(Modal::Audio) {
            self.modal = None;
        }
        self.notify("Playback cancelled / stopped", false);
        vec![Action::CancelPlayback]
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
                if self.anilist_user.is_some() {
                    self.notify(
                        format!(
                            "AniList: syncing as {} · log out with `anime-levisione logout`",
                            self.anilist_user.clone().unwrap_or_default()
                        ),
                        false,
                    );
                    return vec![];
                }
                self.login_field = if self.login_client_id.value.trim().parse::<u32>().is_ok() {
                    1
                } else {
                    0
                };
                self.modal = Some(Modal::Login);
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
        if self.focus == Focus::Activity {
            self.activity_scroll =
                (self.activity_scroll as i32 + delta).clamp(0, u16::MAX as i32) as u16;
            return;
        }
        if !matches!(self.focus, Focus::Titles | Focus::Episodes) {
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
            self.playback_id = self.playback_id.wrapping_add(1);
            self.playing = true;
            self.playback_label = format!("{} · Ep {}", anime.title, episode.number);
            self.notify("Resolving stream…", false);
            return vec![Action::Play {
                request_id: self.playback_id,
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

    fn show_audio(
        app: &mut App,
        request_id: u64,
    ) -> tokio::sync::oneshot::Receiver<Option<AudioTrack>> {
        let (reply, receive) = tokio::sync::oneshot::channel();
        let tracks = anime_levisione_core::audio::hls_tracks("#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,NAME=\"Italian\",LANGUAGE=\"ita\",DEFAULT=YES\n#EXT-X-MEDIA:TYPE=AUDIO,NAME=\"English\",LANGUAGE=\"eng\"").expect("tracks");
        app.message(Message::ChooseAudio {
            request_id,
            tracks,
            reply,
        });
        receive
    }

    #[test]
    fn audio_picker_waits_for_confirmation_and_returns_the_selected_language() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        app.playing = true;
        app.playback_id = 7;
        let mut receive = show_audio(&mut app, 7);
        assert_eq!(app.modal, Some(Modal::Audio));
        assert!(
            receive.try_recv().is_err(),
            "do not launch before confirmation"
        );
        app.key(K::Down.into());
        assert!(app.key(K::Enter.into()).is_empty());
        let track = receive.try_recv().expect("choice").expect("track");
        assert_eq!(track.language.as_deref(), Some("eng"));
        assert!(app.playing, "the original player task continues");
        assert!(app.modal.is_none());
        assert!(app.audio_prompt.is_none());
    }

    #[test]
    fn cancelling_audio_rejects_late_prompts_and_old_player_events() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        app.playing = true;
        app.playback_id = 7;
        let mut receive = show_audio(&mut app, 7);
        assert!(matches!(
            app.key(K::Esc.into()).as_slice(),
            [Action::CancelPlayback]
        ));
        assert!(!app.playing);
        assert!(app.audio_prompt.is_none());
        assert!(receive.try_recv().is_err());
        let mut stale = show_audio(&mut app, 7);
        assert!(stale.try_recv().is_err());
        assert!(app.modal.is_none());
        app.playing = true;
        app.message(Message::PlaybackEnded {
            request_id: 7,
            result: Ok(()),
        });
        assert!(app.playing, "an old completion cannot clear a new request");
    }

    #[test]
    fn audio_prompt_survives_quit_and_command_overlays_without_leaking_paste() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        app.playing = true;
        app.playback_id = 7;
        app.modal = Some(Modal::Quit);
        let _receive = show_audio(&mut app, 7);
        assert_eq!(app.modal, Some(Modal::Quit));
        app.key(K::Char('n').into());
        assert_eq!(app.modal, Some(Modal::Audio));
        app.focus = Focus::Search;
        app.paste("not a search");
        assert!(app.query.value.is_empty());
        app.key(KeyEvent::new(K::Char('p'), M::CONTROL));
        assert_eq!(app.modal, Some(Modal::Commands));
        app.key(K::Esc.into());
        assert_eq!(app.modal, Some(Modal::Audio));
        app.key(KeyEvent::new(K::Char('x'), M::CONTROL));
        assert!(app.audio_prompt.is_none());
        assert!(!app.playing);
    }

    #[test]
    fn tab_and_shift_tab_visit_every_pane_including_text_editors() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        app.focus = Focus::Search;
        for expected in Pane::ALL.into_iter().cycle().skip(1).take(16) {
            app.key(K::Tab.into());
            assert_eq!(app.pane(), expected);
        }
        for expected in Pane::ALL.into_iter().rev().cycle().take(16) {
            app.key(K::BackTab.into());
            assert_eq!(app.pane(), expected);
        }
        app.focus = Focus::EpisodeFilter;
        app.episode_filter = Input::new("12");
        app.key(K::Tab.into());
        assert_eq!(app.pane(), Pane::Activity);
        assert_eq!(app.episode_filter.value, "12");
        app.focus = Focus::Search;
        app.key(KeyEvent::new(K::Tab, M::SHIFT));
        assert_eq!(app.pane(), Pane::Activity);
        assert!(app.query.value.is_empty());
    }

    #[test]
    fn search_submit_from_a_library_pane_returns_to_search_results() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        app.query = Input::new("frieren");
        app.start_catalog(Some("frieren".into()));
        app.switch_view(View::Watchlist);
        app.focus_pane(Pane::Search);
        app.key(K::Enter.into());
        assert_eq!(app.view, View::Search);
        assert_eq!(app.pane(), Pane::Results);
    }

    #[test]
    fn tabbing_past_empty_offline_library_keeps_online_details_cached() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        crate::demo::populate(&mut app);
        let count = app.details.len();
        app.focus_pane(Pane::Offline);
        app.focus_pane(Pane::Results);
        assert_eq!(app.details.len(), count);
    }

    #[test]
    fn offline_scan_result_preserves_focus_and_selected_title() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        let offline = ["a", "b"]
            .map(|id| OfflineEpisode {
                anime: sample(ProviderId::AnimeWorld, id),
                episode: Episode {
                    id: "local".into(),
                    number: "1".into(),
                    title: String::new(),
                },
                path: "/tmp/episode.mkv".into(),
            })
            .to_vec();
        app.offline = offline.clone();
        app.switch_view(View::Offline);
        app.titles.select(Some(1));
        app.focus = Focus::Episodes;
        app.message(Message::Offline(Ok(offline)));
        assert_eq!(app.titles.selected(), Some(1));
        assert_eq!(app.focus, Focus::Episodes);
    }

    #[test]
    fn offline_detail_debounce_requests_a_redraw_even_without_effects() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        assert!(!app.needs_tick_redraw());
        let anime = sample(ProviderId::AnimeWorld, "offline");
        app.offline.push(OfflineEpisode {
            anime: anime.clone(),
            episode: Episode {
                id: "local".into(),
                number: "1".into(),
                title: String::new(),
            },
            path: "/tmp/episode.mkv".into(),
        });
        app.switch_view(View::Offline);
        app.detail_pending = Some(Instant::now() - Duration::from_secs(1));
        assert!(app.needs_tick_redraw());
        assert!(app.on_tick().is_empty());
        assert_eq!(app.details[&anime.key()].1.len(), 1);
        assert!(!app.needs_tick_redraw());
    }

    #[test]
    fn non_list_panes_do_not_move_the_title_selection() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        crate::demo::populate(&mut app);
        let selected = app.titles.selected();
        for focus in [Focus::Search, Focus::Activity, Focus::Details] {
            app.focus = focus;
            app.move_selection(3);
            assert_eq!(app.titles.selected(), selected);
        }
        assert_eq!(app.detail_scroll, 3);
        assert_eq!(app.activity_scroll, 3);
    }

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
    fn anilist_login_form_opens_submits_and_reports_outcomes() {
        let mut app = App::new(Config::default(), Library::default(), "test".into(), true);
        // Command palette entry opens the form when logged out, keeps the
        // status notification when already syncing.
        app.anilist_user = None;
        assert!(app.execute_command(16).is_empty());
        assert_eq!(app.modal, Some(Modal::Login));
        app.anilist_user = Some("viewer".into());
        app.key(K::Esc.into()); // close the form first
        assert!(app.execute_command(16).is_empty());
        assert_eq!(app.modal, None, "status peek must not reopen the form");
        app.anilist_user = None;
        app.execute_command(16);
        // A whole redirect URL pasted into the token field survives intact.
        app.key(K::Tab.into()); // move from the client id to the token field
        let url =
            "https://anilist.co/api/v2/oauth/paste#access_token=tok.veryLong&token_type=Bearer";
        app.paste(url);
        assert_eq!(app.login_token.value, url);
        // Submit without a client id complains and focuses that field.
        app.key(K::Enter.into());
        assert!(app.login_field == 0 && !app.login_busy);
        app.login_client_id.insert("54777");
        // Enter on the client id moves to the token; Enter there submits.
        app.key(K::Enter.into());
        assert_eq!(app.login_field, 1);
        let actions = app.key(K::Enter.into());
        match actions.first() {
            Some(Action::AniListLogin { client_id, token }) => {
                assert_eq!(*client_id, 54777);
                assert_eq!(token.as_str(), url);
            }
            other => panic!("expected a login action, got {}", other.is_some()),
        }
        assert!(app.login_busy);
        // Failure keeps the form open and clears the bad token.
        app.message(Message::Login(Err("rejected".into())));
        assert_eq!(app.modal, Some(Modal::Login));
        assert!(app.login_token.value.is_empty());
        assert!(!app.login_busy);
        // Success closes it, records the account, remembers the client id.
        app.login_token.insert("tok");
        app.key(K::Enter.into());
        app.message(Message::Login(Ok("k33wee".into())));
        assert_eq!(app.modal, None);
        assert_eq!(app.anilist_user.as_deref(), Some("k33wee"));
        assert_eq!(app.config.anilist_client_id, 54777);
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
        let progress = |completed| anime_levisione_core::store::Progress {
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
