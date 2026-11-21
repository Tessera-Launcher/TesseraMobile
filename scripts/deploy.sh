#!/bin/sh
# Build the debug APK and put it on the connected phone.
#
# Slint's Android backend pulls prebuilt Skia binaries that need the C++ runtime
# of NDK r30 or newer; older NDKs build but crash at startup. Point
# ANDROID_NDK_ROOT somewhere else to override.

set -e
cd "$(dirname "$0")/.."

ANDROID_HOME=${ANDROID_HOME:-$HOME/java/android-sdk}
ANDROID_NDK_ROOT=${ANDROID_NDK_ROOT:-$HOME/android-ndk-r30}
ANDROID_NDK_HOME=$ANDROID_NDK_ROOT
PATH="$ANDROID_HOME/build-tools/34.0.0:$PATH"
export ANDROID_HOME ANDROID_NDK_ROOT ANDROID_NDK_HOME PATH

TARGET=${TARGET:-aarch64-linux-android}
APK=target/debug/apk/tessera.apk
PACKAGE=org.tessera.mobile
ACTIVITY=android.app.NativeActivity

cargo apk2 build --target "$TARGET" --lib
adb install -r "$APK"
adb shell am force-stop "$PACKAGE"
adb shell am start -n "$PACKAGE/$ACTIVITY" >/dev/null
echo "launched on $(adb shell getprop ro.product.model | tr -d '\r')"
