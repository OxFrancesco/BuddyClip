use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::net::lookup_host;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TailnetPolicy {
    /// Strict production mode: only Tailscale CGNAT (100.64.0.0/10) and
    /// Tailscale IPv6 ULA (fd7a:115c:a1e0::/48) addresses are permitted.
    #[default]
    Strict,
    /// Test-only mode that additionally permits loopback (127.0.0.1 / ::1).
    AllowLoopbackForTests,
}

#[derive(Debug, Error)]
pub enum TailnetError {
    #[error("address {0} is outside the Tailscale tailnet (100.64.0.0/10 or fd7a:115c:a1e0::/48)")]
    NonTailnetAddress(IpAddr),
    #[error("could not resolve host '{host}': {source}")]
    ResolveFailed {
        host: String,
        #[source]
        source: std::io::Error,
    },
    #[error("host '{0}' resolved, but none of its addresses belong to the Tailscale tailnet")]
    NoTailnetResolution(String),
    #[error("tailscale CLI unavailable or not running: {0}")]
    TailscaleUnavailable(String),
}

/// Returns true iff `ip` lies inside Tailscale's CGNAT IPv4 range (`100.64.0.0/10`)
/// or Tailscale's IPv6 ULA range (`fd7a:115c:a1e0::/48`).
pub fn is_tailscale_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_tailscale_ipv4(v4),
        IpAddr::V6(v6) => is_tailscale_ipv6(v6),
    }
}

pub fn is_tailscale_ipv4(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    o[0] == 100 && (o[1] & 0b1100_0000) == 64
}

pub fn is_tailscale_ipv6(v6: Ipv6Addr) -> bool {
    let s = v6.segments();
    s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
}

pub fn validate_ip(ip: IpAddr, policy: TailnetPolicy) -> Result<(), TailnetError> {
    if is_tailscale_ip(ip) {
        return Ok(());
    }
    if policy == TailnetPolicy::AllowLoopbackForTests && ip.is_loopback() {
        return Ok(());
    }
    Err(TailnetError::NonTailnetAddress(ip))
}

