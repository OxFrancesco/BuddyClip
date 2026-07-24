# Clipboard Vault

A private macOS menu-bar clipboard history with a CLI. Text entries are encrypted using AES-GCM. The encryption key is generated once and kept in the current macOS user's Keychain; the encrypted history is stored in Application Support.

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

The app records only copied plain text, ignores empty copies and immediate duplicates, keeps the latest 250 entries, and never syncs data or contacts a network service.
