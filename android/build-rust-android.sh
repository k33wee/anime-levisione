#!/usr/bin/env bash
# Build libanime_levisione_core for Android and drop it into jniLibs/.
#
# Needs: Android NDK (ANDROID_NDK_HOME or ANDROID_HOME/ndk-bundle),
#        rustup targets, cargo-ndk. See android/README.md.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

command -v cargo ndk >/dev/null 2>&1 || cargo install cargo-ndk
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android

cargo ndk \
  -t arm64-v8a -t armeabi-v7a -t x86_64 -t x86 \
  -o android/app/src/main/jniLibs \
  build -p anime-levisione-core --features uniffi --release

echo "done: android/app/src/main/jniLibs/<abi>/libanime_levisione_core.so"