/// Resolves `host:port` (raw IP or MagicDNS name) and guarantees the returned
/// `SocketAddr` satisfies `policy` (Tailscale tailnet only in Strict mode).
pub async fn resolve_tailnet_addr(
    host: &str,
    port: u16,
    policy: TailnetPolicy,
) -> Result<SocketAddr, TailnetError> {
    let trimmed = host.trim().trim_end_matches('.');
    if let Ok(ip) = trimmed.parse::<IpAddr>() {
        validate_ip(ip, policy)?;
        return Ok(SocketAddr::new(ip, port));
    }

    let addrs = lookup_host((trimmed, port))
        .await
        .map_err(|source| TailnetError::ResolveFailed {
            host: trimmed.to_string(),
            source,
        })?;

    let mut ipv6_candidate: Option<SocketAddr> = None;
    for addr in addrs {
        if validate_ip(addr.ip(), policy).is_ok() {
            if addr.is_ipv4() {
                return Ok(addr);
            }
            if ipv6_candidate.is_none() {
                ipv6_candidate = Some(addr);
            }
        }
    }

    ipv6_candidate.ok_or_else(|| TailnetError::NoTailnetResolution(trimmed.to_string()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TailscalePeer {
    pub host_name: String,
    pub dns_name: String,
    pub os: String,
    pub ips: Vec<IpAddr>,
    pub online: bool,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TailscaleStatus {
    pub backend_state: String,
    pub self_node: TailscalePeer,
    pub peers: Vec<TailscalePeer>,
}

impl TailscaleStatus {
    pub fn self_ipv4(&self) -> Option<Ipv4Addr> {
        self.self_node.ips.iter().find_map(|ip| match ip {
            IpAddr::V4(v4) if is_tailscale_ipv4(*v4) => Some(*v4),
            _ => None,
        })
    }

    pub fn self_ipv6(&self) -> Option<Ipv6Addr> {
        self.self_node.ips.iter().find_map(|ip| match ip {
            IpAddr::V6(v6) if is_tailscale_ipv6(*v6) => Some(*v6),
            _ => None,
        })
    }
}

#[derive(Deserialize)]
struct RawTailscaleStatus {
    #[serde(rename = "BackendState", default)]
    backend_state: String,
    #[serde(rename = "Self")]
    self_node: RawNode,
    #[serde(rename = "Peer", default)]
    peer: std::collections::HashMap<String, RawNode>,
}

#[derive(Deserialize)]
struct RawNode {
    #[serde(rename = "HostName", default)]
    host_name: String,
    #[serde(rename = "DNSName", default)]
    dns_name: String,
    #[serde(rename = "OS", default)]
    os: String,
    #[serde(rename = "TailscaleIPs", default)]
    tailscale_ips: Vec<String>,
    #[serde(rename = "Online", default)]
    online: bool,
    #[serde(rename = "Tags", default)]
    tags: Option<Vec<String>>,
}

impl From<RawNode> for TailscalePeer {
    fn from(raw: RawNode) -> Self {
        let ips = raw
            .tailscale_ips
            .into_iter()
            .filter_map(|s| s.parse::<IpAddr>().ok())
            .collect();
        Self {
            host_name: raw.host_name,
            dns_name: raw.dns_name.trim_end_matches('.').to_string(),
            os: raw.os,
            ips,
            online: raw.online,
            tags: raw.tags.unwrap_or_default(),
        }
    }
}

/// Queries `tailscale status --json` to discover local tailnet IPs and peers.
pub async fn detect_tailscale_status() -> Result<TailscaleStatus, TailnetError> {
    let output = tokio::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .await
        .map_err(|e| TailnetError::TailscaleUnavailable(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TailnetError::TailscaleUnavailable(stderr.trim().to_string()));
    }

    let raw: RawTailscaleStatus = serde_json::from_slice(&output.stdout)
        .map_err(|e| TailnetError::TailscaleUnavailable(format!("invalid JSON: {e}")))?;

    if raw.backend_state != "Running" {
        return Err(TailnetError::TailscaleUnavailable(format!(
            "backend state is '{}' (expected 'Running')",
            raw.backend_state
        )));
    }

    let mut peers: Vec<TailscalePeer> = raw.peer.into_values().map(TailscalePeer::from).collect();
    peers.sort_by_key(|a| a.host_name.to_lowercase());

    Ok(TailscaleStatus {
        backend_state: raw.backend_state,
        self_node: raw.self_node.into(),
        peers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tailscale_ip_classification() {
        assert!(is_tailscale_ip("100.64.0.1".parse().unwrap()));
        assert!(is_tailscale_ip("100.64.0.10".parse().unwrap()));
        assert!(is_tailscale_ip("100.64.0.20".parse().unwrap()));
        assert!(is_tailscale_ip("100.127.255.254".parse().unwrap()));
        assert!(!is_tailscale_ip("100.63.255.255".parse().unwrap()));
        assert!(!is_tailscale_ip("100.128.0.1".parse().unwrap()));
        assert!(!is_tailscale_ip("192.168.1.10".parse().unwrap()));
        assert!(!is_tailscale_ip("10.0.0.1".parse().unwrap()));
        assert!(!is_tailscale_ip("127.0.0.1".parse().unwrap()));
        assert!(!is_tailscale_ip("8.8.8.8".parse().unwrap()));

        assert!(is_tailscale_ip("fd7a:115c:a1e0::7438:a416".parse().unwrap()));
        assert!(!is_tailscale_ip("fd7a:115c:a1e1::1".parse().unwrap()));
        assert!(!is_tailscale_ip("::1".parse().unwrap()));
    }

    #[test]
    fn strict_policy_rejects_lan_and_loopback() {
        assert!(validate_ip("100.64.0.10".parse().unwrap(), TailnetPolicy::Strict).is_ok());
        assert!(validate_ip("127.0.0.1".parse().unwrap(), TailnetPolicy::Strict).is_err());
        assert!(validate_ip("192.168.1.50".parse().unwrap(), TailnetPolicy::Strict).is_err());
        assert!(
            validate_ip("127.0.0.1".parse().unwrap(), TailnetPolicy::AllowLoopbackForTests).is_ok()
        );
    }
}
