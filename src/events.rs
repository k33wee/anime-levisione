use crate::{
    model::{Anime, Episode, ProviderId},
    store::Progress,
};

pub enum Message {
    /// A provider rotated to a new domain mid-run; the UI refreshes the shown
    /// URL and the triggering request is retried on the live domain.
    DomainMoved {
        provider: ProviderId,
        url: String,
    },
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
