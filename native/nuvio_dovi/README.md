# nuvio_dovi

JNI bridge from Nuvio's ExoPlayer pipeline to quietvoid's [`dolby_vision`](https://github.com/quietvoid/dovi_tool) crate.

For each HEVC access unit of a Dolby Vision **Profile 7** track it drops the enhancement layer (NAL 63) and
converts every RPU (NAL 62) to **Profile 8.1** (dovi_tool `-m 2`), the same approach Kodi uses on Android.
All other NAL units are copied byte-for-byte.

- Kotlin side: `composeApp/src/androidMain/kotlin/com/nuvio/app/features/player/dovi/`
- Rebuild the `.so` files: `ANDROID_NDK_HOME=... ./build-android.sh`
- Tests (uses dovi_tool's sample RPUs): `DOVI_TOOL_DIR=/path/to/dovi_tool cargo test --release`
