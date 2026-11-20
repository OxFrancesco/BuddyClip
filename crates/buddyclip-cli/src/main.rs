mod backend;
mod config;
mod ipc;
mod qr;
mod server;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use buddyclip_core::DEFAULT_PORT;
use buddyclip_core::clipboard::MIME_TEXT_PLAIN;
use buddyclip_core::crypto::MasterKey;
use buddyclip_core::pairing::PairingBundle;
use buddyclip_core::tailnet::{TailnetPolicy, detect_tailscale_status};
use tracing_subscriber::EnvFilter;

use crate::config::AppConfig;
use crate::ipc::{IpcRequest, IpcResponse, decode_payload, encode_payload, send_ipc_request};
use crate::qr::render_terminal_qr;

#[derive(Parser, Debug)]
#[command(
    name = "buddyclip",
    version,
    about = "End-to-end encrypted universal clipboard strictly over your Tailscale tailnet"
)]
struct Cli {
    /// Custom path to config.toml (default: ~/.config/buddyclip/config.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Generate a 256-bit encryption key, detect Tailscale IP, and print an Android pairing QR code
    Init {
        /// Human-friendly name for this device (defaults to Tailscale hostname)
        #[arg(long)]
        name: Option<String>,
        /// Port to listen on over the tailnet
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
        /// Import an existing pairing URI (`buddyclip://pair?...`) instead of generating a new key
        #[arg(long)]
        pair_uri: Option<String>,
        /// Derive the 256-bit key deterministically from a passphrase (Argon2id)
        #[arg(long)]
        passphrase: Option<String>,
        /// Overwrite existing config.toml if present
        #[arg(long)]
        force: bool,
    },
    /// Run the tailnet-only encrypted clipboard sync daemon
    Daemon {
        /// Disable desktop notifications when receiving clips
        #[arg(long)]
        no_notify: bool,
    },
    /// Display the QR code and pairing URI to connect your Android phone or another PC
    Pair {
        /// Use MagicDNS hostname (e.g. my-pc.tailnet.ts.net) instead of Tailscale IPv4 in the QR code
        #[arg(long)]
        use_dns: bool,
        /// Print only the raw buddyclip://pair URI (no QR code)
        #[arg(long)]
        uri_only: bool,
    },
    /// Push text (arguments or stdin) to your local clipboard and all connected tailnet devices
    Copy {
        /// Text to copy; if omitted, reads raw bytes from stdin
        text: Vec<String>,
        /// Explicit MIME type (default: text/plain;charset=utf-8)
        #[arg(long, default_value = MIME_TEXT_PLAIN)]
        mime: String,
    },
    /// Print the latest synced clip from the running daemon to stdout
    Paste,
    /// Show live daemon status, Tailscale bind address, key fingerprint, and connected devices
    Status,
    /// List recent RAM-only synced clipboard items
    History {
        #[arg(short = 'n', long, default_value_t = 15)]
        limit: usize,
    },
    /// Clear the daemon's in-memory clipboard history
    Clear,
    /// Inspect Tailscale tailnet peers and check for tagged ACL requirements
    Peers,
    /// Install and enable a systemd user service (`buddyclip.service`) for seamless auto-start
    InstallService {
        /// Start the service immediately after installing
        #[arg(long, default_value_t = true)]
        start: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let cfg_path = match cli.config {
        Some(p) => p,
        None => AppConfig::default_path()?,
    };
    let sock_path = AppConfig::runtime_socket_path();

    match cli.command {
        Commands::Init {
            name,
            port,
            pair_uri,
            passphrase,
            force,
        } => cmd_init(&cfg_path, name, port, pair_uri, passphrase, force).await,
        Commands::Daemon { no_notify } => {
            let mut cfg = AppConfig::load(&cfg_path)?;
            if no_notify {
                cfg.notifications = false;
            }
            server::run_daemon(cfg, sock_path, TailnetPolicy::Strict).await
        }
        Commands::Pair { use_dns, uri_only } => cmd_pair(&cfg_path, use_dns, uri_only).await,
        Commands::Copy { text, mime } => cmd_copy(&sock_path, text, mime).await,
        Commands::Paste => cmd_paste(&sock_path).await,
        Commands::Status => cmd_status(&sock_path).await,
        Commands::History { limit } => cmd_history(&sock_path, limit).await,
        Commands::Clear => cmd_clear(&sock_path).await,
        Commands::Peers => cmd_peers(&cfg_path).await,
        Commands::InstallService { start } => cmd_install_service(&cfg_path, start).await,
    }
}

async fn cmd_init(
    cfg_path: &Path,
    name: Option<String>,
    port: u16,
    pair_uri: Option<String>,
    passphrase: Option<String>,
    force: bool,
) -> Result<()> {
    if cfg_path.exists() && !force {
        anyhow::bail!(
            "config already exists at {} (pass --force to overwrite, or run `buddyclip pair` to show your QR code)",
            cfg_path.display()
        );
    }

    let ts = detect_tailscale_status().await.ok();
    let detected_name = ts
        .as_ref()
        .map(|s| s.self_node.host_name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| {
            std::env::var("HOSTNAME").unwrap_or_else(|_| "linux-pc".to_string())
        });

    let (key, peers) = if let Some(uri) = pair_uri {
        let bundle = PairingBundle::from_uri(&uri)?;
        let peer_endpoint = format!("{}:{}", bundle.host, bundle.port);
        (bundle.key, vec![peer_endpoint])
    } else if let Some(pass) = passphrase {
        (MasterKey::derive_from_passphrase(&pass)?, Vec::new())
    } else {
        (MasterKey::generate(), Vec::new())
    };

    let mut cfg = AppConfig::new(name.unwrap_or(detected_name), &key, port);
    cfg.peers = peers;
    cfg.save(cfg_path)?;

    println!("Initialized buddyclip config at {}", cfg_path.display());
    println!("  Device name:     {}", cfg.device_name);
    println!("  Key fingerprint: {} (XChaCha20-Poly1305 + X25519 PFS)", key.fingerprint());
    println!("  Tailnet port:    {}", cfg.port);

    if let Some(ts) = ts {
        if let Some(ip) = ts.self_ipv4() {
            println!("  Tailscale IP:    {ip}");
            let bundle = PairingBundle {
                key,
                host: ip.to_string(),
                port: cfg.port,
                host_name: cfg.device_name.clone(),
            };
            let uri = bundle.to_uri();
            println!("\nScan this QR code in the buddyclip Android app to pair:\n");
            println!("{}", render_terminal_qr(&uri)?);
            println!("Pairing URI (keep private — contains your 256-bit key):\n  {uri}\n");
        }
        print_tagged_peer_acl_hint(&ts.peers, cfg.port);
    } else {
        println!("\nNote: Tailscale was not detected right now. Start Tailscale, then run `buddyclip pair`.");
    }

    println!("Next steps:");
    println!("  1. Run `buddyclip daemon` (or `buddyclip install-service` for automatic background startup)");
    println!("  2. Open the buddyclip Android app and scan the QR code above");
    Ok(())
}

async fn cmd_pair(cfg_path: &Path, use_dns: bool, uri_only: bool) -> Result<()> {
    let cfg = AppConfig::load(cfg_path)?;
    let key = cfg.master_key()?;

    let host = if let Some(ref bind_ip) = cfg.bind_ip {
        bind_ip.clone()
    } else {
        let ts = detect_tailscale_status().await.context(
            "could not detect Tailscale IP (is Tailscale connected?)",
        )?;
        if use_dns && !ts.self_node.dns_name.is_empty() {
            ts.self_node.dns_name
        } else if let Some(v4) = ts.self_ipv4() {
            v4.to_string()
        } else if let Some(v6) = ts.self_ipv6() {
            v6.to_string()
        } else {
            anyhow::bail!("no Tailscale IP found on this device");
        }
    };

    let bundle = PairingBundle {
        key: key.clone(),
        host: host.clone(),
        port: cfg.port,
        host_name: cfg.device_name.clone(),
    };
    let uri = bundle.to_uri();

    if uri_only {
        println!("{uri}");
        return Ok(());
    }

    println!("Device:          {} ({}:{})", cfg.device_name, host, cfg.port);
    println!("Key fingerprint: {}\n", key.fingerprint());
    println!("{}", render_terminal_qr(&uri)?);
    println!("Pairing URI (contains your secret key — do not share publicly):\n  {uri}");
    Ok(())
}

async fn cmd_copy(sock_path: &Path, text: Vec<String>, mime: String) -> Result<()> {
    let payload = if !text.is_empty() {
        text.join(" ").into_bytes()
    } else {
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf)?;
        buf
    };

