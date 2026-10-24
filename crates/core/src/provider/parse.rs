use super::http::web_url;
use crate::model::{Anime, Episode, ProviderId, clean, sort_episodes};
use anyhow::{Context, Result, bail};
use regex::Regex;
use scraper::{Html, Selector};
use serde_json::Value;
use std::sync::LazyLock;
use url::Url;

fn selector(s: &str) -> Selector {
    Selector::parse(s).expect("constant CSS selector")
}
fn constant(pattern: &str) -> Regex {
    Regex::new(pattern).expect("constant regex")
}
static EPISODE_NUMBER_RE: LazyLock<Regex> = LazyLock::new(|| constant(r"(?i)\s+Ep[.\s]+(\S+)\s*$"));
static DOWNLOAD_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| constant(r#"window\.downloadUrl\s*=\s*['"]([^'"]*)['"]"#));
static PLAYLIST_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| constant(r#"\burl\s*:\s*['"]([^'"]+)['"]"#));
static FHD_RE: LazyLock<Regex> = LazyLock::new(|| constant(r"window\.canPlayFHD\s*=\s*true"));
fn param_regex(key: &str) -> Regex {
    constant(&format!(r#"['"]?{key}['"]?\s*:\s*['"]([^'"]*)['"]"#))
}
static PARAM_RES: [(&str, LazyLock<Regex>); 3] = [
    ("token", LazyLock::new(|| param_regex("token"))),
    ("expires", LazyLock::new(|| param_regex("expires"))),
    ("asn", LazyLock::new(|| param_regex("asn"))),
];
pub fn text(v: &Value, key: &str) -> String {
    match &v[key] {
        Value::String(s) => clean(s),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}
fn number(v: &Value, key: &str) -> u32 {
    text(v, key).parse().unwrap_or(0)
}
fn yes(v: &Value, key: &str) -> bool {
    v[key] == true || v[key] == 1 || v[key] == "1"
}
pub fn plain_html(raw: &str) -> String {
    clean(
        &Html::parse_fragment(raw)
            .root_element()
            .text()
            .collect::<Vec<_>>()
            .join(" "),
    )
}

pub fn csrf(html: &str) -> Result<String> {
    let doc = Html::parse_document(html);
    doc.select(&selector("meta[name='csrf-token'], meta#csrf-token"))
        .find_map(|e| e.value().attr("content"))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .context("Provider page has no CSRF token; its layout may have changed")
}
fn image_url(raw: &str, base: &Url) -> String {
    base.join(raw)
        .ok()
        .filter(|u| !raw.is_empty() && u.scheme() == "https")
        .map(|u| u.to_string())
        .unwrap_or_default()
}

pub fn unity_anime(v: &Value, base: &Url) -> Result<Anime> {
    let id = text(v, "id");
    if id.is_empty() {
        bail!("AnimeUnity record has no id");
    }
    let aliases: Vec<String> = ["title_eng", "title", "title_it"]
        .map(|k| text(v, k))
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    let title = aliases
        .first()
        .context("AnimeUnity record has no title")?
        .clone();
    Ok(Anime {
        provider: ProviderId::AnimeUnity,
        id: id.clone(),
        title,
        aliases,
        synopsis: plain_html(&text(v, "plot")),
        year: text(v, "date"),
        kind: text(v, "type"),
        status: text(v, "status"),
        score: text(v, "score"),
        episodes: number(v, "real_episodes_count").max(number(v, "episodes_count")),
        dub: yes(v, "dub"),
        genres: v["genres"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).map(clean).collect())
            .unwrap_or_default(),
        page_url: base
            .join(&format!("anime/{}-{}", id, text(v, "slug")))?
            .to_string(),
        cover_url: image_url(
            &["cover", "imageurl"]
                .iter()
                .map(|k| text(v, k))
                .find(|s| !s.is_empty())
                .unwrap_or_default(),
            base,
        ),
    })
}
pub fn unity_search(raw: &str, base: &Url) -> Result<Vec<Anime>> {
    let value: Value =
        serde_json::from_str(raw).context("AnimeUnity returned invalid search JSON")?;
    value["records"]
        .as_array()
        .context("AnimeUnity search response is missing records")?
        .iter()
        .map(|v| unity_anime(v, base))
        .collect()
}
pub fn unity_latest(raw: &str, base: &Url) -> Result<Vec<Anime>> {
    let doc = Html::parse_document(raw);
    let json = doc
        .select(&selector("layout-items[items-json]"))
        .next()
        .and_then(|e| e.value().attr("items-json"))
        .context("AnimeUnity latest layout changed: items-json not found")?;
    let value: Value = serde_json::from_str(json)?;
    let mut result = Vec::new();
    for record in value["data"]
        .as_array()
        .context("AnimeUnity latest response is missing data")?
    {
        let mut anime = unity_anime(&record["anime"], base)?;
        anime.episodes = anime.episodes.max(number(record, "number"));
        if !result.iter().any(|a: &Anime| a.id == anime.id) {
            result.push(anime);
        }
    }
    Ok(result)
}
pub fn unity_episodes(raw: &str) -> Result<Vec<Episode>> {
    let value: Value = serde_json::from_str(raw)?;
    let mut episodes = Vec::new();
    for v in value["episodes"]
        .as_array()
        .context("AnimeUnity response is missing episodes")?
    {
        let id = text(v, "id");
        let number = text(v, "number");
        if id.is_empty() || number.is_empty() {
            bail!("AnimeUnity returned an episode without id or number");
        }
        episodes.push(Episode {
            id,
            number,
            title: text(v, "title"),
        });
    }
    sort_episodes(&mut episodes);
    Ok(episodes)
}
pub fn world_search(raw: &str, base: &Url) -> Result<Vec<Anime>> {
    let value: Value =
        serde_json::from_str(raw).context("AnimeWorld returned invalid search JSON")?;
    let mut result = Vec::new();
    for v in value["animes"]
        .as_array()
        .context("AnimeWorld search response is missing animes")?
    {
        let link = text(v, "link");
        let id = text(v, "identifier");
        let title = text(v, "name");
        if link.is_empty() || id.is_empty() || title.is_empty() {
            bail!("AnimeWorld search record is missing its title or identifier");
        }
        result.push(Anime {
            provider: ProviderId::AnimeWorld,
            id: format!("{link}.{id}"),
            title,
            aliases: vec![],
            synopsis: plain_html(&text(v, "story")),
            year: text(v, "year"),
            kind: if text(v, "animeType") == "0" {
                "TV".into()
            } else {
                "Movie / Special".into()
            },
            status: text(v, "stateName"),
            score: text(v, "malVote"),
            episodes: number(v, "episodes"),
            dub: yes(v, "dub"),
            genres: v["categories"]
                .as_array()
                .map(|a| a.iter().map(|v| text(v, "name")).collect())
                .unwrap_or_default(),
            page_url: base.join(&format!("play/{link}.{id}"))?.to_string(),
            cover_url: image_url(&text(v, "image"), base),
        });
    }
    Ok(result)
}
pub fn world_latest(raw: &str, base: &Url) -> Result<Vec<Anime>> {
    let doc = Html::parse_document(raw);
    let mut result = Vec::new();
    for element in doc.select(&selector("a.poster[href][title]")) {
        let href = element.value().attr("href").unwrap_or_default();
        let url = base.join(href)?;
        let parts: Vec<_> = url
            .path_segments()
            .context("Invalid AnimeWorld page URL")?
            .collect();
        if parts.first() != Some(&"play") || parts.len() < 2 {
            continue;
        }
        let id = parts[1].to_string();
        if result.iter().any(|a: &Anime| a.id == id) {
            continue;
        }
        let raw_title = element.value().attr("title").unwrap_or_default();
        let count = EPISODE_NUMBER_RE
            .captures(raw_title)
            .and_then(|c| c[1].parse().ok())
            .unwrap_or(0);
        let title = clean(&EPISODE_NUMBER_RE.replace(raw_title, ""));
        result.push(Anime {
            provider: ProviderId::AnimeWorld,
            id: id.clone(),
            dub: title.to_lowercase().contains("(ita)"),
            title,
            aliases: vec![],
            synopsis: String::new(),
            year: String::new(),
            kind: String::new(),
            status: String::new(),
            score: String::new(),
            episodes: count,
            genres: vec![],
            page_url: base.join(&format!("play/{id}"))?.to_string(),
            cover_url: element
                .select(&selector("img"))
                .next()
                .and_then(|img| {
                    img.value()
                        .attr("src")
                        .or_else(|| img.value().attr("data-src"))
                })
                .map(|raw| image_url(raw, base))
                .unwrap_or_default(),
        });
    }
    if result.is_empty() {
        bail!("AnimeWorld latest layout changed: no posters found");
    }
    Ok(result)
}
pub fn world_detail(raw: &str, anime: &mut Anime) -> Result<Vec<Episode>> {
    let doc = Html::parse_document(raw);
    if let Some(desc) = doc.select(&selector(".desc")).next() {
        anime.synopsis = clean(&desc.text().collect::<Vec<_>>().join(" "));
    }
    let mut episodes = Vec::new();
    for e in doc.select(&selector("li.episode a[data-id][data-num]")) {
        episodes.push(Episode {
            id: e.value().attr("data-id").unwrap_or_default().into(),
            number: e.value().attr("data-num").unwrap_or_default().into(),
            title: String::new(),
        });
    }
    sort_episodes(&mut episodes);
    anime.episodes = anime.episodes.max(episodes.len() as u32);
    Ok(episodes)
}
/// AnimeWorld now resolves episodes via api/episode/info, whose JSON exposes a direct
/// `grabber` file URL; keep the legacy <source> markup as a fallback.
pub fn world_source(raw: &str) -> Result<Url> {
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        let grabber = text(&value, "grabber");
        if !grabber.is_empty() {
            return web_url(&grabber);
        }
        bail!("AnimeWorld returned no video URL for this episode");
    }
    let doc = Html::parse_document(raw);
    web_url(
        doc.select(&selector("source[src]"))
            .next()
            .and_then(|s| s.value().attr("src"))
            .context("AnimeWorld player changed: no video source found")?,
    )
}
/// Parse data assignments only; never execute provider JavaScript.
pub fn unity_source(raw: &str) -> Result<Url> {
    if let Some(c) = DOWNLOAD_URL_RE
        .captures(raw)
        .filter(|c| !c[1].trim().is_empty())
    {
        return web_url(&html_escape::decode_html_entities(&c[1]));
    }
    vixcloud_playlist(raw)
}

/// The signed HLS playlist of a VixCloud player page. StreamingCommunity uses
/// this directly: its `downloadUrl` cross-links to another site's page
/// (`animeunity.so/video/...`), which is not a media file.
pub fn vixcloud_playlist(raw: &str) -> Result<Url> {
    let start = raw
        .find("window.masterPlaylist")
        .context("VixCloud player changed: no HLS playlist found")?;
    let chunk: String = raw[start..].chars().take(8000).collect();
    let url = PLAYLIST_URL_RE
        .captures(&chunk)
        .context("VixCloud playlist has no URL")?;
    let mut url = web_url(&html_escape::decode_html_entities(&url[1]))?;
    for (key, pattern) in &PARAM_RES {
        if let Some(c) = pattern.captures(&chunk).filter(|c| !c[1].is_empty()) {
            url.query_pairs_mut().append_pair(key, &c[1]);
        }
    }
    // The token signature covers the full parameter set the player sends;
    // without h=1 (canPlayFHD) the playlist rejects the request.
    if FHD_RE.is_match(raw) {
        url.query_pairs_mut().append_pair("h", "1");
    }
    Ok(url)
}

// ---------- StreamingCommunity ----------
// Laravel Inertia pages: every page embeds its data as an HTML-escaped JSON
// blob in a `data-page` attribute, so parsing is JSON-driven, not DOM-driven.

static INERTIA_RE: LazyLock<Regex> = LazyLock::new(|| constant(r#"data-page="([^"]*)""#));
static IFRAME_SRC_RE: LazyLock<Regex> = LazyLock::new(|| constant(r#"<iframe[^>]+src="([^"]+)""#));

fn inertia_page(raw: &str) -> Result<Value> {
    let attr = INERTIA_RE
        .captures(raw)
        .context("StreamingCommunity page has no Inertia data; its layout may have changed")?;
    let json = html_escape::decode_html_entities(&attr[1]);
    serde_json::from_str(&json).context("StreamingCommunity returned malformed page data")
}

fn year_of(v: &Value) -> String {
    for key in ["release_date", "last_air_date"] {
        if let Some(year) = v[key].as_str().and_then(|d| d.get(0..4)) {
            return year.to_string();
        }
    }
    String::new()
}

/// Map a title row from search results or homepage sliders onto the shared
/// Anime model. The id doubles as the URL path segment `it/titles/{id}-{slug}`.
fn community_row(v: &Value, base: &Url, cdn: &str) -> Option<Anime> {
    let id = v["id"].as_u64()?;
    let slug = v["slug"].as_str()?;
    let name = clean(v["name"].as_str()?);
    Some(Anime {
        provider: ProviderId::StreamingCommunity,
        id: format!("{id}-{slug}"),
        title: name,
        aliases: vec![],
        synopsis: String::new(),
        year: year_of(v),
        kind: if v["type"] == "tv" { "TV" } else { "Movie" }.into(),
        status: String::new(),
        score: text(v, "score"),
        episodes: 0,
        dub: !yes(v, "sub_ita"),
        genres: vec![],
        page_url: format!("{base}it/titles/{id}-{slug}"),
        cover_url: v["images"]
            .as_array()
            .and_then(|images| images.iter().find(|image| image["type"] == "poster"))
            .and_then(|image| image["filename"].as_str())
            .filter(|name| !name.contains('/') && !cdn.is_empty())
            .map(|name| {
                image_url(
                    &format!("{}/images/{name}", cdn.trim_end_matches('/')),
                    base,
                )
            })
            .unwrap_or_default(),
    })
}

fn community_rows(titles: &[Value], base: &Url, cdn: &str) -> Result<Vec<Anime>> {
    let rows = titles
        .iter()
        .filter_map(|t| community_row(t, base, cdn))
        .collect::<Vec<_>>();
    if !titles.is_empty() && rows.is_empty() {
        bail!("StreamingCommunity title rows could not be parsed");
    }
    Ok(rows)
}

/// Homepage check used by domain probes: a real homepage carries sliders.
pub fn community_home(raw: &str) -> Result<()> {
    let page = inertia_page(raw)?;
    if page["props"]["sliders"].is_array() || page["props"]["titles"].is_array() {
        Ok(())
    } else {
        bail!("StreamingCommunity homepage has no catalogue data")
    }
}

pub fn community_search(raw: &str, base: &Url) -> Result<Vec<Anime>> {
    let page = inertia_page(raw)?;
    let titles = page["props"]["titles"]
        .as_array()
        .context("StreamingCommunity search returned no titles")?;
    community_rows(
        titles,
        base,
        page["props"]["cdn_url"].as_str().unwrap_or_default(),
    )
}

pub fn community_latest(raw: &str, base: &Url) -> Result<Vec<Anime>> {
    let page = inertia_page(raw)?;
    let sliders = page["props"]["sliders"]
        .as_array()
        .context("StreamingCommunity homepage has no sliders")?;
    // Prefer the "recently added" slider; fall back to the first with titles.
    let titles = sliders
        .iter()
        .find(|s| s["label"].as_str().is_some_and(|l| l.contains("Aggiunti")))
        .or_else(|| {
            sliders
                .iter()
                .find(|s| s["titles"].as_array().is_some_and(|t| !t.is_empty()))
        })
        .and_then(|s| s["titles"].as_array())
        .context("StreamingCommunity homepage sliders are empty")?;
    community_rows(
        titles,
        base,
        page["props"]["cdn_url"].as_str().unwrap_or_default(),
    )
}

/// Parse a title page: fills detail fields on the Anime and returns the
/// season numbers to fetch (empty for movies).
pub fn community_detail(raw: &str, anime: &mut Anime) -> Result<Vec<u32>> {
    let page = inertia_page(raw)?;
    let title = &page["props"]["title"];
    if title.is_null() {
        bail!("StreamingCommunity title page has no data");
    }
    if let Some(alias) = title["original_name"].as_str().map(clean)
        && !alias.is_empty()
        && alias != anime.title
    {
        anime.aliases.push(alias);
    }
    anime.synopsis = title["plot"].as_str().map(clean).unwrap_or_default();
    anime.status = text(title, "status");
    anime.score = text(title, "score");
    anime.year = year_of(title);
    anime.dub = yes(title, "dub_ita");
    anime.genres = title["genres"]
        .as_array()
        .map(|g| {
            g.iter()
                .filter_map(|x| x["name"].as_str().map(clean))
                .collect()
        })
        .unwrap_or_else(|| anime.genres.clone());
    let mut seasons = title["seasons"]
        .as_array()
        .map(|s| {
            s.iter()
                .filter_map(|x| x["number"].as_u64())
                .map(|n| n as u32)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    seasons.sort_unstable();
    Ok(seasons)
}

pub fn community_season(raw: &str) -> Result<Vec<Episode>> {
    let page = inertia_page(raw)?;
    let episodes = page["props"]["loadedSeason"]["episodes"]
        .as_array()
        .context("StreamingCommunity season page has no episodes")?;
    let out = episodes
        .iter()
        .filter_map(|e| {
            Some(Episode {
                id: e["id"].as_u64()?.to_string(),
                number: text(e, "number"),
                title: e["name"].as_str().map(clean).unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();
    if !episodes.is_empty() && out.is_empty() {
        bail!("StreamingCommunity episode rows could not be parsed");
    }
    Ok(out)
}

/// The iframe page hands out a single-use, fully-signed VixCloud embed URL,
/// exactly like AnimeUnity's embed endpoint.
pub fn community_embed(raw: &str) -> Result<Url> {
    let src = IFRAME_SRC_RE
        .captures(raw)
        .context("StreamingCommunity iframe page has no player")?;
    web_url(&html_escape::decode_html_entities(&src[1]))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn inertia_html(json: &str) -> String {
        format!(
            r#"<div id="app" data-page="{}"></div>"#,
            json.replace('"', "&quot;")
        )
    }
    #[test]
    fn artwork_is_extracted_without_changing_title_identity() -> Result<()> {
        let base = Url::parse("https://provider.example/")?;
        let unity = unity_anime(
            &serde_json::json!({"id":1,"title_eng":"Title","cover":"https://cdn.example/poster.jpg"}),
            &base,
        )?;
        assert_eq!(unity.cover_url, "https://cdn.example/poster.jpg");
        let world = world_latest(
            r#"<a class="poster" href="/play/title.1" title="Title Ep 1"><img src="https://cdn.example/a.jpg"></a>"#,
            &base,
        )?;
        assert_eq!(world[0].cover_url, "https://cdn.example/a.jpg");
        let sc = community_search(
            &inertia_html(
                r#"{"props":{"cdn_url":"https://cdn.example","titles":[{"id":1,"slug":"title","name":"Title","images":[{"type":"poster","filename":"cover.webp"}]}]}}"#,
            ),
            &base,
        )?;
        assert_eq!(sc[0].cover_url, "https://cdn.example/images/cover.webp");
        assert!(image_url("file:///private", &base).is_empty());
        assert!(image_url("javascript:alert(1)", &base).is_empty());
        Ok(())
    }
    #[test]
    fn community_search_maps_rows_and_keeps_signed_embed_urls() -> Result<()> {
        let base = Url::parse("https://streamingcommunity.example")?;
        let page = r#"{"props":{"titles":[{"id":6654,"slug":"one-piece","name":"ONE PIECE","type":"tv","score":"8.5","sub_ita":0,"release_date":"2023-08-31","seasons_count":2}]}}"#;
        let mut animes = community_search(&inertia_html(page), &base)?;
        assert_eq!(animes.len(), 1);
        let anime = animes.remove(0);
        assert_eq!(anime.id, "6654-one-piece");
        assert_eq!(anime.title, "ONE PIECE");
        assert_eq!(anime.kind, "TV");
        assert_eq!(anime.year, "2023");
        assert!(anime.dub, "Italian dub by default when sub_ita is 0");
        assert_eq!(
            anime.page_url,
            "https://streamingcommunity.example/it/titles/6654-one-piece"
        );
        // A search page without a titles array is a parse error, never an empty result.
        assert!(community_search(&inertia_html("{\"props\":{}}"), &base).is_err());
        assert!(community_home("<html>parked</html>").is_err());
        Ok(())
    }
    #[test]
    fn community_latest_prefers_recently_added_slider() -> Result<()> {
        let base = Url::parse("https://streamingcommunity.example")?;
        let page = r#"{"props":{"sliders":[{"label":"I titoli del momento","titles":[{"id":1,"slug":"trending","name":"Trending","type":"tv"}]},{"label":"Aggiunti di recente","titles":[{"id":2,"slug":"recent","name":"Recent","type":"movie","score":null}]}]}}"#;
        let animes = community_latest(&inertia_html(page), &base)?;
        assert_eq!(animes.len(), 1);
        assert_eq!(animes[0].id, "2-recent");
        assert_eq!(animes[0].kind, "Movie");
        Ok(())
    }
    #[test]
    fn community_detail_fills_fields_and_lists_seasons() -> Result<()> {
        let base = Url::parse("https://streamingcommunity.example")?;
        let mut anime = community_search(
        &inertia_html(
            r#"{"props":{"titles":[{"id":6654,"slug":"one-piece","name":"One Piece","type":"tv"}]}}"#,
        ),
        &base,
    )?
    .remove(0);
        let page = r#"{"props":{"title":{"id":6654,"type":"tv","status":"Returning Series","score":"8.5","sub_ita":0,"dub_ita":1,"plot":"La storia segue…","original_name":"ONE PIECE","release_date":"2023-08-31","genres":[{"name":"Sci-Fi & Fantasy"}],"seasons":[{"number":2},{"number":1}]}}}"#;
        let seasons = community_detail(&inertia_html(page), &mut anime)?;
        assert_eq!(seasons, [1, 2]);
        assert_eq!(anime.status, "Returning Series");
        assert_eq!(anime.synopsis, "La storia segue…");
        assert_eq!(anime.genres, ["Sci-Fi & Fantasy"]);
        assert!(anime.dub);
        assert_eq!(anime.aliases, ["ONE PIECE"]);
        Ok(())
    }
    #[test]
    fn community_season_and_embed_extract_player_data() -> Result<()> {
        let page = r#"{"props":{"loadedSeason":{"episodes":[{"id":42659,"number":1,"name":"ROMANCE DAWN, L'ALBA","scws_id":682356}]}}}"#;
        let episodes = community_season(&inertia_html(page))?;
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].id, "42659");
        assert_eq!(episodes[0].number, "1");
        assert_eq!(episodes[0].title, "ROMANCE DAWN, L'ALBA");
        let iframe = "<iframe src=\"https://vixcloud.cc/embed/538142?token=abc&amp;t=QnVkZHk&amp;expires=42\" allowfullscreen></iframe>";
        let embed = community_embed(iframe)?;
        assert_eq!(embed.host_str(), Some("vixcloud.cc"));
        assert!(embed.query_pairs().any(|(k, v)| k == "token" && v == "abc"));
        assert!(community_embed("<html>no player</html>").is_err());
        Ok(())
    }
    #[test]
    fn signed_hls_and_direct_file() -> Result<()> {
        let raw = include_str!("../../tests/fixtures/vixcloud.html");
        let url = unity_source(raw)?;
        let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(params["token"], "a+b/c=");
        assert_eq!(params["h"], "1");
        assert_eq!(params["existing"], "yes");
        assert_eq!(
            unity_source("window.downloadUrl = 'https://cdn.example/video.mp4';")?.path(),
            "/video.mp4"
        );
        assert!(unity_source("window.downloadUrl = 'file:///etc/passwd'").is_err());
        Ok(())
    }
    #[test]
    fn world_grabber_json_and_legacy_source_markup() -> Result<()> {
        let json = r#"{"grabber":"https://cdn.example/video.mp4","name":"x","target":"/api/episode/server"}"#;
        assert_eq!(
            world_source(json)?.as_str(),
            "https://cdn.example/video.mp4"
        );
        let legacy = "<video><source src=\"https://cdn.example/legacy.mp4\"></video>";
        assert_eq!(
            world_source(legacy)?.as_str(),
            "https://cdn.example/legacy.mp4"
        );
        assert!(world_source("{\"target\": \"/api/episode/server\"}").is_err());
        Ok(())
    }
    #[test]
    fn world_uses_dom_not_attribute_order() -> Result<()> {
        let mut a = world_latest(
            include_str!("../../tests/fixtures/world.html"),
            &Url::parse("https://example.com")?,
        )?
        .remove(0);
        let eps = world_detail(include_str!("../../tests/fixtures/world.html"), &mut a)?;
        assert_eq!(a.title, "Pokémon & Friends");
        assert_eq!(eps[0].number, "1.5");
        assert_eq!(a.synopsis, "A story & an adventure.");
        Ok(())
    }
    #[test]
    fn community_playlist_ignores_cross_site_download_link() -> Result<()> {
        // StreamingCommunity player pages: the downloadUrl cross-links to
        // another vixcloud site's page, so only the signed playlist is a
        // real source.
        let page = "<script>\nwindow.masterPlaylist = {\nparams: { 'token': 'sig', 'expires': '42', 'asn': '', },\nurl: 'https://vixcloud.cc/playlist/538142',\n}\nwindow.canPlayFHD = true\nwindow.downloadUrl = 'https://animeunity.so/video/538142'\n</script>";
        let playlist = vixcloud_playlist(page)?;
        assert_eq!(
            playlist.as_str(),
            "https://vixcloud.cc/playlist/538142?token=sig&expires=42&h=1"
        );
        Ok(())
    }
    #[test]
    fn missing_schema_is_error_not_empty_search() {
        let base = Url::parse("https://example.com").expect("test URL");
        assert!(world_search("{\"error\":true}", &base).is_err());
        assert!(unity_search("{}", &base).is_err());
        assert!(unity_source("<html>changed</html>").is_err());
    }
}
