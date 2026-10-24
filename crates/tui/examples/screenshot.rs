//! Renders README screenshots of the TUI from the synthetic screenshot
//! catalogue (docs/screenshots/fixtures/catalog.json) and writes them as SVG.
//! tools/capture-screenshots.sh converts the SVG to PNG; no terminal needed.
//!
//!     cargo run -p anime-levisione --example screenshot -- browse
//!
//! Not shipped: examples are excluded from release builds and cargo install.

use anime_levisione::app::{App, SourceStatus};
use anime_levisione_core::model::ProviderId;
use anime_levisione_core::store::{Config, Library, Progress};
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use serde_json::Value;
use std::path::PathBuf;

const WIDTH: u16 = 160;
const HEIGHT: u16 = 50;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots/fixtures")
}

/// Build the app from the committed synthetic catalogue; no demo::populate, so
/// no real-world title can appear in the screenshots.
fn synthetic_app() -> App {
    let catalog: Vec<Value> = serde_json::from_reader(
        std::fs::File::open(fixture_dir().join("catalog.json")).expect("catalog.json fixture"),
    )
    .expect("valid catalog fixture");
    let providers = [
        ProviderId::AnimeWorld,
        ProviderId::AnimeUnity,
        ProviderId::StreamingCommunity,
    ];
    let mut app = App::new(
        Config::default(),
        Library::default(),
        "/tmp/anime-levisione-screenshot".into(),
        true,
    );
    let mut entries = Vec::new();
    for (i, row) in catalog.iter().enumerate() {
        let count = row["episodes"].as_u64().unwrap_or(8) as u32;
        let anime = anime_levisione_core::model::Anime {
            provider: providers[i % providers.len()],
            id: row["id"].as_str().unwrap_or_default().to_string(),
            title: row["title"].as_str().unwrap_or_default().to_string(),
            aliases: vec![],
            synopsis: row["synopsis"].as_str().unwrap_or_default().to_string(),
            year: row["year"].as_str().unwrap_or_default().to_string(),
            kind: "TV".to_string(),
            status: if i % 3 == 0 { "Finished" } else { "Airing" }.to_string(),
            score: row["score"].as_str().unwrap_or_default().to_string(),
            episodes: count,
            dub: row["dub"].as_bool().unwrap_or(false),
            genres: vec!["Adventure".to_string()],
            page_url: String::new(),
            cover_url: String::new(),
        };
        let episodes = (1..=count)
            .map(|n| anime_levisione_core::model::Episode {
                id: format!("{}-{n}", anime.id),
                number: n.to_string(),
                title: format!("Episode {n}"),
            })
            .collect::<Vec<_>>();
        app.details
            .insert(anime.key(), (anime.clone(), episodes.clone()));
        entries.push((anime, episodes));
    }
    app.library.watchlist = vec![entries[0].0.clone(), entries[5].0.clone()];
    app.library.record(
        entries[1].0.clone(),
        Progress {
            episode: entries[1].1[4].clone(),
            seconds: 462.0,
            duration: 1440.0,
            completed: false,
        },
    );
    app.discovered = entries.iter().map(|(anime, _)| anime.clone()).collect();
    app.searched = app.discovered.clone();
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
    app.titles.select(Some(0));
    app.rebuild();
    app.notify("Offline demo catalogue · no provider data", false);
    app
}

fn render(app: &mut App) -> Buffer {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(WIDTH, HEIGHT)).expect("backend");
    terminal
        .draw(|f| anime_levisione::ui::draw(f, app))
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn svg_color(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Reset => "#0b0e14".into(),
        Color::Black => "#0d0d0d".into(),
        Color::Red => "#cd3131".into(),
        Color::Green => "#0dbc79".into(),
        Color::Yellow => "#e5e510".into(),
        Color::Blue => "#2472c8".into(),
        Color::Magenta => "#bc3fbc".into(),
        Color::Cyan => "#11a8cd".into(),
        Color::Gray => "#e5e5e5".into(),
        Color::DarkGray => "#616161".into(),
        Color::LightRed => "#f14c4c".into(),
        Color::LightGreen => "#23d18b".into(),
        Color::LightYellow => "#f5f543".into(),
        Color::LightBlue => "#3b8eea".into(),
        Color::LightMagenta => "#d670d6".into(),
        Color::LightCyan => "#29b8db".into(),
        Color::White => "#e5e5e5".into(),
        _ => "#e5e5e5".into(),
    }
}

fn buffer_to_svg(buffer: &Buffer, name: &str) {
    let cell = 9.0;
    let row = 16.0;
    let width = f32::from(WIDTH) * cell + 24.0;
    let height = f32::from(HEIGHT) * row + 24.0;
    let mut body = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let cell_content = &buffer[(x, y)];
            let background = cell_content.bg;
            let symbol = cell_content.symbol();
            let blank = symbol.chars().all(char::is_whitespace);
            if blank && matches!(background, Color::Reset) {
                continue;
            }
            let px = f32::from(x) * cell + 12.0;
            let py = f32::from(y) * row + 12.0;
            if !matches!(background, Color::Reset) {
                body.push_str(&format!(
                    r#"<rect x="{px}" y="{py}" width="{cell}" height="{row}" fill="{}"/>"#,
                    svg_color(background),
                ));
            }
            if !blank {
                let bold = if cell_content.modifier.contains(Modifier::BOLD) {
                    " font-weight=\"bold\""
                } else {
                    ""
                };
                let escaped = symbol
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;");
                body.push_str(&format!(
                    r#"<text x="{:.1}" y="{:.1}" font-family="DejaVu Sans Mono" font-size="13"{bold} fill="{}">{escaped}</text>"#,
                    px,
                    py + 12.5,
                    if cell_content.fg == Color::Reset { "#dce4eb".into() } else { svg_color(cell_content.fg) },
                ));
            }
        }
    }
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"><rect width="{width}" height="{height}" fill="#0b0e14"/>{body}</svg>"##
    );
    let output = std::env::var_os("ANIME_SCREENSHOT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/doc-screenshots")
        });
    std::fs::create_dir_all(&output).expect("screenshot output directory");
    let path = output.join(format!("tui-{name}.svg"));
    std::fs::write(&path, svg).expect("write svg");
    println!("{}", path.display());
}

fn main() {
    let requested = std::env::args().nth(1).unwrap_or_else(|| "browse".into());
    if requested == "audio" {
        let mut app = synthetic_app();
        app.playing = true;
        app.playback_label = "Orbit Garden · Episode 1".into();
        let tracks = anime_levisione_core::audio::hls_tracks(
            "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,NAME=\"Italian\",LANGUAGE=\"ita\",DEFAULT=YES\n#EXT-X-MEDIA:TYPE=AUDIO,NAME=\"English\",LANGUAGE=\"eng\"",
        ).expect("synthetic language metadata");
        let (reply, _receive) = tokio::sync::oneshot::channel();
        app.message(anime_levisione::events::Message::ChooseAudio {
            request_id: app.playback_id,
            tracks,
            reply,
        });
        buffer_to_svg(&render(&mut app), &requested);
        return;
    }
    if requested != "browse" {
        eprintln!("unknown shot {requested}; available: browse, audio");
        std::process::exit(2);
    }
    let mut app = synthetic_app();
    let buffer = render(&mut app);
    buffer_to_svg(&buffer, &requested);
}