    if payload.is_empty() {
        anyhow::bail!("nothing to copy (pass text arguments or pipe stdin)");
    }

    let resp = send_ipc_request(
        sock_path,
        &IpcRequest::Push {
            mime_type: mime,
            payload_b64: encode_payload(&payload),
        },
    )
    .await?;

    match resp {
        IpcResponse::Ok { message } => println!("{message}"),
        IpcResponse::Error { message } => anyhow::bail!("{message}"),
        _ => {}
    }
    Ok(())
}

async fn cmd_paste(sock_path: &Path) -> Result<()> {
    let resp = send_ipc_request(sock_path, &IpcRequest::Latest).await?;
    match resp {
        IpcResponse::Latest {
            payload_b64: Some(b64),
            ..
        } => {
            let bytes = decode_payload(&b64)?;
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(&bytes)?;
            stdout.flush()?;
        }
        IpcResponse::Latest { payload_b64: None, .. } => {
            anyhow::bail!("no synced clipboard items in daemon memory yet");
        }
        IpcResponse::Error { message } => anyhow::bail!("{message}"),
        _ => {}
    }
    Ok(())
}

async fn cmd_status(sock_path: &Path) -> Result<()> {
    let resp = send_ipc_request(sock_path, &IpcRequest::Status).await?;
    match resp {
        IpcResponse::Status(s) => {
            println!("buddyclip daemon running");
            println!("  Device:          {} ({})", s.device_name, s.device_id);
            println!("  Tailnet listen:  {}", s.listen_addr);
            println!("  Key fingerprint: {}", s.key_fingerprint);
            println!("  Clipboard:       {}", s.backend);
            println!(
                "  Stats:           {} sent, {} received, {} in RAM history",
                s.clips_sent, s.clips_received, s.history_count
            );
            if s.connected_peers.is_empty() {
                println!("  Connected peers: none");
            } else {
                println!("  Connected peers ({}):", s.connected_peers.len());
                for p in s.connected_peers {
                    println!("    - {} [{}] ({})", p.device_name, p.platform, p.peer_addr);
                }
            }
        }
        IpcResponse::Error { message } => anyhow::bail!("{message}"),
        _ => {}
    }
    Ok(())
}

