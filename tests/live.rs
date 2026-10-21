//! Live provider checks. These hit the real AnimeWorld/AnimeUnity sites, so they are
//! excluded from normal runs: `cargo test -- --ignored`.

use anime_levisione::{
    model::{Anime, ProviderId},
    provider::{Registry, bounded},
    store::Config,
};

fn registry() -> Registry {
    Registry::new(&Config::default()).expect("providers")
}

async fn first_with_episodes(
    registry: &Registry,
    id: ProviderId,
    query: &str,
) -> (Anime, Vec<anime_levisione::model::Episode>) {
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
#[ignore = "hits live providers"]
async fn animeunity_search_lists_episodes_and_resolves_a_stream() {
    let registry = registry();
    let (_anime, episodes) =
        first_with_episodes(&registry, ProviderId::AnimeUnity, "frieren").await;
    let stream = bounded(registry.get(ProviderId::AnimeUnity).resolve(&episodes[0]))
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
    let registry = registry();
    let (_anime, episodes) =
        first_with_episodes(&registry, ProviderId::AnimeWorld, "frieren").await;
    let stream = bounded(registry.get(ProviderId::AnimeWorld).resolve(&episodes[0]))
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
    let registry = registry();
    for id in ProviderId::ALL {
        let latest = bounded(registry.get(id).latest()).await.expect("latest");
        assert!(!latest.is_empty(), "{id} latest is empty");
        let search = bounded(registry.get(id).search("naruto"))
            .await
            .expect("search");
        assert!(!search.is_empty(), "{id} search is empty");
    }
}
