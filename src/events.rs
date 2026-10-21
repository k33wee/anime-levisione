use crate::{
    model::{Anime, Episode, ProviderId},
    store::Progress,
};

pub enum Message {
    Catalog {
        generation: u64,
        provider: ProviderId,
        result: Result<Vec<Anime>, String>,
    },
    Episodes {
        key: String,
        result: Result<(Anime, Vec<Episode>), String>,
    },
    PlaybackStarted,
    Progress {
        anime: Anime,
        progress: Progress,
    },
    PlaybackEnded(Result<(), String>),
    DownloadStarted,
    DownloadEnded(Result<String, String>),
}
