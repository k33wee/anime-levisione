#!/usr/bin/env bash
# Build libanime_levisione_core for Android and drop it into jniLibs/.
#
# Needs: Android SDK/NDK (ANDROID_HOME and optionally ANDROID_NDK_HOME).
# Installs cargo-ndk and Rust Android targets if needed. See android/README.md.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

command -v cargo-ndk >/dev/null 2>&1 || cargo install cargo-ndk --version 4.1.2 --locked
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android

cargo ndk \
  --platform 26 \
  -t arm64-v8a -t armeabi-v7a -t x86_64 -t x86 \
  -o android/app/src/main/jniLibs \
  build -p anime-levisione-core --features uniffi --release --locked

echo "done: android/app/src/main/jniLibs/<abi>/libanime_levisione_core.so"
