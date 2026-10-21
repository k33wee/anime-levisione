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
    let start = raw
        .find("window.masterPlaylist")
        .context("VixCloud player changed: no download or HLS playlist found")?;
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
    if FHD_RE.is_match(raw) {
        url.query_pairs_mut().append_pair("h", "1");
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn missing_schema_is_error_not_empty_search() {
        let base = Url::parse("https://example.com").expect("test URL");
        assert!(world_search("{\"error\":true}", &base).is_err());
        assert!(unity_search("{}", &base).is_err());
        assert!(unity_source("<html>changed</html>").is_err());
    }
}
