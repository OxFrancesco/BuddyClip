use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use buddyclip_core::MAX_PAYLOAD_BYTES;
use buddyclip_core::clipboard::{MIME_IMAGE_JPEG, MIME_IMAGE_PNG, MIME_TEXT_PLAIN};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

const SENSITIVE_MIME_MARKERS: &[&str] = &[
    "x-kde-passwordmanagerhint",
    "application/x-keepassxc-secret",
    "application/x-bitwarden-secret",
    "application/x-1password-secret",
    "org.freedesktop.portal.clipboard.sensitive",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardBackendKind {
    Wayland,
    X11,
    Headless,
}

impl ClipboardBackendKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wayland => "wayland (wl-clipboard)",
            Self::X11 => "x11 (xclip)",
            Self::Headless => "headless (no display)",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LocalClip {
    pub mime_type: String,
    pub payload: Vec<u8>,
}

pub fn detect_backend() -> ClipboardBackendKind {
    let has_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty());
    if has_wayland && command_exists("wl-paste") && command_exists("wl-copy") {
        return ClipboardBackendKind::Wayland;
    }

    let has_x11 = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty());
    if has_x11 && command_exists("xclip") {
        return ClipboardBackendKind::X11;
    }

    ClipboardBackendKind::Headless
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let candidate = dir.join(name);
            candidate.is_file()
        })
    })
}

pub async fn write_system_clipboard(
    kind: ClipboardBackendKind,
    mime_type: &str,
    payload: &[u8],
) -> Result<()> {
    match kind {
        ClipboardBackendKind::Wayland => {
            // Note: wl-copy forks into background to serve selection requests.
            // Using Stdio::null() for stdout/stderr prevents waiting on the forked daemon's pipes.
            let mut child = Command::new("wl-copy")
                .args(["--type", mime_type])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("failed to spawn wl-copy")?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(payload).await?;
                drop(stdin);
            }

            let status = tokio::time::timeout(Duration::from_secs(3), child.wait())
                .await
                .context("wl-copy timed out")??;
            if !status.success() {
                anyhow::bail!("wl-copy exited with status {status}");
            }
            Ok(())
        }
        ClipboardBackendKind::X11 => {
            let mut child = Command::new("xclip")
                .args(["-selection", "clipboard", "-t", mime_type, "-i"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .context("failed to spawn xclip")?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(payload).await?;
                drop(stdin);
            }
            let status = tokio::time::timeout(Duration::from_secs(3), child.wait())
                .await
                .context("xclip timed out")??;
            if !status.success() {
                anyhow::bail!("xclip exited with status {status}");
            }
            Ok(())
        }
        ClipboardBackendKind::Headless => Ok(()),
    }
}

pub async fn read_wayland_clipboard(
    sync_images: bool,
    skip_sensitive: bool,
) -> Result<Option<LocalClip>> {
    let types_out = tokio::time::timeout(
        Duration::from_secs(2),
        Command::new("wl-paste").arg("--list-types").output(),
    )
    .await
    .context("wl-paste --list-types timed out")??;

    if !types_out.status.success() {
        return Ok(None);
    }

    let types: Vec<String> = String::from_utf8_lossy(&types_out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    if types.is_empty() {
        return Ok(None);
    }

    if skip_sensitive && has_sensitive_marker(&types).await {
        debug!("skipping sensitive clipboard entry (password manager hint detected)");
        return Ok(None);
    }

    // Prefer plain text when offered (avoids sending bloated HTML/rich blobs as images).
    let selected_mime = select_best_mime(&types, sync_images);
    let Some((wl_type, normalized_mime)) = selected_mime else {
        debug!(?types, "skipping unsupported clipboard MIME types");
        return Ok(None);
    };

    let data_out = tokio::time::timeout(
        Duration::from_secs(4),
        Command::new("wl-paste")
            .args(["--no-newline", "--type", &wl_type])
            .output(),
    )
    .await
    .context("wl-paste read timed out")??;

    if !data_out.status.success() || data_out.stdout.is_empty() {
        return Ok(None);
    }
    if data_out.stdout.len() > MAX_PAYLOAD_BYTES {
        warn!(
            bytes = data_out.stdout.len(),
            "skipping clipboard item exceeding 16 MiB cap"
        );
        return Ok(None);
    }

    Ok(Some(LocalClip {
        mime_type: normalized_mime.to_string(),
        payload: data_out.stdout,
    }))
}

async fn has_sensitive_marker(types: &[String]) -> bool {
    for t in types {
        let lower = t.to_ascii_lowercase();
        if SENSITIVE_MIME_MARKERS.iter().any(|m| lower == *m) {
            if lower == "x-kde-passwordmanagerhint" {
                // Verify hint value if readable; fail closed (treat as sensitive) if "secret" or unreadable.
                if let Ok(Ok(out)) = tokio::time::timeout(
                    Duration::from_secs(1),
                    Command::new("wl-paste")
                        .args(["--no-newline", "--type", t])
                        .output(),
                )
                .await
                {
                    let val = String::from_utf8_lossy(&out.stdout).trim().to_ascii_lowercase();
                    if val == "public" || val == "false" || val == "0" {
                        continue;
                    }
                }
            }
            return true;
        }
    }
    false
}

fn select_best_mime(types: &[String], sync_images: bool) -> Option<(String, &'static str)> {
    // 1. Check UTF-8 text MIME types first
    for preferred in ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"] {
        if let Some(found) = types
            .iter()
            .find(|t| t.eq_ignore_ascii_case(preferred))
        {
            return Some((found.clone(), MIME_TEXT_PLAIN));
        }
    }

    // 2. Check images (screenshots / copied images)
    if sync_images {
        for (preferred, norm) in [
            ("image/png", MIME_IMAGE_PNG),
            ("image/jpeg", MIME_IMAGE_JPEG),
            ("image/jpg", MIME_IMAGE_JPEG),
        ] {
            if let Some(found) = types
                .iter()
                .find(|t| t.eq_ignore_ascii_case(preferred))
            {
                return Some((found.clone(), norm));
            }
        }
    }

    None
}

