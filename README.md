# BuddyClip

> **Highly experimental (`v0.1.3`)** — early personal tool, not independently security-audited, protocol and CLI flags may change.

A fast, zero-cloud, **end-to-end encrypted universal clipboard** built in **Rust** for **Linux PC (Wayland / X11)** and **Android**, working strictly over your private **Tailscale tailnet**.

**Landing page:** [oxfrancesco.github.io/BuddyClip](https://oxfrancesco.github.io/BuddyClip/)

Both the Linux daemon and the Android app share the exact same Rust core ([`buddyclip-core`](crates/buddyclip-core)) — compiled natively to a 16KB-page-aligned ARM64 JNI library (`libbuddyclip_android.so`) for Android.

---

## Install

### Linux PC (One-Line Installer)

Installs the prebuilt `buddyclip` binary (`x86_64` or `aarch64`) to `~/.local/bin/buddyclip` with SHA256 verification:

```bash
curl -fsSL https://raw.githubusercontent.com/OxFrancesco/BuddyClip/main/install.sh | bash
```

Then initialize your 256-bit key and enable automatic background startup:

```bash
buddyclip init              # generates key, detects Tailscale IP, prints pairing QR code
buddyclip install-service   # installs & starts ~/.config/systemd/user/buddyclip.service
```

*(Or run the installer with `--service` to do all three in one go: `curl -fsSL https://raw.githubusercontent.com/OxFrancesco/BuddyClip/main/install.sh | bash -s -- --service`)*

### Android App

1. Download **`BuddyClip-*-android-arm64.apk`** from the [**Latest GitHub Release**](https://github.com/OxFrancesco/BuddyClip/releases/latest) (directly on your phone or send via Taildrop: `tailscale file cp BuddyClip-*-android-arm64.apk <your-phone>:`).
2. Install the APK on Android.
3. Open **BuddyClip** → tap **Scan QR** → scan the terminal QR code from `buddyclip init` (or `buddyclip pair`).

---

## Seamless Workflow

### PC → Android (Automatic, 0 taps)
- Press `Ctrl+C` on your PC (plain text or PNG/JPEG screenshot).
- `wl-paste --watch` captures it immediately, encrypts it, and pushes it over the persistent Tailscale connection.
- Android's foreground service writes it straight to the Android system clipboard and displays a quiet preview notification with a 1-tap **Copy** fallback.

### Android → PC (1 tap from anywhere)
- **Floating text-selection menu:** Highlight text in any app → tap **"Send to PC (BuddyClip)"** right next to Copy (`ACTION_PROCESS_TEXT`).
- **Share Sheet:** Share any link, text, or image → pick **BuddyClip** (`ACTION_SEND`).
- **Quick Settings Tile:** Swipe down → tap **"Send to PC"** (50ms invisible focus trampoline reads Android's clipboard and sends without switching apps).
- **Persistent Notification:** Tap **"Send Clipboard"** directly from the notification shade.
- **Open App:** Auto-syncs your current Android clipboard on open and shows RAM-only history with image previews.

---

## Security Architecture

1. **Strictly Tailnet-Only (`100.64.0.0/10` & `fd7a:115c:a1e0::/48`)**
   - The Linux daemon binds exclusively to your local Tailscale IP — never `0.0.0.0` or LAN.
   - Both server and client reject any IP outside Tailscale's CGNAT/ULA prefixes before exchanging a single protocol byte.
2. **End-to-End Encryption with Perfect Forward Secrecy (on top of WireGuard)**
   - Every TCP session performs a mutually-authenticated **X25519 ephemeral ECDH + 256-bit PSK** handshake (`BLAKE3` keyed tags, constant-time verification, ±5 minute replay window).
   - Directional session keys and nonces are derived via `BLAKE3` KDF and encrypted with **XChaCha20-Poly1305 AEAD** using strictly monotonic frame sequence counters and authenticated framing headers (AAD).
   - Even another device on your tailnet without your 256-bit key cannot connect, read, inject, or replay clips.
3. **Zero Plaintext Disk History & Sensitive Clip Filtering**
   - Synced clips live in **RAM only** (bounded ring buffer: 25 items / 64 MiB).
   - On Wayland, password manager copies (`CLIPBOARD_STATE=sensitive`, `x-kde-passwordManagerHint`, KeePassXC/Bitwarden/1Password hints) are automatically skipped.
   - On Android 13+, clips flagged with `ClipDescription.EXTRA_IS_SENSITIVE` are automatically skipped.
   - On Android, your paired key is encrypted at rest using a hardware-backed **Android Keystore AES-256-GCM** key.

---

## Tailscale ACL Note (Tagged Devices)

If your Android phone uses a Tailscale tag (for example `tag:phone`) with restricted ACLs, allow TCP port `9455` to your PC in the Tailscale Admin Console **Access Controls**:

```json
{
  "action": "accept",
  "src": ["tag:phone"],
  "dst": ["autogroup:self:9455"]
}
```

*(Run `buddyclip peers` on your PC to automatically check for tagged devices and print the exact ACL snippet.)*

---

## CLI Reference

| Command | Description |
|---|---|
| `buddyclip init` | Generate 256-bit key (or `--passphrase` / `--pair-uri`), detect Tailscale IP, print QR code |
| `buddyclip daemon` | Run the tailnet-only encrypted clipboard daemon |
| `buddyclip install-service` | Install, enable, and start `~/.config/systemd/user/buddyclip.service` |
| `buddyclip pair` | Re-display the terminal QR code and `buddyclip://pair?...` URI |
| `buddyclip status` | Show Tailscale bind address, key fingerprint, clipboard backend, and connected peers |
| `buddyclip history` | List recent in-memory synced clips |
| `buddyclip copy [text]` | Push text (args or stdin) to local clipboard and all connected tailnet devices |
| `buddyclip paste` | Output the latest synced clip to stdout |
| `buddyclip clear` | Wipe in-memory clipboard history |
| `buddyclip peers` | Inspect Tailscale peers and tagged ACL requirements |

---

## Building from Source

### Linux
```bash
./scripts/install-linux.sh
```

### Android APK (with Rust JNI library)
Requires Rust (`rustup target add aarch64-linux-android`), Android SDK, Android NDK 29+, and Java 21:
```bash
./scripts/build-apk.sh
```

## Project Layout

- [`crates/buddyclip-core`](crates/buddyclip-core) — Pure-Rust E2EE crypto, binary protocol, Tailscale IP enforcement, pairing URI, and persistent sync client.
- [`crates/buddyclip-cli`](crates/buddyclip-cli) — Linux Wayland/X11 daemon, CLI, local IPC socket, terminal QR renderer, and systemd installer.
- [`crates/buddyclip-android`](crates/buddyclip-android) — Rust `cdylib` (`libbuddyclip_android.so`) JNI bridge for Android.
- [`android/`](android) — Material 3 Jetpack Compose Android app.
- [`install.sh`](install.sh) — One-line Linux installer downloading verified release binaries.
