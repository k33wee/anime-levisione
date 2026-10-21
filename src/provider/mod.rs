pub mod http;
pub mod parse;

use crate::{
    model::{Anime, Episode, ProviderId, sort_episodes},
    store::Config,
};
use anyhow::{Context, Result, bail};
use http::{Http, web_url};
use serde_json::Value;
use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio::sync::Mutex;
use url::Url;

static COOKIE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"document\.cookie\s*=\s*["']([^=;"']+)=([^;"']+)"#).expect("constant regex")
});

#[derive(Debug, Clone)]
pub struct Stream {
    pub url: Url,
    pub referer: String,
}
pub struct Provider {
    pub id: ProviderId,
    pub base: Url,
    http: Http,
    token: Mutex<Option<String>>,
}
impl Provider {
    fn new(id: ProviderId, base: &str) -> Result<Self> {
        let mut base = web_url(base)?;
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(Self {
            id,
            base,
            http: Http::new()?,
            token: Mutex::new(None),
        })
    }
    fn endpoint(&self, path: &str) -> Result<Url> {
        Ok(self.base.join(path)?)
    }
    async fn page(&self, url: Url) -> Result<String> {
        let mut html = self.http.text(self.http.client.get(url.clone())).await?;
        if self.id == ProviderId::AnimeWorld
            && let Some(c) = COOKIE_RE.captures(&html)
        {
            self.http
                .jar
                .add_cookie_str(&format!("{}={}; Path=/", &c[1], &c[2]), &self.base);
            html = self.http.text(self.http.client.get(url)).await?;
        }
        Ok(html)
    }
    async fn session(&self, refresh: bool) -> Result<String> {
        let mut token = self.token.lock().await;
        if refresh {
            *token = None;
        }
        if token.is_none() {
            let html = self.page(self.base.clone()).await?;
            *token = Some(parse::csrf(&html)?);
        }
        token.clone().context("Provider session not initialized")
    }
    pub async fn search(&self, query: &str) -> Result<Vec<Anime>> {
        let mut token = self.session(false).await?;
        for attempt in 0..2 {
            let req = match self.id {
                ProviderId::AnimeWorld => self
                    .http
                    .client
                    .post(self.endpoint("api/search/v2")?)
                    .query(&[("keyword", query)])
                    .header("csrf-token", &token),
                ProviderId::AnimeUnity => self
                    .http
                    .client
                    .post(self.endpoint("livesearch")?)
                    .form(&[("title", query)])
                    .header("x-csrf-token", &token),
            }
            .header("referer", self.base.as_str())
            .header("x-requested-with", "XMLHttpRequest");
            let result = self.http.text(req).await;
            match result {
                Err(e) if attempt == 0 && e.to_string().contains("HTTP 419") => {
                    token = self.session(true).await?;
                }
                Err(e) => return Err(e),
                Ok(raw) => {
                    return match self.id {
                        ProviderId::AnimeWorld => parse::world_search(&raw, &self.base),
                        ProviderId::AnimeUnity => parse::unity_search(&raw, &self.base),
                    };
                }
            }
        }
        bail!("Provider session expired after refresh")
    }
    pub async fn latest(&self) -> Result<Vec<Anime>> {
        let raw = self.page(self.base.clone()).await?;
        match self.id {
            ProviderId::AnimeWorld => parse::world_latest(&raw, &self.base),
            ProviderId::AnimeUnity => parse::unity_latest(&raw, &self.base),
        }
    }
    pub async fn episodes(&self, mut anime: Anime) -> Result<(Anime, Vec<Episode>)> {
        match self.id {
            ProviderId::AnimeWorld => {
                let html = self
                    .page(self.endpoint(&format!("play/{}", anime.id))?)
                    .await?;
                let eps = parse::world_detail(&html, &mut anime)?;
                Ok((anime, eps))
            }
            ProviderId::AnimeUnity => {
                if !anime.id.chars().all(|c| c.is_ascii_digit()) {
                    bail!("Invalid AnimeUnity id");
                }
                let raw = self
                    .http
                    .text(
                        self.http
                            .client
                            .get(self.endpoint(&format!("info_api/{}/", anime.id))?),
                    )
                    .await?;
                let info: Value = serde_json::from_str(&raw)?;
                let total: u32 = parse::text(&info, "episodes_count")
                    .parse()
                    .unwrap_or(anime.episodes);
                anime.episodes = total.max(anime.episodes);
                anime.genres = info["genres"]
                    .as_array()
                    .map(|g| {
                        g.iter()
                            .filter_map(Value::as_str)
                            .map(crate::model::clean)
                            .collect()
                    })
                    .unwrap_or(anime.genres);
                let mut eps = Vec::new();
                // Inclusive ranges match info_api; an unknown count still fetches the first page.
                let mut start = 0u32;
                loop {
                    if start > 100_000 {
                        bail!("Provider returned an unreasonable episode count");
                    }
                    let end = start + 119;
                    let raw = self
                        .http
                        .text(
                            self.http
                                .client
                                .get(self.endpoint(&format!("info_api/{}/1", anime.id))?)
                                .query(&[("start_range", start), ("end_range", end)]),
                        )
                        .await?;
                    let batch = parse::unity_episodes(&raw)?;
                    let count = batch.len();
                    eps.extend(batch);
                    if end >= anime.episodes && count < 120 {
                        break;
                    }
                    start = end + 1;
                }
                sort_episodes(&mut eps);
                Ok((anime, eps))
            }
        }
    }
    pub async fn resolve(&self, episode: &Episode) -> Result<Stream> {
        match self.id {
            ProviderId::AnimeWorld => {
                let token = self.session(false).await?;
                let raw = self
                    .http
                    .text(
                        self.http
                            .client
                            .get(self.endpoint("api/episode/info")?)
                            .query(&[("id", &episode.id), ("alt", &String::from("0"))])
                            .header("csrf-token", &token)
                            .header("x-requested-with", "XMLHttpRequest")
                            .header("referer", self.base.as_str()),
                    )
                    .await?;
                Ok(Stream {
                    url: parse::world_source(&raw)?,
                    referer: self.base.to_string(),
                })
            }
            ProviderId::AnimeUnity => {
                if !episode.id.chars().all(|c| c.is_ascii_digit()) {
                    bail!("Invalid AnimeUnity episode id");
                }
                let raw = self
                    .http
                    .text(
                        self.http
                            .client
                            .get(self.endpoint(&format!("embed-url/{}", episode.id))?),
                    )
                    .await?;
                let embed = web_url(raw.trim().trim_matches('"'))?;
                let html = self
                    .http
                    .text(
                        self.http
                            .client
                            .get(embed.clone())
                            .header("referer", self.base.as_str()),
                    )
                    .await?;
                Ok(Stream {
                    url: parse::unity_source(&html)?,
                    referer: embed.to_string(),
                })
            }
        }
    }
}
#[derive(Clone)]
pub struct Registry {
    providers: [Arc<Provider>; 2],
}
impl Registry {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            providers: [
                Arc::new(Provider::new(
                    ProviderId::AnimeWorld,
                    &config.animeworld_url,
                )?),
                Arc::new(Provider::new(
                    ProviderId::AnimeUnity,
                    &config.animeunity_url,
                )?),
            ],
        })
    }
    pub fn get(&self, id: ProviderId) -> Arc<Provider> {
        self.providers[match id {
            ProviderId::AnimeWorld => 0,
            ProviderId::AnimeUnity => 1,
        }]
        .clone()
    }
}
pub async fn bounded<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .context("Provider timed out after 30 seconds; press r to retry or use the other source")?
}
