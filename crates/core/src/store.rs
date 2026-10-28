use crate::model::{Anime, Episode};
use anyhow::{Context, Result, bail};
use fs4::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Executable, not a shell command. Use player_args for arguments.
    pub player: String,
    pub player_args: Vec<String>,
    pub download_dir: PathBuf,
    pub animeworld_url: String,
    /// Extra AnimeWorld mirror domains probed when the configured URL stops
    /// answering. Full `https://host` URLs.
    #[serde(default)]
    pub animeworld_domains: Vec<String>,
    pub animeunity_url: String,
    /// Extra AnimeUnity mirror domains probed when the configured URL stops
    /// answering. Full `https://host` URLs.
    #[serde(default)]
    pub animeunity_domains: Vec<String>,
    pub streamingcommunity_url: String,
    /// Extra StreamingCommunity mirror domains probed when the configured URL
    /// stops answering. Full `https://host` URLs.
    #[serde(default)]
    pub streamingcommunity_domains: Vec<String>,
    /// AniList OAuth client id from https://anilist.co/settings/developer.
    /// 0 (unset) disables the guided login; metadata still works anonymously.
    #[serde(default)]
    pub anilist_client_id: u32,
    pub private: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            player: default_player(),
            player_args: vec![],
            download_dir: directories::UserDirs::new()
                .and_then(|d| d.video_dir().map(Path::to_path_buf))
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Anime"),
            animeworld_url: "https://www.animeworld.ac".into(),
            animeworld_domains: vec![],
            animeunity_url: "https://www.animeunity.to".into(),
            animeunity_domains: vec![],
            streamingcommunity_url: "https://streamingcommunityz.day".into(),
            streamingcommunity_domains: vec![],
            anilist_client_id: 0,
            private: false,
        }
    }
}
impl Config {
    /// Older releases persisted the bare `mpv` fallback even when no player was
    /// installed. Retry auto-detection so installing VLC later repairs that config.
    pub fn refresh_default_player(&mut self) -> bool {
        if !self.player.eq_ignore_ascii_case("mpv")
            || resolve_executable(Path::new(&self.player)).is_some()
        {
            return false;
        }
        let Some(player) = detect_player() else {
            return false;
        };
        self.player = player;
        true
    }
}
/// VLC is the default when it can be found (bundle path first on macOS); mpv is the
/// fallback. A valid player already stored in config.toml always wins.
fn default_player() -> String {
    detect_player().unwrap_or_else(|| "mpv".into())
}
fn detect_player() -> Option<String> {
    first_available(player_candidates())
}
#[cfg(target_os = "macos")]
fn player_candidates() -> Vec<PathBuf> {
    [
        "/Applications/VLC.app/Contents/MacOS/VLC",
        "/opt/homebrew/bin/vlc",
        "/usr/local/bin/vlc",
        "vlc",
        "/opt/homebrew/bin/mpv",
        "mpv",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}
#[cfg(target_os = "windows")]
fn player_candidates() -> Vec<PathBuf> {
    let mut vlc = Vec::new();
    let mut mpv = Vec::new();
    if let Some(directory) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
    {
        vlc.push(directory.join("vlc.exe"));
        mpv.push(directory.join("mpv.exe"));
    }
    for variable in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(directory) = std::env::var_os(variable).filter(|value| !value.is_empty()) {
            let directory = PathBuf::from(directory);
            vlc.push(directory.join(r"VideoLAN\VLC\vlc.exe"));
            mpv.push(directory.join(r"mpv\mpv.exe"));
        }
    }
    if let Some(directory) = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        let directory = PathBuf::from(directory).join("Programs");
        vlc.push(directory.join(r"VideoLAN\VLC\vlc.exe"));
        mpv.push(directory.join(r"mpv\mpv.exe"));
    }
    vlc.extend([PathBuf::from("vlc.exe"), PathBuf::from("vlc")]);
    mpv.extend([PathBuf::from("mpv.exe"), PathBuf::from("mpv")]);
    vlc.extend(mpv);
    vlc
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn player_candidates() -> Vec<PathBuf> {
    ["vlc", "mpv"].into_iter().map(PathBuf::from).collect()
}
fn first_available<I, P>(candidates: I) -> Option<String>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    candidates.into_iter().find_map(|candidate| {
        resolve_executable(candidate.as_ref())?
            .into_os_string()
            .into_string()
            .ok()
    })
}
fn resolve_executable(candidate: &Path) -> Option<PathBuf> {
    if candidate.components().count() == 1 {
        return find_in_path(candidate);
    }
    candidate.is_file().then(|| candidate.to_path_buf())
}
fn find_in_path(name: &Path) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    find_in_directories(name, std::env::split_paths(&path))
}
fn find_in_directories(
    name: &Path,
    directories: impl IntoIterator<Item = PathBuf>,
) -> Option<PathBuf> {
    let names = executable_names(name);
    directories.into_iter().find_map(|directory| {
        names
            .iter()
            .map(|name| directory.join(name))
            .find(|candidate| candidate.is_file())
    })
}
#[cfg(target_os = "windows")]
fn executable_names(name: &Path) -> Vec<PathBuf> {
    let mut names = vec![name.to_path_buf()];
    if name.extension().is_none() {
        let mut executable = name.as_os_str().to_owned();
        executable.push(".exe");
        names.insert(0, PathBuf::from(executable));
    }
    names
}
#[cfg(not(target_os = "windows"))]
fn executable_names(name: &Path) -> [PathBuf; 1] {
    [name.to_path_buf()]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    pub episode: Episode,
    pub seconds: f64,
    pub duration: f64,
    pub completed: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub anime: Anime,
    pub last_episode: String,
    pub updated: u64,
    pub progress: BTreeMap<String, Progress>,
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    pub version: u32,
    pub watchlist: Vec<Anime>,
    pub history: Vec<HistoryEntry>,
    pub recent_searches: Vec<String>,
}
impl Library {
    pub fn bookmarked(&self, anime: &Anime) -> bool {
        self.watchlist.iter().any(|a| a.key() == anime.key())
    }
    pub fn toggle_bookmark(&mut self, anime: Anime) {
        if let Some(i) = self.watchlist.iter().position(|a| a.key() == anime.key()) {
            self.watchlist.remove(i);
        } else {
            self.watchlist.insert(0, anime);
        }
    }
    pub fn progress(&self, anime: &Anime, episode: &Episode) -> Option<&Progress> {
        self.history
            .iter()
            .find(|h| h.anime.key() == anime.key())?
            .progress
            .get(&episode.number)
    }
    pub fn record(&mut self, anime: Anime, mut progress: Progress) {
        if !progress.seconds.is_finite() || progress.seconds < 0.0 {
            progress.seconds = 0.0;
        }
        if !progress.duration.is_finite() || progress.duration < 0.0 {
            progress.duration = 0.0;
        }
        let key = anime.key();
        let mut entry = self
            .history
            .iter()
            .position(|h| h.anime.key() == key)
            .map(|i| self.history.remove(i))
            .unwrap_or_else(|| HistoryEntry {
                anime: anime.clone(),
                last_episode: String::new(),
                updated: 0,
                progress: BTreeMap::new(),
            });
        entry.anime = anime;
        entry.last_episode = progress.episode.number.clone();
        entry.updated = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        entry
            .progress
            .insert(progress.episode.number.clone(), progress);
        self.history.insert(0, entry);
    }
    pub fn remember_search(&mut self, query: &str) {
        if query.trim().is_empty() {
            return;
        }
        self.recent_searches.retain(|q| q != query);
        self.recent_searches.insert(0, query.to_owned());
        self.recent_searches.truncate(12);
    }
}

