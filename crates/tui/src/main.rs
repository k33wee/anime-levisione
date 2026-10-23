use anime_levisione::{app, demo, events, input, playback, ui};
use anime_levisione_core::{anilist, model, provider, store};
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
    /// Connect AniList: open the authorization page and store your token
    Login {
        /// AniList OAuth client id (else anilist_client_id from config.toml)
        #[arg(long)]
        client_id: Option<u32>,
        /// Non-interactive: the access token (or the whole redirect URL)
        #[arg(long)]
        token: Option<String>,
    },
    /// Catch up AniList with the saved watchlist and watched progress
    /// (activity recorded before login; remote entries are never downgraded)
    Sync,
    /// Remove the stored AniList token (metadata stays anonymous)
    Logout,
}
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.demo && cli.command.is_some() {
        bail!("--demo is a TUI preview; do not combine it with subcommands");
    }
    // `sync` only reads the library and pushes to AniList: it must not need
    // the exclusive lock, so it can run while the TUI is open.
    let one_shot_readonly = matches!(cli.command, Some(Commands::Sync));
    let store = if cli.demo {
        None
    } else if one_shot_readonly {
        Some(Store::open_readonly(cli.data_dir)?)
    } else {
        Some(Store::open(cli.data_dir)?)
    };
    let mut config = store.as_ref().map(|s| s.config.clone()).unwrap_or_default();
    if cli.private {
        config.private = true;
    }
    if let Some(player) = cli.player {
        config.player = player;
    }
    let registry = if cli.demo {
        None
    } else {
        Some(Registry::new(
            &config,
            store.as_ref().map(|s| s.dir.as_path()),
        )?)
    };
    let anilist = if cli.demo {
        None
    } else {
        store
            .as_ref()
            .map(|s| anilist::Client::open(&s.dir).map(Arc::new))
            .transpose()?
    };
    if let Some(command) = cli.command {
        if let Commands::Sync = command {
            return sync(store.as_ref().context("No config directory")?).await;
        }
        let registry = registry.context("No providers configured")?;
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
            Commands::Login { client_id, token } => {
                login(
                    client_id,
                    token,
                    store.as_ref().context("No config directory")?,
                )
                .await
            }
            Commands::Logout => logout(store.as_ref().context("No config directory")?),
            Commands::Sync => unreachable!("handled before the registry is needed"),
        };
    }
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
    app.anilist_user = anilist.as_ref().and_then(|client| client.user());
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
        anilist,
        lookup: None,
        track: vec![],
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
    anilist: Option<Arc<anilist::Client>>,
    lookup: Option<JoinHandle<()>>,
    track: Vec<JoinHandle<()>>,
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
                        let result = async {
                            let media = match local {
                                Some(media) => media,
                                None => {
                                    let provider = registry.ready(anime.provider).await;
                                    let mut stream =
                                        bounded(provider.resolve(&anime, &episode)).await;
                                    if stream.is_err()
                                        && registry.healable(anime.provider)
                                        && let Ok(fresh) =
                                            relocate(&registry, anime.provider, &tx, &provider)
                                                .await
                                    {
                                        stream = bounded(fresh.resolve(&anime, &episode)).await;
                                    }
                                    playback::Media::Remote(stream?)
                                }
                            };
                            let (tracks, audio_warning) =
                                if playback::supports_audio_choice(&config) {
                                    match anime_levisione_core::audio::probe(&media).await {
                                        Ok(tracks) => (tracks, None),
                                        Err(error) => (vec![], Some(format!("{error:#}"))),
                                    }
                                } else {
                                    (vec![], None)
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
                        .await
                        .map_err(|e| format!("{e:#}"));
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
                        .await
                        .map_err(|e| format!("{e:#}"));
                        let _ = tx.send(Message::DownloadEnded(result));
                    }));
                }
                Action::Lookup(anime) => {
                    abort(&mut self.lookup);
                    let Some(client) = self.anilist.clone() else {
                        continue;
                    };
                    let tx = self.tx.clone();
                    let key = anime.key();
                    self.lookup = Some(tokio::spawn(async move {
                        let result = bounded(client.lookup(&anime))
                            .await
                            .map_err(|e| format!("{e:#}"));
                        let _ = tx.send(Message::Meta { key, result });
                    }));
                }
                Action::Track { anime, event } => {
                    let Some(client) = self.anilist.clone() else {
                        continue;
                    };
                    let tx = self.tx.clone();
                    self.track.retain(|handle| !handle.is_finished());
                    self.track.push(tokio::spawn(async move {
                        let result = bounded(client.track(&anime, event))
                            .await
                            .map_err(|e| format!("{e:#}"));
                        let _ = tx.send(Message::Tracked(result));
                    }));
                }
                Action::OpenAniListAuth(client_id) => {
                    let url = anilist::authorize_url(client_id);
                    let outcome =
                        open_browser(&url).map_err(|e| format!("could not open a browser: {e:#}"));
                    if let Err(error) = outcome {
                        let _ = self.tx.send(Message::Login(Err(error)));
                    }
                }
                Action::AniListLogin { client_id, token } => {
                    let token = extract_token(&token);
                    // Synchronous bookkeeping first, so the spawned future
                    // only owns clones (borrow checker cannot follow `self`
                    // across the await points).
                    if let Some(store) = &self.store
                        && client_id != 0
                        && client_id != store.config.anilist_client_id
                    {
                        let mut config = store.config.clone();
                        config.anilist_client_id = client_id;
                        let _ = store.save_config(&config);
                    }
                    let dir = self.store.as_ref().map(|s| s.dir.clone());
                    let live = self.anilist.clone();
                    let tx = self.tx.clone();
                    tokio::spawn(async move {
                        if token.is_empty() {
                            let _ = tx.send(Message::Login(Err("empty token".into())));
                            return;
                        }
                        // Verify through a scratch client so a bad token never
                        // disturbs the live one, then store and adopt it.
                        let user = match anilist::Client::with_token(token.clone()) {
                            Ok(client) => client.viewer().await.map_err(|e| format!("{e:#}")),
                            Err(e) => Err(format!("{e:#}")),
                        };
                        let user = match user {
                            Ok(user) => user,
                            Err(error) => {
                                let _ = tx.send(Message::Login(Err(error)));
                                return;
                            }
                        };
                        let stored = std::time::SystemTime::now()
                            .duration_since(std::time::SystemTime::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        if let Some(dir) = &dir {
                            let persist = anilist::Token {
                                user: user.clone(),
                                token: token.clone(),
                                stored,
                            };
                            if let Err(e) = persist.save(dir) {
                                let _ = tx.send(Message::Login(Err(format!(
                                    "token verified but not saved: {e:#}"
                                ))));
                                return;
                            }
                        }
                        if let Some(client) = &live {
                            client.adopt(token, user.clone());
                        }
                        let _ = tx.send(Message::Login(Ok(user)));
                    });
                }
            }
        }
        false
    }
    fn save(&mut self, app: &mut App) {
        self.last_save = Instant::now();
        if let Some(store) = &self.store
            && let Err(e) = store.save(&app.library)
        {
            app.notify(format!("Could not save your library: {e:#}"), true);
            return;
        }
        self.progress_dirty = false;
    }
    fn flush_progress(&mut self, app: &mut App) {
        if self.progress_dirty && self.last_save.elapsed() >= PROGRESS_SAVE_DEBOUNCE {
            self.save(app);
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
        abort(&mut self.lookup);
        for handle in self.track.drain(..) {
            handle.abort();
        }
        abort(&mut self.player);
        abort(&mut self.download);
        // Dropping a std thread handle detaches this read-only scan.
        self.rescan.take();
    }
}
async fn login(client_id: Option<u32>, token: Option<String>, store: &Store) -> Result<()> {
    let client_id = client_id.unwrap_or(store.config.anilist_client_id);
    if client_id == 0 {
        println!("AniList login needs a client id from https://anilist.co/settings/developer");
        println!("Create a client (any name, no redirect URL needed), then either:");
        println!("  anime-levisione login --client-id 1234");
        println!(
            "  or set anilist_client_id = 1234 in {}",
            store.dir.display()
        );
        return Ok(());
    }
    let url = anilist::authorize_url(client_id);
    let pasted = match token {
        // Non-interactive path: `--token` accepts the bare token or the
        // whole redirect URL, wrapped however the caller has it.
        Some(pasted) => pasted,
        None => {
            println!("1. Open this URL in a browser and authorize anime-levisione:");
            println!("   {url}");
            if let Err(e) = open_browser(&url) {
                println!("   (could not open a browser automatically: {e:#})");
            }
            println!("2. Copy the token shown on the authorization page.");
            println!(
                "   Pasting the whole https://anilist.co/...#access_token=... URL also works;"
            );
            println!(
                "   if it wraps over several lines, keep going and finish with an empty line."
            );
            read_pasted_token()?
        }
    };
    let token = extract_token(&pasted);
    if token.is_empty() {
        bail!("AniList token cannot be empty — paste the token or the full redirect URL");
    }
    let user = anilist::Client::with_token(token.clone())?
        .viewer()
        .await
        .context("AniList rejected this token")?;
    anilist::Token {
        user: user.clone(),
        token,
        stored: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    }
    .save(&store.dir)?;
    println!("Logged in as {user}. Watchlist adds and watched episodes sync from the TUI.");
    Ok(())
}
/// Read the pasted token from stdin. Long tokens (and whole redirect URLs)
/// often arrive wrapped over several lines; every line is kept until a blank
/// line (interactive) or EOF (pipe) closes the paste. A full redirect URL is
/// complete on its own, so reading stops as soon as one is recognized.
fn read_pasted_token() -> Result<String> {
    use std::io::Write as _;
    print!("3. Paste the token here: ");
    io::stdout().flush()?;
    let interactive = io::stdin().is_terminal();
    let mut pasted = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        let read = io::stdin().read_line(&mut line)?;
        if line.trim().is_empty() || read == 0 {
            if !pasted.is_empty() {
                return Ok(pasted);
            }
            if read == 0 && !interactive {
                bail!("No token received on stdin");
            }
            println!("Token cannot be empty; paste it again (Ctrl+c to abort):");
            continue;
        }
        pasted.push_str(line.trim());
        if pasted.contains("token_type=") {
            return Ok(pasted);
        }
    }
}
/// Reduce whatever was pasted — bare token, wrapped over lines, or the full
/// `...#access_token=<token>&token_type=...` redirect URL — to the token.
fn extract_token(pasted: &str) -> String {
    let joined: String = pasted.split_whitespace().collect();
    match joined.split_once("access_token=") {
        Some((_, tail)) => tail.split('&').next().unwrap_or_default().to_owned(),
        None => joined,
    }
}
fn logout(store: &Store) -> Result<()> {
    let path = anilist::Token::path(&store.dir);
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("Cannot remove {}", path.display()))?;
        println!("AniList token removed; metadata still loads anonymously.");
    } else {
        println!("No AniList token stored.");
    }
    Ok(())
}
/// Best effort: open the authorization page; a missing helper is not an error.
fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let (program, args): (&str, Vec<&str>) = ("open", vec![url]);
    #[cfg(target_os = "windows")]
    let (program, args): (&str, Vec<&str>) = ("cmd", vec!["/C", "start", "", url]);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let (program, args): (&str, Vec<&str>) = ("xdg-open", vec![url]);
    std::process::Command::new(program)
        .args(&args)
        .spawn()
        .with_context(|| format!("Cannot launch {program}"))?;
    Ok(())
}
/// Backfill the saved watchlist and watched progress to AniList. Runs the
/// same `track` path the TUI uses — so remote entries are never downgraded —
/// and reads `library.json` without the lock, so it can run while the TUI is
/// open.
async fn sync(store: &Store) -> Result<()> {
    if store.config.private {
        bail!("AniList sync is disabled while private mode is on in config.toml");
    }
    let client = anilist::Client::open(&store.dir)?;
    if !client.logged_in() {
        bail!("Not logged in — run `anime-levisione login` first");
    }
    println!(
        "Syncing to AniList as {} …",
        client.user().unwrap_or_else(|| "your account".into())
    );
    let library: Library = match std::fs::read(store.dir.join("library.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("Invalid library.json")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Library::default(),
        Err(e) => return Err(e.into()),
    };
    let mut failures = 0u32;
    for anime in &library.watchlist {
        match client
            .track(anime, anilist::TrackEvent::Watchlist { added: true })
            .await
        {
            Ok(text) => println!("  {text}"),
            Err(e) => {
                failures += 1;
                eprintln!("  {e:#}");
            }
        }
    }
    for entry in &library.history {
        let event = anilist::TrackEvent::Watched {
            number: entry.last_episode.clone(),
            total: entry.anime.episodes,
        };
        match client.track(&entry.anime, event).await {
            Ok(text) => println!("  {text}"),
            Err(e) => {
                failures += 1;
                eprintln!("  {e:#}");
            }
        }
    }
    if failures == 0 {
        println!("Done.");
    } else {
        println!("{failures} title(s) could not be synced (see above).");
    }
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
    for (exe, args) in [
        (&*config.player, vec!["--version"]),
        ("ffmpeg", vec!["-version"]),
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
        println!(
            "{} {exe}{}",
            if ok { "OK" } else { "!!" },
            if ok {
                ""
            } else {
                " not available (ffmpeg is optional unless downloading)"
            }
        );
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
    match anilist::Client::open(&store.dir) {
        Ok(client) if client.logged_in() => match bounded(client.viewer()).await {
            Ok(user) => println!("OK AniList: logged in as {user}"),
            Err(e) => println!("!! AniList: {e:#}"),
        },
        Ok(client) => match bounded(client.search("frieren")).await {
            Ok(_) => {
                println!("-- AniList: reachable · not logged in (`anime-levisione login` to sync)")
            }
            Err(e) => println!("!! AniList: {e:#}"),
        },
        Err(e) => println!("!! AniList: {e:#}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_token_accepts_bare_tokens_wrapped_lines_and_redirect_urls() {
        let token = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.veryLongPadding";
        // Bare token, single line.
        assert_eq!(extract_token(token), token);
        // Same token wrapped over three pasted lines.
        assert_eq!(
            extract_token(&format!(
                "{}\n{}\n{}\n",
                &token[..20],
                &token[20..40],
                &token[40..]
            )),
            token
        );
        // Full redirect URL, URL-fragment style.
        assert_eq!(
            extract_token(&format!(
                "https://anilist.co/api/v2/oauth/paste#access_token={token}&token_type=Bearer&expires_in=315360000"
            )),
            token
        );
        // Redirect URL itself wrapped mid-token.
        assert_eq!(
            extract_token(&format!(
                "https://anilist.co/api/v2/oauth/paste#access_token={}\n{}&token_type=Bearer",
                &token[..30],
                &token[30..]
            )),
            token
        );
        assert_eq!(extract_token("   \n  "), "");
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
            anilist: None,
            lookup: None,
            track: vec![],
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
