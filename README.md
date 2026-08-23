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

The app records only copied plain text, ignores empty copies, aggregates repeated copies of the same text into a single entry (see below), keeps the latest 250 entries, and never syncs data or contacts a network service.

## Global shortcut

Open the panel from anywhere with a keyboard shortcut you choose:

- Default is **⇧⌘V**; change it in the panel under *Panel Shortcut* — click **Press keys…** and press any combo (Esc cancels).
- The shortcut works system-wide (registered via `RegisterEventHotKey`, no Accessibility permission needed) and toggles the panel open/closed.
- If another app already owns a combo you record, the panel shows a warning so you can pick a different one; the toggle switches the hotkey off entirely.

## Duplicate copies are one entry

Copying the same text again never creates a second row. The existing entry jumps to the top of the list, its timestamp becomes the most recent copy, and its capture count grows (`×3`, `×4`, …). **Press and hold an entry** (or right-click → *Show Copy Times*) to expand it and see every single time that text was copied, newest first.

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
| `list [--json] [--limit N]` | List stored entries, newest first (`×N` column = total captures of that text) |
| `search <text> [--json] [--limit N]` | Fuzzy-search stored entries, best matches first |
| `get <entry-id>` | Print an entry's full text to stdout |
| `copy <entry-id> [--sensitive] [--clear-after S] [--no-wait]` | Copy an entry to the clipboard |
| `latest [--sensitive] [--clear-after S] [--no-wait]` | Copy the most recent entry |
| `secret (<text> \| -) [--clear-after S] [--no-wait]` | Clipboard-only copy, never stored (`-` reads stdin) |
| `forget <entry-id>` | Delete a single entry |
| `count` | Number of stored entries |
| `clear` | Delete every entry |

`--json` emits machine-readable output (`id`, ISO-8601 `createdAt`, `copyCount`, full `text`) for scripts and agents.

## Menu-bar app

- Open from the status-bar lock icon, or with your global shortcut (default ⇧⌘V)
- Live fuzzy search across history — typo-tolerant subsequence matching (`gthb` finds GitHub links) with ranked results and highlighted hits; search is focused as soon as the panel opens
- Click an entry to copy (checkmark confirms); Option-click for a sensitive auto-clearing copy
- Repeated copies aggregate into one row: it moves to the top, shows the latest timestamp plus a ×N count badge; **press and hold** (or right-click → *Show Copy Times*) to list every capture
- Right-click for Copy / Copy Once / Show Copy Times / Delete
- Relative timestamps, entry count, clear-all with confirmation
- Errors from the encrypted vault are surfaced inline

## Security notes

- AES-GCM sealed vault at `~/Library/Application Support/ClipboardVault/vault.bin`, permissions `0600` (directory `0700`)
- 256-bit key in the login Keychain as `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` — unavailable while locked, excluded from backups; older keys are upgraded in place
- Sensitive copies never touch disk and self-destruct from the clipboard
- No network access anywhere in the codebase
