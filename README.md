# Clipboard Vault

A private macOS menu-bar clipboard history with a CLI. Text entries are encrypted using AES-GCM. The encryption key is generated once and kept in the current macOS user's Keychain (readable only while unlocked, never leaves the device); the encrypted history is stored in Application Support with `0600` permissions.

## Build and run

```sh
swift build -c release
.build/release/ClipboardVault
```

In another terminal:

```sh
.build/release/clipboard-vault list
.build/release/clipboard-vault search substack
.build/release/clipboard-vault copy <entry-id>
```

> After rebuilding, macOS asks once whether the new binary may read the vault key in your Keychain — choose **Always Allow**.

The app records only copied plain text, ignores empty copies and immediate duplicates, keeps the latest 250 entries, and never syncs data or contacts a network service.

## Sensitive copies (passwords)

Sometimes you copy something you don't want stored at all — a password, a one-time code. Sensitive copies go straight to the clipboard, are **never written to the vault**, and are wiped automatically after a delay (default 45 s). They carry a private pasteboard marker so the menu-bar app refuses to re-capture them.

- **Menu-bar app:** Option-click an entry (or use its context menu → *Copy Once*) for a sensitive copy. A banner shows the countdown with a *Clear Now* button.
- **CLI:** `clipboard-vault secret -` reads text from stdin and places it on the clipboard without storing it:

  ```sh
  pbpass | clipboard-vault secret -          # e.g. pipe from a password manager
  clipboard-vault copy <id> --sensitive      # re-copy a stored entry securely
  clipboard-vault latest --sensitive --clear-after 30
  ```

  Sensitive CLI copies wait until the wipe has fired so short-lived processes can't leave secrets behind; pass `--no-wait` to skip the wait (the menu-bar app then adopts the copy and wipes it).

## CLI reference

| Command | Description |
| --- | --- |
| `list [--json] [--limit N]` | List stored entries, newest first |
| `search <text> [--json] [--limit N]` | Search stored entries |
| `get <entry-id>` | Print an entry's full text to stdout |
| `copy <entry-id> [--sensitive] [--clear-after S] [--no-wait]` | Copy an entry to the clipboard |
| `latest [--sensitive] [--clear-after S] [--no-wait]` | Copy the most recent entry |
| `secret (<text> \| -) [--clear-after S] [--no-wait]` | Clipboard-only copy, never stored (`-` reads stdin) |
| `forget <entry-id>` | Delete a single entry |
| `count` | Number of stored entries |
| `clear` | Delete every entry |

`--json` emits machine-readable output (`id`, ISO-8601 `createdAt`, full `text`) for scripts and agents.

## Menu-bar app

- Live search across history
- Click an entry to copy (checkmark confirms); Option-click for a sensitive auto-clearing copy
- Right-click for Copy / Copy Once / Delete
- Relative timestamps, entry count, clear-all with confirmation
- Errors from the encrypted vault are surfaced inline

## Security notes

- AES-GCM sealed vault at `~/Library/Application Support/ClipboardVault/vault.bin`, permissions `0600` (directory `0700`)
- 256-bit key in the login Keychain as `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` — unavailable while locked, excluded from backups; older keys are upgraded in place
- Sensitive copies never touch disk and self-destruct from the clipboard
- No network access anywhere in the codebase
