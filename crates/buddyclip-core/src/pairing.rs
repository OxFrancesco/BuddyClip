use thiserror::Error;

use crate::DEFAULT_PORT;
use crate::crypto::MasterKey;

#[derive(Debug, Error)]
pub enum PairingError {
    #[error("invalid pairing URI scheme (expected buddyclip://pair?...)")]
    InvalidScheme,
    #[error("unsupported pairing version: {0}")]
    UnsupportedVersion(String),
    #[error("missing required field '{0}' in pairing URI")]
    MissingField(&'static str),
    #[error("invalid key in pairing URI: {0}")]
    InvalidKey(String),
    #[error("invalid port in pairing URI: {0}")]
    InvalidPort(String),
}

#[derive(Debug, Clone)]
pub struct PairingBundle {
    pub key: MasterKey,
    pub host: String,
    pub port: u16,
    pub host_name: String,
}

impl PairingBundle {
    pub fn to_uri(&self) -> String {
        format!(
            "buddyclip://pair?v=1&k={}&h={}&p={}&n={}",
            self.key.to_base64(),
            percent_encode(&self.host),
            self.port,
            percent_encode(&self.host_name),
        )
    }

    pub fn from_uri(uri: &str) -> Result<Self, PairingError> {
        let trimmed = uri.trim();
        let query = trimmed
            .strip_prefix("buddyclip://pair?")
            .ok_or(PairingError::InvalidScheme)?;

        let mut version: Option<String> = None;
        let mut key_b64: Option<String> = None;
        let mut host: Option<String> = None;
        let mut port: u16 = DEFAULT_PORT;
        let mut host_name: Option<String> = None;

        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let decoded = percent_decode(v);
            match k {
                "v" => version = Some(decoded),
                "k" => key_b64 = Some(decoded),
                "h" => host = Some(decoded),
                "p" => {
                    port = decoded
                        .parse::<u16>()
                        .map_err(|_| PairingError::InvalidPort(decoded))?;
                }
                "n" => host_name = Some(decoded),
                _ => {}
            }
        }

        if let Some(v) = version
            && v != "1"
        {
            return Err(PairingError::UnsupportedVersion(v));
        }

        let key_str = key_b64.ok_or(PairingError::MissingField("k"))?;
        let key = MasterKey::from_base64(&key_str)
            .map_err(|e| PairingError::InvalidKey(e.to_string()))?;
        let host = host
            .filter(|h| !h.is_empty())
            .ok_or(PairingError::MissingField("h"))?;
        let host_name = host_name
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| host.clone());

        Ok(Self {
            key,
            host,
            port,
            host_name,
        })
    }
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b':') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_pairing_uri() {
        let key = MasterKey::generate();
        let bundle = PairingBundle {
            key: key.clone(),
            host: "100.64.0.10".into(),
            port: 9455,
            host_name: "My Linux PC".into(),
        };
        let uri = bundle.to_uri();
        assert!(uri.starts_with("buddyclip://pair?v=1&k="));
        let parsed = PairingBundle::from_uri(&uri).unwrap();
        assert_eq!(parsed.key.fingerprint(), key.fingerprint());
        assert_eq!(parsed.host, "100.64.0.10");
        assert_eq!(parsed.port, 9455);
        assert_eq!(parsed.host_name, "My Linux PC");
    }
}
