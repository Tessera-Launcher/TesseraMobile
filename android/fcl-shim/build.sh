#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
export ANDROID_NDK_ROOT="${ANDROID_NDK_ROOT:-$HOME/android-ndk-r30}"
export CC_aarch64_linux_android="$ANDROID_NDK_ROOT/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android26-clang"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
export RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384"
cargo build --release --target aarch64-linux-android
cp target/aarch64-linux-android/release/libfcl.so ../jniLibs/arm64-v8a/libfcl.so
