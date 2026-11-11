# Android screen server

`scrcpy-server-v5.0.1` is the unmodified official Genymobile scrcpy Android server,
licensed under Apache-2.0. Its source is https://github.com/Genymobile/scrcpy/tree/v5.0.1.

The version, download URL and SHA-256 are pinned in
`scripts/release/android-package.json`. Run `scripts/release/prepare_android.py`
to obtain and verify the asset. No APK is installed; the server runs as the
authorized ADB shell user and is removed when its screen session closes.
