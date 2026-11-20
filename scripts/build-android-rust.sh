#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LINKER="$ROOT_DIR/scripts/android-aarch64-linker.sh"
chmod +x "$LINKER"

echo "==> Building buddyclip-android (aarch64-linux-android, release, 16KB aligned)..."
CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$LINKER" \
  cargo build \
    --manifest-path "$ROOT_DIR/Cargo.toml" \
    -p buddyclip-android \
    --target aarch64-linux-android \
    --release

SRC_SO="$ROOT_DIR/target/aarch64-linux-android/release/libbuddyclip_android.so"
DST_DIR="$ROOT_DIR/android/app/src/main/jniLibs/arm64-v8a"
mkdir -p "$DST_DIR"
cp "$SRC_SO" "$DST_DIR/libbuddyclip_android.so"

echo "==> Built and staged: $DST_DIR/libbuddyclip_android.so ($(du -h "$DST_DIR/libbuddyclip_android.so" | cut -f1))"
if command -v readelf >/dev/null 2>&1; then
  echo "==> Verifying 16KB ELF LOAD segment alignment:"
  readelf -l "$DST_DIR/libbuddyclip_android.so" | grep -E "Type|LOAD" || true
fi
