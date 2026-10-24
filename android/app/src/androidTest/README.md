# Synthetic playback fixtures

`assets/bilingual.mp4` is a 30-second flat-colour H.264 video with two generated
sine-wave AAC tracks labelled Italian (`ita`, 440 Hz) and English (`eng`, 660 Hz).
It contains no third-party media. `assets/hls/` contains an HLS remux of that clip,
with separate Italian/English renditions and a video rendition. These fixtures
keep audio-choice and offline-download regression tests independent of providers.
They are packaged only in the instrumented-test APK, never the application APK.

Regenerate with ffmpeg (from the repository root):

```sh
assets=android/app/src/androidTest/assets
ffmpeg -y -f lavfi -i 'color=c=0x192136:s=160x90:r=10:d=30' \
  -f lavfi -i 'sine=frequency=440:sample_rate=22050:duration=30' \
  -f lavfi -i 'sine=frequency=660:sample_rate=22050:duration=30' \
  -map 0:v -map 1:a -map 2:a -c:v libx264 -preset ultrafast -crf 35 \
  -pix_fmt yuv420p -c:a aac -b:a 24k \
  -metadata:s:a:0 language=ita -metadata:s:a:0 title=Italian \
  -metadata:s:a:1 language=eng -metadata:s:a:1 title=English \
  -movflags +faststart "$assets/bilingual.mp4"
ffmpeg -y -i "$assets/bilingual.mp4" -map 0:v:0 -map 0:a:0 -map 0:a:1 \
  -c copy -f hls -hls_time 3 -hls_playlist_type vod \
  -var_stream_map 'v:0,agroup:audio a:0,agroup:audio,language:ita,default:yes a:1,agroup:audio,language:eng' \
  -master_pl_name master.m3u8 -hls_segment_filename "$assets/hls/stream_%v_%03d.ts" \
  "$assets/hls/stream_%v.m3u8"
```
