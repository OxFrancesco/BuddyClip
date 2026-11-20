#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

"$ROOT_DIR/scripts/build-android-rust.sh"

if [[ -z "${JAVA_HOME:-}" ]] && command -v mise >/dev/null 2>&1; then
  JAVA_HOME="$(mise where java@zulu-21.54.109.0 2>/dev/null || true)"
  export JAVA_HOME
fi

GRADLE_ARGS=(":app:assembleDebug" "--no-daemon")

# On ARM64 Linux hosts, Android build-tools' bundled aapt2 is x86_64; pass a wrapper if present.
AAPT2_BIN="${AAPT2_OVERRIDE:-}"
if [[ -z "$AAPT2_BIN" && "$(uname -m)" == "aarch64" && -x "$HOME/build/aapt2-x86/aapt2" ]]; then
  AAPT2_BIN="$HOME/build/aapt2-x86/aapt2"
fi
if [[ -n "$AAPT2_BIN" ]]; then
  GRADLE_ARGS+=("-Pandroid.aapt2FromMavenOverride=$AAPT2_BIN")
fi

echo "==> Building BuddyClip Android APK..."
cd "$ROOT_DIR/android"
./gradlew "${GRADLE_ARGS[@]}"

APK="$ROOT_DIR/android/app/build/outputs/apk/debug/app-debug.apk"
echo "==> Built APK: $APK ($(du -h "$APK" | cut -f1))"
echo "    Install over USB/wireless ADB: adb install -r \"$APK\""
echo "    Or send via Taildrop:          tailscale file cp \"$APK\" <your-android-device>:"
