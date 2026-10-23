//! FFI surface for the Android app, built with `--features uniffi`.
//!
//! The facade is deliberately small: search/browse, episode lists and a
//! resolved stream URL that the native player (Media3/ExoPlayer) opens.
//! Methods are blocking — call them off the Android main thread; each one
//! drives the internal tokio runtime and hits every provider in parallel.

use crate::{
    model::{Anime, Episode, ProviderId},
    provider::{Provider, Registry, bounded},
    store::Config,
};
use anyhow::Result;

/// Every failure crosses the FFI as a plain message: the Kotlin side shows
/// it, nothing here is recoverable programmatically.
#[derive(uniffi::Error, Debug)]
#[uniffi(flat_error)]
pub enum ApiError {
    Failure { message: String },
}
impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        Self::Failure {
            message: format!("{error:#}"),
        }
    }
}
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failure { message } => write!(f, "{message}"),
        }
    }
}
pub type ApiResult<T> = std::result::Result<T, ApiError>;

/// Blocking entry point over the provider registry.
#[derive(uniffi::Object)]
pub struct AnimeLib {
    runtime: tokio::runtime::Runtime,
    registry: Registry,
}

#[uniffi::export]
impl AnimeLib {
    #[uniffi::constructor]
    pub fn new() -> ApiResult<Self> {
        let registry = Registry::new(&Config::default(), None)?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow::anyhow!("tokio runtime: {e}"))?;
        Ok(Self { runtime, registry })
    }

    /// Search every provider in parallel; empty query lists the latest.
    pub fn search(&self, query: String) -> ApiResult<Vec<Anime>> {
        let query = query.trim().to_owned();
        Ok(self.runtime.block_on(self.gather(move |provider| {
            let query = query.clone();
            async move {
                if query.is_empty() {
                    provider.latest().await
                } else {
                    provider.search(&query).await
                }
            }
        }))?)
    }

    /// Episodes for a title (its provider is part of the record).
    pub fn episodes(&self, anime: &Anime) -> ApiResult<Vec<Episode>> {
        self.runtime
            .block_on(async {
                let provider = self.registry.ready(anime.provider).await;
                let (_, episodes) = bounded(provider.episodes(anime.clone())).await?;
                anyhow::Ok(episodes)
            })
            .map_err(ApiError::from)
    }

    /// Resolve an episode to a playable URL (HLS or direct file).
    pub fn stream_url(&self, anime: &Anime, episode: &Episode) -> ApiResult<String> {
        self.runtime
            .block_on(async {
                let provider = self.registry.ready(anime.provider).await;
                let stream = bounded(provider.resolve(anime, episode)).await?;
                anyhow::Ok(stream.url.to_string())
            })
            .map_err(ApiError::from)
    }
}

impl AnimeLib {
    /// Fan a request out to all providers and merge whatever answered.
    async fn gather<F, Fut>(&self, request: F) -> Result<Vec<Anime>>
    where
        F: Fn(std::sync::Arc<Provider>) -> Fut,
        Fut: std::future::Future<Output = Result<Vec<Anime>>>,
    {
        let providers =
            futures_util::future::join_all(ProviderId::ALL.map(|id| self.registry.ready(id))).await;
        let futures: Vec<_> = providers.into_iter().map(request).collect();
        let mut merged = Vec::new();
        let mut failures = Vec::new();
        for result in futures_util::future::join_all(futures).await {
            match result {
                Ok(mut animes) => merged.append(&mut animes),
                Err(error) => failures.push(format!("{error:#}")),
            }
        }
        if merged.is_empty() && !failures.is_empty() {
            anyhow::bail!("every provider failed: {}", failures.join("; "));
        }
        Ok(merged)
    }
}
