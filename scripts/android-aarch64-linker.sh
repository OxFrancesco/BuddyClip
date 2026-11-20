#!/usr/bin/env bash
set -euo pipefail

# Cross-linker targeting aarch64-linux-android26 with 16KB ELF page alignment.
# Works seamlessly on both x86_64 Linux (using NDK's bundled clang) and aarch64 Linux
# (using host clang + lld pointed at the NDK sysroot and clang runtime).
ANDROID_SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/sdk}}"
NDK_DIR="${ANDROID_NDK_HOME:-$(ls -d "$ANDROID_SDK"/ndk/* 2>/dev/null | sort -V | tail -n 1)}"

if [[ -z "${NDK_DIR:-}" || ! -d "$NDK_DIR" ]]; then
  echo "error: Android NDK not found under $ANDROID_SDK/ndk" >&2
  exit 1
fi

TOOLCHAIN="$NDK_DIR/toolchains/llvm/prebuilt/linux-x86_64"

if [[ "$(uname -m)" == "x86_64" && -x "$TOOLCHAIN/bin/aarch64-linux-android26-clang" ]]; then
  exec "$TOOLCHAIN/bin/aarch64-linux-android26-clang" \
    -Wl,-z,max-page-size=16384 \
    "$@"
fi

SYSROOT="$TOOLCHAIN/sysroot"
CLANG_RES="$(ls -d "$TOOLCHAIN"/lib/clang/* | sort -V | tail -n 1)"

exec clang \
  --target=aarch64-linux-android26 \
  --sysroot="$SYSROOT" \
  -resource-dir="$CLANG_RES" \
  -fuse-ld=lld \
  -Wl,-z,max-page-size=16384 \
  "$@"
