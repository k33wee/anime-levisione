//! AniList metadata and tracking.
//!
//! AniList hosts no video. It enriches the details panel (score, genres,
//! episode count, status, synopsis) and, when the user has logged in, mirrors
//! the local watchlist and episode progress outward. Playback and downloads
//! stay on the streaming providers, and private mode keeps everything local:
//! lookups and syncs are simply never issued while it is on.
//!
//! API: `https://graphql.anilist.co` (public GraphQL; list mutations need the
//! user's OAuth token, stored in `anilist.json` in the config directory —
//! never inside the library file). Requests are paced to stay well inside
//! AniList's ~90/minute limit.

use crate::{
    model::{Anime, normalize},
    provider::http::Http,
    store,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, RwLock},
    time::{Duration, Instant},
};

const ENDPOINT: &str = "https://graphql.anilist.co";
/// AniList allows roughly 90 requests per minute; pacing lookups to slightly
/// under one per second keeps scrolling bursts inside the budget.
const REQUEST_SPACING: Duration = Duration::from_millis(750);
/// A match must at least contain the local title; fuzzier similarity is not
/// enough to attach metadata to a title.
const MATCH_THRESHOLD: u32 = 6_000;

const SEARCH_QUERY: &str = "query ($q: String) {
    Page(perPage: 8) {
        media(search: $q, type: ANIME, sort: SEARCH_MATCH) {
            id
            title { romaji english native }
            synonyms
            format
            status
            episodes
            averageScore
            genres
            description(asHtml: false)
            coverImage { extraLarge }
            startDate { year }
        }
    }
}";
const VIEWER_QUERY: &str = "{ Viewer { name } }";
const LIST_QUERY: &str = "query ($id: Int, $user: String) { MediaList(mediaId: $id, userName: $user) { id status progress } }";
const SAVE_MUTATION: &str = "mutation ($id: Int, $status: MediaListStatus, $progress: Int) {
    SaveMediaListEntry(mediaId: $id, status: $status, progress: $progress) { id }
}";
const DELETE_MUTATION: &str = "mutation ($id: Int) { DeleteMediaListEntry(id: $id) { deleted } }";

