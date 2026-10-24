#!/usr/bin/env bash
# Offline documentation captures. Creates/deletes its own temporary AVD only.
# Requires Python 3 + Pillow, DejaVu fonts, ImageMagick, Rust and Android SDK.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
ROOT="$PWD"
OUT="$ROOT/target/doc-screenshots"
mkdir -p "$OUT/new-images"
export ANIME_SCREENSHOT_DIR="$OUT"

python3 tools/screenshot-fixtures.py generate
python3 tools/screenshot-fixtures.py verify
for shot in browse audio; do
  cargo run -q -p anime-levisione --example screenshot -- "$shot"
done
convert -background '#0b0e14' "$OUT/tui-browse.svg" -resize 1200x "$OUT/new-images/library.png"
convert -background '#0b0e14' "$OUT/tui-audio.svg" -resize 1200x "$OUT/new-images/audio-picker.png"

: "${ANDROID_HOME:?Set ANDROID_HOME to your Android SDK}"
PORT="${PORT:-5560}"
SERIAL="emulator-$PORT"
AVD="Anime_Documentation_$$"
IMAGE="system-images;android-35;google_apis;x86_64"
EMULATOR_PID=""
AVD_CREATED=false
# Never attach to or wipe an existing device.
if adb devices | awk '{print $1}' | grep -Fxq "$SERIAL"; then
  echo "$SERIAL is in use; choose an unused even PORT" >&2
  exit 1
fi
cleanup() {
  if [ -n "$EMULATOR_PID" ] && kill -0 "$EMULATOR_PID" 2>/dev/null; then
    timeout 10 adb -s "$SERIAL" emu kill >/dev/null 2>&1 || kill "$EMULATOR_PID" 2>/dev/null || true
    wait "$EMULATOR_PID" 2>/dev/null || true
  fi
  if "$AVD_CREATED"; then avdmanager delete avd --name "$AVD" >/dev/null 2>&1 || true; fi
}
trap cleanup EXIT
printf 'no\n' | avdmanager create avd --name "$AVD" --package "$IMAGE" --device pixel > "$OUT/avd.log" 2>&1
AVD_CREATED=true
"$ANDROID_HOME/emulator/emulator" -avd "$AVD" -port "$PORT" -no-window -no-audio -no-boot-anim \
  -no-snapshot -gpu swiftshader_indirect > "$OUT/emulator.log" 2>&1 &
EMULATOR_PID=$!
timeout 180 adb -s "$SERIAL" wait-for-device
ready=false
for i in $(seq 1 120); do
  if [ "$(adb -s "$SERIAL" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ]; then ready=true; break; fi
  sleep 2
done
"$ready" || { echo 'Emulator did not boot' >&2; exit 1; }
adb -s "$SERIAL" shell wm size 1080x2400
for setting in animator_duration_scale window_animation_scale transition_animation_scale; do
  adb -s "$SERIAL" shell settings put global "$setting" 0
done
adb -s "$SERIAL" shell settings put secure immersive_mode_confirmations confirmed
adb -s "$SERIAL" shell cmd connectivity airplane-mode enable
adb -s "$SERIAL" shell svc wifi disable
adb -s "$SERIAL" shell svc data disable
sleep 3

./android/gradlew -p android :app:assembleDebug :app:assembleDebugAndroidTest --console=plain > "$OUT/build.log" 2>&1 || {
  tail -50 "$OUT/build.log"; exit 1;
}
adb -s "$SERIAL" install -r android/app/build/outputs/apk/debug/app-debug.apk >/dev/null
adb -s "$SERIAL" install -r android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk >/dev/null
adb -s "$SERIAL" shell am instrument -w -r -e captureScreenshots true \
  -e class 'dev.keewee.animelevisione.DemoScreenshotsTest' \
  dev.keewee.animelevisione.test/androidx.test.runner.AndroidJUnitRunner | tee "$OUT/instrumentation.log"
grep -Fq 'OK (1 test)' "$OUT/instrumentation.log"
for name in home detail player-audio player-controls fullscreen settings; do
  raw="$OUT/android-$name.png"
  adb -s "$SERIAL" exec-out run-as dev.keewee.animelevisione cat \
    "cache/screenshots/android-$name.png" > "$raw"
  width=540
  if [ "$name" = fullscreen ]; then width=960; fi
  convert "$raw" -resize "${width}x" "$OUT/new-images/android-$name.png"
done
python3 - <<'PY'
from pathlib import Path
from PIL import Image
output = Path('target/doc-screenshots/new-images')
expected = {'library.png', 'audio-picker.png'} | {
    f'android-{name}.png' for name in ['home','detail','player-audio','player-controls','fullscreen','settings']
}
assert {p.name for p in output.iterdir()} == expected
for name in sorted(expected):
    image = Image.open(output/name)
    assert image.width >= 500 and image.height >= 300, (name, image.size)
    if name == 'android-fullscreen.png':
        assert image.width > image.height
    elif name.startswith('android-'):
        assert image.height > image.width
    print(name, image.size)
PY
# Replace the documentation only after every capture and validation succeeded.
cp "$OUT/new-images/"*.png docs/screenshots/
echo 'Updated all eight documentation screenshots with synthetic content.'
