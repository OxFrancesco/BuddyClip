#!/usr/bin/env bash
set -euo pipefail

REPO="${BUDDYCLIP_REPO:-OxFrancesco/BuddyClip}"
VERSION="${BUDDYCLIP_VERSION:-latest}"
INSTALL_DIR="${BUDDYCLIP_INSTALL_DIR:-$HOME/.local/bin}"
AUTO_SERVICE=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION="${2:?missing value for --version}"
      shift 2
      ;;
    --dir)
      INSTALL_DIR="${2:?missing value for --dir}"
      shift 2
      ;;
    --service|--setup)
      AUTO_SERVICE=1
      shift
      ;;
    -h|--help)
      cat <<'EOF'
BuddyClip Linux installer

Usage:
  curl -fsSL https://raw.githubusercontent.com/OxFrancesco/BuddyClip/main/install.sh | bash
  curl -fsSL https://raw.githubusercontent.com/OxFrancesco/BuddyClip/main/install.sh | bash -s -- --service

Options:
  --version <tag>   Install a specific release tag (default: latest)
  --dir <path>      Installation directory (default: ~/.local/bin)
  --service         Initialize config (if missing) and install+start systemd user service
EOF
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

OS="$(uname -s)"
if [[ "$OS" != "Linux" ]]; then
  echo "error: BuddyClip's Linux installer supports Linux (detected: $OS)." >&2
  exit 1
fi

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64)
    ASSET="buddyclip-linux-x86_64"
    ;;
  aarch64|arm64)
    ASSET="buddyclip-linux-aarch64"
    ;;
  *)
    echo "error: unsupported CPU architecture '$ARCH' (expected x86_64 or aarch64)." >&2
    exit 1
    ;;
esac

if [[ "$VERSION" == "latest" ]]; then
  BASE_URL="https://github.com/$REPO/releases/latest/download"
else
  BASE_URL="https://github.com/$REPO/releases/download/$VERSION"
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "==> Downloading $ASSET from $REPO ($VERSION)..."
if ! curl -fsSL "$BASE_URL/$ASSET" -o "$TMP_DIR/buddyclip"; then
  echo "error: failed to download $BASE_URL/$ASSET" >&2
  echo "You can also build from source: git clone https://github.com/$REPO && cd BuddyClip && ./scripts/install-linux.sh" >&2
  exit 1
fi

if curl -fsSL "$BASE_URL/checksums.txt" -o "$TMP_DIR/checksums.txt" 2>/dev/null; then
  EXPECTED="$(awk -v f="$ASSET" '$2 == f || $2 == "*"f {print $1}' "$TMP_DIR/checksums.txt" | head -n 1)"
  if [[ -n "$EXPECTED" ]] && command -v sha256sum >/dev/null 2>&1; then
    ACTUAL="$(sha256sum "$TMP_DIR/buddyclip" | awk '{print $1}')"
    if [[ "$ACTUAL" != "$EXPECTED" ]]; then
      echo "error: SHA256 checksum mismatch for $ASSET!" >&2
      echo "  expected: $EXPECTED" >&2
      echo "  actual:   $ACTUAL" >&2
      exit 1
    fi
    echo "==> SHA256 verified ($ACTUAL)"
  fi
fi

mkdir -p "$INSTALL_DIR"
install -m 0755 "$TMP_DIR/buddyclip" "$INSTALL_DIR/buddyclip"
echo "==> Installed BuddyClip to $INSTALL_DIR/buddyclip"

if ! command -v wl-copy >/dev/null 2>&1 && ! command -v xclip >/dev/null 2>&1; then
  echo ""
  echo "Note: install 'wl-clipboard' (Wayland) or 'xclip' (X11) for automatic desktop clipboard sync."
fi

if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
  echo ""
  echo "Note: add $INSTALL_DIR to your PATH (e.g. export PATH=\"$INSTALL_DIR:\$PATH\")."
fi

if [[ "$AUTO_SERVICE" -eq 1 ]]; then
  CFG="${XDG_CONFIG_HOME:-$HOME/.config}/buddyclip/config.toml"
  if [[ ! -f "$CFG" ]]; then
    "$INSTALL_DIR/buddyclip" init
  fi
  "$INSTALL_DIR/buddyclip" install-service
else
  echo ""
  echo "Next steps:"
  echo "  1. buddyclip init              # Generate 256-bit key & show Android QR code"
  echo "  2. buddyclip install-service   # Auto-start encrypted tailnet daemon on login"
  echo "  3. Install the Android APK from: https://github.com/$REPO/releases/latest"
fi
