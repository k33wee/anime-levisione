use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, fmt};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    AnimeWorld,
    AnimeUnity,
}
impl ProviderId {
    pub const ALL: [Self; 2] = [Self::AnimeWorld, Self::AnimeUnity];
    pub fn short(self) -> &'static str {
        match self {
            Self::AnimeWorld => "AW",
            Self::AnimeUnity => "AU",
        }
    }
}
impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AnimeWorld => "AnimeWorld",
            Self::AnimeUnity => "AnimeUnity",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Anime {
    pub provider: ProviderId,
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub synopsis: String,
    #[serde(default)]
    pub year: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub score: String,
    #[serde(default)]
    pub episodes: u32,
    #[serde(default)]
    pub dub: bool,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub page_url: String,
}
impl Anime {
    pub fn key(&self) -> String {
        format!("{}:{}", self.provider.short(), self.id)
    }
    pub fn audio(&self) -> &'static str {
        if self.dub { "ITA DUB" } else { "SUB ITA" }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Episode {
    pub id: String,
    pub number: String,
    #[serde(default)]
    pub title: String,
}
impl Episode {
    pub fn label(&self) -> String {
        if self.title.is_empty() {
            format!("Episode {}", self.number)
        } else {
            format!("{} · {}", self.number, self.title)
        }
    }
}
pub fn sort_episodes(episodes: &mut Vec<Episode>) {
    episodes.sort_by(
        |a, b| match (a.number.parse::<f64>(), b.number.parse::<f64>()) {
            (Ok(a), Ok(b)) => a.total_cmp(&b),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            _ => a.number.cmp(&b.number),
        },
    );
    // Dedup on id over the whole list: a repeated id can land anywhere once sorted,
    // not only next to its twin.
    let mut seen = std::collections::HashSet::new();
    episodes.retain(|e| seen.insert(e.id.clone()));
}

/// Plain text only: provider strings must never inject terminal controls.
pub fn clean(value: &str) -> String {
    html_escape::decode_html_entities(value)
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn normalize(value: &str) -> String {
    value
        .nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn rank(anime: &Anime, query: &str) -> u32 {
    let query = normalize(query);
    if query.is_empty() {
        return 0;
    }
    std::iter::once(&anime.title)
        .chain(anime.aliases.iter())
        .map(|name| {
            let name = normalize(name);
            if name == query {
                10_000
            } else if name.starts_with(&query) {
                8_000
            } else if name.contains(&query) {
                6_000
            } else {
                let tokens = query
                    .split_whitespace()
                    .filter(|q| name.contains(q))
                    .count() as u32;
                tokens * 1000 + (strsim::jaro_winkler(&name, &query) * 900.0) as u32
            }
        })
        .max()
        .unwrap_or(0)
}
/// Relevance sort that ranks each title exactly once instead of recomputing (and
/// re-normalizing) inside every comparator call. Ties break by title, then provider.
pub fn sort_by_rank(animes: &mut Vec<Anime>, query: &str) {
    let mut decorated: Vec<(u32, Anime)> = animes.drain(..).map(|a| (rank(&a, query), a)).collect();
    decorated.sort_by(|(ra, a), (rb, b)| {
        rb.cmp(ra)
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.provider.short().cmp(b.provider.short()))
    });
    animes.extend(decorated.into_iter().map(|(_, a)| a));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_normalization() {
        assert_eq!(normalize("  Pokémon: ÉVOLUTIONS！ "), "pokemon evolutions");
        assert_eq!(clean("a\x1b[31m\r\nb &amp; c"), "a[31m b & c");
    }
    #[test]
    fn fractional_episodes_sort_numerically() {
        let mut eps = ["10", "1.5", "2", "Special", "1"]
            .map(|n| Episode {
                id: n.into(),
                number: n.into(),
                title: String::new(),
            })
            .to_vec();
        sort_episodes(&mut eps);
        assert_eq!(
            eps.iter().map(|e| e.number.as_str()).collect::<Vec<_>>(),
            ["1", "1.5", "2", "10", "Special"]
        );
    }
    #[test]
    fn duplicate_episode_ids_are_removed_even_when_not_adjacent() {
        let mut eps = [("a", "5"), ("b", "4"), ("a", "3")]
            .map(|(id, number)| Episode {
                id: id.into(),
                number: number.into(),
                title: String::new(),
            })
            .to_vec();
        sort_episodes(&mut eps);
        assert_eq!(
            eps.iter().map(|e| e.number.as_str()).collect::<Vec<_>>(),
            ["3", "4"]
        );
    }
    #[test]
    fn rank_sort_prefers_exact_then_prefix_then_substring() {
        fn anime(title: &str) -> Anime {
            Anime {
                provider: ProviderId::AnimeUnity,
                id: title.into(),
                title: title.into(),
                aliases: vec![],
                synopsis: String::new(),
                year: String::new(),
                kind: String::new(),
                status: String::new(),
                score: String::new(),
                episodes: 0,
                dub: false,
                genres: vec![],
                page_url: String::new(),
            }
        }
        let mut animes = [
            "Boruto: Naruto Next Generations",
            "Naruto Shippuden",
            "Naruto",
        ]
        .map(anime)
        .to_vec();
        sort_by_rank(&mut animes, "naruto");
        assert_eq!(
            animes.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(),
            [
                "Naruto",
                "Naruto Shippuden",
                "Boruto: Naruto Next Generations"
            ]
        );
    }
}