/// What the user did locally; tracking mirrors it outward.
#[derive(Debug, Clone)]
pub enum TrackEvent {
    /// The local watchlist gained (`true`) or lost (`false`) a title.
    Watchlist { added: bool },
    /// An episode was completed locally.
    Watched { number: String, total: u32 },
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MediaEntry {
    pub id: i64,
    pub title: MediaTitle,
    pub synonyms: Vec<String>,
    pub format: Option<String>,
    pub status: Option<String>,
    pub episodes: Option<u32>,
    pub average_score: Option<u32>,
    pub genres: Vec<String>,
    pub description: Option<String>,
    pub start_date: Option<StartDate>,
    pub cover_image: Option<MediaCover>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCover {
    pub extra_large: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MediaTitle {
    pub romaji: Option<String>,
    pub english: Option<String>,
    pub native: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StartDate {
    pub year: Option<u32>,
}

impl MediaEntry {
    /// Every name AniList knows the entry by.
    pub fn names(&self) -> Vec<&str> {
        let title = &self.title;
        [
            title.romaji.as_deref(),
            title.english.as_deref(),
            title.native.as_deref(),
        ]
        .into_iter()
        .flatten()
        .chain(self.synonyms.iter().map(String::as_str))
        .collect()
    }
    pub fn year(&self) -> Option<u32> {
        self.start_date.as_ref().and_then(|d| d.year)
    }
    /// Score on the familiar 0–10 scale, empty when unrated.
    pub fn display_score(&self) -> String {
        self.average_score
            .map(|s| format!("{:.1}", s as f64 / 10.0))
            .unwrap_or_default()
    }
    pub fn display_status(&self) -> String {
        match self.status.as_deref() {
            Some("FINISHED") => "Finished".into(),
            Some("RELEASING") => "Airing".into(),
            Some("NOT_YET_RELEASED") => "Upcoming".into(),
            Some("CANCELLED") => "Cancelled".into(),
            Some("HIATUS") => "Hiatus".into(),
            _ => String::new(),
        }
    }
    /// AniList descriptions carry HTML markup; strip it for the panel.
    pub fn plain_description(&self) -> String {
        let plain = self
            .description
            .as_deref()
            .map(crate::provider::parse::plain_html)
            .unwrap_or_default();
        truncate(&plain, 700)
    }
}

/// Cap text on a char boundary so the details panel stays readable.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}

fn friendly_status(status: &str) -> &str {
    match status {
        "CURRENT" => "Watching",
        "PLANNING" => "Planning",
        "COMPLETED" => "Completed",
        "PAUSED" => "Paused",
        "DROPPED" => "Dropped",
        "REPEATING" => "Rewatching",
        _ => "listed",
    }
}

/// Pick the AniList entry that is clearly the same title as a streaming
/// result: normalized name containment is required, with year and format
/// agreement breaking ties between remakes and same-named sequels.
pub fn match_media(anime: &Anime, entries: &[MediaEntry]) -> Option<MediaEntry> {
    let local: Vec<String> = std::iter::once(&anime.title)
        .chain(anime.aliases.iter())
        .map(|s| normalize(s))
        .filter(|s| !s.is_empty())
        .collect();
    if local.is_empty() {
        return None;
    }
    let mut best: Option<(u32, &MediaEntry)> = None;
    for entry in entries {
        let mut score = 0;
        for name in entry.names() {
            let remote = normalize(name);
            if remote.is_empty() {
                continue;
            }
            for query in &local {
                let s = if query == &remote {
                    10_000
                } else if query.starts_with(&remote) || remote.starts_with(query) {
                    8_000
                } else if query.contains(&remote) || remote.contains(query) {
                    6_000
                } else {
                    continue;
                };
                score = score.max(s);
            }
        }
        if score < MATCH_THRESHOLD {
            continue;
        }
        if let (Some(year), Some(start)) = (anime.year.parse::<i32>().ok(), entry.year()) {
            score += match (year - start as i32).abs() {
                0 => 1_200,
                1 => 400,
                _ => 0,
            };
        }
        let kind = anime.kind.to_ascii_lowercase();
        let format = entry.format.as_deref().unwrap_or_default();
        if (kind.contains("tv") && format == "TV") || (kind.contains("movi") && format == "MOVIE") {
            score += 600;
        }
        if best.is_none_or(|(s, _)| score > s) {
            best = Some((score, entry));
        }
    }
    best.map(|(_, entry)| entry.clone())
}

/// Stored AniList credentials. Lives in its own file in the config directory,
/// deliberately outside `library.json` so library sharing never leaks tokens.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Token {
    pub user: String,
    pub token: String,
    pub stored: u64,
}
impl Token {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("anilist.json")
    }
    /// A missing or corrupt token file is simply "logged out".
    pub fn load(dir: &Path) -> Option<Self> {
        let token: Self =
            serde_json::from_str(&std::fs::read_to_string(Self::path(dir)).ok()?).ok()?;
        (!token.token.is_empty()).then_some(token)
    }
    pub fn save(&self, dir: &Path) -> Result<()> {
        store::atomic_write(&Self::path(dir), &serde_json::to_vec_pretty(self)?)
    }
}

pub fn authorize_url(client_id: u32) -> String {
    format!("https://anilist.co/api/v2/oauth/authorize?client_id={client_id}&response_type=token")
}

struct ListEntry {
    id: i64,
    status: String,
    progress: i32,
}

pub struct Client {
    http: Http,
    token: RwLock<Option<String>>,
    user: RwLock<Option<String>>,
    next: Mutex<Instant>,
}
impl Client {
    pub fn anonymous() -> Result<Self> {
        Ok(Self {
            http: Http::new()?,
            token: RwLock::new(None),
            user: RwLock::new(None),
            next: Mutex::new(Instant::now()),
        })
    }
    pub fn logout(&self) {
        *self.token.write().expect("anilist token lock") = None;
        *self.user.write().expect("anilist user lock") = None;
    }
    pub fn open(dir: &Path) -> Result<Self> {
        let token = Token::load(dir);
        Ok(Self {
            user: RwLock::new(
                token
                    .as_ref()
                    .map(|t| t.user.clone())
                    .filter(|user| !user.is_empty()),
            ),
            token: RwLock::new(token.map(|t| t.token)),
            http: Http::new()?,
            next: Mutex::new(Instant::now()),
        })
    }
    /// Client for verifying a freshly pasted token before storing it.
    pub fn with_token(token: String) -> Result<Self> {
        Ok(Self {
            http: Http::new()?,
            token: RwLock::new(Some(token)),
            user: RwLock::new(None),
            next: Mutex::new(Instant::now()),
        })
    }
    /// A login completed while the app was already running: swap the live
    /// credentials without restarting. Sync starts using them immediately.
    pub fn adopt(&self, token: String, user: String) {
        *self.token.write().expect("anilist token lock") = Some(token);
        *self.user.write().expect("anilist user lock") = (!user.is_empty()).then_some(user);
    }
    pub fn user(&self) -> Option<String> {
        self.user.read().expect("anilist user lock").clone()
    }
    pub fn logged_in(&self) -> bool {
        self.token.read().expect("anilist token lock").is_some()
    }
    /// Space requests apart so bursts of lookups cannot trip the rate limit.
    async fn pace(&self) {
        let wait = {
            let mut next = self.next.lock().expect("anilist pacing lock");
            let now = Instant::now();
            if *next > now {
                let wait = *next - now;
                *next += REQUEST_SPACING;
                Some(wait)
            } else {
                *next = now + REQUEST_SPACING;
                None
            }
        };
        if let Some(wait) = wait {
            tokio::time::sleep(wait).await;
        }
    }
    async fn graphql(&self, query: &str, variables: Value, authed: bool) -> Result<Value> {
        self.pace().await;
        let mut request = self
            .http
            .client
            .post(ENDPOINT)
            .json(&json!({ "query": query, "variables": variables }));
        if authed {
            let token = self
                .token
                .read()
                .expect("anilist token lock")
                .clone()
                .context("AniList: log in first with `anime-levisione login`")?;
            request = request.bearer_auth(token);
        }
        let raw = self.http.text(request).await.map_err(|e| {
            let message = format!("{e:#}");
            // AniList answers invalid credentials with HTTP 400 and loses the
            // body through the generic client; give it an actionable message.
            if authed && (message.contains("HTTP 400") || message.contains("HTTP 401")) {
                anyhow::anyhow!(
                    "AniList rejected the stored token — log in again (`anime-levisione login`)"
                )
            } else {
                e.context("AniList request failed")
            }
        })?;
        let value: Value = serde_json::from_str(&raw).context("AniList returned malformed data")?;
        if let Some(error) = value["errors"].as_array().and_then(|errors| errors.first()) {
            bail!(
                "AniList: {}",
                error["message"].as_str().unwrap_or("request rejected")
            );
        }
        Ok(value["data"].clone())
    }
    pub async fn search(&self, query: &str) -> Result<Vec<MediaEntry>> {
        let data = self
            .graphql(SEARCH_QUERY, json!({ "q": query }), false)
            .await?;
        let media = data["Page"]["media"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(media
            .into_iter()
            .filter_map(|m| serde_json::from_value(m).ok())
            .collect())
    }
    pub async fn viewer(&self) -> Result<String> {
        let data = self.graphql(VIEWER_QUERY, json!({}), true).await?;
        data["Viewer"]["name"]
            .as_str()
            .map(str::to_owned)
            .context("AniList did not return your profile")
    }
    async fn list_entry(&self, media_id: i64) -> Result<Option<ListEntry>> {
        // AniList intermittently 500s on MediaList for some media when the
        // viewer is inferred from the token alone; naming them explicitly
        // sidesteps it. The name is cached from login; fetch it if missing.
        let user = match self.user() {
            Some(user) => user,
            None => self.viewer().await?,
        };
        let data = self
            .graphql(LIST_QUERY, json!({ "id": media_id, "user": user }), true)
            .await?;
        let entry = &data["MediaList"];
        if entry.is_null() {
            return Ok(None);
        }
        Ok(Some(ListEntry {
            id: entry["id"]
                .as_i64()
                .context("AniList list entry has no id")?,
            status: entry["status"].as_str().unwrap_or_default().to_owned(),
            progress: entry["progress"].as_i64().unwrap_or_default() as i32,
        }))
    }
    async fn save(&self, media_id: i64, status: &str, progress: Option<u32>) -> Result<()> {
        self.graphql(
            SAVE_MUTATION,
            json!({ "id": media_id, "status": status, "progress": progress }),
            true,
        )
        .await?;
        Ok(())
    }
    async fn delete(&self, list_id: i64) -> Result<()> {
        self.graphql(DELETE_MUTATION, json!({ "id": list_id }), true)
            .await?;
        Ok(())
    }
    /// Find the AniList entry matching a streaming title, if any.
    pub async fn lookup(&self, anime: &Anime) -> Result<Option<MediaEntry>> {
        if anime.title.trim().is_empty() {
            return Ok(None);
        }
        let results = self.search(anime.title.trim()).await?;
        Ok(match_media(anime, &results))
    }
    /// Mirror a local watchlist or progress change to AniList. Never
    /// downgrades the remote list: existing entries keep their status unless
    /// progress moves forward.
    pub async fn track(&self, anime: &Anime, event: TrackEvent) -> Result<String> {
        let entry = self
            .lookup(anime)
            .await?
            .with_context(|| format!("AniList: no confident match for “{}”", anime.title))?;
        match event {
            TrackEvent::Watchlist { added } => {
                let current = self.list_entry(entry.id).await?;
                match (&current, added) {
                    (None, true) => {
                        self.save(entry.id, "PLANNING", None).await?;
                        Ok(format!("AniList: “{}” added as Planning", anime.title))
                    }
                    (Some(list), true) => Ok(format!(
                        "AniList: “{}” already on your list ({})",
                        anime.title,
                        friendly_status(&list.status)
                    )),
                    (Some(list), false) if list.status == "PLANNING" => {
                        self.delete(list.id).await?;
                        Ok("AniList: removed from your list".into())
                    }
                    (Some(list), false) => Ok(format!(
                        "AniList: kept your entry ({}) — delete it on anilist.co if you want",
                        friendly_status(&list.status)
                    )),
                    (None, false) => Ok("AniList: nothing to remove".into()),
                }
            }
            TrackEvent::Watched { number, total } => {
                let progress: i32 = number.trim().parse().unwrap_or(0);
                if progress <= 0 {
                    bail!("AniList: cannot sync episode “{number}”");
                }
                let episodes = entry.episodes.unwrap_or(total).max(1);
                let done = progress >= episodes as i32;
                let current = self.list_entry(entry.id).await?;
                if let Some(list) = &current
                    && list.status == "COMPLETED"
                {
                    return Ok(format!("AniList: “{}” already completed", anime.title));
                }
                if let Some(list) = &current
                    && list.progress >= progress
                    && !done
                {
                    return Ok(format!("AniList: already at episode {}", list.progress));
                }
                let status = if done {
                    "COMPLETED"
                } else if current.as_ref().map(|l| l.status.as_str()) == Some("REPEATING") {
                    "REPEATING"
                } else {
                    "CURRENT"
                };
                self.save(entry.id, status, Some(progress as u32)).await?;
                Ok(if done {
                    format!("AniList: “{}” completed", anime.title)
                } else {
                    format!("AniList: “{}” · watching episode {progress}", anime.title)
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anime(title: &str, year: &str, kind: &str) -> Anime {
        Anime {
            provider: crate::model::ProviderId::AnimeUnity,
            id: "1".into(),
            title: title.into(),
            aliases: vec![],
            synopsis: String::new(),
            year: year.into(),
            kind: kind.into(),
            status: String::new(),
            score: String::new(),
            episodes: 0,
            dub: false,
            genres: vec![],
            page_url: String::new(),
            cover_url: String::new(),
        }
    }
    fn entry(id: i64, names: &[&str], year: u32, format: &str, episodes: u32) -> MediaEntry {
        MediaEntry {
            id,
            title: MediaTitle {
                romaji: Some(names.first().copied().unwrap_or_default().into()),
                english: names.get(1).copied().map(String::from),
                native: None,
            },
            synonyms: names.iter().skip(2).map(|s| s.to_string()).collect(),
            format: Some(format.into()),
            episodes: Some(episodes),
            start_date: Some(StartDate { year: Some(year) }),
            ..MediaEntry::default()
        }
    }
    #[test]
    fn matching_requires_name_overlap_then_prefers_year_and_format() {
        let local = anime("Frieren: Beyond Journey's End", "2023", "TV");
        let entries = vec![
            // Same title, wrong year, wrong format.
            entry(1, &["Frieren: Beyond Journey's End"], 1980, "MOVIE", 1),
            // Same title, same year, same format.
            entry(2, &["Frieren: Beyond Journey's End"], 2023, "TV", 28),
            // Contains the local title as a substring.
            entry(3, &["Frieren: Beyond Journey's End (ITA)"], 2023, "TV", 28),
            // Unrelated.
            entry(4, &["Sousou no Frieren II"], 2026, "TV", 12),
        ];
        let matched = match_media(&local, &entries).expect("a match");
        assert_eq!(matched.id, 2, "year and format agreement must win");
        // A sequel whose name merely contains the query still matches, but a
        // fully distinct name does not.
        assert!(match_media(&anime("Frieren 2", "2025", "TV"), &entries).is_none());
        assert!(match_media(&anime("Totally Different", "2023", "TV"), &entries).is_none());
    }
    #[test]
    fn matching_uses_local_aliases_and_italian_titles() {
        let mut local = anime("L'attacco dei giganti", "2023", "TV");
        local.aliases.push("Attack on Titan".into());
        let entries = vec![entry(
            9,
            &["Attack on Titan", "Shingeki no Kyojin"],
            2023,
            "TV",
            25,
        )];
        assert_eq!(match_media(&local, &entries).expect("alias match").id, 9);
    }
    #[test]
    fn token_store_roundtrip_and_missing_file() {
        let dir = tempfile::tempdir().expect("dir");
        assert!(Token::load(dir.path()).is_none());
        let token = Token {
            user: "viewer".into(),
            token: "secret".into(),
            stored: 42,
        };
        token.save(dir.path()).expect("save");
        assert_eq!(Token::load(dir.path()).expect("load").user, "viewer");
        // Corrupt file degrades to logged out, not an error.
        std::fs::write(Token::path(dir.path()), "{broken").expect("corrupt");
        assert!(Token::load(dir.path()).is_none());
    }
    #[test]
    fn authorize_url_uses_implicit_grant() {
        assert_eq!(
            authorize_url(1234),
            "https://anilist.co/api/v2/oauth/authorize?client_id=1234&response_type=token"
        );
    }
    #[test]
    fn display_helpers_format_scores_status_and_description() {
        let mut media = entry(5, &["Test"], 2020, "TV", 12);
        media.average_score = Some(86);
        media.status = Some("FINISHED".into());
        media.description = Some("<i>Great</i><br>show".into());
        assert_eq!(media.display_score(), "8.6");
        assert_eq!(media.display_status(), "Finished");
        assert_eq!(media.plain_description(), "Great show");
        assert_eq!(truncate("héllo", 700), "héllo");
        assert!(truncate(&"x".repeat(800), 700).ends_with('…'));
    }
    #[test]
    fn graphql_entries_parse_from_response_shape() {
        let raw = r#"{
            "data": { "Page": { "media": [
                { "id": 21, "title": { "romaji": "ONE PIECE", "english": "One Piece" },
                  "synonyms": [], "format": "TV", "status": "RELEASING", "episodes": null,
                  "averageScore": 87, "genres": ["Action"], "description": null,
                  "startDate": { "year": 1999 } }
            ] } }
        }"#;
        let value: Value = serde_json::from_str(raw).expect("fixture");
        let media = value["data"]["Page"]["media"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let parsed: Vec<MediaEntry> = media
            .into_iter()
            .filter_map(|m| serde_json::from_value(m).ok())
            .collect();
        let entry = parsed.first().expect("entry");
        assert_eq!(entry.id, 21);
        assert_eq!(entry.names().first().copied(), Some("ONE PIECE"));
        assert_eq!(entry.year(), Some(1999));
        assert_eq!(entry.display_status(), "Airing");
    }
}