pub async fn read_x11_clipboard(sync_images: bool) -> Result<Option<LocalClip>> {
    let targets_out = Command::new("xclip")
        .args(["-selection", "clipboard", "-o", "-t", "TARGETS"])
        .output()
        .await?;
    if !targets_out.status.success() {
        return Ok(None);
    }
    let types: Vec<String> = String::from_utf8_lossy(&targets_out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    let Some((x_type, normalized_mime)) = select_best_mime(&types, sync_images) else {
        return Ok(None);
    };

    let out = Command::new("xclip")
        .args(["-selection", "clipboard", "-o", "-t", &x_type])
        .output()
        .await?;
    if !out.status.success() || out.stdout.is_empty() || out.stdout.len() > MAX_PAYLOAD_BYTES {
        return Ok(None);
    }
    Ok(Some(LocalClip {
        mime_type: normalized_mime.to_string(),
        payload: out.stdout,
    }))
}

/// Spawns a background task watching the local OS clipboard and pushing local copies into `tx`.
pub fn spawn_clipboard_watcher(
    kind: ClipboardBackendKind,
    sync_images: bool,
    skip_sensitive: bool,
    tx: mpsc::Sender<LocalClip>,
) {
    match kind {
        ClipboardBackendKind::Wayland => {
            tokio::spawn(async move {
                loop {
                    info!("starting Wayland clipboard watcher (`wl-paste --watch`)");
                    let child = Command::new("wl-paste")
                        .args([
                            "--watch",
                            "sh",
                            "-c",
                            "printf '%s\\n' \"${CLIPBOARD_STATE:-data}\"",
                        ])
                        .stdin(Stdio::null())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::null())
                        .kill_on_drop(true)
                        .spawn();

                    match child {
                        Ok(mut proc) => {
                            if let Some(stdout) = proc.stdout.take() {
                                let mut lines = BufReader::new(stdout).lines();
                                while let Ok(Some(state)) = lines.next_line().await {
                                    let state = state.trim().to_ascii_lowercase();
                                    if state == "nil" || state == "clear" {
                                        continue;
                                    }
                                    if skip_sensitive && state == "sensitive" {
                                        debug!("wl-paste signaled CLIPBOARD_STATE=sensitive; skipping");
                                        continue;
                                    }
                                    match read_wayland_clipboard(sync_images, skip_sensitive).await {
                                        Ok(Some(clip)) => {
                                            if tx.send(clip).await.is_err() {
                                                return;
                                            }
                                        }
                                        Ok(None) => {}
                                        Err(e) => debug!(error = %e, "failed reading wayland clipboard"),
                                    }
                                }
                            }
                            let _ = proc.wait().await;
                        }
                        Err(e) => {
                            warn!(error = %e, "failed to start wl-paste --watch; retrying in 3s");
                        }
                    }

                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            });
        }
        ClipboardBackendKind::X11 => {
            tokio::spawn(async move {
                let mut last_hash = String::new();
                loop {
                    if let Ok(Some(clip)) = read_x11_clipboard(sync_images).await {
                        let hash = buddyclip_core::clipboard::compute_content_hash(
                            &clip.mime_type,
                            &clip.payload,
                        );
                        if hash != last_hash {
                            last_hash = hash;
                            if tx.send(clip).await.is_err() {
                                return;
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
            });
        }
        ClipboardBackendKind::Headless => {
            info!("no display detected; running in headless tailnet relay mode");
        }
    }
}

pub fn notify_desktop(title: &str, body: &str) {
    if !command_exists("notify-send") {
        return;
    }
    let title = title.to_string();
    let body = body.to_string();
    tokio::spawn(async move {
        let _ = Command::new("notify-send")
            .args([
                "-a",
                "buddyclip",
                "-t",
                "2500",
                "-i",
                "edit-paste",
                "--",
                &title,
                &body,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    });
}
