//! Live checks for the uniffi facade. These hit the real provider sites, so
//! they are excluded from normal runs: `cargo test -p anime-levisione-core
//! --features uniffi --test api -- --ignored`.

#![cfg(feature = "uniffi")]

use anime_levisione_core::api::AnimeLib;

#[ignore = "hits live providers"]
#[test]
fn facade_searches_lists_episodes_and_resolves_a_stream() {
    let lib = AnimeLib::new().expect("library");
    let results = lib.search("frieren".into()).expect("search");
    assert!(!results.is_empty(), "at least one provider must answer");
    let anime = &results[0];
    let episodes = lib.episodes(anime).expect("episodes");
    assert!(!episodes.is_empty());
    let url = lib.stream_url(anime, &episodes[0]).expect("stream url");
    assert!(url.starts_with("http"), "got {url}");
}
