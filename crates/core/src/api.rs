//! Blocking mobile facade. Call on an I/O dispatcher, never the UI thread.
//! The same providers, atomic library store and personal Telegram sync as the TUI.
use crate::{
    model::{self, Anime, Episode, ProviderId},
    provider::{Registry, bounded, http::web_url},
    store::{Config, Library, Progress, Store},
    sync::{self, Account},
};
use anyhow::bail;
use std::{
    path::PathBuf,
    sync::{Mutex, RwLock},
};

#[derive(uniffi::Error, Debug)]
#[uniffi(flat_error)]
pub enum ApiError {
    Failure { message: String },
}
impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        crate::diagnostics::record(crate::diagnostics::DiagnosticEvent::CoreFailure {
            kind: crate::diagnostics::failure(&error),
        });
        Self::Failure {
            message: format!("{error:#}"),
        }
    }
}
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failure { message } => f.write_str(message),
        }
    }
}
impl std::error::Error for ApiError {}
pub type ApiResult<T> = std::result::Result<T, ApiError>;

#[derive(uniffi::Record)]
pub struct StreamInfo {
    pub url: String,
    pub referer: String,
}
#[derive(uniffi::Record)]
pub struct TitleDetails {
    pub anime: Anime,
    pub episodes: Vec<Episode>,
}
#[derive(uniffi::Record)]
pub struct ResumePoint {
    pub anime: Anime,
    pub episode: Episode,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    pub completed: bool,
}
#[derive(uniffi::Record)]
pub struct AppSettings {
    pub private_mode: bool,
    pub animeworld_url: String,
    pub animeunity_url: String,
    pub streamingcommunity_url: String,
}
impl From<&Config> for AppSettings {
    fn from(c: &Config) -> Self {
        Self {
            private_mode: c.private,
            animeworld_url: c.animeworld_url.clone(),
            animeunity_url: c.animeunity_url.clone(),
            streamingcommunity_url: c.streamingcommunity_url.clone(),
        }
    }
}

#[derive(uniffi::Object)]
pub struct AnimeLib {
    runtime: tokio::runtime::Runtime,
    registry: RwLock<Registry>,
    library: Mutex<Library>,
    settings: Mutex<Config>,
    store: Option<Store>,
    account: Mutex<()>,
}
#[uniffi::export]
impl AnimeLib {
    #[uniffi::constructor]
    pub fn new() -> ApiResult<Self> {
        Self::build(None)
    }
    #[uniffi::constructor]
    pub fn with_library(dir: String) -> ApiResult<Self> {
        Self::build(Some(PathBuf::from(dir)))
    }

