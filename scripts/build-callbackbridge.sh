#!/bin/sh
# Build the game-side CallbackBridge and place it in the shared game directory.
set -e
cd "$(dirname "$0")/.."
OUT=$(mktemp -d)
trap 'rm -rf "$OUT" "$OUT.jar"' EXIT
javac -source 8 -target 8 -Xlint:-options -d "$OUT" \
  android/game-java/org/lwjgl/glfw/CallbackBridge.java
jar cf "$OUT.jar" -C "$OUT" .
adb push "$OUT.jar" /sdcard/Tessera/游戏/fcl-callbackbridge.jar
echo "pushed fcl-callbackbridge.jar"
