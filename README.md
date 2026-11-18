<div align="center">

  <img src="https://nuvio.tv/assets/nuvio-app-logo-wordmark.webp" alt="Nuvio" width="320" />

  <p>
    A free, open-source media app for your phone, your desktop, and the TV you already own.
    <br />
    Bring your own sources. Nuvio turns them into a library with artwork, ratings, subtitles, and your place saved on every screen.
  </p>

  [Website](https://nuvio.tv) · [GitHub releases](https://github.com/NuvioMedia/NuvioMobile/releases/latest) · [Support Nuvio](https://nuvio.tv/support)

</div>

> **This is Nuvio DoVi**, a fork of [NuvioMedia/NuvioMobile](https://github.com/NuvioMedia/NuvioMobile) that fixes Dolby Vision playback on Android devices without full Dolby Vision support. The changes are described below; everything else is upstream Nuvio.

## Nuvio DoVi: Dolby Vision fixes

### The problem

On many Android phones and TVs, some Dolby Vision streams played with **sound but a black screen**:

- **Profile 7** (UHD Blu-ray remuxes): Media3/ExoPlayer has no fallback decoder for Profile 7, so devices whose Dolby Vision decoder only supports Profile 8 (or that have none) cannot play the video at all.
- **Profile 5** (most streaming WEB-DL releases): Profile 5 has no HDR10 base layer. Its pixels are in Dolby's IPT-PQ-c2 colour space, so a device without a Dolby Vision decoder (for example the Samsung Galaxy S26 Ultra) has nothing to fall back to. ExoPlayer silently played audio only.

### What changed

| | Profile 7 (remuxes) | Profile 5 (WEB-DL) |
|---|---|---|
| Before | Black screen | Black screen |
| Now, device with a DV decoder | Real Dolby Vision (converted to Profile 8.1) | Real Dolby Vision (unchanged) |
| Now, device without a DV decoder | HDR10 base layer | Correct colours in **HDR10** via libmpv |

#### 1. On-the-fly Profile 7 to 8.1 conversion (ExoPlayer)

The same approach Kodi uses on Android:

1. An extractor wrapper sees every HEVC access unit after the container (MKV/MP4) is read and before it reaches `MediaCodec`.
2. Enhancement-layer NAL units (type 63) are dropped. Profile 8.1 has no enhancement layer.
3. Every RPU (NAL type 62) is converted to Profile 8.1 with [quietvoid's dovi_tool](https://github.com/quietvoid/dovi_tool) `dolby_vision` crate, mode `-m 2`.
4. The track is relabelled `dvhe.07.xx` to `dvhe.08.xx`, so the device's Dolby Vision Profile 8 decoder is used, or the HEVC HDR10 fallback on devices without one.

All other NAL units are passed through byte-for-byte. The conversion is tested against dovi_tool's reference outputs (the MEL result is byte-identical to `mel_to_81`).

New setting: **Settings > Playback > Convert DV Profile 7 to 8.1** (on by default).

#### 2. Profile 5 fallback to libmpv (no more audio-only playback)

When the only video track is Dolby Vision and the device has no decoder for it, ExoPlayer now reports a playback error instead of playing audio only. Nuvio's existing **Auto-switch engine on playback error** setting (on by default) then reopens the stream in **libmpv** at the same position.

#### 3. Profile 5 colour reshaping in libmpv

For a Dolby Vision Profile 5 file, libmpv switches from direct MediaCodec decoding to software decoding. FFmpeg then exports the RPU and libplacebo (`vo=gpu-next`) reshapes IPT-PQ-c2 into correct colours on the GPU.

#### 4. HDR output from libmpv

libmpv now prefers the **Vulkan** Android context (`gpu-context=androidvk,android`) with `target-colorspace-hint=yes`. HDR and Dolby Vision content is sent to the display as HDR10 (BT.2020 PQ, 10-bit) instead of being tone-mapped to SDR. If the driver refuses Vulkan, it falls back to OpenGL (SDR output).

Verified on a Galaxy S26 Ultra: SurfaceFlinger reports the video layer as `BT2020_PQ`, `RGBA_1010102`, with HDR metadata.

#### 5. Side-by-side install

The `full` flavor uses the application ID `com.nuvio.app.dovi` and the name **Nuvio DoVi**, so it installs next to the official Nuvio app.

### Logs

Everything is logged under the `NuvioDovi` tag:

```bash
adb logcat -s NuvioDovi
```

Typical lines: `Track 1 video format: ... codecs=dvhe.07.06`, `dvhe.07.06 -> dvhe.08.06`, `ExoPlayer has no decoder for Dolby Vision dvhe.05.03; reporting to switch engine`, `libmpv output: context=androidvk ... target=pq/bt.2020`.

### Where the code is

- `native/nuvio_dovi/`: Rust JNI library (`libnuvio_dovi.so`) wrapping dovi_tool's `dolby_vision` crate, with tests and `build-android.sh`. Prebuilt libraries for all ABIs are in `androidApp/src/main/jniLibs/`.
- `composeApp/src/androidMain/kotlin/com/nuvio/app/features/player/dovi/`: the extractor wrapper and JNI bindings.
- `composeApp/src/androidMain/kotlin/com/nuvio/app/features/player/PlayerEngine.android.kt`: the ExoPlayer unsupported-Dolby-Vision check, libmpv Profile 5 software decoding, and the Vulkan HDR output options.

### Building Nuvio DoVi

Create `local.properties` with your SDK path, the Supabase values, and (for a signed release) your keystore:

```properties
sdk.dir=/path/to/android-sdk
NUVIO_SUPABASE_URL=...
NUVIO_SUPABASE_ANON_KEY=...
NUVIO_SUPABASE_FALLBACK_URL=...
NUVIO_RELEASE_STORE_FILE=/path/to/keystore.jks
NUVIO_RELEASE_STORE_PASSWORD=...
NUVIO_RELEASE_KEY_ALIAS=...
NUVIO_RELEASE_KEY_PASSWORD=...
```

Then build:

```bash
./gradlew :androidApp:assembleFullRelease -PreleaseMinifyEnabled=false
```

The APK for most phones is `androidApp/build/outputs/apk/full/release/androidApp-full-arm64-v8a-release.apk`.

To rebuild the native library after changing `native/nuvio_dovi`:

```bash
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android
ANDROID_NDK_HOME=/path/to/ndk native/nuvio_dovi/build-android.sh
```

### Known limitations

- Profile 7 with the enhancement layer in a separate track (some `.ts`/`.m2ts` files) is not converted.
- Profile 5 in libmpv uses software decoding. 1080p is easy; 4K Profile 5 may be heavy on some devices.
- On devices without a Dolby Vision decoder, the result is HDR10 rather than Dolby Vision.
- libmpv uses Vulkan for all content, not only HDR. If something looks wrong in libmpv, this is the first thing to check.


## Get Nuvio Mobile

- [Android on Google Play](https://play.google.com/store/apps/details?id=com.nuvio.app)
- [Android APK](https://github.com/NuvioMedia/NuvioMobile/releases/latest)
- iOS via AltStore or SideStore: add [this source URL](https://raw.githubusercontent.com/NuvioMedia/NuvioMobile/cmp-rewrite/store.json) in the app's Sources section, then install Nuvio.
- iOS beta builds: add [the beta source URL](https://raw.githubusercontent.com/NuvioMedia/NuvioMobile/cmp-rewrite/store-beta.json) instead to get pre-releases as well.

## Build from source

```bash
git clone https://github.com/NuvioMedia/NuvioMobile.git
cd NuvioMobile
```

### Android

Android development requires Android Studio and the Android SDK.

```bash
./gradlew :androidApp:assembleFullDebug
```

### iOS

iOS development requires macOS and Xcode.

```bash
env NUVIO_IOS_DISTRIBUTION=full xcodebuild \
  -project iosApp/iosApp.xcodeproj \
  -scheme iosApp \
  -configuration Debug \
  -sdk iphonesimulator \
  -derivedDataPath build/ios-derived-full-simulator \
  CODE_SIGNING_ALLOWED=NO \
  build
```

The shared app is built with Kotlin Multiplatform and Compose Multiplatform.

## License

[GNU General Public License v3.0](./LICENSE)