pub struct Store {
    pub dir: PathBuf,
    pub config: Config,
    pub library: Library,
    _lock: Option<File>,
}
impl Store {
    pub fn open(dir: Option<PathBuf>) -> Result<Self> {
        let (dir, config, library) = Self::load(dir)?;
        fs::create_dir_all(&dir).with_context(|| format!("Cannot create {}", dir.display()))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join(".lock"))?;
        FileExt::try_lock(&lock).context(
            "Another anime-levisione instance is using this library; use a different --data-dir",
        )?;
        Ok(Self {
            dir,
            config,
            library,
            _lock: Some(lock),
        })
    }
    /// Read-only view: same config and library as `open`, but no lock is
    /// taken, so one-shot commands like AniList `sync` work even while the
    /// TUI is running. `save` refuses rather than racing the TUI.
    pub fn open_readonly(dir: Option<PathBuf>) -> Result<Self> {
        let (dir, config, library) = Self::load(dir)?;
        Ok(Self {
            dir,
            config,
            library,
            _lock: None,
        })
    }
    fn load(dir: Option<PathBuf>) -> Result<(PathBuf, Config, Library)> {
        let dir = dir
            .or_else(|| {
                directories::ProjectDirs::from("dev", "keewee", "anime-levisione")
                    .map(|d| d.config_dir().to_path_buf())
            })
            .context("Cannot locate configuration directory; pass --data-dir")?;
        fs::create_dir_all(&dir).with_context(|| format!("Cannot create {}", dir.display()))?;
        let config_path = dir.join("config.toml");
        let config = match fs::read_to_string(&config_path) {
            Ok(text) => toml::from_str(&text).with_context(|| {
                format!(
                    "Invalid {}; fix it before starting (file was not changed)",
                    config_path.display()
                )
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let config = Config::default();
                atomic_write(&config_path, toml::to_string_pretty(&config)?.as_bytes())?;
                config
            }
            Err(e) => return Err(e.into()),
        };
        let library_path = dir.join("library.json");
        let library: Library = match fs::read(&library_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| {
                format!(
                    "Invalid {}; preserved without overwriting",
                    library_path.display()
                )
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Library::default(),
            Err(e) => return Err(e.into()),
        };
        if library.version > 1 {
            bail!("This library was written by a newer anime-levisione; update before opening it");
        }
        Ok((dir, config, library))
    }
    pub fn save(&self, library: &Library) -> Result<()> {
        if self._lock.is_none() {
            bail!("This store is read-only; the TUI owns the library right now");
        }
        let mut library = library.clone();
        library.version = 1;
        atomic_write(
            &self.dir.join("library.json"),
            &serde_json::to_vec_pretty(&library)?,
        )
    }
    /// Persist config.toml (e.g. a client id entered in the TUI login form).
    pub fn save_config(&self, config: &Config) -> Result<()> {
        if self._lock.is_none() {
            bail!("This store is read-only; the TUI owns the library right now");
        }
        atomic_write(
            &self.dir.join("config.toml"),
            toml::to_string_pretty(config)?.as_bytes(),
        )
    }
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("File must have a parent directory")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path)
        .with_context(|| format!("Cannot atomically save {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn store_roundtrip_and_exclusive_lock() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(Some(dir.path().into()))?;
        assert!(Store::open(Some(dir.path().into())).is_err());
        let mut lib = Library::default();
        lib.remember_search("Naruto");
        lib.remember_search("Naruto");
        store.save(&lib)?;
        drop(store);
        let store = Store::open(Some(dir.path().into()))?;
        assert_eq!(store.library.recent_searches, ["Naruto"]);
        Ok(())
    }
    #[test]
    fn player_detection_prefers_vlc_then_falls_back() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let vlc = dir.path().join("VLC");
        std::fs::write(&vlc, "")?;
        let mpv = dir.path().join("mpv");
        std::fs::write(&mpv, "")?;
        let candidates = [vlc.as_path(), Path::new("/nonexistent/vlc"), mpv.as_path()];
        assert_eq!(
            first_available(candidates).as_deref(),
            Some(vlc.to_str().expect("path"))
        );
        assert_eq!(
            first_available(["/nonexistent/vlc", "/nonexistent/mpv"]),
            None
        );
        Ok(())
    }
    #[test]
    fn player_detection_searches_path_directories() -> Result<()> {
        let dir = tempfile::tempdir()?;
        #[cfg(target_os = "windows")]
        let executable = dir.path().join("mpv.exe");
        #[cfg(not(target_os = "windows"))]
        let executable = dir.path().join("mpv");
        std::fs::write(&executable, "")?;
        assert_eq!(
            find_in_directories(Path::new("mpv"), [dir.path().to_path_buf()]).as_deref(),
            Some(executable.as_path())
        );
        Ok(())
    }
    #[test]
    fn corrupted_library_is_not_silently_reset() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("library.json");
        fs::write(&path, "{broken")?;
        assert!(Store::open(Some(dir.path().into())).is_err());
        assert_eq!(fs::read_to_string(path)?, "{broken");
        Ok(())
    }
}
