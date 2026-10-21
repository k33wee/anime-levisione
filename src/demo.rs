use crate::{
    app::{App, SourceStatus},
    model::{Anime, Episode, ProviderId},
    store::Progress,
};

/// Explicit offline UI preview; no provider/network requests or persistent writes.
pub fn populate(app: &mut App) {
    let titles = [
        (
            "Sousou no Frieren",
            "Frieren: Beyond Journey's End",
            "2023",
            "9.3",
            "An elven mage retraces the journey she once made with a party of heroes. With a new generation of companions, she learns what their brief time together meant.",
            28,
        ),
        (
            "Cowboy Bebop",
            "Cowboy Bebop",
            "1998",
            "8.8",
            "Aboard the Bebop, a mismatched crew of bounty hunters chases dangerous criminals across the solar system, while the past refuses to stay behind.",
            26,
        ),
        (
            "Steins;Gate",
            "Steins Gate",
            "2011",
            "9.1",
            "A self-proclaimed mad scientist and his friends discover a way to send messages into the past. Every experiment changes more than they expect.",
            24,
        ),
        (
            "Dungeon Meshi",
            "Delicious in Dungeon",
            "2024",
            "8.6",
            "To rescue a friend deep inside a dungeon, a party of adventurers must survive on an unusual source of food: the monsters they meet.",
            24,
        ),
        (
            "Shingeki no Kyojin",
            "Attack on Titan",
            "2013",
            "8.5",
            "Humanity shelters behind enormous walls. When the walls are breached, a young recruit sets out to discover the truth beyond them.",
            25,
        ),
        (
            "One Piece",
            "One Piece",
            "1999",
            "8.7",
            "Monkey D. Luffy and his crew sail the Grand Line in search of the legendary treasure known as the One Piece.",
            1100,
        ),
        (
            "Violet Evergarden",
            "Violet Evergarden",
            "2018",
            "8.7",
            "A former soldier becomes a letter writer, learning to understand the emotions she could never put into words.",
            13,
        ),
        (
            "Mob Psycho 100",
            "Mob Psycho 100",
            "2016",
            "8.5",
            "A quiet student with extraordinary psychic powers would rather make friends than use his abilities.",
            12,
        ),
    ];
    let mut catalog = Vec::new();
    for (i, (title, alias, year, score, synopsis, count)) in titles.iter().enumerate() {
        let anime = Anime {
            provider: ProviderId::ALL[i % 2],
            id: format!("demo-{i}"),
            title: (*title).into(),
            aliases: vec![(*alias).into()],
            synopsis: (*synopsis).into(),
            year: (*year).into(),
            kind: "TV".into(),
            status: "Finished".into(),
            score: (*score).into(),
            episodes: *count,
            dub: i % 3 == 1,
            genres: vec!["Adventure".into(), "Drama".into()],
            page_url: String::new(),
        };
        let episodes = (1..=*count)
            .map(|n| Episode {
                id: format!("demo-{i}-{n}"),
                number: n.to_string(),
                title: String::new(),
            })
            .collect();
        app.details.insert(anime.key(), (anime.clone(), episodes));
        catalog.push(anime);
    }
    app.library.watchlist = vec![catalog[0].clone(), catalog[3].clone(), catalog[6].clone()];
    app.library.record(
        catalog[1].clone(),
        Progress {
            episode: Episode {
                id: "demo-1-5".into(),
                number: "5".into(),
                title: "Ballad of Fallen Angels".into(),
            },
            seconds: 462.0,
            duration: 1440.0,
            completed: false,
        },
    );
    app.library.record(
        catalog[2].clone(),
        Progress {
            episode: Episode {
                id: "demo-2-3".into(),
                number: "3".into(),
                title: String::new(),
            },
            seconds: 1400.0,
            duration: 1440.0,
            completed: true,
        },
    );
    app.discovered = catalog;
    app.statuses = ProviderId::ALL
        .into_iter()
        .map(|provider| SourceStatus {
            provider,
            pending: false,
            count: app
                .discovered
                .iter()
                .filter(|a| a.provider == provider)
                .count(),
            error: None,
        })
        .collect();
    app.rebuild();
    app.notify(
        "Offline demo · / search · Tab switch pane · Enter episodes · ? help",
        false,
    );
}
