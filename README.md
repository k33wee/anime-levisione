# anime-levisione

**Watch anime from your terminal — a fast, keyboard-first TUI inspired by lazygit.**

`anime-levisione` is a TUI that searches three Italian streaming libraries in parallel (AnimeWorld, AnimeUnity, and StreamingCommunity), keeps a watchlist, resumes episodes where you left off, and plays or downloads them without leaving the app.

Pure Rust. No fzf. Just one binary.

```
╭[1]-Search──────────────────────────╮ ╭[0]-Anime details────────────────────────────╮
│> search any title…                 │ │Frieren: Beyond Journey's End                │
│AW 12  AU 9  SC 30                   │ │TV · 2023 · Finished                         │
│p: All sources  a: All audio        │ │AnimeWorld | SUB ITA | 28 episodes | Score 9.03│
╰────────────────────────────────────╯ ╰──────────────────────────────────────────────╯
╭[2]-Discover - Search───────────────╮ ╭Episodes · Enter: play · f: filter───────────╮
│  AW Frieren: Beyond Journey's End  │ │ > Episode 1      resume 12:04               │
│  AU Frieren: Beyond Journey's End  │ │ + Episode 2      watched                    │
│  AW Frieren (ITA)                  │ │   Episode 3                                 │
╰────────────────────────────────────╯ ╰──────────────────────────────────────────────╯
╭[3]-Continue──────╮╭[4]-Watchlist───╮╭[5]-Offline─────╮ ╭Activity──────────────────╮
```

## Requirements

| Tool                                 | Needed for     | Notes                                                                                                             |
| ------------------------------------ | -------------- | ----------------------------------------------------------------------------------------------------------------- |
| [VLC](https://www.videolan.org/vlc/) | playback       | **Default** when installed (on macOS the bundle path `/Applications/VLC.app/Contents/MacOS/VLC` is auto-detected) |
| [mpv](https://mpv.io/installation/)  | playback       | Fallback default; recommended for automatic resume and watched tracking                                           |
| [ffmpeg](https://ffmpeg.org/)        | downloads only | Handles both direct files and HLS streams                                                                         |

Works on Linux, macOS, and Windows terminals (including WSL).

## Install

From source (Rust 1.88+):

```sh
git clone https://github.com/k33wee/anime-levisione
cd anime-levisione
cargo install --path .
```

## Usage

```sh
anime-levisione                    # open the TUI on Discover
anime-levisione "frieren"          # open with a search already running
anime-levisione --demo             # offline preview of the UI (no network, no writes)
anime-levisione search "naruto"    # scriptable search, ranks both providers
anime-levisione search "naruto" --json --provider animeunity
anime-levisione doctor             # check player, ffmpeg, and provider connectivity
```

### Keyboard-first

- `/` search (debounced, both sources, ranked results) · `2` results · `3` continue · `4` watchlist · `5` offline · `0` details
- `j/k ↑/↓ g/G PgUp/PgDn` navigate · `Tab h/l` switch pane
- `Enter` open episodes, then play · `s` restart · `n` next episode
- `w` watchlist · `m` mark watched · `d` download · `f` filter episodes · `p/a` provider & sub/dub filters
- `r` retry/refresh · `P` private mode · `:` command palette · `?` full keybindings
- `Ctrl+x / Ctrl+d` stop playback / cancel download · `q` quit (asks when jobs are running)

### Domains that keep rotating

The providers rotate their domains every few weeks (AnimeUnity alone went `animeunity.so` → `.to` → `.co` in days, and StreamingCommunity changes even faster). Instead of breaking until the config is edited, each provider locates itself:

- The configured URL is probed first, then the last domain that worked (`domains.json`), extra mirrors from `config.toml` (`animeunity_domains = ["https://…"]`, `animeworld_domains`, `streamingcommunity_domains`), and a built-in list of known domains — in parallel, primary first.
- Redirects are followed, so a dead domain forwarding to its successor is adopted automatically.
- A domain only counts as live once its homepage can bootstrap a provider session (a CSRF token for the anime sites, catalogue data for StreamingCommunity; parked or hijacked domains cannot).
- If a request fails mid-session, the candidates are probed again and the request is retried once on the new domain; the UI announces the move.

`anime-levisione doctor` shows what was resolved, e.g. `Domain AnimeUnity: auto-detected https://www.animeunity.co/ (configured https://www.animeunity.to/)`.

### Search that stays reliable

- All providers are queried **in parallel**; if one fails you still see the others, with failures shown in `[1]` and `!` diagnostics. Cycle a single source with `p`.
- Typed searches debounce at 350 ms and each request is generation-tagged, so slow responses can never overwrite a newer query.
- Requests retry on transient errors and 419 sessions are refreshed automatically; domain rotations are handled as described above.
- Ranking prefers exact/prefix/title matches, then token and fuzzy similarity; English, Italian, and romaji titles all work (accent-insensitive).

### Watching, resuming, downloading

- mpv resume/watched progress is captured through mpv's IPC (nothing is written by the provider page). With VLC (the default when installed) playback and `--start-time` resume work, but mark episodes watched manually with `m`.
- Downloads go to `<Videos>/Anime/<title> [AW-id]/Episode N.mkv` via ffmpeg stream copy, land atomically, and appear in the `[5] Offline` pane with a manifest that never escapes its folder.
- `--private` (or `P`) disables automatic history saving for a session.

### Files

| Path                                                  | Purpose                                            |
| ----------------------------------------------------- | -------------------------------------------------- |
| `~/.config/anime-levisione/config.toml` (platform equivalent) | player, `player_args`, download dir, provider URLs and mirror candidates |
| `~/.config/anime-levisione/domains.json`                      | auto-detected live provider domains (disposable cache) |
| `~/.config/anime-levisione/library.json`                      | watchlist, history, resume points, recent searches |

Player and its arguments are launched directly, never through a shell. A second instance refuses to open the same library instead of racing writes.
ty.
