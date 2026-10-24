# Android app

A streaming-first Compose UI over the same Rust core as the TUI. Dark, artwork-led
Home rails, a hero feature, searchable poster grids, and a full-screen Media3 player.

<img src="../docs/screenshots/android-home.png" alt="Android Home: featured title, poster rail and bottom navigation" width="300" />

## What works

| TUI feature | Android equivalent |
| --- | --- |
| Three-provider search and latest releases | Home rails and Search, provider and catalogue-audio filters |
| Watchlist | My List, persisted in the shared `library.json` format |
| Resume and watched episodes | Continue watching, per-episode resume, watched/unwatched controls, history |
| Pre-play audio picker | Playback waits for an explicit choice when multiple audio tracks exist; change tracks in-player too |
| Next/restart episode | Player controls, optional autoplay-next, automatic 90% watched threshold |
| Offline downloads | Native Media3 progressive/HLS downloads, quality/audio preference, pause/resume/cancel/remove, offline-only cached playback |
| AniList metadata | Optional detail enrichment (synopsis, genres, score, artwork) |
| AniList login/sync | OAuth browser authorization + pasted token/redirect URL, automatic watchlist/watched sync and manual backfill |
| Private mode | No automatic search/progress persistence or AniList requests; explicit library edits still work |
| Provider healing/config/doctor | Persistent domain cache, HTTPS mirror settings and connectivity diagnostics |

Desktop executable selection (`mpv`/VLC), subprocess arguments and `ffprobe` are
replaced by Media3's native player/track discovery. Downloads live in app-private
storage, not the desktop download directory; they do not require ffmpeg. Separate
subtitles can be selected in the player; subtitles burned into a video cannot be
turned off. Streams/available languages depend on the upstream provider.

## Build

Prerequisites: Rust 1.88+, JDK 17+, Android SDK command-line tools or Android Studio,
SDK platform 35 and NDK 27.2.12479018. The committed Gradle wrapper needs no system
Gradle installation.

```sh
sdkmanager "platforms;android-35" "ndk;27.2.12479018"
export ANDROID_HOME="$HOME/Android/Sdk"  # adjust for your machine
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.2.12479018"

# From the repository root:
make bindings                    # after changing the Rust FFI API
./android/build-rust-android.sh   # installs cargo-ndk/targets if missing
./android/gradlew -p android :app:assembleDebug :app:bundleDebug
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
adb shell am start -n dev.keewee.animelevisione/.MainActivity
```

For several attached devices, use `adb -s SERIAL …`. A running emulator works too;
list available definitions with `"$ANDROID_HOME/emulator/emulator" -list-avds`.

The APK is `app/build/outputs/apk/debug/app-debug.apk`, and the AAB is
`app/build/outputs/bundle/debug/app-debug.aab` (relative to `android/`). Release
builds need your own signing configuration before distribution. Gradle rejects
missing Rust ABI libraries; rebuild the native core after changing Rust sources.

## Test

With a device/emulator connected (CI covers API 26 and 35):

```sh
./android/gradlew -p android :app:lintDebug :app:connectedDebugAndroidTest
# Opt-in live providers, including real StreamingCommunity English-audio playback:
./android/gradlew -p android :app:connectedDebugAndroidTest -PliveProviders=true
```

The default tests exercise navigation/activity recreation, real UniFFI/JNA calls,
pre-play language selection with a synthetic bilingual clip, extensionless HLS MIME
inference, and progressive/HLS downloads that still play after their source files
are deleted. They do not depend on provider availability (Home may request the
latest catalogue in the background). The live tests need internet and can fail
when a provider/CDN is unavailable. Use a test device/emulator: tests create their
own small fixture downloads and watchlist record. `connectedDebugAndroidTest`
runs on attached devices; disconnect personal phones or filter with
`ANDROID_SERIAL=emulator-5554` when testing locally.

## Account, privacy and storage

- Settings → AniList: create your own [OAuth client], save its numeric client ID,
  authorize in your browser, then paste the token or redirect URL. No bundled
  secret/client credentials are required. Tokens are verified before saving.
- Local edits are saved first; an AniList/network failure never undoes them.
  Use **Sync library** to retry/backfill. Remote progress is never downgraded.
- Private viewing disables new automatic history/search writes and AniList
  requests. It does not erase existing history or explicit My List/watched edits.
- Library, AniList token, download database, and offline media are app-private.
  Cloud backup/device transfer are disabled to avoid copying credentials/media.
  Uninstalling the app removes them. Removing a download asks for confirmation and
  never removes progress, installed apps, or personal photos/files.
- Downloads require Wi-Fi by default. Android's notification permission enables
  foreground-download notifications. If a signed URL expires, use Retry to resolve
  a fresh one; completed downloads are played strictly from cache without network
  fallback. Quality/language preferences use the best available source rendition.
- ExoPlayer sends the provider's referer on manifests **and segments**. Extensionless
  VixCloud `/playlist/` URLs are explicitly identified as HLS, rather than MP4.
- JNA uses its Android **AAR**, not the desktop JAR. Rustls' companion Kotlin AAR
  version is read from `Cargo.lock`; `AnimeApplication` initializes its JVM context
  before HTTPS. Certificate validation remains enabled.

## Layout

- `crates/core/src/api.rs` — blocking mobile API over providers, atomic Store and AniList
- `crates/core/uniffi.toml` — Android-specific Kotlin generation
- `app/src/main/java/uniffi/` — committed generated bindings; `make bindings` regenerates them
- `AppRepository.kt` / `CatalogViewModel.kt` — application-scoped core, cancellation/state/persistence
- `BrowseScreen.kt` / `Theme.kt` — Home, Search, Library, Details, Downloads, Settings
- `PlaybackScreen.kt` — lifecycle-aware Media3 player, audio/subtitle/quality controls
- `OfflineLibrary.kt` — native download manager/service, private non-evicting offline cache

Not a Google Play distribution; side-load the APK. Provider access and media
availability remain subject to upstream services and applicable rights.

[OAuth client]: https://anilist.co/settings/developer
