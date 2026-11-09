use crate::playback::OfflineEpisode;
use anime_levisione_core::{
    audio::AudioTrack,
    model::{Anime, Episode, ProviderId},
    store::Progress,
    sync::{Account, Outcome},
};

pub enum Message {
    /// A provider rotated to a new domain mid-run; the UI refreshes the shown
    /// URL and the triggering request is retried on the live domain.
    DomainMoved {
        provider: ProviderId,
        url: String,
    },
    Synced(Result<Outcome, String>),
    Login(Result<Account, String>),
    Catalog {
        generation: u64,
        provider: ProviderId,
        result: Result<Vec<Anime>, String>,
    },
    Episodes {
        key: String,
        result: Result<(Anime, Vec<Episode>), String>,
    },
    Offline(Result<Vec<OfflineEpisode>, String>),
    ChooseAudio {
        request_id: u64,
        tracks: Vec<AudioTrack>,
        reply: tokio::sync::oneshot::Sender<Option<AudioTrack>>,
    },
    /// Same picker as `ChooseAudio`, but the reply drives the ffmpeg audio
    /// mapping of a download instead of a player.
    ChooseDownloadAudio {
        request_id: u64,
        tracks: Vec<AudioTrack>,
        reply: tokio::sync::oneshot::Sender<Option<AudioTrack>>,
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
    DownloadStarted {
        request_id: u64,
        path: String,
    },
    /// Periodic ffmpeg download telemetry: bytes on disk, an estimated total
    /// size, and the current write speed in bytes per second.
    DownloadProgress {
        request_id: u64,
        bytes: u64,
        total: Option<u64>,
        speed: Option<u64>,
    },
    DownloadEnded {
        request_id: u64,
        result: Result<String, String>,
    },
    /// Non-modal status text for background tasks (e.g. download fallbacks).
    Notice {
        text: String,
        error: bool,
    },
}
