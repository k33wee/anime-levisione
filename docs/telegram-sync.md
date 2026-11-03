# Personal Telegram library sync

No application account, shared bot, backend, or Supabase project is needed. Each
user supplies a **dedicated Telegram bot**. APK and TUI use the same Rust sync
implementation and can synchronize at different times. Telegram is still the
remote storage provider: this is not a fully decentralized or self-hosted network.

## First device

1. Open the official **@BotFather** in Telegram, send `/newbot`, and follow its
   instructions. Do not reuse a bot serving other people or another application.
2. Open your new bot's private chat and send `/start` yourself.
3. Within 24 hours, connect it:
   - **Android:** Settings → Personal Telegram sync → Connect bot / another device.
   - **TUI:** command palette → Connect personal Telegram bot, or
     `anime-levisione login`.
4. Paste the BotFather token into the app, **not into the bot chat**. Leave Chat ID
   blank to detect the one recent private chat. If detection is ambiguous, send
   `/start` again or explicitly enter your own numeric private Chat ID.
5. Save the **secret `alib1.…` connection key** privately. It bundles the bot token,
   chat ID, and random encryption key; Base64 encoding is not protection. Anyone
   holding it can read/change the library and control the bot.
6. Complete a sync. The bot posts and pins `anime-levisione-v1.alib` in your chat.
   Keep that document pinned, without other pinned messages in this dedicated chat.

The connection key is saved **before** the first upload. If the network fails,
retry Sync library; do not create a second bot or replace the document.

## Other devices

Choose Connect, then paste the first device's **full connection key**, not just
its bot token. No Chat ID entry or fresh `/start` is necessary. The existing remote
file is verified before credentials are adopted. Existing local lists/progress
are merged, not discarded. The provider and provider ID identify a title; titles
on different providers remain separate, even if their displayed names match.

Retrieve the key again with Settings → Show secret connection key, the TUI command
palette, or:

```sh
anime-levisione account    # prints credentials: run privately, never paste into issues/logs
anime-levisione sync       # manual pull + merge + upload
anime-levisione logout     # disconnect only; neither local nor remote data is deleted
```

`login` reads credentials from a hidden terminal prompt or stdin, never command
arguments. Bidirectional CLI sync holds the library lock: close the running TUI
or use its Sync library command instead of opening a second writer.

## Offline use, conflicts and limitations

- All library edits are saved locally first. Network errors never undo them.
- While running, clients sync on connection/startup, after explicit edits, and
  approximately every minute. Desktop playback uploads are debounced. Android
  does not install an always-on sync service; reopen the app to synchronize.
- Watchlist membership, watched/unwatched flags, and per-episode resume points are
  shared. Downloads, local paths, player/device preferences and search history are
  **not** shared. Automatic progress updates do not clear explicit watched markers.
- Replicas retain per-field logical clocks and deletion records. They merge by
  clock and device ID, not by title or wall-clock time. Competing changes to the
  same field resolve deterministically; this is not a merge of two playback
  positions. Explicit unwatched markers and watchlist removals are supported.
- Telegram does **not** provide compare-and-swap or transactions. The client
  checks for racing uploads, but the pinned snapshot can temporarily omit a
  simultaneous writer. Retained local merge records restore its changes on a
  subsequent sync. **Do not uninstall/reset a racing device until it syncs again.**
  There is no promise of atomic remote commits or exactly-once storage.
- The encrypted snapshot limit is **4 MiB**, including retained deletion records.
  Telegram rate limits, connectivity, service availability, or bot revocation may
  delay synchronization. No application-server fallback exists.
- Do not delete/unpin the library document. If accidentally unpinned, re-pin the
  original document in Telegram. An unrelated pinned message is never replaced.
  Bot updates are used only for initial chat discovery: the 24-hour updates
  retention limit does not govern the pinned library file.

## Privacy and recovery

Snapshots use **XChaCha20-Poly1305** authenticated encryption with a random
256-bit secret held in the connection key. Telegram receives the encrypted file;
it can still observe bot/chat metadata, file sizes and access times. Bot tokens
necessarily authenticate requests to Telegram. Tokens are removed from network
error URLs before errors reach the UI or logs.

Credentials live in `sync-account.json`, separate from `library.json`; on Unix
that file is created with mode 0600. Android stores both in its private app data
and disables automatic backup. The library itself contains non-secret merge
records so data and checkpoint can be saved atomically. Protect your local profile
and connection key as you would any account credential.

**Private mode** disables Telegram sync and automatic progress/search recording,
not existing library data or explicit local edits. An already-running request may
finish before the mode change takes effect. Disconnect is local only. To delete
the remote copy, delete the library document/chat yourself in Telegram. Losing
all local profiles **and** the connection key makes the encrypted copy
unrecoverable; BotFather's token alone cannot decrypt it. If credentials leak,
revoke the bot token through BotFather and migrate your surviving local library
to a fresh dedicated bot with a new connection key.

Existing libraries are upgraded to format v2 on save (older app versions cannot
open them). The old AniList client-ID setting is ignored during migration and
is not written back. Old `anilist.json` credentials are never read or sent; you
may remove that obsolete file manually. AniList metadata/login/tracking have
been removed entirely.

## Implementation and validation

- `crates/core/src/sync.rs`: Telegram protocol, encryption, replica merge records.
- `crates/core/src/sync/tests.rs`: local Bot API fixture, encrypted roundtrips,
  offline devices, removals, concurrent snapshot recovery, in-flight edits, and
  token-redacted failures. Tests never contact a real bot.
- `crates/core/src/api.rs`: the same engine exposed to Kotlin through UniFFI.

Telegram references: [Bot setup](https://core.telegram.org/bots/tutorial#obtain-your-bot-token),
[Bot API](https://core.telegram.org/bots/api) (`getChat`, `getFile`,
`sendDocument`, `editMessageMedia`, `pinChatMessage`, `getUpdates`).
