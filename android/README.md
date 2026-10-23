# Android app (skeleton)

Compose UI over the shared Rust core (`crates/core`) via [UniFFI]:
search across providers, episode lists, playback with Media3/ExoPlayer.
The Rust core handles providers, domain healing and stream resolution;
the app only renders and plays.

## Layout

- `app/src/main/java/uniffi/anime_levisione_core/` — generated Kotlin
  bindings (**committed**: regenerate with `make bindings` from the repo
  root after touching the core API)
- `app/src/main/java/dev/keewee/animelevisione/` — the Compose app
- `build-rust-android.sh` — cross-builds `libanime_levisione_core.so`
  for the four ABIs into `app/src/main/jniLibs/`

## Prerequisites

- Android Studio (or command-line tools) with SDK 35 and an NDK
- Rust with the Android targets:
  `rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android`
- [cargo-ndk]: `cargo install cargo-ndk`

## Build

```sh
make bindings                 # regenerate the Kotlin bindings (host build)
./android/build-rust-android.sh   # .so for every ABI into jniLibs/

cd android
./gradlew :app:assembleDebug   # needs JDK 17+
adb install app/build/outputs/apk/debug/app-debug.apk
```

No Gradle wrapper is committed; generate one with `gradle wrapper` or
open the folder in Android Studio once.

## Notes

- All `AnimeLib` methods are blocking (they drive the Rust tokio
  runtime and hit the network): the app calls them on `Dispatchers.IO`.
- AniList and the local library are not exposed through the FFI yet —
  that is the next step (`anime-levisione-core::api`).
- Not distributable on Google Play (scraping-based providers);
  side-load or F-Droid style distribution only.

[UniFFI]: https://mozilla.github.io/uniffi-rs/
[cargo-ndk]: https://github.com/bbqsrc/cargo-ndk
