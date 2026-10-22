use crate::{
    anilist::MediaEntry,
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
    /// AniList metadata for a title; `None` means no confident match.
    Meta {
        key: String,
        result: Result<Option<MediaEntry>, String>,
    },
    /// Outcome of mirroring a watchlist/progress change to AniList.
    Tracked(Result<String, String>),
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
