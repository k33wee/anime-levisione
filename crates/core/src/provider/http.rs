use anyhow::{Context, Result, bail};
use reqwest::{Client, RequestBuilder, StatusCode};
use std::{sync::Arc, time::Duration};
use url::Url;

pub const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; anime-levisione/",
    env!("CARGO_PKG_VERSION"),
    "; terminal anime browser)"
);
const MAX_BODY: usize = 8 * 1024 * 1024;

fn build_client(
    jar: &Arc<reqwest::cookie::Jar>,
    timeout: Duration,
    connect_timeout: Duration,
) -> Result<Client> {
    Ok(Client::builder()
        .user_agent(USER_AGENT)
        .cookie_provider(jar.clone())
        .timeout(timeout)
        .connect_timeout(connect_timeout)
        .redirect(reqwest::redirect::Policy::limited(6))
        .build()?)
}

pub fn web_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).context("Provider returned an invalid URL")?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("Only HTTP(S) URLs without credentials are allowed");
    }
    Ok(url)
}
#[derive(Clone)]
pub struct Http {
    pub client: Client,
    pub jar: Arc<reqwest::cookie::Jar>,
}
impl Http {
    pub fn new() -> Result<Self> {
        Self::with_timeouts(Duration::from_secs(15), Duration::from_secs(5))
    }
    /// A normal-timeout client that shares an existing cookie jar, used when a
    /// probe provider is promoted to real traffic without losing its session.
    pub fn sharing(jar: Arc<reqwest::cookie::Jar>) -> Result<Self> {
        Ok(Self {
            client: build_client(&jar, Duration::from_secs(15), Duration::from_secs(5))?,
            jar,
        })
    }
    /// Fail-fast client for parallel domain probes, where waiting out a full
    /// retry cycle would stall startup.
    pub fn with_timeouts(timeout: Duration, connect_timeout: Duration) -> Result<Self> {
        let jar = Arc::new(reqwest::cookie::Jar::default());
        Ok(Self {
            client: build_client(&jar, timeout, connect_timeout)?,
            jar,
        })
    }
    pub async fn text(&self, request: RequestBuilder) -> Result<String> {
        Ok(self.fetch(request, true).await?.1)
    }
    /// Single-shot variant of [`Http::text`] that also reports the final URL
    /// after redirects, so domain probes can notice provider rotations.
    pub async fn text_at(&self, request: RequestBuilder) -> Result<(Url, String)> {
        self.fetch(request, false).await
    }
    async fn fetch(&self, request: RequestBuilder, retry: bool) -> Result<(Url, String)> {
        for attempt in 0..3 {
            let req = request.try_clone().context("Cannot retry request body")?;
            let result = req.send().await;
            match result {
                Ok(mut response) => {
                    let status = response.status();
                    if retry
                        && (status.is_server_error()
                            || status == StatusCode::TOO_MANY_REQUESTS
                            || status == StatusCode::REQUEST_TIMEOUT)
                        && attempt < 2
                    {
                        let wait = response
                            .headers()
                            .get("retry-after")
                            .and_then(|h| h.to_str().ok())
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(1)
                            .min(2);
                        tokio::time::sleep(Duration::from_millis(wait * 500 * (attempt + 1))).await;
                        continue;
                    }
                    if !status.is_success() {
                        bail!(
                            "HTTP {}{}",
                            status.as_u16(),
                            match status.as_u16() {
                                403 =>
                                    " — provider blocked the request; try the other source or update its URL in config",
                                419 => " — provider session expired",
                                429 => " — rate limited; wait and retry",
                                _ => "",
                            }
                        );
                    }
                    let final_url = response.url().clone();
                    let mut bytes = Vec::new();
                    while let Some(chunk) = response.chunk().await? {
                        if bytes.len() + chunk.len() > MAX_BODY {
                            bail!("Provider response exceeded the 8 MiB safety limit");
                        }
                        bytes.extend_from_slice(&chunk);
                    }
                    return Ok((
                        final_url,
                        String::from_utf8(bytes).context("Provider returned non-UTF-8 text")?,
                    ));
                }
                Err(e) if retry && attempt < 2 && (e.is_timeout() || e.is_connect()) => {
                    tokio::time::sleep(Duration::from_millis(250 * (attempt + 1))).await;
                }
                Err(e) => {
                    return Err(e.without_url())
                        .context("Network request failed; check your connection and provider URL");
                }
            }
        }
        bail!("Provider unavailable after retries")
    }
}