async fn cmd_history(sock_path: &Path, limit: usize) -> Result<()> {
    let resp = send_ipc_request(sock_path, &IpcRequest::History { limit }).await?;
    match resp {
        IpcResponse::History { items } => {
            if items.is_empty() {
                println!("No clipboard items synced yet.");
                return Ok(());
            }
            for (i, entry) in items.iter().enumerate() {
                println!(
                    "{:>2}. [{}] ({}, {} B) {}",
                    i + 1,
                    entry.meta.origin_device_name,
                    entry.meta.mime_type,
                    entry.meta.byte_len,
                    entry.preview
                );
            }
        }
        IpcResponse::Error { message } => anyhow::bail!("{message}"),
        _ => {}
    }
    Ok(())
}

async fn cmd_clear(sock_path: &Path) -> Result<()> {
    let resp = send_ipc_request(sock_path, &IpcRequest::Clear).await?;
    match resp {
        IpcResponse::Ok { message } => println!("{message}"),
        IpcResponse::Error { message } => anyhow::bail!("{message}"),
        _ => {}
    }
    Ok(())
}

async fn cmd_peers(cfg_path: &Path) -> Result<()> {
    let port = AppConfig::load(cfg_path)
        .map(|c| c.port)
        .unwrap_or(DEFAULT_PORT);
    let ts = detect_tailscale_status().await?;
    println!(
        "This device: {} ({})",
        ts.self_node.host_name,
        ts.self_ipv4()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "no IPv4".into())
    );
    println!("\nTailnet peers:");
    for p in &ts.peers {
        let ip = p
            .ips
            .first()
            .map(|i| i.to_string())
            .unwrap_or_else(|| "-".into());
        let state = if p.online { "online" } else { "offline" };
        let tags = if p.tags.is_empty() {
            String::new()
        } else {
            format!(" tags={}", p.tags.join(","))
        };
        println!("  - {:<22} {:<15} {:<8} [{}]{}", p.host_name, ip, p.os, state, tags);
    }
    print_tagged_peer_acl_hint(&ts.peers, port);
    Ok(())
}

fn print_tagged_peer_acl_hint(peers: &[buddyclip_core::tailnet::TailscalePeer], port: u16) {
    let tagged: Vec<_> = peers.iter().filter(|p| !p.tags.is_empty()).collect();
    if tagged.is_empty() {
        return;
    }
    println!("\nTailscale ACL reminder for tagged devices:");
    for p in tagged {
        let tag_list = p
            .tags
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "  {} uses {}. Ensure your Tailscale ACL policy allows port {port}/tcp:",
            p.host_name,
            p.tags.join(", ")
        );
        println!("    {{\"action\": \"accept\", \"src\": [{tag_list}], \"dst\": [\"autogroup:self:{port}\"]}}");
    }
}

async fn cmd_install_service(cfg_path: &Path, start: bool) -> Result<()> {
    let _ = AppConfig::load(cfg_path)?;
    let exe = std::env::current_exe().context("could not resolve current buddyclip binary path")?;
    let unit_dir = dirs::config_dir()
        .context("could not find ~/.config")?
        .join("systemd")
        .join("user");
    std::fs::create_dir_all(&unit_dir)?;
    let unit_path = unit_dir.join("buddyclip.service");

    let unit = format!(
        "[Unit]\n\
         Description=buddyclip - Tailnet-only E2EE Universal Clipboard\n\
         After=graphical-session.target network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={} --config {} daemon\n\
         Restart=always\n\
         RestartSec=3\n\
         Environment=RUST_LOG=info\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n",
        exe.display(),
        cfg_path.display(),
    );

    std::fs::write(&unit_path, unit)?;
    println!("Wrote systemd user unit: {}", unit_path.display());

    let reload = tokio::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .await;
    if reload.is_ok_and(|s| s.success()) {
        let mut args = vec!["--user", "enable"];
        if start {
            args.push("--now");
        }
        args.push("buddyclip.service");
        let en = tokio::process::Command::new("systemctl")
            .args(&args)
            .status()
            .await?;
        if en.success() {
            println!("Enabled{} buddyclip.service", if start { " and started" } else { "" });
        } else {
            println!("Run manually: systemctl --user enable --now buddyclip.service");
        }
    } else {
        println!("Run manually: systemctl --user daemon-reload && systemctl --user enable --now buddyclip.service");
    }

    Ok(())
}
