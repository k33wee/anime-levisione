use super::*;
use crate::model::{Episode, ProviderId};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn account() -> Account {
    Account {
        token: format!("123:{}", "x".repeat(32)),
        chat_id: 42,
        key: "ab".repeat(32),
        initialized: false,
    }
}
fn anime(provider: ProviderId) -> Anime {
    serde_json::from_value(json!({"provider":provider,"id":"7","title":"The exact provider title"}))
        .expect("anime")
}
fn progress(number: &str, completed: bool) -> Progress {
    Progress {
        episode: Episode {
            id: format!("ep-{number}"),
            number: number.into(),
            title: String::new(),
        },
        seconds: 123.0,
        duration: 1400.0,
        completed,
    }
}
fn state(library: &Library, device: &str) -> State {
    let mut state = State::default();
    state
        .edit(&View::default(), &View::capture(library), device)
        .expect("edit");
    state
}

#[test]
fn encrypted_snapshot_is_randomized_authenticated_and_versioned() -> Result<()> {
    let mut library = Library::default();
    library.toggle_bookmark(anime(ProviderId::AnimeWorld));
    let expected = state(&library, "device");
    let account = account();
    let mut encrypted = account.encrypt(&expected)?;
    assert!(!String::from_utf8_lossy(&encrypted).contains("provider title"));
    assert_ne!(encrypted, account.encrypt(&expected)?);
    assert_eq!(expected, account.decrypt(&encrypted)?);
    let mut wrong = account.clone();
    wrong.key = "cd".repeat(32);
    assert!(wrong.decrypt(&encrypted).is_err());
    encrypted[40] ^= 1;
    assert!(account.decrypt(&encrypted).is_err());
    assert!(account.decrypt(b"short").is_err());
    let mut future = expected;
    future.version = 2;
    assert!(account.decrypt(&account.encrypt(&future)?).is_err());
    Ok(())
}

#[test]
fn connection_key_and_private_credentials_roundtrip_without_leaking_into_library() -> Result<()> {
    let account = account();
    let key = account.connection_key()?;
    assert_eq!(Account::decode(&key)?.identity(), account.identity());
    assert!(Account::decode("alib1.not-valid").is_err());
    let dir = tempfile::tempdir()?;
    account.save(dir.path())?;
    assert_eq!(
        Account::load(dir.path())?.expect("saved").identity(),
        account.identity()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.path().join("sync-account.json"))?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let library = Library {
        sync_checkpoint: Some(Checkpoint {
            account: account.identity(),
            state: State::default(),
        }),
        ..Library::default()
    };
    let json = serde_json::to_string(&library)?;
    assert!(!json.contains(&account.token));
    assert!(!json.contains(&account.key));
    Account::remove(dir.path())?;
    assert!(Account::load(dir.path())?.is_none());
    Ok(())
}

#[test]
fn merge_is_commutative_idempotent_and_keeps_provider_ids_separate() {
    let mut a = Library::default();
    a.toggle_bookmark(anime(ProviderId::AnimeWorld));
    a.record(anime(ProviderId::AnimeWorld), progress("1.5", true));
    let mut b = Library::default();
    b.toggle_bookmark(anime(ProviderId::AnimeUnity));
    b.record(anime(ProviderId::AnimeWorld), progress("2", false));
    let a = state(&a, "a");
    let b = state(&b, "b");
    let mut ab = a.clone();
    ab.merge(&b);
    let mut ba = b;
    ba.merge(&a);
    assert_eq!(ab, ba);
    ba.merge(&ab);
    assert_eq!(ab, ba);
    assert_eq!(ab.view().watchlist.len(), 2);
    assert_eq!(ab.view().history[0].progress.len(), 2);
}

#[test]
fn removals_unwatched_and_deleted_progress_beat_stale_replicas() -> Result<()> {
    let anime = anime(ProviderId::AnimeWorld);
    let mut library = Library::default();
    library.toggle_bookmark(anime.clone());
    library.record(anime.clone(), progress("1", true));
    let mut current = state(&library, "a");
    let stale = current.clone();
    let baseline = current.view();
    library.toggle_bookmark(anime.clone());
    library.record(anime.clone(), progress("1", false));
    current.edit(&baseline, &View::capture(&library), "b")?;
    current.merge(&stale);
    assert!(current.view().watchlist.is_empty());
    assert!(!current.view().history[0].progress["1"].completed);
    let baseline = current.view();
    library.history.clear();
    current.edit(&baseline, &View::capture(&library), "b")?;
    current.merge(&stale);
    assert!(current.view().history.is_empty());
    Ok(())
}

