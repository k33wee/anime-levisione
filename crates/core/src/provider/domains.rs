//! Provider domain discovery.
//!
//! Both libraries rotate their domains every few weeks (AnimeUnity alone went
//! `.so` → `.to` → `.co` within days). Instead of failing until `config.toml`
//! is edited by hand, each provider locates itself at startup and after failed
//! requests: the configured URL, the last domain that worked (`domains.json`),
//! extra mirrors from `config.toml`, and a built-in list of known domains are
//! probed in parallel. Redirects are followed, so a dead domain that forwards
//! to its successor is adopted automatically. A candidate only counts as live
//! once its homepage can bootstrap a provider session (a CSRF token), which
//! parked or hijacked domains cannot.

use super::{Provider, ProviderId, http::web_url};
use crate::store::Config;
use anyhow::{Result, bail};
use futures_util::{StreamExt, stream::FuturesUnordered};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc, time::Duration};
use url::Url;

/// Known AnimeUnity domains, most recent first.
const ANIMEUNITY_DOMAINS: &[&str] = &[
    "https://www.animeunity.to",
    "https://www.animeunity.co",
    "https://www.animeunity.so",
    "https://www.animeunity.tv",
];
/// Known AnimeWorld domains, most recent first.
const ANIMEWORLD_DOMAINS: &[&str] = &[
    "https://www.animeworld.ac",
    "https://www.animeworld.play",
    "https://www.animeworld.tv",
];
/// Known StreamingCommunity domains, most recent first. This provider rotates
/// even faster than the anime sites.
const STREAMINGCOMMUNITY_DOMAINS: &[&str] = &[
    "https://streamingcommunityz.build",
    "https://streamingcommunity.to",
    "https://streamingcommunity.net",
    "https://streamingcommunity.xyz",
    "https://streamingcommunity.bond",
    "https://streamingcommunity.cafe",
    "https://streamingcommunity.co.uk",
];

/// The primary candidate gets a head start so healthy setups never touch mirrors.
const HEAD_START: Duration = Duration::from_millis(1500);
#[derive(Debug, Clone)]
pub struct Candidate {
    pub url: Url,
    /// Configured, cached, and user-provided mirrors are trusted to redirect
    /// wherever they like; built-in candidates are only accepted when the
    /// final host still names the provider.
    pub trusted: bool,
}

#[derive(Clone)]
pub struct Resolution {
    pub id: ProviderId,
    /// The candidate that was tried first (the configured URL).
    pub configured: Url,
    /// Validated live homepage, when any candidate responded.
    pub live: Option<Url>,
    /// A provider fully bootstrapped on the live domain: its homepage answered,
    /// its session (cookies and CSRF token) is already established.
    pub provider: Option<Arc<Provider>>,
    /// Why validation failed, when nothing responded.
    pub error: Option<String>,
}
impl Resolution {
    /// True when the live domain differs from the configured one.
    pub fn moved(&self) -> bool {
        self.live
            .as_ref()
            .is_some_and(|live| live.host_str() != self.configured.host_str())
    }
    pub fn describe(&self) -> String {
        match (&self.live, self.moved()) {
            (Some(live), true) => {
                format!("auto-detected {live} (configured {})", self.configured)
            }
            (Some(live), false) => format!("{live}"),
            (None, _) => format!(
                "no live domain found ({}) — keeping {}",
                self.error.as_deref().unwrap_or("no candidates"),
                self.configured
            ),
        }
    }
}

/// Homepage of a URL: same origin, root path, no query or fragment.
pub(crate) fn root_of(url: &Url) -> Url {
    let mut root = url.clone();
    root.set_path("/");
    root.set_query(None);
    root.set_fragment(None);
    root
}

fn family(id: ProviderId) -> &'static str {
    match id {
        ProviderId::AnimeUnity => "animeunity",
        ProviderId::AnimeWorld => "animeworld",
        ProviderId::StreamingCommunity => "streamingcommunity",
    }
}

fn known(id: ProviderId) -> &'static [&'static str] {
    match id {
        ProviderId::AnimeUnity => ANIMEUNITY_DOMAINS,
        ProviderId::AnimeWorld => ANIMEWORLD_DOMAINS,
        ProviderId::StreamingCommunity => STREAMINGCOMMUNITY_DOMAINS,
    }
}

fn primary(config: &Config, id: ProviderId) -> &str {
    match id {
        ProviderId::AnimeUnity => &config.animeunity_url,
        ProviderId::AnimeWorld => &config.animeworld_url,
        ProviderId::StreamingCommunity => &config.streamingcommunity_url,
    }
}

