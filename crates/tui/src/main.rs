use anime_levisione::{app, demo, events, input, playback, ui};
use anime_levisione_core::{
    audio::{self, AudioTrack},
    diagnostics::{self, DiagnosticEvent, DiagnosticOperation},
    media_error::MediaError,
    model, provider, store,
    sync::{self, Account},
};
use anyhow::{Context, Result, bail};
use app::{Action, App};
use clap::{Parser, Subcommand};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream, KeyEventKind, MouseButton, MouseEventKind,
    },
    execute,
};
use events::Message;
use futures_util::StreamExt;
use model::{Anime, ProviderId, rank, sort_by_rank};
use provider::{Provider, Registry, bounded};
use std::{
    collections::VecDeque,
    io::{self, IsTerminal},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use store::{Config, Library, Store};
use tokio::{sync::mpsc, task::JoinHandle};

/// The player monitor reports progress every ~2 s; writing (and fsync-ing) the whole
/// library on each report is wasteful, so progress saves are debounced and always
/// flushed once more on exit.
const PROGRESS_SAVE_DEBOUNCE: Duration = Duration::from_secs(30);

#[derive(Parser)]
#[command(
    version,
    about,
    long_about = "A lazygit-style anime browser. Search AnimeWorld and AnimeUnity, save a watchlist, and watch with mpv or VLC. Italian subtitles/dubs. GPLv3; no warranty."
)]
struct Cli {
    /// Start with a search query
    query: Option<String>,
    /// Configuration/library directory (also permits isolated instances)
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// Offline UI preview; never accesses providers or writes your library
    #[arg(long)]
    demo: bool,
    /// Do not save automatic watch progress or search history
    #[arg(long)]
    private: bool,
    /// Override player executable (arguments belong in config.toml)
    #[arg(long)]
    player: Option<String>,
    #[command(subcommand)]
    command: Option<Commands>,
}
#[derive(Subcommand)]
enum Commands {
    /// Search without opening the TUI (partial provider failures go to stderr)
    Search {
        query: String,
        #[arg(long)]
        json: bool,
        #[arg(long, value_enum)]
        provider: Option<ProviderId>,
    },
    /// Check player, downloader, configuration and live provider connectivity
    Doctor,
    /// Export local diagnostic logs to stdout (no library data or credentials)
    Logs,
    /// Connect a personal Telegram bot or another device's secret connection key (stdin)
    Login {
        /// Optional private Telegram chat ID; otherwise detected after a recent /start
        #[arg(long)]
        chat_id: Option<String>,
    },
    /// Merge the remote Telegram library with local lists and progress
    Sync,
    /// Print the secret connection key for another device (do not share publicly)
    Account,
    /// Disconnect locally, keeping local and Telegram libraries
    Logout,
}
#[tokio::main]
async fn main() -> Result<()> {
    let result = run().await;
    if let Err(error) = &result {
        diagnostics::record(DiagnosticEvent::CoreFailure {
            kind: diagnostics::failure(error),
        });
    }
    diagnostics::record(DiagnosticEvent::SessionEnded);
    result
}
async fn run() -> Result<()> {
    let cli = Cli::parse();
    if cli.demo && cli.command.is_some() {
        bail!("--demo is a TUI preview; do not combine it with subcommands");
    }
    if matches!(cli.command, Some(Commands::Logs)) {
        let directory = Store::directory(cli.data_dir)?.join("logs");
        print!("{}", diagnostics::read_snapshot(&directory)?);
        return Ok(());
    }
    let mut store = if cli.demo {
        None
    } else {
        Some(Store::open(cli.data_dir)?)
    };
    if let Some(store) = &store
        && diagnostics::initialize(&store.dir.join("logs")).is_err()
    {
        eprintln!("Warning: local diagnostic logging is unavailable.");
    }
    let mut config = store.as_ref().map(|s| s.config.clone()).unwrap_or_default();
    if cli.player.is_none()
        && config.refresh_default_player()
        && let Some(store) = store.as_mut()
    {
        store.save_config(&config)?;
        store.config = config.clone();
    }
    if cli.private {
        config.private = true;
    }
    if let Some(player) = cli.player {
        config.player = player;
    }
    if let Some(command) = cli.command {
        if !matches!(command, Commands::Search { .. } | Commands::Doctor) {
            let store = store.as_ref().context("No config directory")?;
            return match command {
                Commands::Login { chat_id } => login(chat_id, store, config.private).await,
                Commands::Sync => sync_library(store, config.private).await,
                Commands::Account => {
                    println!(
                        "{}",
                        Account::load(&store.dir)?
                            .context("Connect a bot first")?
                            .connection_key()?
                    );
                    Ok(())
                }
                Commands::Logout => {
                    Account::remove(&store.dir)?;
                    println!("Disconnected. Local and Telegram libraries are unchanged.");
                    Ok(())
                }
                _ => unreachable!(),
            };
        }
        let registry = Registry::new(&config, store.as_ref().map(|s| s.dir.as_path()))?;
        return match command {
            Commands::Search {
                query,
                json,
                provider,
            } => search_cli(registry, query, json, provider).await,
            Commands::Doctor => {
                doctor(
                    &config,
                    &registry,
                    store.as_ref().context("No config directory")?,
                )
                .await
            }
            _ => unreachable!("account commands handled above"),
        };
    }
    let registry = if cli.demo {
        None
    } else {
        Some(Registry::new(
            &config,
            store.as_ref().map(|s| s.dir.as_path()),
        )?)
    };
    let account = store
        .as_ref()
        .map(|s| Account::load(&s.dir))
        .transpose()?
        .flatten();
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!(
            "The TUI needs an interactive terminal. Use `anime-levisione search \"title\" --json` for scripts."
        );
    }
    let library = store
        .as_ref()
        .map(|s| s.library.clone())
        .unwrap_or_else(Library::default);
    let path = store
        .as_ref()
        .map(|s| s.dir.display().to_string())
        .unwrap_or_else(|| "Demo (not saved)".into());
    let mut app = App::new(config, library, path, cli.demo);
    if let Some(account) = &account {
        app.sync_identity = Some(account.identity());
        app.sync_label = account.label();
    }
    if cli.demo {
        demo::populate(&mut app);
    }
    let (tx, mut rx) = mpsc::unbounded_channel();
    // Locate live provider domains in the background: the TUI opens instantly,
    // catalog requests wait on the same one-shot resolution, and a rotation is
    // announced once it is found.
    if let Some(registry) = registry.clone() {
        let tx = tx.clone();
        tokio::spawn(async move {
            for id in ProviderId::ALL {
                registry.ready(id).await;
                let resolution = registry.resolution(id);
                if resolution.moved()
                    && let Some(url) = resolution.live.as_ref().map(|u| u.to_string())
                {
                    let _ = tx.send(Message::DomainMoved { provider: id, url });
                }
            }
        });
    }
    let mut effects = Effects {
        store,
        registry,
        account,
        sync_task: None,
        sync_pending: false,
        tx,
        search: vec![],
        detail: None,
        player: None,
        download: None,
        rescan: None,
        rescan_pending: false,
        demo_catalog: app.discovered.clone(),
        progress_dirty: false,
        last_save: Instant::now(),
    };
    let mut initial = if cli.demo {
        vec![]
    } else {
        app.start_catalog(None)
    };
    if let Some(query) = cli.query {
        app.query = input::Input::new(query);
        app.view = app::View::Search;
        app.focus = app::Focus::Titles;
        initial = app.start_catalog(Some(app.query.value.trim().to_owned()));
    }
    initial.push(Action::Rescan);
    effects.apply(initial, &mut app);
    let mut terminal = ratatui::try_init()?;
    let _restore = TerminalRestore;
    execute!(io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;
    let mut input = EventStream::new();
    let mut interval = tokio::time::interval(Duration::from_millis(80));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let result = async {
        let mut dirty = true;
        loop {
            if dirty {
                terminal.draw(|frame| ui::draw(frame, &mut app))?;
                dirty = false;
            }
            let actions = tokio::select! {
                _ = interval.tick() => {
                    // Offline details can be populated by a debounce without
                    // emitting an effect, so check pending work before ticking.
                    let pending = app.needs_tick_redraw();
                    let actions = app.on_tick();
                    dirty = pending || !actions.is_empty();
                    actions
                },
                message = rx.recv() => match message {
                    Some(message) => { dirty = true; app.message(message) },
                    None => break,
                },
                event = input.next() => match event {
                    Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                        dirty = true;
                        app.key(key)
                    },
                    Some(Ok(Event::Paste(text))) => { dirty = true; app.paste(&text); vec![] },
                    Some(Ok(Event::Mouse(mouse))) if matches!(mouse.kind,
                        MouseEventKind::Down(MouseButton::Left) | MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                    ) => { dirty = true; app.mouse(mouse) },
                    Some(Ok(Event::Resize(_, _))) => { dirty = true; vec![] },
                    Some(Err(e)) => return Err(anyhow::Error::from(e)),
                    None => break,
                    _ => vec![],
                },
                _ = tokio::signal::ctrl_c() => {
                    dirty = true;
                    app.key(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('c'), crossterm::event::KeyModifiers::CONTROL))
                },
            };
            if effects.apply(actions, &mut app) {
                break;
            }
            effects.start_pending_rescan(&mut app);
            effects.flush_progress(&mut app);
            effects.start_pending_sync(&mut app);
        }
        Ok::<_, anyhow::Error>(())
    }.await;
    effects.abort_all();
    // Persist progress updates that were still inside the debounce window.
    effects.flush_pending(&mut app);
    result
}
/// After a failed provider request, probe the candidate domains again. When
/// the provider has moved, notify the UI and hand back a provider pointed at
/// the live domain so the caller can retry once.
async fn relocate(
    registry: &Registry,
    id: ProviderId,
    tx: &mpsc::UnboundedSender<Message>,
    stale: &Provider,
) -> std::result::Result<Arc<Provider>, String> {
    let fresh = registry.revalidate(id).await?;
    if fresh.base.origin() != stale.base.origin() {
        let _ = tx.send(Message::DomainMoved {
            provider: id,
            url: fresh.base.to_string(),
        });
    }
    Ok(fresh)
}
async fn catalog(provider: &Provider, query: &Option<String>) -> Result<Vec<Anime>> {
    match query {
        Some(q) => bounded(provider.search(q)).await,
        None => bounded(provider.latest()).await,
    }
}
async fn resolve_media(
    registry: &Registry,
    anime: &Anime,
    episode: &model::Episode,
    tx: &mpsc::UnboundedSender<Message>,
) -> Result<playback::Media> {
    let provider = registry.ready(anime.provider).await;
    let mut stream = bounded(provider.resolve(anime, episode)).await;
    if stream.is_err()
        && registry.healable(anime.provider)
        && let Ok(fresh) = relocate(registry, anime.provider, tx, &provider).await
    {
        stream = bounded(fresh.resolve(anime, episode)).await;
    }
    Ok(playback::Media::Remote(stream?))
}

