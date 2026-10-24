# App icon

[`icon.svg`](icon.svg) is the editable colour artwork: a woodland TV with ivy,
twig aerials and brass knobs, combined with big anime eyes and spiky purple hair.
It takes inspiration from Melevisione's TV sign without using its lettering or
an existing anime character. This is original project artwork, not an official
Rai/Melevisione logo.

Android uses an adaptive launcher icon, plus a monochrome variant for themed
icons on Android 13+. The flat colour vector is also used in the app header.

After editing the SVG, regenerate the Android colour assets from the repo root:

```sh
python3 tools/generate-branding.py
```

This uses only Python's standard library. The generated vectors are committed;
Python is not required to build the APK. The simplified monochrome drawing is
maintained separately in
`android/app/src/main/res/drawable/ic_launcher_monochrome.xml`.