fn mirrors(config: &Config, id: ProviderId) -> &[String] {
    match id {
        ProviderId::AnimeUnity => &config.animeunity_domains,
        ProviderId::AnimeWorld => &config.animeworld_domains,
        ProviderId::StreamingCommunity => &config.streamingcommunity_domains,
    }
}

/// Ordered candidates for a provider: configured URL, cached live domain, user
/// mirrors, then built-ins. Duplicates are dropped keeping the first occurrence.
/// The configured URL is skipped when invalid; [`super::Provider::new`] is the
/// gate that rejects a broken config URL.
pub fn candidates(config: &Config, cache: &DomainCache, id: ProviderId) -> Vec<Candidate> {
    let mut entries: Vec<(String, bool)> = Vec::new();
    entries.push((primary(config, id).to_owned(), true));
    if let Some(cached) = cache.get(id) {
        entries.push((cached.to_owned(), true));
    }
    entries.extend(mirrors(config, id).iter().map(|m| (m.clone(), true)));
    entries.extend(known(id).iter().map(|k| ((*k).to_owned(), false)));
    let mut seen: Vec<Url> = Vec::new();
    let mut out = Vec::new();
    for (raw, trusted) in entries {
        // Only the primary entry is potentially invalid, and Provider::new
        // reports that; extras and cache rows are best-effort hints.
        let Ok(url) = web_url(&raw) else { continue };
        let root = root_of(&url);
        if seen.iter().any(|s| s.as_str() == root.as_str()) {
            continue;
        }
        seen.push(root.clone());
        out.push(Candidate { url: root, trusted });
    }
    out
}

/// Whether a probe landing is acceptable for this candidate: built-in
/// candidates must land on a host that still names the provider, and so must
/// any candidate that itself lives on a provider-family domain — a hijacked or
/// expired family domain must not be able to redirect the app (and its cache)
/// to an unrelated site. Only mirrors on custom hosts configured by the user
/// are trusted to redirect wherever they like.
fn landing_allowed(id: ProviderId, source_host: &str, landing_host: &str, trusted: bool) -> bool {
    let name = family(id);
    let landing = landing_host.to_ascii_lowercase();
    landing.contains(name) || (trusted && !source_host.to_ascii_lowercase().contains(name))
}

/// Fetch a candidate homepage and build a provider around it, session
/// included. The provider ends up on the final (post-redirect) domain, so a
/// dead TLD forwarding to its successor is adopted automatically.
async fn probe(id: ProviderId, candidate: &Candidate) -> Result<Provider> {
    let mut provider = Provider::for_probe(id, &candidate.url)?;
    let (final_url, html) = provider.page_at(root_of(&candidate.url)).await?;
    let landing = final_url.host_str().unwrap_or_default();
    if !landing_allowed(
        id,
        candidate.url.host_str().unwrap_or_default(),
        landing,
        candidate.trusted,
    ) {
        bail!(
            "{landing} (reached from {}) does not look like a {id} domain",
            candidate.url
        );
    }
    let token = provider.validate_home(&html)?;
    provider.base = root_of(&final_url);
    if let Some(token) = token {
        *provider.token.lock().await = Some(token);
    }
    // Promote to normal traffic timeouts; the session (cookies, token) carries
    // over because the cookie jar is shared with the new client.
    provider.promote()
}

/// A probe with an optional head start; used so the primary candidate is
/// always tried before any mirror but all of them can run in parallel.
async fn delayed_probe(id: ProviderId, candidate: Candidate, delay: Duration) -> Result<Provider> {
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    probe(id, &candidate).await
}

