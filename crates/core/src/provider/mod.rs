pub mod domains;
pub mod http;
pub mod parse;

use crate::{
    model::{Anime, Episode, ProviderId, sort_episodes},
    store::Config,
};
use anyhow::{Context, Result, bail};
use domains::{Candidate, DomainCache, Resolution, root_of};
use http::{Http, web_url};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, RwLock},
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use url::Url;

static COOKIE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"document\.cookie\s*=\s*["']([^=;"']+)=([^;"']+)"#).expect("constant regex")
});

/// The cookie AnimeWorld's own page script asks clients to echo back.
fn world_cookie(html: &str) -> Option<String> {
    COOKIE_RE
        .captures(html)
        .map(|c| format!("{}={}; Path=/", &c[1], &c[2]))
}

/// StreamingCommunity's iframe path always identifies the title. Series use
/// the episode's database id as a query parameter; putting it in the path can
/// silently select an unrelated movie whose title happens to have that id.
fn community_player_urls(base: &Url, anime: &Anime, episode: &Episode) -> Result<(Url, Url)> {
    let (title_id, slug) = anime
        .id
        .split_once('-')
        .context("Invalid StreamingCommunity title id")?;
    let numeric = |value: &str| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit());
    if !numeric(title_id) || slug.is_empty() {
        bail!("Invalid StreamingCommunity title id");
    }
    if !numeric(&episode.id) {
        bail!("Invalid StreamingCommunity episode id");
    }
    let series = if anime.kind.trim().eq_ignore_ascii_case("tv") {
        true
    } else if anime.kind.trim().eq_ignore_ascii_case("movie") {
        false
    } else {
        bail!("Invalid StreamingCommunity title type");
    };
    if !series && episode.id != title_id {
        bail!("StreamingCommunity movie id does not match its title");
    }
    let mut watch = base.join(&format!("it/watch/{title_id}"))?;
    let mut iframe = base.join(&format!("it/iframe/{title_id}"))?;
    if series {
        watch
            .query_pairs_mut()
            .append_pair("episode_id", &episode.id);
        iframe
            .query_pairs_mut()
            .append_pair("episode_id", &episode.id)
            .append_pair("next_episode", "1");
    }
    Ok((watch, iframe))
}