type PreparedAudio = (playback::Media, Vec<AudioTrack>, Option<String>);
/// A resolved download link can already be unavailable although its embed page
/// returned 200. Refresh it once for a known transport failure, never for missing
/// ffprobe/unsupported metadata. Metadata remains optional if recovery fails.
async fn prepare_audio<Refresh, RefreshFuture, Probe, ProbeFuture>(
    mut media: playback::Media,
    refresh: Refresh,
    mut probe: Probe,
) -> PreparedAudio
where
    Refresh: FnOnce() -> RefreshFuture,
    RefreshFuture: std::future::Future<Output = Result<playback::Media>>,
    Probe: FnMut(playback::Media) -> ProbeFuture,
    ProbeFuture: std::future::Future<Output = Result<Vec<AudioTrack>>>,
{
    let mut result = probe(media.clone()).await;
    if matches!(media, playback::Media::Remote(_))
        && result
            .as_ref()
            .err()
            .and_then(|e| e.downcast_ref::<MediaError>())
            .is_some_and(MediaError::retryable)
    {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Ok(fresh) = refresh().await {
            media = fresh;
            result = probe(media.clone()).await;
        }
    }
    match result {
        Ok(tracks) => (media, tracks, None),
        Err(error) => (media, vec![], Some(format!("{error:#}"))),
    }
}