#[test]
fn corrupt_text_and_clock_overflow_fail_safely() -> Result<()> {
    let mut library = Library::default();
    let mut bad = anime(ProviderId::AnimeWorld);
    bad.title = "\x1b[31m".into();
    library.toggle_bookmark(bad);
    let mut state = state(&library, "a");
    assert!(state.validate().is_err());
    state
        .bookmarks
        .values_mut()
        .next()
        .expect("record")
        .stamp
        .clock = u64::MAX;
    assert!(state.edit(&state.view(), &View::default(), "b").is_err());
    Ok(())
}

#[derive(Default)]
struct Bot {
    pin: Option<Value>,
    uploaded: Option<Value>,
    files: BTreeMap<String, Vec<u8>>,
    uploads: usize,
    fail_upload: bool,
    updates: Option<Value>,
    calls: Vec<String>,
}
impl Bot {
    fn store(&mut self, bytes: Vec<u8>, pinned: bool) -> Value {
        self.uploads += 1;
        let file_id = format!("f{}", self.uploads);
        self.files.insert(file_id.clone(), bytes);
        let message = json!({"message_id":100,"from":{"id":123},
            "document":{"file_id":file_id,"file_unique_id":file_id,"file_name":FILE_NAME}});
        self.uploaded = Some(message.clone());
        if pinned {
            self.pin = Some(message.clone());
        }
        message
    }
    fn handle(&mut self, path: &str, headers: &str, body: &[u8]) -> (u16, Vec<u8>) {
        let operation = path.rsplit('/').next().expect("path");
        self.calls.push(operation.into());
        if path.contains("/file/") {
            return (200, self.files.get(operation).expect("file").clone());
        }
        let input: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let result = match operation {
            "getMe" => json!({"id":123,"is_bot":true}),
            "getUpdates" => self
                .updates
                .clone()
                .unwrap_or_else(|| json!([{"message":{"chat":{"id":42,"type":"private"}}}])),
            "getChat" => json!({"id":42,"type":"private","pinned_message":self.pin}),
            "getFile" => {
                json!({"file_path":format!("documents/{}", input["file_id"].as_str().expect("file id"))})
            }
            "sendDocument" | "editMessageMedia" => {
                if self.fail_upload {
                    return (503, Vec::new());
                }
                let boundary = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once("boundary=")
                            .map(|(_, b)| b.trim().trim_matches('"'))
                    })
                    .expect("boundary");
                let start = body
                    .windows(MAGIC.len())
                    .position(|w| w == MAGIC)
                    .expect("encrypted multipart file");
                let end_marker = format!("\r\n--{boundary}");
                let end = body[start..]
                    .windows(end_marker.len())
                    .position(|w| w == end_marker.as_bytes())
                    .expect("end of file")
                    + start;
                self.store(body[start..end].to_vec(), operation == "editMessageMedia")
            }
            "pinChatMessage" => {
                self.pin = self.uploaded.clone();
                json!(true)
            }
            _ => panic!("unexpected Telegram method: {operation}"),
        };
        (
            200,
            serde_json::to_vec(&json!({"ok":true,"result":result})).expect("response"),
        )
    }
}
struct Fixture {
    bot: Arc<Mutex<Bot>>,
    base: Url,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn start() -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = Url::parse(&format!("http://{}/", listener.local_addr()?))?;
        let bot = Arc::new(Mutex::new(Bot::default()));
        let shared = bot.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.expect("accept");
                let shared = shared.clone();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let split = loop {
                        let mut buf = [0; 8192];
                        let size = socket.read(&mut buf).await.expect("read");
                        if size == 0 {
                            return;
                        }
                        request.extend_from_slice(&buf[..size]);
                        if let Some(i) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                            break i + 4;
                        }
                    };
                    let headers = String::from_utf8_lossy(&request[..split]).into_owned();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().expect("length"))
                        })
                        .unwrap_or(0);
                    while request.len() < split + length {
                        let mut buf = [0; 8192];
                        let size = socket.read(&mut buf).await.expect("body");
                        if size == 0 {
                            return;
                        }
                        request.extend_from_slice(&buf[..size]);
                    }
                    let path = headers.split_whitespace().nth(1).expect("request path");
                    let (status, body) = shared.lock().expect("bot").handle(
                        path,
                        &headers,
                        &request[split..split + length],
                    );
                    let head = format!(
                        "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    socket.write_all(head.as_bytes()).await.expect("headers");
                    socket.write_all(&body).await.expect("response");
                });
            }
        });
        Ok(Self { bot, base, task })
    }
    fn client(&self) -> Client {
        Client {
            base: self.base.clone(),
            ..Client::new(account()).expect("client")
        }
    }
    async fn sync(&self, library: &mut Library) -> Result<()> {
        self.client().sync(library).await?.apply(library)
    }
    async fn connect(&self, input: &str, chat: &str) -> Result<Account> {
        Client::connect_using(input, chat, |account| {
            Ok(Client {
                base: self.base.clone(),
                ..Client::new(account)?
            })
        })
        .await
    }
}