/// Probe candidates until one answers: the primary immediately, the rest after
/// a short head start, all in parallel; the first validated answer wins.
pub async fn resolve(id: ProviderId, candidates: &[Candidate]) -> Resolution {
    let mut resolution = Resolution {
        id,
        configured: candidates
            .first()
            .map(|c| c.url.clone())
            .unwrap_or_else(|| root_of(&web_url(known(id)[0]).expect("constant domain"))),
        live: None,
        provider: None,
        error: None,
    };
    let Some((primary, rest)) = candidates.split_first() else {
        resolution.error = Some("no candidate domains configured".into());
        return resolution;
    };
    let mut attempts = FuturesUnordered::new();
    attempts.push(delayed_probe(id, primary.clone(), Duration::ZERO));
    for candidate in rest {
        attempts.push(delayed_probe(id, candidate.clone(), HEAD_START));
    }
    while let Some(outcome) = attempts.next().await {
        match outcome {
            Ok(provider) => {
                resolution.live = Some(provider.base.clone());
                resolution.provider = Some(Arc::new(provider));
                resolution.error = None;
                return resolution;
            }
            Err(e) => resolution.error = Some(format!("{e:#}")),
        }
    }
    resolution
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedDomain {
    pub url: String,
    pub updated: u64,
}

/// Last-known-good provider domains, persisted next to `config.toml` so runs
/// after a rotation skip straight to the live domain. A cache is disposable:
/// corrupt files are ignored and saves are best-effort.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DomainCache {
    pub version: u32,
    pub animeworld: Option<CachedDomain>,
    pub animeunity: Option<CachedDomain>,
    pub streamingcommunity: Option<CachedDomain>,
}
impl DomainCache {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut doc = self.clone();
        doc.version = 1;
        crate::store::atomic_write(path, &serde_json::to_vec_pretty(&doc)?)
    }
    pub fn get(&self, id: ProviderId) -> Option<&str> {
        match id {
            ProviderId::AnimeUnity => self.animeunity.as_ref(),
            ProviderId::AnimeWorld => self.animeworld.as_ref(),
            ProviderId::StreamingCommunity => self.streamingcommunity.as_ref(),
        }
        .map(|entry| entry.url.as_str())
    }
    pub fn record(&mut self, id: ProviderId, url: &Url) {
        let entry = CachedDomain {
            url: url.to_string(),
            updated: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        match id {
            ProviderId::AnimeUnity => self.animeunity = Some(entry),
            ProviderId::AnimeWorld => self.animeworld = Some(entry),
            ProviderId::StreamingCommunity => self.streamingcommunity = Some(entry),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    const UNITY_PAGE: &str =
        r#"<html><head><meta name="csrf-token" content="probe-token"></head></html>"#;

    fn http_response(status: &str, headers: &[(&str, &str)], body: &str) -> Vec<u8> {
        let mut head = format!("{status}\r\nconnection: close\r\n");
        for (key, value) in headers {
            head.push_str(&format!("{key}: {value}\r\n"));
        }
        head.push_str(&format!("content-length: {}\r\n\r\n", body.len()));
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(body.as_bytes());
        bytes
    }

    /// Minimal HTTP/1.1 server answering every request with `responder`.
    fn spawn_server(responder: impl Fn() -> Vec<u8> + Send + 'static) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buf = [0u8; 4096];
                let mut got = Vec::new();
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = stream.write_all(&responder());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}/")
    }

    fn dead_url() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);
        format!("http://{addr}/")
    }

    #[test]
    fn candidates_prefer_config_then_cache_mirrors_builtins() {
        let config = Config {
            animeunity_domains: vec!["https://au-mirror.example".into()],
            ..Config::default()
        };
        let mut cache = DomainCache::default();
        cache.record(
            ProviderId::AnimeUnity,
            &web_url("https://www.animeunity.co").expect("url"),
        );
        let list = candidates(&config, &cache, ProviderId::AnimeUnity);
        let urls: Vec<&str> = list.iter().map(|c| c.url.as_str()).collect();
        assert_eq!(urls.first().copied(), Some("https://www.animeunity.to/"));
        assert!(list.first().expect("primary").trusted);
        // The cached .co domain is deduplicated against the built-in entry.
        assert_eq!(
            list.iter()
                .filter(|c| c.url.host_str() == Some("www.animeunity.co"))
                .count(),
            1
        );
        assert!(
            list.iter()
                .any(|c| c.url.host_str() == Some("au-mirror.example"))
        );
        assert!(!list.last().expect("builtin").trusted);
    }

    #[test]
    fn landing_guard_keeps_family_domains_in_the_family() {
        // Built-in candidates must land on a provider host.
        assert!(landing_allowed(
            ProviderId::AnimeUnity,
            "www.animeunity.so",
            "www.animeunity.co",
            false
        ));
        assert!(!landing_allowed(
            ProviderId::AnimeUnity,
            "www.animeunity.so",
            "ads.example",
            false
        ));
        // A configured family domain must not redirect to a foreign host either.
        assert!(!landing_allowed(
            ProviderId::AnimeUnity,
            "www.animeunity.to",
            "ads.example",
            true
        ));
        // Custom mirrors configured by the user are trusted wholesale.
        assert!(landing_allowed(
            ProviderId::AnimeUnity,
            "my-mirror.example",
            "anything.example",
            true
        ));
        // A custom mirror landing back on a provider host is fine too.
        assert!(landing_allowed(
            ProviderId::AnimeUnity,
            "my-mirror.example",
            "www.animeunity.co",
            true
        ));
    }

    #[test]
    fn root_of_keeps_only_the_origin() {
        let url = web_url("https://x.animeunity.to/play/42?e=1#top").expect("url");
        assert_eq!(root_of(&url).as_str(), "https://x.animeunity.to/");
    }

    #[test]
    fn resolution_reports_movement() {
        let configured = web_url("https://www.animeunity.to/").expect("url");
        let mut resolution = Resolution {
            id: ProviderId::AnimeUnity,
            configured: configured.clone(),
            live: Some(web_url("https://www.animeunity.co/").expect("url")),
            provider: None,
            error: None,
        };
        assert!(resolution.moved());
        assert!(resolution.describe().contains("auto-detected"));
        resolution.live = Some(configured);
        assert!(!resolution.moved());
        resolution.live = None;
        resolution.error = Some("everything down".into());
        assert!(resolution.describe().contains("no live domain found"));
    }

    #[test]
    fn cache_roundtrip_and_record() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("domains.json");
        let mut cache = DomainCache::default();
        cache.record(
            ProviderId::AnimeUnity,
            &web_url("https://www.animeunity.co/").expect("url"),
        );
        cache.save(&path).expect("save");
        let loaded = DomainCache::load(&path).expect("load");
        assert_eq!(
            loaded.get(ProviderId::AnimeUnity),
            Some("https://www.animeunity.co/")
        );
        assert_eq!(loaded.get(ProviderId::AnimeWorld), None);
    }

    #[tokio::test]
    async fn probe_accepts_homepage_with_session_token() {
        let base = spawn_server(|| {
            http_response(
                "HTTP/1.1 200 OK",
                &[("content-type", "text/html")],
                UNITY_PAGE,
            )
        });
        let candidate = Candidate {
            url: web_url(&base).expect("url"),
            trusted: true,
        };
        let provider = probe(ProviderId::AnimeUnity, &candidate)
            .await
            .expect("probe");
        assert_eq!(provider.base.as_str(), base);
        assert_eq!(
            *provider.token.lock().await,
            Some("probe-token".to_string())
        );
    }

    #[tokio::test]
    async fn probe_follows_redirects_when_trusted() {
        let page = spawn_server(|| {
            http_response(
                "HTTP/1.1 200 OK",
                &[("content-type", "text/html")],
                UNITY_PAGE,
            )
        });
        let redirect = {
            let page = page.clone();
            spawn_server(move || {
                let page = page.clone();
                http_response(
                    "HTTP/1.1 301 Moved Permanently",
                    &[("location", page.as_str())],
                    "",
                )
            })
        };
        let candidate = Candidate {
            url: web_url(&redirect).expect("url"),
            trusted: true,
        };
        let provider = probe(ProviderId::AnimeUnity, &candidate)
            .await
            .expect("probe");
        assert_eq!(provider.base.as_str(), page);
        assert!(provider.token.lock().await.is_some());
    }

    #[tokio::test]
    async fn probe_rejects_builtin_redirect_to_foreign_host() {
        let page = spawn_server(|| {
            http_response(
                "HTTP/1.1 200 OK",
                &[("content-type", "text/html")],
                UNITY_PAGE,
            )
        });
        let redirect = {
            let page = page.clone();
            spawn_server(move || {
                let page = page.clone();
                http_response(
                    "HTTP/1.1 301 Moved Permanently",
                    &[("location", page.as_str())],
                    "",
                )
            })
        };
        let candidate = Candidate {
            url: web_url(&redirect).expect("url"),
            trusted: false,
        };
        let error = probe(ProviderId::AnimeUnity, &candidate)
            .await
            .err()
            .expect("foreign host must be rejected");
        assert!(error.to_string().contains("does not look like"));
    }

    #[tokio::test]
    async fn resolve_skips_dead_candidates() {
        let live = spawn_server(|| {
            http_response(
                "HTTP/1.1 200 OK",
                &[("content-type", "text/html")],
                UNITY_PAGE,
            )
        });
        let list = [dead_url(), dead_url(), live.clone()]
            .iter()
            .map(|url| Candidate {
                url: web_url(url).expect("url"),
                trusted: true,
            })
            .collect::<Vec<_>>();
        let resolution = resolve(ProviderId::AnimeUnity, &list).await;
        assert_eq!(resolution.live.expect("live").as_str(), live);
        assert!(
            resolution.provider.is_some(),
            "live resolution carries its provider"
        );
        assert!(resolution.error.is_none());
    }

    #[tokio::test]
    async fn resolve_reports_error_when_everything_is_dead() {
        let list = [dead_url(), dead_url()]
            .iter()
            .map(|url| Candidate {
                url: web_url(url).expect("url"),
                trusted: true,
            })
            .collect::<Vec<_>>();
        let resolution = resolve(ProviderId::AnimeWorld, &list).await;
        assert!(resolution.live.is_none());
        assert!(resolution.error.is_some());
    }
}
