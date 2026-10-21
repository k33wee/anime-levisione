use anime_levisione::{app, demo, events, input, model, playback, provider, store, ui};
use anyhow::{Context, Result, bail};
use app::{Action, App};
use clap::{Parser, Subcommand};
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyEventKind},
    execute,
};
use events::Message;
use futures_util::StreamExt;
use model::{ProviderId, rank, sort_by_rank};
use provider::{Registry, bounded};
use std::{
    collections::VecDeque,
    io::{self, IsTerminal},
    path::PathBuf,
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
}
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.demo && cli.command.is_some() {
        bail!("--demo is a TUI preview; do not combine it with subcommands");
    }
    let store = if cli.demo {
        None
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
        Some(Registry::new(&config)?)
    };
    if let Some(command) = cli.command {
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
    if cli.demo {
        demo::populate(&mut app);
    }
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut effects = Effects {
        store,
        registry,
        tx,
        search: vec![],
        detail: None,
        player: None,
        download: None,
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
    execute!(io::stdout(), EnableBracketedPaste)?;
    let mut input = EventStream::new();
    let mut interval = tokio::time::interval(Duration::from_millis(80));
    let result = async {
        loop {
            terminal.draw(|frame| ui::draw(frame, &mut app))?;
            let actions = tokio::select! {
                _ = interval.tick() => app.on_tick(),
                message = rx.recv() => match message { Some(message) => app.message(message), None => break },
                event = input.next() => match event {
                    Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => app.key(key),
                    Some(Ok(Event::Paste(text))) => { app.paste(&text); vec![] },
                    Some(Ok(Event::Resize(_, _))) => vec![],
                    Some(Err(e)) => return Err(anyhow::Error::from(e)),
                    None => break,
                    _ => vec![],
                },
                _ = tokio::signal::ctrl_c() => app.key(crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Char('c'), crossterm::event::KeyModifiers::CONTROL)),
            };
            if effects.apply(actions, &mut app) {
                break;
            }
            effects.flush_progress(&mut app);
        }
        Ok::<_, anyhow::Error>(())
    }.await;
    effects.abort_all();
    // Persist progress updates that were still inside the debounce window.
    effects.flush_pending(&mut app);
    result
}
struct TerminalRestore;
impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        ratatui::restore();
    }
}
struct Effects {
    store: Option<Store>,
    registry: Option<Registry>,
    tx: mpsc::UnboundedSender<Message>,
    search: Vec<JoinHandle<()>>,
    detail: Option<JoinHandle<()>>,
    player: Option<JoinHandle<()>>,
    download: Option<JoinHandle<()>>,
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
                        match playback::scan_offline(&app.config.download_dir) {
                            Ok(offline) => {
                                app.offline = offline;
                                if app.view == app::View::Offline {
                                    app.details.clear();
                                    app.rebuild();
                                }
                            }
                            Err(e) => {
                                app.notify(format!("Cannot read offline library: {e:#}"), true)
                            }
                        }
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
                            let provider = registry.get(id);
                            let query = query.clone();
                            self.search.push(tokio::spawn(async move {
                                let result = bounded(async {
                                    match query {
                                        Some(q) => provider.search(&q).await,
                                        None => provider.latest().await,
                                    }
                                })
                                .await
                                .map_err(|e| format!("{e:#}"));
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
                        let provider = registry.get(anime.provider);
                        let tx = self.tx.clone();
                        let key = anime.key();
                        self.detail = Some(tokio::spawn(async move {
                            let result = bounded(provider.episodes(anime))
                                .await
                                .map_err(|e| format!("{e:#}"));
                            let _ = tx.send(Message::Episodes { key, result });
                        }));
                    } else {
                        app.detail_loading = false;
                    }
                }
                Action::Play {
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
                                None => playback::Media::Remote(
                                    bounded(registry.get(anime.provider).resolve(&episode)).await?,
                                ),
                            };
                            playback::play(config, media, anime, episode, resume, tx.clone()).await
                        }
                        .await
                        .map_err(|e| format!("{e:#}"));
                        let _ = tx.send(Message::PlaybackEnded(result));
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
                            let stream =
                                bounded(registry.get(anime.provider).resolve(&episode)).await?;
                            let _ = tx.send(Message::DownloadStarted);
                            playback::download(config, stream, anime, episode).await
                        }
                        .await
                        .map_err(|e| format!("{e:#}"));
                        let _ = tx.send(Message::DownloadEnded(result));
                    }));
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
    fn abort_all(&mut self) {
        for handle in self.search.drain(..) {
            handle.abort();
        }
        abort(&mut self.detail);
        abort(&mut self.player);
        abort(&mut self.download);
    }
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
            let provider = registry.get(id);
            let query = query.clone();
            async move { (id, bounded(provider.search(&query)).await) }
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
        let provider = registry.get(id);
        async move { (id, bounded(provider.latest()).await) }
    });
    let mut healthy = 0;
    for (id, result) in futures_util::future::join_all(checks).await {
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
    Ok(())
}
