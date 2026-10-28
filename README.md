<div align="center">

<img src="docs/branding/icon.svg" width="96" alt="anime-levisione logo">

# anime-levisione

[![CI](https://github.com/k33wee/anime-levisione/actions/workflows/ci.yml/badge.svg)](https://github.com/k33wee/anime-levisione/actions/workflows/ci.yml)
![Version](https://img.shields.io/badge/version-0.2.3-73e2a7)
[![GPL-3.0-only](https://img.shields.io/badge/license-GPL--3.0--only-73e2a7)](https://www.gnu.org/licenses/gpl-3.0.html)

English · [Italiano](README.it.md)

</div>

An anime browser for **desktop terminals** and **Android**. Both clients search
AnimeWorld, AnimeUnity and StreamingCommunity, keep a watchlist and viewing
progress, and let you choose the audio language before playback.

The providers serve mainly Italian-dubbed and Italian-subtitled content. Their
domains change often; the shared Rust core checks alternative addresses when one
stops working.

**[TUI](#tui)** · **[Android app](#android-app)** · [Development](#development)

---

## TUI

The desktop client: a pane-based interface with keyboard shortcuts and mouse
support. VLC or mpv plays the video; the terminal stays available for browsing.

![TUI: search, title details, episodes and watchlist](docs/screenshots/library.png)

*Offline sample catalogue: the titles and descriptions shown are fictional.*

### Install and run

Requires **Rust 1.88+** and **VLC or mpv**. Install `ffmpeg` for downloads;
`ffprobe` is optional for audio-track detection outside HLS. Runs on Linux,
macOS and Windows/WSL. Release archives do not bundle a media player.

On Windows, VLC is detected in its standard Program Files locations, beside
`anime-levisione.exe`, or through `PATH`. For a custom installation, run e.g.
`anime-levisione.exe --player "C:/path/to/vlc.exe"`.

```sh
git clone https://github.com/k33wee/anime-levisione
cd anime-levisione
cargo install --path crates/tui --locked

anime-levisione "frieren"
```

Other ways to start it:

```sh
anime-levisione --demo          # offline preview; no network or saved data
anime-levisione --player mpv    # choose a player
anime-levisione --private       # disable AniList and automatic history writes
anime-levisione doctor          # check player, configuration and providers
```

### Controls

| Key | Action |
| --- | --- |
| `/` | Search |
| `Tab` / `Shift+Tab`, `0`–`7` | Switch or jump to a pane |
| `j/k`, arrows | Navigate |
| `Enter` | Open a title / play the selected episode |
| `s`, `n` | Restart / next episode |
| `w`, `m`, `d` | Watchlist / mark watched / download |
| `Ctrl+x`, `Ctrl+d` | Stop playback / cancel download |
| `?`, `:`, `q` | Shortcuts / command palette / quit |

Click to focus a pane or select a row; scroll with the wheel. The layout fits
from 60×18; smaller windows show the focused pane. When several audio languages
are available, select one with the arrows and confirm with `Enter`.

### Library and AniList

Watchlist, history and resume points are stored locally. Press `,` to see data
paths; `config.toml` holds the player, its arguments, download directory and
provider mirrors. Use `--data-dir` for a separate library.

AniList is optional. [Create an OAuth client](https://anilist.co/settings/developer),
then run:

```sh
anime-levisione login --client-id YOUR_ID
anime-levisione sync
anime-levisione logout
```

Login is also available in the command palette. Sync sends your watchlist and
watched progress without lowering existing AniList progress. Private mode stops
AniList requests and automatic history/search writes, but does not erase your
existing library or prevent explicit watchlist edits.

---

## Android app

The mobile client uses Compose and Media3: no external player, `ffmpeg` or
`ffprobe` needed. Requires **Android 8.0 or newer**.

<p>
<img src="docs/screenshots/android-home.png" width="210" alt="Android home">
<img src="docs/screenshots/android-detail.png" width="210" alt="Title details and episodes">
<img src="docs/screenshots/android-player-audio.png" width="210" alt="Audio language selection">
</p>

*Screenshots use [original sample artwork and synthetic video](docs/screenshots/README.md),
not provider posters or footage.*

- **Browse:** featured title, provider rows, search filters and My List.
- **Watch:** resume points, watched episodes, history and optional autoplay-next.
- **Player:** audio selection before playback, subtitles, quality limit and
  landscape fullscreen. Tap the video for play/pause, seeking and other controls;
  they hide during playback rather than shrinking the video.
- **Download:** HLS/MP4 episodes with audio and quality preferences, Wi-Fi only by
  default. Completed downloads play from the app's cache even if the source link
  expires. Pause, resume and remove them from Downloads.
- **Settings:** AniList metadata/login/sync, private mode, provider mirrors and
  connectivity diagnostics.

<img src="docs/screenshots/android-fullscreen.png" width="680" alt="Landscape fullscreen player">

### Get the app

Installers are attached to the [releases](https://github.com/k33wee/anime-levisione/releases/latest):

- `anime-levisione-<version>-android.apk` — signed Android package (Android 8+)
- `anime-levisione-<version>-<platform>.tar.gz|.zip` — TUI binaries for Linux
  (glibc 2.35+), macOS (Apple Silicon and Intel) and Windows x86_64
- `SHA256SUMS` — checksums for every asset

Download on the phone and open the APK, or install with ADB:

```sh
adb install anime-levisione-<version>-android.apk
```

The release APK is signed with the project's release key. Debug builds (CI
artifacts, `assembleDebug`) use a debug key: Android refuses to update one with
the other, and uninstalling deletes the app's local library and downloads.
Prefer the release APK.

<details>
<summary>Alternatives</summary>

- Debug APK and AAB from any successful
  [CI run on main](https://github.com/k33wee/anime-levisione/actions/workflows/ci.yml?query=branch%3Amain)
  (`android-debug` artifact, GitHub sign-in required). The AAB is not directly
  installable.
- Build from source — set up Rust, JDK 17+, Android SDK and NDK as described in
  the [Android build guide](android/README.md#build), then:

```sh
./android/build-rust-android.sh
./android/gradlew -p android :app:assembleDebug
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
```

</details>

### Library and account

Downloads, library data and AniList credentials stay in app-private storage.
Removing a download keeps your progress; uninstalling the app removes its data.
The library format matches the TUI, but files and resume positions are not
automatically shared between devices.

To connect AniList, enter your OAuth client ID in Settings, authorize in the
browser and paste the token or redirect URL. Private viewing disables new
automatic history/search writes and AniList requests, not existing library data
or explicit edits.

[Android setup, tests and storage details →](android/README.md)

---

## Development

```sh
make help        # available targets
make ci          # fmt, clippy, tests and commitlint; run before pushing
make bindings    # regenerate Kotlin bindings after core API changes
make hooks       # enable commit-msg and pre-push hooks
make act         # run the GitHub workflow locally; requires Docker
```

| Path | Contents |
| --- | --- |
| `crates/core` | Providers, library and AniList; shared through UniFFI |
| `crates/tui` | Terminal application |
| `crates/bindgen` | Kotlin bindings generator |
| `android` | Android application and build scripts |
| `docs/branding` | [Editable icon and asset generation](docs/branding/README.md) |

CI checks Rust on Linux, macOS and Windows, verifies generated bindings, and
builds/lints/tests Android on API 26 and 35 emulators. Commits follow
[Conventional Commits](https://www.conventionalcommits.org).

## License

GPL-3.0-only. No content is bundled. The project is not affiliated with its
providers, AniList or Rai/Melevisione. Availability depends on the upstream
services; use them in accordance with applicable rights and terms.