#[tokio::test]
async fn telegram_roundtrip_merges_offline_devices_and_does_not_use_updates() -> Result<()> {
    let fixture = Fixture::start().await?;
    let mut desktop = Library::default();
    desktop.toggle_bookmark(anime(ProviderId::AnimeWorld));
    desktop.record(anime(ProviderId::AnimeWorld), progress("1.5", false));
    desktop.remember_search("private local search");
    fixture.sync(&mut desktop).await?;
    let mut mobile = Library::default();
    mobile.toggle_bookmark(anime(ProviderId::AnimeUnity));
    mobile.record(anime(ProviderId::AnimeWorld), progress("2", true));
    fixture.sync(&mut mobile).await?;
    fixture.sync(&mut desktop).await?;
    assert_eq!(desktop.watchlist, mobile.watchlist);
    assert_eq!(desktop.history, mobile.history);
    assert_eq!(desktop.watchlist.len(), 2);
    assert_eq!(desktop.history[0].progress["1.5"].seconds, 123.0);
    assert!(mobile.recent_searches.is_empty());
    desktop.toggle_bookmark(anime(ProviderId::AnimeWorld));
    desktop.record(anime(ProviderId::AnimeWorld), progress("2", false));
    fixture.sync(&mut desktop).await?;
    fixture.sync(&mut mobile).await?;
    assert_eq!(mobile.watchlist.len(), 1);
    assert!(!mobile.history[0].progress["2"].completed);
    let uploads = fixture.bot.lock().expect("bot").uploads;
    fixture.sync(&mut mobile).await?;
    let bot = fixture.bot.lock().expect("bot");
    assert_eq!(
        uploads, bot.uploads,
        "unchanged snapshots do not upload again"
    );
    assert!(!bot.calls.iter().any(|c| c == "getUpdates"));
    assert!(bot.calls.iter().any(|c| c == "pinChatMessage"));
    assert!(bot.calls.iter().any(|c| c == "editMessageMedia"));
    Ok(())
}

#[tokio::test]
async fn racing_snapshot_can_be_recovered_from_retained_replica_records() -> Result<()> {
    let fixture = Fixture::start().await?;
    let mut a = Library::default();
    fixture.sync(&mut a).await?;
    let mut b = a.clone();
    b.sync_checkpoint = None;
    fixture.sync(&mut b).await?;
    a.toggle_bookmark(anime(ProviderId::AnimeWorld));
    fixture.sync(&mut a).await?;
    b.toggle_bookmark(anime(ProviderId::AnimeUnity));
    // Simulate a simultaneous writer publishing a snapshot before seeing A's upload.
    let b_state = state(&b, "b");
    fixture
        .bot
        .lock()
        .expect("bot")
        .store(account().encrypt(&b_state)?, true);
    fixture.sync(&mut a).await?;
    fixture.sync(&mut b).await?;
    // B has now committed its pending local edit; A observes that final stamp too.
    fixture.sync(&mut a).await?;
    assert_eq!(a.watchlist.len(), 2);
    assert_eq!(a.watchlist, b.watchlist);
    Ok(())
}