/// The player task holds the resolved URL while the UI asks for a language.
/// Cancelling/aborting drops the receiver; stale prompts cannot launch a player.
async fn choose_audio(
    request_id: u64,
    tracks: Vec<anime_levisione_core::audio::AudioTrack>,
    tx: &mpsc::UnboundedSender<Message>,
) -> Result<Option<anime_levisione_core::audio::AudioTrack>> {
    if tracks.len() <= 1 {
        return Ok(None);
    }
    let (reply, receive) = tokio::sync::oneshot::channel();
    tx.send(Message::ChooseAudio {
        request_id,
        tracks,
        reply,
    })
    .map_err(|_| anyhow::anyhow!("Audio selection UI closed"))?;
    let track = receive
        .await
        .context("Audio selection cancelled")?
        .context("Audio selection cancelled")?;
    Ok(Some(track))
}

struct TerminalRestore;
impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
        ratatui::restore();
    }
}
struct Effects {
    store: Option<Store>,
    registry: Option<Registry>,
    account: Option<Account>,
    sync_task: Option<JoinHandle<()>>,
    sync_pending: bool,
    tx: mpsc::UnboundedSender<Message>,
    search: Vec<JoinHandle<()>>,
    detail: Option<JoinHandle<()>>,
    player: Option<JoinHandle<()>>,
    download: Option<JoinHandle<()>>,
    rescan: Option<std::thread::JoinHandle<()>>,
    rescan_pending: bool,
    demo_catalog: Vec<model::Anime>,
    progress_dirty: bool,
    last_save: Instant,
}
fn abort(handle: &mut Option<JoinHandle<()>>) {
    if let Some(handle) = handle.take() {
        handle.abort();
    }
}
impl Effects {
    fn apply(&mut self, actions: Vec<Action>, app: &mut App) -> bool {
        let mut actions: VecDeque<_> = actions.into();
        while let Some(action) = actions.pop_front() {
            match action {
                Action::Quit => return true,
                Action::Save => self.save(app),
                Action::SaveProgress => self.progress_dirty = true,
                Action::Rescan => {
                    if !app.demo {
                        self.rescan_pending = true;
                        self.start_pending_rescan(app);
                    }
                }
                Action::CancelPlayback => abort(&mut self.player),
                Action::CancelDownload => abort(&mut self.download),
                Action::Catalog { generation, query } => {
                    for handle in self.search.drain(..) {
                        handle.abort();
                    }
                    for id in ProviderId::ALL {
                        let tx = self.tx.clone();
                        if let Some(registry) = &self.registry {
                            let registry = registry.clone();
                            let query = query.clone();
                            self.search.push(tokio::spawn(async move {
                                let provider = registry.ready(id).await;
                                let mut result = catalog(&provider, &query)
                                    .await
                                    .map_err(|e| format!("{e:#}"));
                                if result.is_err()
                                    && registry.healable(id)
                                    && let Ok(fresh) = relocate(&registry, id, &tx, &provider).await
                                {
                                    result =
                                        catalog(&fresh, &query).await.map_err(|e| format!("{e:#}"));
                                }
                                let _ = tx.send(Message::Catalog {
                                    generation,
                                    provider: id,
                                    result,
                                });
                            }));
                        } else {
                            let results = self
                                .demo_catalog
                                .iter()
                                .filter(|a| a.provider == id)
                                .filter(|a| query.as_ref().is_none_or(|q| rank(a, q) >= 2000))
                                .cloned()
                                .collect();
                            let _ = tx.send(Message::Catalog {
                                generation,
                                provider: id,
                                result: Ok(results),
                            });
                        }
                    }
                }
                Action::Episodes(anime) => {
                    abort(&mut self.detail);
                    if let Some(registry) = &self.registry {
                        let registry = registry.clone();
                        let tx = self.tx.clone();
                        let key = anime.key();
                        self.detail = Some(tokio::spawn(async move {
                            let provider = registry.ready(anime.provider).await;
                            let mut result = bounded(provider.episodes(anime.clone()))
                                .await
                                .map_err(|e| format!("{e:#}"));
                            if result.is_err()
                                && registry.healable(anime.provider)
                                && let Ok(fresh) =
                                    relocate(&registry, anime.provider, &tx, &provider).await
                            {
                                result = bounded(fresh.episodes(anime.clone()))
                                    .await
                                    .map_err(|e| format!("{e:#}"));
                            }
                            let _ = tx.send(Message::Episodes { key, result });
                        }));
                    } else {
                        app.detail_loading = false;
                    }
                }
                Action::Play {
                    request_id,
                    anime,
                    episode,
                    local,
                    resume,
                } => {
                    abort(&mut self.player);
                    let Some(registry) = self.registry.clone() else {
                        continue;
                    };
                    let config = app.config.clone();
                    let tx = self.tx.clone();
                    self.player = Some(tokio::spawn(async move {
                        let mut diagnostic = diagnostics::Operation::new(
                            DiagnosticOperation::Playback,
                            Some(anime.provider),
                        );
                        let result = async {
                            let media = match local {
                                Some(media) => media,
                                None => resolve_media(&registry, &anime, &episode, &tx).await?,
                            };
                            let (media, tracks, audio_warning) =
                                if playback::supports_audio_choice(&config) {
                                    prepare_audio(
                                        media,
                                        || resolve_media(&registry, &anime, &episode, &tx),
                                        |media| async move {
                                            diagnostics::observe(
                                                DiagnosticOperation::AudioProbe,
                                                None,
                                                audio::probe(&media),
                                            )
                                            .await
                                        },
                                    )
                                    .await
                                } else {
                                    (media, vec![], None)
                                };
                            let audio = choose_audio(request_id, tracks, &tx).await?;
                            playback::play(
                                config,
                                playback::PlayRequest {
                                    id: request_id,
                                    media,
                                    anime,
                                    episode,
                                    resume,
                                    audio,
                                    audio_warning,
                                },
                                tx.clone(),
                            )
                            .await
                        }
                        .await;
                        diagnostic.finish(&result);
                        let result = result.map_err(|e| format!("{e:#}"));
                        let _ = tx.send(Message::PlaybackEnded { request_id, result });
                    }));
                }
                Action::Download { anime, episode } => {
                    abort(&mut self.download);
                    let Some(registry) = self.registry.clone() else {
                        continue;
                    };
                    let config = app.config.clone();
                    let tx = self.tx.clone();
                    self.download = Some(tokio::spawn(async move {
                        let mut diagnostic = diagnostics::Operation::new(
                            DiagnosticOperation::Download,
                            Some(anime.provider),
                        );
                        let result = async {
                            let provider = registry.ready(anime.provider).await;
                            let mut stream = bounded(provider.resolve(&anime, &episode)).await;
                            if stream.is_err()
                                && registry.healable(anime.provider)
                                && let Ok(fresh) =
                                    relocate(&registry, anime.provider, &tx, &provider).await
                            {
                                stream = bounded(fresh.resolve(&anime, &episode)).await;
                            }
                            let stream = stream?;
                            let _ = tx.send(Message::DownloadStarted);
                            playback::download(config, stream, anime, episode).await
                        }
                        .await;
                        diagnostic.finish(&result);
                        let result = result.map_err(|e| format!("{e:#}"));
                        let _ = tx.send(Message::DownloadEnded(result));
                    }));
                }
                Action::Sync => {
                    if app.sync_enabled() {
                        self.sync_pending = true;
                    }
                }
                Action::Connect {
                    credential,
                    chat_id,
                } => {
                    if app.config.private || app.demo || self.account.is_some() {
                        continue;
                    }
                    let tx = self.tx.clone();
                    abort(&mut self.sync_task);
                    self.sync_task = Some(tokio::spawn(async move {
                        let result = sync::Client::connect(&credential, &chat_id)
                            .await
                            .map_err(|e| format!("{e:#}"));
                        let _ = tx.send(Message::Login(result));
                    }));
                }
                Action::AdoptAccount(account) => {
                    if app.config.private || app.demo || self.account.is_some() {
                        continue;
                    }
                    let result = self
                        .store
                        .as_ref()
                        .context("No persistent library")
                        .and_then(|s| account.save(&s.dir));
                    match result {
                        Ok(()) => {
                            app.sync_identity = Some(account.identity());
                            app.sync_label = account.label();
                            app.connection_key = account.connection_key().unwrap_or_default();
                            app.modal = Some(app::Modal::ConnectionKey);
                            self.account = Some(account);
                            self.sync_pending = true;
                        }
                        Err(e) => {
                            app.notify(format!("Cannot save Telegram credentials: {e:#}"), true)
                        }
                    }
                }
                Action::ShowConnectionKey => {
                    if let Some(account) = &self.account {
                        match account.connection_key() {
                            Ok(key) => {
                                app.connection_key = key;
                                app.modal = Some(app::Modal::ConnectionKey);
                            }
                            Err(e) => app.notify(format!("{e:#}"), true),
                        }
                    } else {
                        app.notify("Connect a Telegram bot first", true);
                    }
                }
                Action::Disconnect => {
                    abort(&mut self.sync_task);
                    self.sync_pending = false;
                    if let Some(store) = &self.store
                        && let Err(e) = Account::remove(&store.dir)
                    {
                        app.notify(format!("Cannot disconnect: {e:#}"), true);
                        continue;
                    }
                    self.account = None;
                    app.sync_identity = None;
                    app.sync_label.clear();
                    app.connection_key.clear();
                    app.sync_error = None;
                    app.notify(
                        "Disconnected; local and Telegram libraries are unchanged",
                        false,
                    );
                }
            }
        }
        false
    }
    fn start_pending_sync(&mut self, app: &mut App) {
        if !app.sync_enabled() {
            if app.config.private {
                abort(&mut self.sync_task);
            }
            self.sync_pending = false;
            return;
        }
        if !self.sync_pending
            || self
                .sync_task
                .as_ref()
                .is_some_and(|task| !task.is_finished())
        {
            return;
        }
        // A failed local save is retried by the normal debounce, not every UI tick.
        self.sync_pending = false;
        self.save(app);
        if self.progress_dirty {
            return;
        }
        let Some(account) = self.account.clone() else {
            return;
        };
        let library = app.library.clone();
        let tx = self.tx.clone();
        self.sync_task = Some(tokio::spawn(async move {
            let result = match sync::Client::new(account) {
                Ok(client) => client.sync(&library).await,
                Err(e) => Err(e),
            }
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Message::Synced(result));
        }));
    }
    fn save(&mut self, app: &mut App) {
        self.last_save = Instant::now();
        if let Some(store) = &self.store
            && let Err(e) = store.save(&app.library)
        {
            self.progress_dirty = true;
            app.notify(format!("Could not save your library: {e:#}"), true);
            return;
        }
        self.progress_dirty = false;
    }
    fn flush_progress(&mut self, app: &mut App) {
        if self.progress_dirty && self.last_save.elapsed() >= PROGRESS_SAVE_DEBOUNCE {
            self.save(app);
            if app.sync_enabled() {
                self.sync_pending = true;
            }
        }
    }
    fn flush_pending(&mut self, app: &mut App) {
        if self.progress_dirty {
            self.save(app);
        }
    }
    fn start_pending_rescan(&mut self, app: &mut App) {
        if !self.rescan_pending || self.rescan.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        self.rescan_pending = false;
        let root = app.config.download_dir.clone();
        let tx = self.tx.clone();
        // A dedicated read-only worker cannot delay Tokio shutdown if the
        // filesystem stalls. Repeated requests coalesce into a follow-up scan.
        match std::thread::Builder::new()
            .name("offline-scan".into())
            .spawn(move || {
                let result = playback::scan_offline(&root).map_err(|e| format!("{e:#}"));
                let _ = tx.send(Message::Offline(result));
            }) {
            Ok(task) => self.rescan = Some(task),
            Err(error) => app.notify(format!("Cannot scan offline library: {error}"), true),
        }
    }
    fn abort_all(&mut self) {
        for handle in self.search.drain(..) {
            handle.abort();
        }
        abort(&mut self.detail);
        abort(&mut self.sync_task);
        abort(&mut self.player);
        abort(&mut self.download);
        // Dropping a std thread handle detaches this read-only scan.
        self.rescan.take();
    }
}
async fn login(chat_id: Option<String>, store: &Store, private: bool) -> Result<()> {
    if private {
        bail!("Disable private mode before connecting");
    }
    if Account::load(&store.dir)?.is_some() {
        bail!("Disconnect the current bot first");
    }
    eprintln!("First device: create a dedicated bot with @BotFather and send /start to it.");
    eprintln!(
        "Other devices: paste the secret alib1 connection key from Settings / `anime-levisione account`."
    );
    let credential = if io::stdin().is_terminal() {
        rpassword::prompt_password("Bot token or connection key: ")?
    } else {
        use std::io::Read;
        let mut value = String::new();
        io::stdin().take(8192).read_to_string(&mut value)?;
        value
    };
    let account =
        sync::Client::connect(&credential, chat_id.as_deref().unwrap_or_default()).await?;
    account.save(&store.dir)?;
    println!(
        "Connected. Save the secret key with `anime-levisione account` before syncing other devices."
    );
    sync_library(store, private).await
}
async fn sync_library(store: &Store, private: bool) -> Result<()> {
    if private {
        bail!("Sync is disabled in private mode");
    }
    let mut account = Account::load(&store.dir)?.context("Connect your Telegram bot first")?;
    let outcome = sync::Client::new(account.clone())?
        .sync(&store.library)
        .await?;
    let mut library = store.library.clone();
    outcome.apply(&mut library)?;
    store.save(&library)?;
    account.mark_initialized();
    account.save(&store.dir)?;
    println!("Library synchronized with your Telegram bot.");
    Ok(())
}
async fn search_cli(
    registry: Registry,
    query: String,
    json: bool,
    only: Option<ProviderId>,
) -> Result<()> {
    if query.trim().is_empty() {
        bail!("Search query cannot be empty");
    }
    let requests = ProviderId::ALL
        .into_iter()
        .filter(|&id| only.is_none_or(|p| p == id))
        .map(|id| {
            let registry = registry.clone();
            let query = query.clone();
            async move {
                let provider = registry.ready(id).await;
                (id, bounded(provider.search(&query)).await)
            }
        });
    let mut results = Vec::new();
    let mut successes = 0;
    for (id, result) in futures_util::future::join_all(requests).await {
        match result {
            Ok(mut animes) => {
                successes += 1;
                results.append(&mut animes);
            }
            Err(e) => eprintln!("{id}: {e:#}"),
        }
    }
    if successes == 0 {
        bail!(
            "All selected providers failed. Run `anime-levisione doctor` or check URLs in config.toml."
        );
    }
    sort_by_rank(&mut results, &query);
    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else if results.is_empty() {
        println!("No results. Try a shorter title or another spelling.");
    } else {
        for anime in results {
            println!(
                "{}  {}  {}  {}",
                anime.provider.short(),
                anime.audio(),
                anime.title,
                anime.id
            );
        }
    }
    Ok(())
}
async fn doctor(config: &Config, registry: &Registry, store: &Store) -> Result<()> {
    println!(
        "anime-levisione {}\nConfig: {}\nDownloads: {}",
        env!("CARGO_PKG_VERSION"),
        store.dir.display(),
        config.download_dir.display()
    );
    for (label, exe, args, unavailable) in [
        (
            "Player",
            &*config.player,
            vec!["--version"],
            "install VLC or mpv, or set player in config.toml",
        ),
        (
            "Downloader",
            "ffmpeg",
            vec!["-version"],
            "optional unless downloading",
        ),
    ] {
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::process::Command::new(exe)
                .args(args)
                .kill_on_drop(true)
                .output(),
        )
        .await;
        let ok = matches!(result, Ok(Ok(ref o)) if o.status.success());
        if ok {
            println!("OK {label}: {exe}");
        } else {
            println!("!! {label}: {exe} not available — {unavailable}");
        }
    }
    let checks = ProviderId::ALL.map(|id| {
        let registry = registry.clone();
        async move {
            let provider = registry.ready(id).await;
            (
                id,
                registry.resolution(id),
                bounded(provider.latest()).await,
            )
        }
    });
    let mut healthy = 0;
    for (id, resolution, result) in futures_util::future::join_all(checks).await {
        println!("Domain {id}: {}", resolution.describe());
        match result {
            Ok(items) => {
                healthy += 1;
                println!("OK {id}: {} latest titles", items.len());
            }
            Err(e) => println!("!! {id}: {e:#}"),
        }
    }
    if healthy == 0 {
        bail!("No provider is reachable");
    }
    match Account::load(&store.dir)? {
        Some(account) => println!(
            "-- {} · use `anime-levisione sync` to check remote sync",
            account.label()
        ),
        None => {
            println!("-- Telegram: local only; `anime-levisione login` to connect a personal bot")
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_and_connection_key_have_explicit_cli_commands() {
        assert!(matches!(
            Cli::try_parse_from(["anime-levisione", "sync"])
                .expect("CLI")
                .command,
            Some(Commands::Sync)
        ));
        assert!(matches!(
            Cli::try_parse_from(["anime-levisione", "account"])
                .expect("CLI")
                .command,
            Some(Commands::Account)
        ));
        assert!(
            Cli::try_parse_from(["anime-levisione", "login", "--token", "secret"]).is_err(),
            "credentials never belong in process arguments"
        );
    }

    fn remote_media(path: &str) -> playback::Media {
        playback::Media::Remote(provider::Stream {
            url: format!("https://media.example/{path}")
                .parse()
                .expect("test URL"),
            referer: "https://player.example/".into(),
        })
    }

    #[tokio::test]
    async fn known_transport_failure_refreshes_the_media_once_before_the_picker() {
        let mut probes = 0;
        let (media, _, warning) = prepare_audio(
            remote_media("old.mp4"),
            || async { Ok(remote_media("fresh.mp4")) },
            |media| {
                probes += 1;
                std::future::ready(if media.source().ends_with("old.mp4") {
                    Err(MediaError {
                        kind: diagnostics::DiagnosticFailure::Http,
                        status: Some(503),
                    }
                    .into())
                } else {
                    Ok(vec![])
                })
            },
        )
        .await;
        assert_eq!(probes, 2);
        assert!(media.source().ends_with("fresh.mp4"));
        assert!(warning.is_none());
    }

    #[tokio::test]
    async fn persistent_probe_failures_are_bounded_and_metadata_remains_optional() {
        let mut probes = 0;
        let mut refreshes = 0;
        let (_, tracks, warning) = prepare_audio(
            remote_media("old.mp4"),
            || {
                refreshes += 1;
                std::future::ready(Ok(remote_media("fresh.mp4")))
            },
            |_| {
                probes += 1;
                std::future::ready(Err(MediaError {
                    kind: diagnostics::DiagnosticFailure::Http,
                    status: Some(403),
                }
                .into()))
            },
        )
        .await;
        assert_eq!((probes, refreshes), (2, 1));
        assert!(tracks.is_empty());
        assert!(warning.expect("warning").contains("403"));
    }

    #[tokio::test]
    async fn missing_optional_probe_or_local_media_never_refresh_the_provider() {
        for media in [
            remote_media("source.mp4"),
            playback::Media::Local("fixture.mp4".into()),
        ] {
            let is_local = matches!(media, playback::Media::Local(_));
            let (_, _, warning) = prepare_audio(
                media,
                || async { panic!("must not refresh") },
                |_| {
                    std::future::ready(Err(if is_local {
                        MediaError {
                            kind: diagnostics::DiagnosticFailure::Http,
                            status: Some(503),
                        }
                        .into()
                    } else {
                        anyhow::anyhow!("ffprobe not installed")
                    }))
                },
            )
            .await;
            assert!(warning.is_some());
        }
    }

    #[tokio::test]
    async fn failed_refresh_preserves_the_original_player_fallback() {
        let (media, _, warning) = prepare_audio(
            remote_media("old.mp4"),
            || async { anyhow::bail!("resolution unavailable") },
            |_| {
                std::future::ready(Err(MediaError {
                    kind: diagnostics::DiagnosticFailure::Timeout,
                    status: None,
                }
                .into()))
            },
        )
        .await;
        assert!(media.source().ends_with("old.mp4"));
        assert!(warning.is_some());
    }

    #[tokio::test]
    async fn cancellation_during_recovery_never_refreshes_or_starts_a_player() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let refreshed = Arc::new(AtomicBool::new(false));
        let flag = refreshed.clone();
        let task = tokio::spawn(async move {
            prepare_audio(
                remote_media("old.mp4"),
                || async move {
                    flag.store(true, Ordering::SeqCst);
                    Ok(remote_media("fresh.mp4"))
                },
                |_| {
                    std::future::ready(Err(MediaError {
                        kind: diagnostics::DiagnosticFailure::Http,
                        status: Some(503),
                    }
                    .into()))
                },
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        tokio::time::sleep(Duration::from_millis(550)).await;
        assert!(!refreshed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn one_audio_language_bypasses_the_picker() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let tracks = anime_levisione_core::audio::hls_tracks(
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,LANGUAGE=\"ita\"",
        )
        .expect("tracks");
        assert!(
            choose_audio(7, tracks, &tx)
                .await
                .expect("single audio")
                .is_none()
        );
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn multiple_audio_languages_wait_for_the_ui_and_cancel_if_it_closes() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let tracks = anime_levisione_core::audio::hls_tracks("#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,LANGUAGE=\"ita\"\n#EXT-X-MEDIA:TYPE=AUDIO,LANGUAGE=\"eng\"").expect("tracks");
        let task = tokio::spawn(async move { choose_audio(7, tracks, &tx).await });
        match rx.recv().await.expect("audio prompt") {
            Message::ChooseAudio {
                request_id,
                tracks,
                reply,
            } => {
                assert_eq!(request_id, 7);
                assert!(!task.is_finished());
                reply.send(Some(tracks[1].clone())).expect("confirm");
            }
            _ => panic!("expected audio prompt"),
        }
        assert_eq!(
            task.await
                .expect("task")
                .expect("audio")
                .expect("choice")
                .language
                .as_deref(),
            Some("eng")
        );
        let (tx, mut rx) = mpsc::unbounded_channel();
        let tracks = anime_levisione_core::audio::hls_tracks("#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,LANGUAGE=\"ita\"\n#EXT-X-MEDIA:TYPE=AUDIO,LANGUAGE=\"eng\"").expect("tracks");
        let task = tokio::spawn(async move { choose_audio(8, tracks, &tx).await });
        drop(rx.recv().await);
        assert!(task.await.expect("task").is_err());
    }

    #[test]
    fn failed_sync_save_waits_for_debounce_instead_of_retrying_every_tick() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(Some(dir.path().into()))?;
        std::fs::create_dir(dir.path().join("library.json"))?;
        let mut app = App::new(
            store.config.clone(),
            Library::default(),
            "test".into(),
            false,
        );
        app.sync_identity = Some("account".into());
        let (tx, _) = mpsc::unbounded_channel();
        let mut effects = Effects {
            store: Some(store),
            registry: None,
            account: None,
            sync_task: None,
            sync_pending: true,
            tx,
            search: vec![],
            detail: None,
            player: None,
            download: None,
            rescan: None,
            rescan_pending: false,
            demo_catalog: vec![],
            progress_dirty: false,
            last_save: Instant::now(),
        };
        effects.start_pending_sync(&mut app);
        assert!(effects.progress_dirty);
        assert!(!effects.sync_pending);
        let attempted = effects.last_save;
        effects.start_pending_sync(&mut app);
        assert_eq!(attempted, effects.last_save);
        assert!(effects.sync_task.is_none());
        Ok(())
    }

    #[test]
    fn rescans_during_a_running_scan_are_coalesced_not_lost() {
        let directory = tempfile::tempdir().expect("offline directory");
        let config = Config {
            download_dir: directory.path().to_path_buf(),
            ..Config::default()
        };
        let mut app = App::new(config, Library::default(), "test".into(), false);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (release, wait) = std::sync::mpsc::channel();
        let mut effects = Effects {
            store: None,
            registry: None,
            account: None,
            sync_task: None,
            sync_pending: false,
            tx,
            search: vec![],
            detail: None,
            player: None,
            download: None,
            rescan: Some(std::thread::spawn(move || {
                let _ = wait.recv();
            })),
            rescan_pending: false,
            demo_catalog: vec![],
            progress_dirty: false,
            last_save: Instant::now(),
        };
        effects.apply(vec![Action::Rescan, Action::Rescan], &mut app);
        assert!(effects.rescan_pending);
        assert!(
            rx.try_recv().is_err(),
            "the running scan must not be duplicated"
        );
        release.send(()).expect("release fake scan");
        effects
            .rescan
            .take()
            .expect("running scan")
            .join()
            .expect("scan finished");
        effects.start_pending_rescan(&mut app);
        assert!(!effects.rescan_pending);
        effects
            .rescan
            .take()
            .expect("follow-up scan")
            .join()
            .expect("scan finished");
        assert!(matches!(rx.try_recv(), Ok(Message::Offline(Ok(offline))) if offline.is_empty()));
        assert!(
            rx.try_recv().is_err(),
            "requests coalesce into one follow-up"
        );
    }
}