    /// Empty query lists latest releases; failures in one provider don't hide the others.
    pub fn search(&self, query: String) -> ApiResult<Vec<Anime>> {
        let query = query.trim().to_owned();
        let registry = self.registry();
        let mut animes = self.runtime.block_on(async {
            let requests = ProviderId::ALL.map(|id| {
                let registry = registry.clone();
                let query = query.clone();
                async move {
                    let provider = registry.ready(id).await;
                    bounded(async {
                        if query.is_empty() {
                            provider.latest().await
                        } else {
                            provider.search(&query).await
                        }
                    })
                    .await
                }
            });
            let mut merged = Vec::new();
            let mut failures = Vec::new();
            for response in futures_util::future::join_all(requests).await {
                match response {
                    Ok(mut rows) => merged.append(&mut rows),
                    Err(e) => failures.push(format!("{e:#}")),
                }
            }
            if merged.is_empty() && !failures.is_empty() {
                bail!("Providers unavailable: {}", failures.join("; "));
            }
            Ok(merged)
        })?;
        if !query.is_empty() {
            model::sort_by_rank(&mut animes, &query);
            if !self.is_private() {
                self.mutate(|lib| lib.remember_search(&query))?;
            }
        }
        Ok(animes)
    }
    pub fn recent_searches(&self) -> ApiResult<Vec<String>> {
        self.view(|l| l.recent_searches.clone())
    }
    pub fn details(&self, anime: &Anime) -> ApiResult<TitleDetails> {
        let registry = self.registry();
        Ok(self.runtime.block_on(async {
            let provider = registry.ready(anime.provider).await;
            let (anime, episodes) = bounded(provider.episodes(anime.clone())).await?;
            anyhow::Ok(TitleDetails { anime, episodes })
        })?)
    }
    pub fn episodes(&self, anime: &Anime) -> ApiResult<Vec<Episode>> {
        Ok(self.details(anime)?.episodes)
    }
    pub fn stream(&self, anime: &Anime, episode: &Episode) -> ApiResult<StreamInfo> {
        let registry = self.registry();
        Ok(self.runtime.block_on(async {
            let provider = registry.ready(anime.provider).await;
            let stream = bounded(provider.resolve(anime, episode)).await?;
            anyhow::Ok(StreamInfo {
                url: stream.url.to_string(),
                referer: stream.referer,
            })
        })?)
    }
    /// Compatibility with existing consumers; new players should use stream() for headers.
    pub fn stream_url(&self, anime: &Anime, episode: &Episode) -> ApiResult<String> {
        Ok(self.stream(anime, episode)?.url)
    }
    pub fn watchlist(&self) -> ApiResult<Vec<Anime>> {
        self.view(|l| l.watchlist.clone())
    }
    pub fn history(&self) -> ApiResult<Vec<Anime>> {
        self.view(|l| l.history.iter().map(|h| h.anime.clone()).collect())
    }
    pub fn toggle_watchlist(&self, anime: &Anime) -> ApiResult<bool> {
        self.mutate(|l| {
            l.toggle_bookmark(anime.clone());
            l.bookmarked(anime)
        })
    }
    pub fn continue_watching(&self) -> ApiResult<Vec<ResumePoint>> {
        self.view(|l| {
            l.history
                .iter()
                .filter_map(|h| {
                    let p = h.progress.get(&h.last_episode)?;
                    (!p.completed && p.seconds > 0.0).then(|| ResumePoint {
                        anime: h.anime.clone(),
                        episode: p.episode.clone(),
                        position_seconds: p.seconds,
                        duration_seconds: p.duration,
                        completed: p.completed,
                    })
                })
                .collect()
        })
    }
    pub fn progress(&self, anime: &Anime, episode: &Episode) -> ApiResult<Option<ResumePoint>> {
        self.view(|l| {
            l.progress(anime, episode).map(|p| ResumePoint {
                anime: anime.clone(),
                episode: episode.clone(),
                position_seconds: p.seconds,
                duration_seconds: p.duration,
                completed: p.completed,
            })
        })
    }
    pub fn record_progress(
        &self,
        anime: &Anime,
        episode: &Episode,
        position_seconds: f64,
        duration_seconds: f64,
        completed: bool,
    ) -> ApiResult<()> {
        if self.is_private() {
            return Ok(());
        }
        self.mutate(|l| {
            // A player stopping at zero must not undo an explicit watched marker.
            let completed = completed || l.progress(anime, episode).is_some_and(|p| p.completed);
            l.record(
                anime.clone(),
                Progress {
                    episode: episode.clone(),
                    seconds: position_seconds,
                    duration: duration_seconds,
                    completed,
                },
            );
        })
    }
    pub fn mark_watched(&self, anime: &Anime, episode: &Episode, watched: bool) -> ApiResult<()> {
        // Explicit library edits remain possible in private mode, like the TUI.
        self.mutate(|l| {
            l.record(
                anime.clone(),
                Progress {
                    episode: episode.clone(),
                    seconds: 0.0,
                    duration: 0.0,
                    completed: watched,
                },
            )
        })
    }
    pub fn watched_episodes(&self, anime: &Anime) -> ApiResult<Vec<String>> {
        self.view(|l| {
            l.history
                .iter()
                .find(|h| h.anime.key() == anime.key())
                .map(|h| {
                    h.progress
                        .values()
                        .filter(|p| p.completed)
                        .map(|p| p.episode.number.clone())
                        .collect()
                })
                .unwrap_or_default()
        })
    }
    pub fn settings(&self) -> ApiResult<AppSettings> {
        Ok(AppSettings::from(
            &*self
                .settings
                .lock()
                .map_err(|_| anyhow::anyhow!("settings lock"))?,
        ))
    }
    pub fn save_settings(&self, settings: AppSettings) -> ApiResult<()> {
        let _account = self
            .account
            .lock()
            .map_err(|_| anyhow::anyhow!("account lock"))?;
        for raw in [
            &settings.animeworld_url,
            &settings.animeunity_url,
            &settings.streamingcommunity_url,
        ] {
            if web_url(raw)?.scheme() != "https" {
                return Err(anyhow::anyhow!("Provider mirrors must use HTTPS").into());
            }
        }
        let mut config = self
            .settings
            .lock()
            .map_err(|_| anyhow::anyhow!("settings lock"))?;
        let mut next = config.clone();
        next.private = settings.private_mode;
        next.animeworld_url = settings.animeworld_url;
        next.animeunity_url = settings.animeunity_url;
        next.streamingcommunity_url = settings.streamingcommunity_url;
        let registry = Registry::new(&next, self.store.as_ref().map(|s| s.dir.as_path()))?;
        if let Some(store) = &self.store {
            store.save_config(&next)?;
        }
        *self
            .registry
            .write()
            .map_err(|_| anyhow::anyhow!("provider registry lock"))? = registry;
        *config = next;
        Ok(())
    }
    pub fn diagnostics(&self) -> ApiResult<Vec<String>> {
        let registry = self.registry();
        Ok(self
            .runtime
            .block_on(futures_util::future::join_all(ProviderId::ALL.map(|id| {
                let registry = registry.clone();
                async move {
                    registry.ready(id).await;
                    let r = registry.resolution(id);
                    format!("{id}: {}", r.describe())
                }
            }))))
    }
    pub fn telegram_status(&self) -> ApiResult<String> {
        Ok(match &self.store {
            Some(store) => Account::load(&store.dir)?
                .map(|a| a.label())
                .unwrap_or_default(),
            None => String::new(),
        })
    }
    /// First device: bot token after /start. Other devices: the secret alib1 connection key.
    pub fn telegram_connect(&self, credential: String, chat_id: String) -> ApiResult<String> {
        let _guard = self
            .account
            .lock()
            .map_err(|_| anyhow::anyhow!("account lock"))?;
        let store = self.sync_store()?;
        if Account::load(&store.dir)?.is_some() {
            return Err(anyhow::anyhow!("Disconnect the current bot first").into());
        }
        let account = self
            .runtime
            .block_on(sync::Client::connect(&credential, &chat_id))?;
        // Save the encryption key before any remote upload. A network failure cannot lose it.
        account.save(&store.dir)?;
        Ok(account.connection_key()?)
    }
    pub fn telegram_connection_key(&self) -> ApiResult<String> {
        let _guard = self
            .account
            .lock()
            .map_err(|_| anyhow::anyhow!("account lock"))?;
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No persistent library"))?;
        Ok(Account::load(&store.dir)?
            .ok_or_else(|| anyhow::anyhow!("Connect a bot first"))?
            .connection_key()?)
    }
    pub fn telegram_disconnect(&self) -> ApiResult<()> {
        let _guard = self
            .account
            .lock()
            .map_err(|_| anyhow::anyhow!("account lock"))?;
        if let Some(store) = &self.store {
            Account::remove(&store.dir)?;
        }
        Ok(())
    }
    pub fn telegram_sync(&self) -> ApiResult<()> {
        let _guard = self
            .account
            .lock()
            .map_err(|_| anyhow::anyhow!("account lock"))?;
        let store = self.sync_store()?;
        let mut account = Account::load(&store.dir)?
            .ok_or_else(|| anyhow::anyhow!("Connect your Telegram bot first"))?;
        let captured = self.view(Clone::clone)?;
        let outcome = self
            .runtime
            .block_on(sync::Client::new(account.clone())?.sync(&captured))?;
        // Merge with edits made during the network request; save checkpoint and data together.
        let mut library = self
            .library
            .lock()
            .map_err(|_| anyhow::anyhow!("library lock"))?;
        let mut next = library.clone();
        outcome.apply(&mut next)?;
        store.save(&next)?;
        *library = next;
        account.mark_initialized();
        account.save(&store.dir)?;
        Ok(())
    }
}
impl AnimeLib {
    fn build(dir: Option<PathBuf>) -> ApiResult<Self> {
        let (store, config, library) = if let Some(dir) = dir {
            let store = Store::open(Some(dir))?;
            let config = store.config.clone();
            let library = store.library.clone();
            (Some(store), config, library)
        } else {
            (None, Config::default(), Library::default())
        };
        let registry = Registry::new(&config, store.as_ref().map(|s| s.dir.as_path()))?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(anyhow::Error::from)?;
        Ok(Self {
            runtime,
            registry: RwLock::new(registry),
            library: Mutex::new(library),
            settings: Mutex::new(config),
            store,
            account: Mutex::new(()),
        })
    }
    fn registry(&self) -> Registry {
        self.registry
            .read()
            .expect("provider registry lock")
            .clone()
    }
    fn is_private(&self) -> bool {
        self.settings.lock().map(|s| s.private).unwrap_or(true)
    }
    fn view<T>(&self, query: impl FnOnce(&Library) -> T) -> ApiResult<T> {
        let library = self
            .library
            .lock()
            .map_err(|_| anyhow::anyhow!("library lock"))?;
        Ok(query(&library))
    }
    fn mutate<T>(&self, change: impl FnOnce(&mut Library) -> T) -> ApiResult<T> {
        let mut library = self
            .library
            .lock()
            .map_err(|_| anyhow::anyhow!("library lock"))?;
        let mut next = library.clone();
        let result = change(&mut next);
        if let Some(store) = &self.store {
            store.save(&next)?;
        }
        *library = next;
        Ok(result)
    }
    fn sync_store(&self) -> ApiResult<&Store> {
        if self.is_private() {
            return Err(anyhow::anyhow!("Sync is disabled in private mode").into());
        }
        self.store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("A persistent library is required for sync").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    fn anime() -> Anime {
        Anime {
            provider: ProviderId::AnimeWorld,
            id: "1".into(),
            title: "Frieren".into(),
            aliases: vec![],
            synopsis: String::new(),
            year: String::new(),
            kind: "TV".into(),
            status: String::new(),
            score: String::new(),
            episodes: 28,
            dub: false,
            genres: vec![],
            page_url: String::new(),
            cover_url: String::new(),
        }
    }
    fn episode(n: &str) -> Episode {
        Episode {
            id: format!("ep-{n}"),
            number: n.into(),
            title: String::new(),
        }
    }
    #[test]
    fn library_survives_reopen_and_uses_tui_lock() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().to_string_lossy().to_string();
        let lib = AnimeLib::with_library(path.clone())?;
        assert!(AnimeLib::with_library(path.clone()).is_err());
        assert!(lib.toggle_watchlist(&anime())?);
        assert!(!lib.toggle_watchlist(&anime())?);
        assert!(lib.toggle_watchlist(&anime())?);
        lib.record_progress(&anime(), &episode("1"), 120.0, 1400.0, false)?;
        drop(lib);
        let lib = AnimeLib::with_library(path)?;
        assert_eq!(lib.watchlist()?, vec![anime()]);
        assert_eq!(lib.continue_watching()?[0].position_seconds, 120.0);
        assert_eq!(
            lib.progress(&anime(), &episode("1"))?
                .expect("progress")
                .position_seconds,
            120.0
        );
        Ok(())
    }
    #[test]
    fn completed_and_private_progress_are_not_resume_points() -> Result<()> {
        let lib = AnimeLib::new()?;
        lib.record_progress(&anime(), &episode("1"), 120.0, 1400.0, false)?;
        assert_eq!(lib.continue_watching()?.len(), 1);
        lib.record_progress(&anime(), &episode("1"), 1400.0, 1400.0, true)?;
        lib.record_progress(&anime(), &episode("1"), 0.0, 0.0, false)?;
        assert!(lib.continue_watching()?.is_empty());
        assert_eq!(lib.watched_episodes(&anime())?, vec!["1"]);
        let mut settings = lib.settings()?;
        settings.private_mode = true;
        lib.save_settings(settings)?;
        lib.record_progress(&anime(), &episode("2"), 30.0, 100.0, false)?;
        assert!(lib.progress(&anime(), &episode("2"))?.is_none());
        lib.mark_watched(&anime(), &episode("1"), false)?;
        assert!(lib.watched_episodes(&anime())?.is_empty());
        assert!(lib.telegram_sync().is_err());
        Ok(())
    }
    #[test]
    fn failed_save_does_not_change_in_memory_library() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let lib = AnimeLib::with_library(dir.path().to_string_lossy().into_owned())?;
        std::fs::create_dir(dir.path().join("library.json"))?;
        assert!(lib.toggle_watchlist(&anime()).is_err());
        assert!(lib.watchlist()?.is_empty());
        Ok(())
    }
    #[test]
    fn old_libraries_without_artwork_still_deserialize() -> Result<()> {
        let mut value = serde_json::to_value(anime())?;
        value.as_object_mut().expect("record").remove("cover_url");
        let saved: Anime = serde_json::from_value(value)?;
        assert!(saved.cover_url.is_empty());
        Ok(())
    }
    #[test]
    fn telegram_requires_persistence_and_is_disabled_in_private_mode() -> Result<()> {
        let lib = AnimeLib::new()?;
        assert!(lib.telegram_status()?.is_empty());
        assert!(lib.telegram_sync().is_err());
        assert!(
            lib.telegram_connect("not-a-token".into(), String::new())
                .is_err()
        );
        let mut settings = lib.settings()?;
        settings.private_mode = true;
        lib.save_settings(settings)?;
        let error = lib
            .telegram_connect("not-a-token".into(), String::new())
            .expect_err("private mode");
        assert!(error.to_string().contains("private mode"));
        lib.telegram_disconnect()?;
        Ok(())
    }
    #[test]
    fn invalid_mirror_does_not_replace_persisted_settings() -> Result<()> {
        let lib = AnimeLib::new()?;
        let mut settings = lib.settings()?;
        settings.animeworld_url = "file:///etc/passwd".into();
        assert!(lib.save_settings(settings).is_err());
        assert!(lib.settings()?.animeworld_url.starts_with("https://"));
        Ok(())
    }
}