#[derive(Debug, Clone)]
pub struct Stream {
    pub url: Url,
    pub referer: String,
}
/// What a playback request points at: a resolved remote stream or a file
/// already on disk. Shared by the TUI player code and the audio prober.
#[derive(Clone)]
pub enum Media {
    Remote(Stream),
    Local(std::path::PathBuf),
}
impl Media {
    pub fn source(&self) -> String {
        match self {
            Self::Remote(s) => s.url.to_string(),
            Self::Local(p) => p.to_string_lossy().into_owned(),
        }
    }
}
pub struct Provider {
    pub id: ProviderId,
    pub base: Url,
    http: Http,
    token: Mutex<Option<String>>,
}
impl Provider {
    fn new(id: ProviderId, base: &str) -> Result<Self> {
        Self::build(id, web_url(base)?, Http::new()?)
    }
    /// Candidate-domain probe constructor: a provider on a fail-fast client,
    /// because probes run in parallel at startup and must not stall it.
    fn for_probe(id: ProviderId, base: &Url) -> Result<Self> {
        Self::build(
            id,
            base.clone(),
            Http::with_timeouts(Duration::from_secs(8), Duration::from_secs(4))?,
        )
    }
    fn build(id: ProviderId, mut base: Url, http: Http) -> Result<Self> {
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(Self {
            id,
            base,
            http,
            token: Mutex::new(None),
        })
    }
    /// Promote a probe provider to normal traffic timeouts, keeping the
    /// session (cookies and CSRF token) it already bootstrapped.
    fn promote(self) -> Result<Self> {
        Ok(Self {
            http: Http::sharing(self.http.jar.clone())?,
            ..self
        })
    }
    fn endpoint(&self, path: &str) -> Result<Url> {
        Ok(self.base.join(path)?)
    }
    /// Fetch a page through the provider's session bootstrap, with the normal
    /// retry policy for transient failures.
    async fn page(&self, url: Url) -> Result<String> {
        let mut html = self.http.text(self.http.client.get(url.clone())).await?;
        if self.id == ProviderId::AnimeWorld
            && let Some(cookie) = world_cookie(&html)
        {
            self.http.jar.add_cookie_str(&cookie, &self.base);
            html = self.http.text(self.http.client.get(url)).await?;
        }
        Ok(html)
    }
    /// Single-shot page fetch that also reports the final URL after redirects,
    /// so domain probes notice provider rotations without retry stacking.
    async fn page_at(&self, url: Url) -> Result<(Url, String)> {
        let (final_url, html) = self.http.text_at(self.http.client.get(url)).await?;
        if self.id == ProviderId::AnimeWorld
            && let Some(cookie) = world_cookie(&html)
        {
            self.http.jar.add_cookie_str(&cookie, &final_url);
            return self.http.text_at(self.http.client.get(final_url)).await;
        }
        Ok((final_url, html))
    }
    /// Confirm a homepage actually belongs to this provider and bootstrap what
    /// its requests need: a CSRF token for AnimeWorld/AnimeUnity; for
    /// StreamingCommunity, a parseable Inertia page.
    fn validate_home(&self, html: &str) -> Result<Option<String>> {
        match self.id {
            ProviderId::AnimeWorld | ProviderId::AnimeUnity => parse::csrf(html).map(Some),
            ProviderId::StreamingCommunity => parse::community_home(html).map(|()| None),
        }
    }
    async fn session(&self, refresh: bool) -> Result<String> {
        if self.id == ProviderId::StreamingCommunity {
            bail!("StreamingCommunity needs no session token");
        }
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
        if self.id == ProviderId::StreamingCommunity {
            let raw = self
                .http
                .text(
                    self.http
                        .client
                        .get(self.endpoint("it/search")?)
                        .query(&[("q", query)])
                        .header("referer", self.base.as_str()),
                )
                .await?;
            return parse::community_search(&raw, &self.base);
        }
        let mut token = self.session(false).await?;
        for attempt in 0..2 {
            // StreamingCommunity returned earlier; only CSRF providers remain.
            let req = match self.id {
                ProviderId::AnimeWorld | ProviderId::StreamingCommunity => self
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
                        ProviderId::AnimeWorld | ProviderId::StreamingCommunity => {
                            parse::world_search(&raw, &self.base)
                        }
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
            ProviderId::StreamingCommunity => parse::community_latest(&raw, &self.base),
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
            ProviderId::StreamingCommunity => {
                if !anime.id.contains('-') {
                    bail!("Invalid StreamingCommunity id");
                }
                let html = self
                    .page(self.endpoint(&format!("it/titles/{}", anime.id))?)
                    .await?;
                let seasons = parse::community_detail(&html, &mut anime)?;
                let mut eps = Vec::new();
                if seasons.is_empty() {
                    // Movies play straight from the title page.
                    eps.push(Episode {
                        id: anime.id.split('-').next().unwrap_or_default().to_string(),
                        number: "1".into(),
                        title: String::new(),
                    });
                } else {
                    let multi = seasons.len() > 1;
                    let mut index = 0u32;
                    for season in seasons {
                        if season > 1000 {
                            bail!("Provider returned an unreasonable season number");
                        }
                        let raw = self
                            .page(
                                self.endpoint(&format!("it/titles/{}/season-{season}", anime.id))?,
                            )
                            .await?;
                        for episode in parse::community_season(&raw)? {
                            index += 1;
                            eps.push(Episode {
                                id: episode.id,
                                number: index.to_string(),
                                title: if multi {
                                    format!("S{season}E{} · {}", episode.number, episode.title)
                                        .trim_end_matches('·')
                                        .trim_end()
                                        .to_string()
                                } else {
                                    episode.title
                                },
                            });
                        }
                    }
                }
                anime.episodes = (eps.len() as u32).max(anime.episodes);
                sort_episodes(&mut eps);
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
    pub async fn resolve(&self, anime: &Anime, episode: &Episode) -> Result<Stream> {
        if anime.provider != self.id {
            bail!("Title belongs to a different provider");
        }
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
            ProviderId::StreamingCommunity => {
                let (watch, iframe) = community_player_urls(&self.base, anime, episode)?;
                let raw = self
                    .http
                    .text(
                        self.http
                            .client
                            .get(iframe)
                            .header("referer", watch.as_str()),
                    )
                    .await?;
                // The token embedded in this URL is single-use, so the embed
                // page is fetched exactly once, here.
                let embed = parse::community_embed(&raw)?;
                let html = self
                    .http
                    .text(
                        self.http
                            .client
                            .get(embed.clone())
                            .header("referer", watch.as_str()),
                    )
                    .await?;
                Ok(Stream {
                    url: parse::vixcloud_playlist(&html)?,
                    referer: embed.to_string(),
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
pub struct Registry(Arc<RegistryInner>);
struct RegistryInner {
    providers: RwLock<[Arc<Provider>; 3]>,
    resolutions: RwLock<[Resolution; 3]>,
    /// Candidate domains per provider, ordered by preference.
    candidates: [Vec<Candidate>; 3],
    cache: std::sync::Mutex<DomainCache>,
    cache_path: Option<PathBuf>,
    /// Per-provider discovery state; the lock also serializes probe rounds so
    /// concurrent failures cannot fan out into a probe storm.
    boot: [Mutex<Boot>; 3],
}
/// How far domain discovery has progressed for a provider in this run.
enum Boot {
    /// Not probed yet.
    Unknown,
    /// A live domain was confirmed.
    Live,
    /// The last probe round failed at this instant; wait before re-probing so
    /// a dead network does not trigger discovery on every action.
    Failed(Instant),
}
/// Cooldown before a failed discovery round is retried.
const RETRY_COOLDOWN: Duration = Duration::from_secs(30);
fn slot(id: ProviderId) -> usize {
    match id {
        ProviderId::AnimeWorld => 0,
        ProviderId::AnimeUnity => 1,
        ProviderId::StreamingCommunity => 2,
    }
}
impl Registry {
    /// Offline construction: providers start on their configured URLs and the
    /// resolution state is unknown until [`Registry::ready`] runs. Never touches
    /// the network, so the TUI can open instantly.
    pub fn new(config: &Config, cache_dir: Option<&Path>) -> Result<Self> {
        let cache_path = cache_dir.map(|dir| dir.join("domains.json"));
        // A corrupt or missing cache is not worth failing over; probe from scratch.
        let cache = cache_path
            .as_deref()
            .and_then(|path| DomainCache::load(path).ok())
            .unwrap_or_default();
        let candidates = [
            domains::candidates(config, &cache, ProviderId::AnimeWorld),
            domains::candidates(config, &cache, ProviderId::AnimeUnity),
            domains::candidates(config, &cache, ProviderId::StreamingCommunity),
        ];
        let providers = [
            Arc::new(Provider::new(
                ProviderId::AnimeWorld,
                &config.animeworld_url,
            )?),
            Arc::new(Provider::new(
                ProviderId::AnimeUnity,
                &config.animeunity_url,
            )?),
            Arc::new(Provider::new(
                ProviderId::StreamingCommunity,
                &config.streamingcommunity_url,
            )?),
        ];
        let resolutions = ProviderId::ALL.map(|id| {
            let index = slot(id);
            Resolution {
                id,
                configured: candidates[index]
                    .first()
                    .map(|c| c.url.clone())
                    .unwrap_or_else(|| root_of(&providers[index].base)),
                live: None,
                provider: None,
                error: None,
            }
        });
        Ok(Self(Arc::new(RegistryInner {
            providers: RwLock::new(providers),
            resolutions: RwLock::new(resolutions),
            candidates,
            cache: std::sync::Mutex::new(cache),
            cache_path,
            boot: [
                Mutex::new(Boot::Unknown),
                Mutex::new(Boot::Unknown),
                Mutex::new(Boot::Unknown),
            ],
        })))
    }
    pub fn get(&self, id: ProviderId) -> Arc<Provider> {
        self.0.providers.read().expect("provider registry")[slot(id)].clone()
    }
    /// Outcome of the latest domain probe round for a provider.
    pub fn resolution(&self, id: ProviderId) -> Resolution {
        self.0.resolutions.read().expect("resolution registry")[slot(id)].clone()
    }
    /// Locate a provider's live domain, once per run when it succeeds and with
    /// a cooldown when it fails; concurrent callers share one probe round.
    /// Always returns a provider — on failure it stays on the configured URL
    /// and the first request reports the error as usual.
    pub async fn ready(&self, id: ProviderId) -> Arc<Provider> {
        let index = slot(id);
        let mut boot = self.0.boot[index].lock().await;
        match *boot {
            Boot::Live => {}
            Boot::Failed(at) if at.elapsed() < RETRY_COOLDOWN => {}
            _ => {
                *boot = if self.probe_and_swap(id).await.is_ok() {
                    Boot::Live
                } else {
                    Boot::Failed(Instant::now())
                };
            }
        }
        drop(boot);
        self.get(id)
    }
    /// Probe all candidate domains again (current one first) and swap in
    /// whatever live domain answers. Bypasses the failure cooldown because it
    /// is only called after a real request failed.
    pub async fn revalidate(&self, id: ProviderId) -> std::result::Result<Arc<Provider>, String> {
        let mut boot = self.0.boot[slot(id)].lock().await;
        let result = self.probe_and_swap(id).await;
        *boot = if result.is_ok() {
            Boot::Live
        } else {
            Boot::Failed(Instant::now())
        };
        result
    }
    /// One probe round: current domain first, then every stored candidate.
    async fn probe_and_swap(&self, id: ProviderId) -> std::result::Result<Arc<Provider>, String> {
        let index = slot(id);
        let current = self.get(id);
        let mut cands = vec![Candidate {
            url: root_of(&current.base),
            trusted: true,
        }];
        for candidate in &self.0.candidates[index] {
            if candidate.url.as_str() != root_of(&current.base).as_str() {
                cands.push(candidate.clone());
            }
        }
        let resolution = domains::resolve(id, &cands).await;
        let Some(live) = resolution.live.clone() else {
            let error = resolution
                .error
                .clone()
                .unwrap_or_else(|| "no live domain found".into());
            self.0.resolutions.write().expect("resolution registry")[index] = resolution;
            return Err(error);
        };
        let provider = resolution
            .provider
            .clone()
            .expect("a live resolution always carries its provider");
        if let Some(path) = &self.0.cache_path
            && let Ok(mut cache) = self.0.cache.lock()
        {
            cache.record(id, &live);
            let _ = cache.save(path);
        }
        self.0.resolutions.write().expect("resolution registry")[index] = resolution;
        self.0.providers.write().expect("provider registry")[index] = provider.clone();
        Ok(provider)
    }
    /// Whether the last probe round found a live domain. Retrying with
    /// relocation is pointless while a provider looks entirely down.
    pub fn healable(&self, id: ProviderId) -> bool {
        self.0.resolutions.read().expect("resolution registry")[slot(id)]
            .live
            .is_some()
    }
}
pub async fn bounded<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .context("Provider timed out after 30 seconds; press r to retry or use the other source")?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn community_anime(id: &str, kind: &str) -> Anime {
        Anime {
            provider: ProviderId::StreamingCommunity,
            id: id.into(),
            title: "Test".into(),
            aliases: vec![],
            synopsis: String::new(),
            year: String::new(),
            kind: kind.into(),
            status: String::new(),
            score: String::new(),
            episodes: 0,
            dub: true,
            genres: vec![],
            page_url: String::new(),
        }
    }

    #[test]
    fn community_series_iframe_uses_title_path_and_episode_query() -> Result<()> {
        let base = Url::parse("https://streamingcommunity.example/")?;
        let anime = community_anime("6654-one-piece", "TV");
        let episode = Episode {
            id: "42659".into(),
            number: "1".into(),
            title: String::new(),
        };
        let (watch, iframe) = community_player_urls(&base, &anime, &episode)?;
        assert_eq!(watch.path(), "/it/watch/6654");
        assert_eq!(iframe.path(), "/it/iframe/6654");
        assert!(
            watch
                .query_pairs()
                .any(|(k, v)| k == "episode_id" && v == "42659")
        );
        let query = iframe
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(query.get("episode_id").map(|v| v.as_ref()), Some("42659"));
        assert_eq!(query.get("next_episode").map(|v| v.as_ref()), Some("1"));
        Ok(())
    }

    #[test]
    fn community_movie_iframe_uses_title_id_without_episode_query() -> Result<()> {
        let base = Url::parse("https://streamingcommunity.example/")?;
        let anime = community_anime("42659-buddy-solitaire", "Movie");
        let episode = Episode {
            id: "42659".into(),
            number: "1".into(),
            title: String::new(),
        };
        let (watch, iframe) = community_player_urls(&base, &anime, &episode)?;
        assert_eq!(
            watch.as_str(),
            "https://streamingcommunity.example/it/watch/42659"
        );
        assert_eq!(
            iframe.as_str(),
            "https://streamingcommunity.example/it/iframe/42659"
        );
        Ok(())
    }

    #[test]
    fn community_player_rejects_empty_or_mismatched_ids() -> Result<()> {
        let base = Url::parse("https://streamingcommunity.example/")?;
        let episode = Episode {
            id: "42659".into(),
            number: "1".into(),
            title: String::new(),
        };
        assert!(community_player_urls(&base, &community_anime("-slug", "TV"), &episode).is_err());
        assert!(community_player_urls(&base, &community_anime("1-", "TV"), &episode).is_err());
        assert!(
            community_player_urls(
                &base,
                &community_anime("6654-one-piece", "TV"),
                &Episode {
                    id: String::new(),
                    ..episode.clone()
                },
            )
            .is_err()
        );
        assert!(
            community_player_urls(&base, &community_anime("6654-one-piece", "Movie"), &episode,)
                .is_err()
        );
        Ok(())
    }
}