#[tokio::test]
async fn edits_during_sync_and_checkpoint_roundtrip_are_not_lost() -> Result<()> {
    let fixture = Fixture::start().await?;
    let mut library = Library::default();
    library.toggle_bookmark(anime(ProviderId::AnimeWorld));
    let outcome = fixture.client().sync(&library).await?;
    library.toggle_bookmark(anime(ProviderId::AnimeWorld));
    library.toggle_bookmark(anime(ProviderId::AnimeUnity));
    outcome.apply(&mut library)?;
    assert_eq!(library.watchlist, vec![anime(ProviderId::AnimeUnity)]);
    let mut reopened: Library = serde_json::from_slice(&serde_json::to_vec(&library)?)?;
    fixture.sync(&mut reopened).await?;
    let mut other = Library::default();
    fixture.sync(&mut other).await?;
    assert_eq!(reopened.watchlist, other.watchlist);
    Ok(())
}

#[tokio::test]
async fn upload_failure_missing_pin_and_unrelated_pin_do_not_change_local_data() -> Result<()> {
    let fixture = Fixture::start().await?;
    let mut library = Library::default();
    fixture.sync(&mut library).await?;
    library.toggle_bookmark(anime(ProviderId::AnimeWorld));
    let before = serde_json::to_vec(&library)?;
    fixture.bot.lock().expect("bot").fail_upload = true;
    assert!(fixture.client().sync(&library).await.is_err());
    assert_eq!(before, serde_json::to_vec(&library)?);
    fixture.bot.lock().expect("bot").pin = None;
    assert!(fixture.client().sync(&library).await.is_err());
    fixture.bot.lock().expect("bot").pin =
        Some(json!({"message_id":9,"from":{"id":123},"text":"unrelated"}));
    assert!(fixture.client().sync(&Library::default()).await.is_err());
    assert_eq!(fixture.bot.lock().expect("bot").uploads, 1);
    Ok(())
}

#[tokio::test]
async fn setup_discovers_chat_and_pairing_verifies_the_existing_encrypted_file() -> Result<()> {
    let fixture = Fixture::start().await?;
    let created = fixture.connect(&account().token, "").await?;
    assert_eq!(created.chat_id, 42);
    let mut library = Library::default();
    library.toggle_bookmark(anime(ProviderId::AnimeWorld));
    let client = Client {
        base: fixture.base.clone(),
        ..Client::new(created.clone())?
    };
    client.sync(&library).await?.apply(&mut library)?;
    let joined = fixture.connect(&created.connection_key()?, "").await?;
    assert_eq!(joined.identity(), created.identity());
    assert!(joined.initialized);
    assert!(
        fixture.connect(&account().token, "").await.is_err(),
        "token alone must not overwrite an existing encrypted library"
    );
    let mut wrong = created;
    wrong.key = "cd".repeat(32);
    assert!(fixture.connect(&wrong.connection_key()?, "").await.is_err());
    Ok(())
}

#[tokio::test]
async fn setup_rejects_missing_ambiguous_chats_and_does_not_consume_updates() -> Result<()> {
    let fixture = Fixture::start().await?;
    fixture.bot.lock().expect("bot").updates = Some(json!([]));
    assert!(fixture.connect(&account().token, "").await.is_err());
    fixture.bot.lock().expect("bot").updates = Some(json!([
        {"message":{"chat":{"id":42,"type":"private"}}},
        {"message":{"chat":{"id":99,"type":"private"}}}
    ]));
    assert!(fixture.connect(&account().token, "").await.is_err());
    let calls = fixture.bot.lock().expect("bot").calls.len();
    let created = fixture.connect(&account().token, "42").await?;
    assert_eq!(created.chat_id, 42);
    assert!(
        !fixture.bot.lock().expect("bot").calls[calls..]
            .iter()
            .any(|c| c == "getUpdates")
    );
    assert!(fixture.connect("invalid/token", "42").await.is_err());
    Ok(())
}

#[tokio::test]
async fn network_errors_never_print_the_bot_token() -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = Url::parse(&format!("http://{}/", listener.local_addr()?))?;
    drop(listener);
    let client = Client {
        base,
        ..Client::new(account())?
    };
    let error = client
        .sync(&Library::default())
        .await
        .err()
        .expect("connection failure");
    assert!(!format!("{error:#}").contains(&account().token));
    Ok(())
}
