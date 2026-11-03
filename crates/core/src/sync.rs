//! Personal Telegram bot storage. No application server or shared bot token.
//! Snapshots are authenticated/encrypted; local replicas retain merge records,
//! including tombstones, so concurrent Telegram uploads can converge on later syncs.
use crate::{
    model::Anime,
    store::{HistoryEntry, Library, Progress, atomic_write},
};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as BASE64};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Duration,
};
use url::Url;

const FILE_NAME: &str = "anime-levisione-v1.alib";
const MAGIC: &[u8] = b"ALSYNC01";
const MAX_BYTES: usize = 4 * 1024 * 1024;
const CAPTION: &str = "anime-levisione · encrypted library\nKeep this document pinned. Connect other devices using the connection key from the app, not this file.";

fn random_hex<const N: usize>() -> Result<String> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("Secure random generator unavailable"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

// Never derive Debug: both fields are credentials. The connection key is a secret
// bundle, not encryption of those credentials; only share it with your devices.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    token: String,
    chat_id: i64,
    key: String,
    initialized: bool,
}
impl Account {
    fn validate(&self) -> Result<()> {
        let Some((id, secret)) = self.token.split_once(':') else {
            bail!("Invalid Telegram bot token");
        };
        if id.parse::<i64>().is_err()
            || id.len() > 20
            || secret.len() < 20
            || secret.len() > 128
            || !secret
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || self.chat_id <= 0
            || self.key.len() != 64
            || !self.key.bytes().all(|b| b.is_ascii_hexdigit())
        {
            bail!("Invalid Telegram connection credentials");
        }
        Ok(())
    }
    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new(&Sha256::digest(format!("anime-levisione:v1:{}", self.key)))
    }
    pub fn identity(&self) -> String {
        let bot = self.token.split(':').next().unwrap_or_default();
        format!(
            "{:x}",
            Sha256::digest(format!("{bot}:{}:{}", self.chat_id, self.key))
        )
    }
    pub fn label(&self) -> String {
        format!("Telegram · chat {}", self.chat_id)
    }
    pub fn connection_key(&self) -> Result<String> {
        Ok(format!(
            "alib1.{}",
            BASE64.encode(serde_json::to_vec(self)?)
        ))
    }
    fn decode(input: &str) -> Result<Self> {
        let encoded = input
            .trim()
            .strip_prefix("alib1.")
            .context("Invalid connection key")?;
        if encoded.len() > 4096 {
            bail!("Invalid connection key");
        }
        let account: Self =
            serde_json::from_slice(&BASE64.decode(encoded).context("Invalid connection key")?)
                .map_err(|_| anyhow::anyhow!("Invalid connection key"))?;
        account.validate()?;
        Ok(account)
    }
    pub fn load(dir: &Path) -> Result<Option<Self>> {
        match std::fs::read(dir.join("sync-account.json")) {
            Ok(bytes) => {
                let account: Self =
                    serde_json::from_slice(&bytes).context("Invalid sync-account.json")?;
                account.validate()?;
                Ok(Some(account))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&self, dir: &Path) -> Result<()> {
        atomic_write(&dir.join("sync-account.json"), &serde_json::to_vec(self)?)
    }
    pub fn remove(dir: &Path) -> Result<()> {
        match std::fs::remove_file(dir.join("sync-account.json")) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn mark_initialized(&mut self) {
        self.initialized = true;
    }
    fn encrypt(&self, state: &State) -> Result<Vec<u8>> {
        let plaintext = serde_json::to_vec(state)?;
        if plaintext.len() > MAX_BYTES - 48 {
            bail!("Library exceeds the 4 MiB sync limit");
        }
        let mut nonce = [0; 24];
        getrandom::fill(&mut nonce)
            .map_err(|_| anyhow::anyhow!("Secure random generator unavailable"))?;
        let ciphertext = self
            .cipher()
            .encrypt(XNonce::from_slice(&nonce), plaintext.as_slice())
            .map_err(|_| anyhow::anyhow!("Cannot encrypt library"))?;
        Ok([MAGIC, &nonce, &ciphertext].concat())
    }
    fn decrypt(&self, bytes: &[u8]) -> Result<State> {
        if bytes.len() < 48 || bytes.len() > MAX_BYTES || !bytes.starts_with(MAGIC) {
            bail!("Invalid encrypted library");
        }
        let plaintext = self
            .cipher()
            .decrypt(XNonce::from_slice(&bytes[8..32]), &bytes[32..])
            .map_err(|_| {
                anyhow::anyhow!("Library authentication failed; check the connection key")
            })?;
        let state: State =
            serde_json::from_slice(&plaintext).context("Invalid library snapshot")?;
        state.validate()?;
        Ok(state)
    }
}

#[derive(Clone, Default, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Stamp {
    clock: u64,
    device: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Record<T> {
    stamp: Stamp,
    value: Option<T>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct EpisodeRecord {
    anime: Anime,
    progress: Progress,
    updated: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    version: u32,
    bookmarks: BTreeMap<String, Record<Anime>>,
    episodes: BTreeMap<String, BTreeMap<String, Record<EpisodeRecord>>>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            bookmarks: BTreeMap::new(),
            episodes: BTreeMap::new(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub account: String,
    state: State,
}

#[derive(Clone, Default)]
struct View {
    watchlist: Vec<Anime>,
    history: Vec<HistoryEntry>,
}
impl View {
    fn capture(library: &Library) -> Self {
        Self {
            watchlist: library.watchlist.clone(),
            history: library.history.clone(),
        }
    }
}
fn merge_records<T: Clone>(
    local: &mut BTreeMap<String, Record<T>>,
    other: &BTreeMap<String, Record<T>>,
) {
    for (key, record) in other {
        if local.get(key).is_none_or(|r| r.stamp < record.stamp) {
            local.insert(key.clone(), record.clone());
        }
    }
}
impl State {
    fn merge(&mut self, other: &Self) {
        merge_records(&mut self.bookmarks, &other.bookmarks);
        for (key, episodes) in &other.episodes {
            merge_records(self.episodes.entry(key.clone()).or_default(), episodes);
        }
    }
    fn clock(&self) -> u64 {
        self.bookmarks
            .values()
            .map(|r| r.stamp.clock)
            .chain(
                self.episodes
                    .values()
                    .flat_map(|eps| eps.values().map(|r| r.stamp.clock)),
            )
            .max()
            .unwrap_or(0)
    }
    fn view(&self) -> View {
        let mut bookmarks: Vec<_> = self
            .bookmarks
            .values()
            .filter(|r| r.value.is_some())
            .collect();
        bookmarks.sort_by(|a, b| b.stamp.cmp(&a.stamp));
        let watchlist = bookmarks
            .into_iter()
            .filter_map(|r| r.value.clone())
            .collect();
        let mut history = Vec::new();
        for episodes in self.episodes.values() {
            let Some(latest) = episodes
                .values()
                .filter(|r| r.value.is_some())
                .max_by_key(|r| &r.stamp)
            else {
                continue;
            };
            let Some(last) = &latest.value else {
                continue;
            };
            history.push(HistoryEntry {
                anime: last.anime.clone(),
                last_episode: last.progress.episode.number.clone(),
                updated: last.updated,
                progress: episodes
                    .iter()
                    .filter_map(|(n, r)| r.value.as_ref().map(|e| (n.clone(), e.progress.clone())))
                    .collect(),
            });
        }
        history.sort_by(|a, b| {
            b.updated
                .cmp(&a.updated)
                .then_with(|| a.anime.key().cmp(&b.anime.key()))
        });
        View { watchlist, history }
    }
    /// Turn only visible local edits into new records. All previously known
    /// records survive uploads, even if a racing writer temporarily hid them.
    fn edit(&mut self, base: &View, local: &View, device: &str) -> Result<()> {
        let mut clock = self.clock();
        let mut stamp = || -> Result<Stamp> {
            clock = clock.checked_add(1).context("Sync clock exhausted")?;
            Ok(Stamp {
                clock,
                device: device.into(),
            })
        };
        let old: BTreeSet<_> = base.watchlist.iter().map(Anime::key).collect();
        let now: BTreeSet<_> = local.watchlist.iter().map(Anime::key).collect();
        for key in old.difference(&now) {
            self.bookmarks.insert(
                key.clone(),
                Record {
                    stamp: stamp()?,
                    value: None,
                },
            );
        }
        for anime in local
            .watchlist
            .iter()
            .rev()
            .filter(|a| !old.contains(&a.key()))
        {
            self.bookmarks.insert(
                anime.key(),
                Record {
                    stamp: stamp()?,
                    value: Some(anime.clone()),
                },
            );
        }
        let old_history: BTreeMap<_, _> = base.history.iter().map(|h| (h.anime.key(), h)).collect();
        let now_history: BTreeMap<_, _> =
            local.history.iter().map(|h| (h.anime.key(), h)).collect();
        for (key, h) in &old_history {
            for number in h.progress.keys() {
                if !now_history
                    .get(key)
                    .is_some_and(|entry| entry.progress.contains_key(number))
                {
                    self.episodes.entry(key.clone()).or_default().insert(
                        number.clone(),
                        Record {
                            stamp: stamp()?,
                            value: None,
                        },
                    );
                }
            }
        }
        for h in local.history.iter().rev() {
            let old = old_history.get(&h.anime.key()).copied();
            let mut changes: Vec<_> = h
                .progress
                .iter()
                .filter(|(n, p)| old.and_then(|h| h.progress.get(*n)) != Some(*p))
                .collect();
            changes.sort_by_key(|(n, _)| *n == &h.last_episode); // The resume episode gets the newest stamp.
            for (number, progress) in changes {
                self.episodes.entry(h.anime.key()).or_default().insert(
                    number.clone(),
                    Record {
                        stamp: stamp()?,
                        value: Some(EpisodeRecord {
                            anime: h.anime.clone(),
                            progress: progress.clone(),
                            updated: h.updated,
                        }),
                    },
                );
            }
        }
        Ok(())
    }
    fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("Unsupported sync version; update the app");
        }
        for (key, record) in &self.bookmarks {
            if record.value.as_ref().is_some_and(|a| a.key() != *key) {
                bail!("Invalid bookmark identity");
            }
        }
        for (key, episodes) in &self.episodes {
            for (number, record) in episodes {
                if let Some(e) = &record.value
                    && (e.anime.key() != *key
                        || &e.progress.episode.number != number
                        || !e.progress.seconds.is_finite()
                        || e.progress.seconds < 0.0
                        || !e.progress.duration.is_finite()
                        || e.progress.duration < 0.0)
                {
                    bail!("Invalid remote progress");
                }
            }
        }
        // Neither Telegram nor a corrupt replica may inject terminal controls.
        let value = serde_json::to_value(self)?;
        fn safe(value: &Value) -> bool {
            match value {
                Value::String(s) => !s.chars().any(|c| c.is_control() && c != '\n' && c != '\t'),
                Value::Array(a) => a.iter().all(safe),
                Value::Object(o) => o
                    .iter()
                    .all(|(k, v)| !k.chars().any(char::is_control) && safe(v)),
                _ => true,
            }
        }
        if !safe(&value) {
            bail!("Invalid library text");
        }
        Ok(())
    }
}

pub struct Outcome {
    pub identity: String,
    device: String,
    captured: View,
    state: State,
}
impl Outcome {
    pub fn apply(&self, library: &mut Library) -> Result<()> {
        let mut state = self.state.clone();
        state.edit(&self.captured, &View::capture(library), &self.device)?;
        let view = state.view();
        library.watchlist = view.watchlist;
        library.history = view.history;
        library.sync_checkpoint = Some(Checkpoint {
            account: self.identity.clone(),
            state,
        });
        Ok(())
    }
}

#[derive(Clone, Deserialize)]
struct Document {
    file_id: String,
    file_unique_id: String,
    file_name: Option<String>,
}
#[derive(Clone, Deserialize)]
struct Remote {
    message_id: i64,
    document: Document,
}
impl Remote {
    fn fingerprint(&self) -> (&str, i64) {
        (&self.document.file_unique_id, self.message_id)
    }
}

pub struct Client {
    account: Account,
    http: reqwest::Client,
    base: Url,
}
impl Client {
    pub fn new(account: Account) -> Result<Self> {
        account.validate()?;
        Ok(Self {
            account,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            base: Url::parse("https://api.telegram.org/")?,
        })
    }
    fn endpoint(&self, method: &str) -> Result<Url> {
        // The bot token contains ':': force a relative path, not a URL scheme.
        Ok(self
            .base
            .join(&format!("./bot{}/{method}", self.account.token))?)
    }
    async fn bytes(&self, mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
        if !response.status().is_success() {
            bail!(
                "Telegram returned HTTP {}; local changes are preserved",
                response.status().as_u16()
            );
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(reqwest::Error::without_url)?
        {
            if bytes.len() + chunk.len() > limit {
                bail!("Telegram response exceeds the size limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    async fn response(&self, response: reqwest::Response) -> Result<Value> {
        let value: Value = serde_json::from_slice(&self.bytes(response, 1024 * 1024).await?)
            .context("Invalid Telegram response")?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            bail!(
                "Telegram rejected the request; verify the bot token, /start and chat permissions"
            );
        }
        value
            .get("result")
            .cloned()
            .context("Telegram response has no result")
    }
    async fn call(&self, method: &str, body: Value) -> Result<Value> {
        let response = self
            .http
            .post(self.endpoint(method)?)
            .json(&body)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("Telegram connection failed")?;
        self.response(response).await
    }
    async fn remote(&self) -> Result<Option<Remote>> {
        let chat = self
            .call("getChat", json!({"chat_id": self.account.chat_id}))
            .await?;
        if chat.get("type").and_then(Value::as_str) != Some("private")
            || chat.get("id").and_then(Value::as_i64) != Some(self.account.chat_id)
        {
            bail!("Sync requires your private chat with a dedicated bot");
        }
        let Some(pin) = chat.get("pinned_message").filter(|p| !p.is_null()) else {
            return Ok(None);
        };
        let bot = self
            .account
            .token
            .split(':')
            .next()
            .and_then(|s| s.parse::<i64>().ok());
        if pin.pointer("/from/id").and_then(Value::as_i64) != bot {
            bail!("The pinned message is not this bot's library; nothing was changed");
        }
        let remote: Remote = serde_json::from_value(pin.clone()).map_err(|_| {
            anyhow::anyhow!("Keep only the library document pinned in the dedicated bot chat")
        })?;
        if remote.document.file_name.as_deref() != Some(FILE_NAME) {
            bail!("An unrelated message is pinned; nothing was changed");
        }
        Ok(Some(remote))
    }
    async fn download(&self, remote: &Remote) -> Result<State> {
        let file = self
            .call("getFile", json!({"file_id": remote.document.file_id}))
            .await?;
        if file
            .get("file_size")
            .and_then(Value::as_u64)
            .is_some_and(|s| s > MAX_BYTES as u64)
        {
            bail!("Remote library is too large");
        }
        let path = file
            .get("file_path")
            .and_then(Value::as_str)
            .context("Telegram file is unavailable")?;
        if path.is_empty()
            || path.starts_with('/')
            || path.split('/').any(|p| p == ".." || p == ".")
            || !path
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
        {
            bail!("Invalid Telegram file path");
        }
        let url = self
            .base
            .join(&format!("file/bot{}/{path}", self.account.token))?;
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        self.account
            .decrypt(&self.bytes(response, MAX_BYTES).await?)
    }
    async fn upload(&self, state: &State, remote: Option<&Remote>) -> Result<Remote> {
        use reqwest::multipart::{Form, Part};
        let part = Part::bytes(self.account.encrypt(state)?)
            .file_name(FILE_NAME)
            .mime_str("application/octet-stream")?;
        let form = Form::new().text("chat_id", self.account.chat_id.to_string());
        let (method, form) = if let Some(remote) = remote {
            (
                "editMessageMedia",
                form.text("message_id", remote.message_id.to_string())
                    .text(
                        "media",
                        json!({"type":"document", "media":"attach://library", "caption":CAPTION})
                            .to_string(),
                    )
                    .part("library", part),
            )
        } else {
            (
                "sendDocument",
                form.text("caption", CAPTION).part("document", part),
            )
        };
        let response = self
            .http
            .post(self.endpoint(method)?)
            .multipart(form)
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        let uploaded: Remote = serde_json::from_value(self.response(response).await?)
            .context("Invalid Telegram upload response")?;
        if remote.is_none() {
            self.call("pinChatMessage", json!({"chat_id":self.account.chat_id, "message_id":uploaded.message_id, "disable_notification":true})).await?;
        }
        Ok(uploaded)
    }
    /// Bootstrap only: /start must be recent (Telegram retains updates at most 24h).
    /// Normal reads use the pinned document, never the updates queue.
    pub async fn connect(input: &str, chat_id: &str) -> Result<Account> {
        Self::connect_using(input, chat_id, Self::new).await
    }
    async fn connect_using(
        input: &str,
        chat_id: &str,
        make_client: impl Fn(Account) -> Result<Self>,
    ) -> Result<Account> {
        if input.trim().starts_with("alib1.") {
            let mut account = Account::decode(input)?;
            let client = make_client(account.clone())?;
            let remote = client
                .remote()
                .await?
                .context("Library document is not pinned in this chat")?;
            client.download(&remote).await?;
            account.initialized = true;
            return Ok(account);
        }
        let explicit = if chat_id.trim().is_empty() {
            None
        } else {
            Some(chat_id.trim().parse::<i64>().context("Invalid chat ID")?)
        };
        let mut account = Account {
            token: input.trim().into(),
            chat_id: explicit.unwrap_or(1),
            key: random_hex::<32>()?,
            initialized: false,
        };
        let client = make_client(account.clone())?;
        client.call("getMe", json!({})).await?;
        if let Some(id) = explicit {
            account.chat_id = id;
        } else {
            let updates = client
                .call(
                    "getUpdates",
                    json!({"limit":100, "timeout":0, "allowed_updates":["message"]}),
                )
                .await?;
            let mut chats = BTreeSet::new();
            for update in updates.as_array().context("Invalid Telegram updates")? {
                if update.pointer("/message/chat/type").and_then(Value::as_str) == Some("private")
                    && let Some(id) = update.pointer("/message/chat/id").and_then(Value::as_i64)
                {
                    chats.insert(id);
                }
            }
            if chats.len() != 1 {
                bail!(
                    "Send /start to your new dedicated bot now, or enter your private Chat ID explicitly"
                );
            }
            account.chat_id = *chats.first().context("No bot chat")?;
        }
        account.validate()?;
        if make_client(account.clone())?.remote().await?.is_some() {
            bail!(
                "A library already exists: use its connection key from another device, not the bot token"
            );
        }
        Ok(account)
    }
    pub async fn sync(&self, library: &Library) -> Result<Outcome> {
        let identity = self.account.identity();
        let checkpoint = library
            .sync_checkpoint
            .as_ref()
            .filter(|c| c.account == identity);
        // A fresh writer ID also keeps copied profiles from creating colliding stamps.
        let device = random_hex::<16>()?;
        let mut state = checkpoint.map_or_else(State::default, |c| c.state.clone());
        let baseline = state.view();
        let captured = View::capture(library);
        // Capture local edits once; retries merge those same records, not new operations.
        let mut local_edits_applied = false;
        for _ in 0..4 {
            let remote = self.remote().await?;
            if remote.is_none() && (self.account.initialized || checkpoint.is_some()) {
                bail!("Library document is missing: re-pin it in Telegram before syncing");
            }
            let remote_state = match &remote {
                Some(r) => self.download(r).await?,
                None => State::default(),
            };
            state.merge(&remote_state);
            if !local_edits_applied {
                state.edit(&baseline, &captured, &device)?;
                local_edits_applied = true;
            }
            state.validate()?;
            if remote.is_some() && state == remote_state {
                return Ok(Outcome {
                    identity,
                    device,
                    captured,
                    state,
                });
            }
            // Best effort race detection; Telegram has no compare-and-swap.
            let current = self.remote().await?;
            if current.as_ref().map(Remote::fingerprint) != remote.as_ref().map(Remote::fingerprint)
            {
                continue;
            }
            let uploaded = self.upload(&state, remote.as_ref()).await?;
            if self.remote().await?.as_ref().map(Remote::fingerprint)
                == Some(uploaded.fingerprint())
            {
                return Ok(Outcome {
                    identity,
                    device,
                    captured,
                    state,
                });
            }
        }
        bail!("Another device is updating the library; retry shortly (local changes are preserved)")
    }
}

#[cfg(test)]
mod tests;
