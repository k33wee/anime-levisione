//! Live provider checks. These hit the real AnimeWorld/AnimeUnity sites, so they are
//! excluded from normal runs: `cargo test -- --ignored`.

use anime_levisione::{
    anilist,
    model::{Anime, Episode, ProviderId},
    provider::{Registry, bounded},
    store::Config,
};

async fn registry() -> Registry {
    let registry = Registry::new(&Config::default(), None).expect("providers");
    for id in ProviderId::ALL {
        registry.ready(id).await;
    }
    registry
}

async fn first_with_episodes(
    registry: &Registry,
    id: ProviderId,
    query: &str,
) -> (Anime, Vec<Episode>) {
    let results = bounded(registry.get(id).search(query))
        .await
        .expect("search");
    let mut errors = Vec::new();
    for anime in results {
        match bounded(registry.get(id).episodes(anime.clone())).await {
            Ok((anime, episodes)) if !episodes.is_empty() => return (anime, episodes),
            Ok(_) => errors.push(format!("{}: no episodes listed", anime.title)),
            Err(e) => errors.push(format!("{}: {e:#}", anime.title)),
        }
    }
    panic!(
        "no result for {query} yielded episodes: {}",
        errors.join("; ")
    );
}

#[tokio::test]
#[ignore = "hits the live AniList API"]
async fn anilist_lookup_enriches_streaming_titles() {
    let dir = tempfile::tempdir().expect("dir");
    let client = anilist::Client::open(dir.path()).expect("client");
    let results = client.search("frieren").await.expect("search");
    assert!(!results.is_empty(), "AniList returned nothing");
    let anime = Anime {
        provider: ProviderId::AnimeUnity,
        id: "4851".into(),
        title: "Frieren: Beyond Journey's End".into(),
        aliases: vec![],
        synopsis: String::new(),
        year: "2023".into(),
        kind: "TV".into(),
        status: String::new(),
        score: String::new(),
        episodes: 28,
        dub: false,
        genres: vec![],
        page_url: String::new(),
    };
    let matched = anilist::match_media(&anime, &results).expect("a confident match");
    assert_eq!(matched.year(), Some(2023));
    assert!(!matched.display_score().is_empty());
    // The full pipeline: streaming title in, metadata out.
    let found = client.lookup(&anime).await.expect("lookup");
    assert!(found.is_some(), "lookup matched nothing");
}

#[tokio::test]
#[ignore = "hits live providers"]
async fn streamingcommunity_search_lists_episodes_and_resolves_a_stream() {
    let registry = registry().await;
    let results = bounded(
        registry
            .get(ProviderId::StreamingCommunity)
            .search("one piece"),
    )
    .await
    .expect("search");
    assert!(!results.is_empty(), "no search results");
    let anime = results.first().expect("a search result").clone();
    let (anime, episodes) = bounded(registry.get(ProviderId::StreamingCommunity).episodes(anime))
        .await
        .expect("episodes");
    assert!(!episodes.is_empty(), "no episodes listed");
    let stream = bounded(
        registry
            .get(ProviderId::StreamingCommunity)
            .resolve(&anime, &episodes[0]),
    )
    .await
    .expect("resolve");
    assert!(!stream.url.host_str().unwrap_or_default().is_empty());
    // Signed VixCloud playlists must carry their token parameters.
    if stream
        .url
        .host_str()
        .is_some_and(|h| h.contains("vixcloud"))
    {
        assert!(
            stream.url.query_pairs().any(|(k, _)| k == "token"),
            "missing signed token"
        );
    }
    let client = reqwest::Client::builder()
        .user_agent(anime_levisione::provider::http::USER_AGENT)
        .build()
        .expect("test client");
    let response = client
        .head(stream.url.clone())
        .header("referer", &stream.referer)
        .send()
        .await
        .expect("HEAD request");
    assert!(
        response.status().as_u16() < 400,
        "source returned {}",
        response.status()
    );
}

#[tokio::test]
#[ignore = "hits live providers"]
async fn all_providers_resolve_a_live_domain() {
    let registry = Registry::new(&Config::default(), None).expect("providers");
    for id in ProviderId::ALL {
        registry.ready(id).await;
        let resolution = registry.resolution(id);
        let live = resolution
            .live
            .as_ref()
            .unwrap_or_else(|| panic!("{id} found no live domain: {:?}", resolution.error));
        let host = live.host_str().unwrap_or_default();
        assert!(
            host.contains("anime") || host.contains("streamingcommunity"),
            "unexpected {id} host {host}"
        );
        assert!(
            resolution.provider.is_some(),
            "{id} probe did not bootstrap a session"
        );
    }
}

#[tokio::test]
#[ignore = "hits live providers"]
async fn animeunity_search_lists_episodes_and_resolves_a_stream() {
    let registry = registry().await;
    let (anime, episodes) = first_with_episodes(&registry, ProviderId::AnimeUnity, "frieren").await;
    let stream = bounded(
        registry
            .get(ProviderId::AnimeUnity)
            .resolve(&anime, &episodes[0]),
    )
    .await
    .expect("resolve");
    assert!(!stream.url.host_str().unwrap_or_default().is_empty());
    // Signed VixCloud playlists must carry their token parameters.
    if stream
        .url
        .host_str()
        .is_some_and(|h| h.contains("vixcloud"))
    {
        assert!(
            stream.url.query_pairs().any(|(k, _)| k == "token"),
            "missing signed token"
        );
    }
}

#[tokio::test]
#[ignore = "hits live providers"]
async fn animeworld_search_lists_episodes_and_resolves_a_playable_file() {
    let registry = registry().await;
    let (anime, episodes) = first_with_episodes(&registry, ProviderId::AnimeWorld, "frieren").await;
    let stream = bounded(
        registry
            .get(ProviderId::AnimeWorld)
            .resolve(&anime, &episodes[0]),
    )
    .await
    .expect("resolve");
    let client = reqwest::Client::builder()
        .user_agent(anime_levisione::provider::http::USER_AGENT)
        .build()
        .expect("test client");
    let response = client
        .head(stream.url.clone())
        .header("referer", &stream.referer)
        .send()
        .await
        .expect("HEAD request");
    assert!(
        response.status().as_u16() < 400,
        "source returned {}",
        response.status()
    );
}

#[tokio::test]
#[ignore = "hits live providers"]
async fn search_and_latest_are_non_empty_for_both_providers() {
    let registry = registry().await;
    for id in ProviderId::ALL {
        let latest = bounded(registry.get(id).latest()).await.expect("latest");
        assert!(!latest.is_empty(), "{id} latest is empty");
        let search = bounded(registry.get(id).search("naruto"))
            .await
            .expect("search");
        assert!(!search.is_empty(), "{id} search is empty");
    }
}
