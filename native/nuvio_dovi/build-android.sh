#!/usr/bin/env bash
# Builds libnuvio_dovi.so for every Android ABI and copies it into androidApp/src/main/jniLibs.
# Needs: rustup targets aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android,
# and ANDROID_NDK_HOME (tested with NDK 28.2.13676358).
set -euo pipefail
cd "$(dirname "$0")"
: "${ANDROID_NDK_HOME:?set ANDROID_NDK_HOME to your NDK}"
BIN="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$(uname -s | tr '[:upper:]' '[:lower:]')-x86_64/bin"
OUT=../../androidApp/src/main/jniLibs
build() {
  local target=$1 abi=$2 clang=$3
  local var="CARGO_TARGET_$(echo "$target" | tr '[:lower:]-' '[:upper:]_')_LINKER"
  export "$var=$BIN/$clang"
  RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384" cargo build --release --target "$target"
  mkdir -p "$OUT/$abi" && cp "target/$target/release/libnuvio_dovi.so" "$OUT/$abi/"
}
build aarch64-linux-android   arm64-v8a   aarch64-linux-android24-clang
build armv7-linux-androideabi armeabi-v7a armv7a-linux-androideabi24-clang
build x86_64-linux-android    x86_64      x86_64-linux-android24-clang
build i686-linux-android      x86         i686-linux-android24-clang
