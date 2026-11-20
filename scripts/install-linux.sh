#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
echo "==> Building buddyclip release binary for Linux..."
cargo build --manifest-path "$ROOT_DIR/Cargo.toml" -p buddyclip-cli --release

INSTALL_DIR="$HOME/.local/bin"
mkdir -p "$INSTALL_DIR"
cp "$ROOT_DIR/target/release/buddyclip" "$INSTALL_DIR/buddyclip"
chmod +x "$INSTALL_DIR/buddyclip"

echo "==> Installed: $INSTALL_DIR/buddyclip"
echo ""
echo "Next steps:"
echo "  1. buddyclip init                # Generate 256-bit key + print QR code"
echo "  2. buddyclip install-service     # Auto-start daemon on login (systemd user service)"
echo "  3. buddyclip status              # Verify tailnet listener & connected devices"
