use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use buddyclip_core::DEFAULT_PORT;
use buddyclip_core::crypto::MasterKey;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub device_id: String,
    pub device_name: String,
    pub master_key: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Optional explicit Tailscale IP override. When omitted, auto-detected from `tailscale status --json`.
    #[serde(default)]
    pub bind_ip: Option<String>,
    /// Optional list of peer PC endpoints (e.g. `["100.64.0.30:9455"]`) for PC-to-PC mesh sync.
    #[serde(default)]
    pub peers: Vec<String>,
    #[serde(default = "default_true")]
    pub notifications: bool,
    #[serde(default = "default_true")]
    pub sync_images: bool,
    #[serde(default = "default_true")]
    pub skip_sensitive: bool,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

fn default_true() -> bool {
    true
}

impl AppConfig {
    pub fn new(device_name: String, key: &MasterKey, port: u16) -> Self {
        Self {
            device_id: Uuid::new_v4().to_string(),
            device_name,
            master_key: key.to_base64(),
            port,
            bind_ip: None,
            peers: Vec::new(),
            notifications: true,
            sync_images: true,
            skip_sensitive: true,
        }
    }

    pub fn master_key(&self) -> Result<MasterKey> {
        MasterKey::from_base64(&self.master_key).context("invalid master_key in config.toml")
    }

    pub fn default_path() -> Result<PathBuf> {
        let base = dirs::config_dir().context("could not determine XDG config directory")?;
        Ok(base.join("buddyclip").join("config.toml"))
    }

    pub fn runtime_socket_path() -> PathBuf {
        if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR")
            && !runtime.is_empty()
        {
            return PathBuf::from(runtime).join("buddyclip.sock");
        }
        std::env::temp_dir().join(format!("buddyclip-{}.sock", std::process::id()))
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read config at {} (run `buddyclip init` first)", path.display()))?;
        let cfg: Self = toml::from_str(&text)
            .with_context(|| format!("failed to parse config at {}", path.display()))?;
        let _ = cfg.master_key()?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }

        let serialized = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, serialized)
            .with_context(|| format!("failed to write temp config {}", tmp.display()))?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to set 0600 permissions on {}", tmp.display()))?;
        fs::rename(&tmp, path)
            .with_context(|| format!("failed to commit config to {}", path.display()))?;
        Ok(())
    }
}
