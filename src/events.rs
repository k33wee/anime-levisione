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
    Offline(Result<Vec<crate::playback::OfflineEpisode>, String>),
    ChooseAudio {
        request_id: u64,
        tracks: Vec<crate::audio::AudioTrack>,
        reply: tokio::sync::oneshot::Sender<Option<crate::audio::AudioTrack>>,
    },
    PlaybackStarted {
        request_id: u64,
        audio: Option<String>,
        warning: Option<String>,
    },
    Progress {
        anime: Anime,
        progress: Progress,
    },
    PlaybackEnded {
        request_id: u64,
        result: Result<(), String>,
    },
    DownloadStarted,
    DownloadEnded(Result<String, String>),
}
