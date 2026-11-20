use std::fmt;

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use rand::RngCore;
use rand::rngs::OsRng;
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use x25519_dalek::{EphemeralSecret, PublicKey};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{MAX_FRAME_BYTES, PROTOCOL_VERSION, now_millis};

const WIRE_MAGIC: &[u8; 4] = b"BCLP";
const CLIENT_HELLO_LEN: usize = 4 + 1 + 32 + 32 + 8 + 32; // 109 bytes
const SERVER_HELLO_LEN: usize = 4 + 1 + 32 + 32 + 32; // 101 bytes
/// Maximum permitted clock skew during handshake (5 minutes).
const MAX_CLOCK_SKEW_MS: u64 = 5 * 60 * 1000;
const DIR_C2S: u8 = 0x01;
const DIR_S2C: u8 = 0x02;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("invalid key format: {0}")]
    InvalidKey(String),
    #[error("key derivation failed: {0}")]
    KdfFailed(String),
    #[error("handshake failed: {0}")]
    HandshakeFailed(&'static str),
    #[error("handshake clock skew too large")]
    ClockSkew,
    #[error("authentication failed (wrong encryption key or tampered message)")]
    AuthenticationFailed,
    #[error("frame exceeds maximum size ({0} bytes)")]
    FrameTooLarge(usize),
    #[error("sequence counter exhausted; reconnect required")]
    SequenceExhausted,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// 256-bit pre-shared group encryption key, zeroized automatically on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct MasterKey([u8; 32]);

impl fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MasterKey(fingerprint={})", self.fingerprint())
    }
}

impl MasterKey {
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_base64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0)
    }

    pub fn from_base64(input: &str) -> Result<Self, CryptoError> {
        let decoded = URL_SAFE_NO_PAD
            .decode(input.trim())
            .map_err(|e| CryptoError::InvalidKey(e.to_string()))?;
        let arr: [u8; 32] = decoded
            .try_into()
            .map_err(|_| CryptoError::InvalidKey("expected 32-byte key".into()))?;
        Ok(Self(arr))
    }

    /// Deterministically derives a 256-bit key from a user passphrase using Argon2id.
    pub fn derive_from_passphrase(passphrase: &str) -> Result<Self, CryptoError> {
        if passphrase.len() < 8 {
            return Err(CryptoError::InvalidKey(
                "passphrase must be at least 8 characters".into(),
            ));
        }
        let params = Params::new(64 * 1024, 3, 1, Some(32))
            .map_err(|e| CryptoError::KdfFailed(e.to_string()))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut out = [0u8; 32];
        argon2
            .hash_password_into(passphrase.as_bytes(), b"buddyclip-v1-tailnet-e2ee-salt", &mut out)
            .map_err(|e| CryptoError::KdfFailed(e.to_string()))?;
        Ok(Self(out))
    }

    /// Short human-verifiable fingerprint (`XXXX-XXXX`) to confirm both devices share the same key.
    pub fn fingerprint(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"buddyclip-v1-fingerprint:");
        hasher.update(&self.0);
        let hash = hasher.finalize();
        let b = hash.as_bytes();
        format!("{:02X}{:02X}-{:02X}{:02X}", b[0], b[1], b[2], b[3])
    }
}

pub struct SecureReader {
    stream: OwnedReadHalf,
    cipher: XChaCha20Poly1305,
    nonce_base: [u8; 16],
    direction: u8,
    seq: u64,
}

pub struct SecureWriter {
    stream: OwnedWriteHalf,
    cipher: XChaCha20Poly1305,
    nonce_base: [u8; 16],
    direction: u8,
    seq: u64,
}

impl SecureWriter {
    pub async fn write_frame(&mut self, plaintext: &[u8]) -> Result<(), CryptoError> {
        if plaintext.len() > MAX_FRAME_BYTES {
            return Err(CryptoError::FrameTooLarge(plaintext.len()));
        }
        if self.seq == u64::MAX {
            return Err(CryptoError::SequenceExhausted);
        }

        let seq = self.seq;
        let nonce = build_nonce(&self.nonce_base, seq);
        let ct_len = plaintext
            .len()
            .checked_add(16)
            .ok_or(CryptoError::FrameTooLarge(plaintext.len()))?;
        let aad = build_aad(self.direction, seq, ct_len as u32);

        let ciphertext = self
            .cipher
            .encrypt(&nonce, Payload { msg: plaintext, aad: &aad })
            .map_err(|_| CryptoError::AuthenticationFailed)?;

        debug_assert_eq!(ciphertext.len(), ct_len);

        let mut header = [0u8; 4];
        header.copy_from_slice(&(ct_len as u32).to_be_bytes());
        self.stream.write_all(&header).await?;
        self.stream.write_all(&ciphertext).await?;
        self.stream.flush().await?;
        self.seq += 1;
        Ok(())
    }
}

impl SecureReader {
    pub async fn read_frame(&mut self) -> Result<Vec<u8>, CryptoError> {
        if self.seq == u64::MAX {
            return Err(CryptoError::SequenceExhausted);
        }

        let mut len_buf = [0u8; 4];
        self.stream.read_exact(&mut len_buf).await?;
        let ct_len = u32::from_be_bytes(len_buf) as usize;
        if !(16..=MAX_FRAME_BYTES + 16).contains(&ct_len) {
            return Err(CryptoError::FrameTooLarge(ct_len));
        }

        let mut ciphertext = vec![0u8; ct_len];
        self.stream.read_exact(&mut ciphertext).await?;

        let seq = self.seq;
        let nonce = build_nonce(&self.nonce_base, seq);
        let aad = build_aad(self.direction, seq, ct_len as u32);

        let plaintext = self
            .cipher
            .decrypt(&nonce, Payload { msg: &ciphertext, aad: &aad })
            .map_err(|_| CryptoError::AuthenticationFailed)?;

        self.seq += 1;
        Ok(plaintext)
    }
}

fn build_nonce(base: &[u8; 16], seq: u64) -> XNonce {
    let mut bytes = [0u8; 24];
    bytes[..16].copy_from_slice(base);
    bytes[16..24].copy_from_slice(&seq.to_be_bytes());
    *XNonce::from_slice(&bytes)
}

fn build_aad(direction: u8, seq: u64, ct_len: u32) -> [u8; 14] {
    let mut aad = [0u8; 14];
    aad[0] = PROTOCOL_VERSION;
    aad[1] = direction;
    aad[2..10].copy_from_slice(&seq.to_be_bytes());
    aad[10..14].copy_from_slice(&ct_len.to_be_bytes());
    aad
}

fn compute_client_auth_tag(
    master_key: &MasterKey,
    version: u8,
    client_pk: &[u8; 32],
    client_nonce: &[u8; 32],
    ts_ms: u64,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_keyed(master_key.as_bytes());
    hasher.update(b"buddyclip-v1-client-hello");
    hasher.update(&[version]);
    hasher.update(client_pk);
    hasher.update(client_nonce);
    hasher.update(&ts_ms.to_be_bytes());
    *hasher.finalize().as_bytes()
}

fn compute_transcript_hash(
    client_hello: &[u8; CLIENT_HELLO_LEN],
    server_pk: &[u8; 32],
    server_nonce: &[u8; 32],
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"buddyclip-v1-transcript");
    hasher.update(client_hello);
    hasher.update(server_pk);
    hasher.update(server_nonce);
    *hasher.finalize().as_bytes()
}

fn compute_server_auth_tag(master_key: &MasterKey, transcript_hash: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_keyed(master_key.as_bytes());
    hasher.update(b"buddyclip-v1-server-hello");
    hasher.update(transcript_hash);
    *hasher.finalize().as_bytes()
}

struct SessionMaterial {
    c2s_key: [u8; 32],
    s2c_key: [u8; 32],
    c2s_nonce_base: [u8; 16],
    s2c_nonce_base: [u8; 16],
}

impl Drop for SessionMaterial {
    fn drop(&mut self) {
        self.c2s_key.zeroize();
        self.s2c_key.zeroize();
        self.c2s_nonce_base.zeroize();
        self.s2c_nonce_base.zeroize();
    }
}

fn derive_session_material(
    master_key: &MasterKey,
    shared_secret: &[u8; 32],
    transcript_hash: &[u8; 32],
) -> Result<SessionMaterial, CryptoError> {
    // Reject all-zero low-order X25519 outputs.
    if bool::from(shared_secret.ct_eq(&[0u8; 32])) {
        return Err(CryptoError::HandshakeFailed("invalid ephemeral public key"));
    }

    let mut ikm_hasher = blake3::Hasher::new_keyed(master_key.as_bytes());
    ikm_hasher.update(b"buddyclip-v1-session-ikm");
    ikm_hasher.update(shared_secret);
    ikm_hasher.update(transcript_hash);
    let mut ikm = *ikm_hasher.finalize().as_bytes();

    let c2s_key = blake3::derive_key("buddyclip-v1-c2s-xchacha20-key", &ikm);
    let s2c_key = blake3::derive_key("buddyclip-v1-s2c-xchacha20-key", &ikm);
    let c2s_nonce_full = blake3::derive_key("buddyclip-v1-c2s-nonce-base", &ikm);
    let s2c_nonce_full = blake3::derive_key("buddyclip-v1-s2c-nonce-base", &ikm);
    ikm.zeroize();

    let mut c2s_nonce_base = [0u8; 16];
    c2s_nonce_base.copy_from_slice(&c2s_nonce_full[..16]);
    let mut s2c_nonce_base = [0u8; 16];
    s2c_nonce_base.copy_from_slice(&s2c_nonce_full[..16]);

    Ok(SessionMaterial {
        c2s_key,
        s2c_key,
        c2s_nonce_base,
        s2c_nonce_base,
    })
}

/// Performs the client side of the mutually-authenticated X25519 + PSK handshake.
pub async fn client_handshake(
    mut stream: TcpStream,
    master_key: &MasterKey,
) -> Result<(SecureReader, SecureWriter), CryptoError> {
    let client_sk = EphemeralSecret::random_from_rng(OsRng);
    let client_pk = PublicKey::from(&client_sk);
    let client_pk_bytes = *client_pk.as_bytes();

    let mut client_nonce = [0u8; 32];
    OsRng.fill_bytes(&mut client_nonce);
    let ts_ms = now_millis();
    let client_tag =
        compute_client_auth_tag(master_key, PROTOCOL_VERSION, &client_pk_bytes, &client_nonce, ts_ms);

    let mut client_hello = [0u8; CLIENT_HELLO_LEN];
    client_hello[0..4].copy_from_slice(WIRE_MAGIC);
    client_hello[4] = PROTOCOL_VERSION;
    client_hello[5..37].copy_from_slice(&client_pk_bytes);
    client_hello[37..69].copy_from_slice(&client_nonce);
    client_hello[69..77].copy_from_slice(&ts_ms.to_be_bytes());
    client_hello[77..109].copy_from_slice(&client_tag);

    stream.write_all(&client_hello).await?;
    stream.flush().await?;

    let mut server_hello = [0u8; SERVER_HELLO_LEN];
    stream.read_exact(&mut server_hello).await?;

    if &server_hello[0..4] != WIRE_MAGIC {
        return Err(CryptoError::HandshakeFailed("invalid protocol magic"));
    }
    if server_hello[4] != PROTOCOL_VERSION {
        return Err(CryptoError::HandshakeFailed("unsupported protocol version"));
    }

    let mut server_pk_bytes = [0u8; 32];
    server_pk_bytes.copy_from_slice(&server_hello[5..37]);
    let mut server_nonce = [0u8; 32];
    server_nonce.copy_from_slice(&server_hello[37..69]);
    let server_tag = &server_hello[69..101];

    let transcript_hash = compute_transcript_hash(&client_hello, &server_pk_bytes, &server_nonce);
    let expected_server_tag = compute_server_auth_tag(master_key, &transcript_hash);
    if !bool::from(server_tag.ct_eq(&expected_server_tag)) {
        return Err(CryptoError::AuthenticationFailed);
    }

    let shared = client_sk.diffie_hellman(&PublicKey::from(server_pk_bytes));
    let material = derive_session_material(master_key, shared.as_bytes(), &transcript_hash)?;

    let (read_half, write_half) = stream.into_split();
    let reader = SecureReader {
        stream: read_half,
        cipher: XChaCha20Poly1305::new(Key::from_slice(&material.s2c_key)),
        nonce_base: material.s2c_nonce_base,
        direction: DIR_S2C,
        seq: 0,
    };
    let writer = SecureWriter {
        stream: write_half,
        cipher: XChaCha20Poly1305::new(Key::from_slice(&material.c2s_key)),
        nonce_base: material.c2s_nonce_base,
        direction: DIR_C2S,
        seq: 0,
    };
    Ok((reader, writer))
}

/// Performs the server side of the mutually-authenticated X25519 + PSK handshake.
pub async fn server_handshake(
    mut stream: TcpStream,
    master_key: &MasterKey,
) -> Result<(SecureReader, SecureWriter), CryptoError> {
    let mut client_hello = [0u8; CLIENT_HELLO_LEN];
    stream.read_exact(&mut client_hello).await?;

    if &client_hello[0..4] != WIRE_MAGIC {
        return Err(CryptoError::HandshakeFailed("invalid protocol magic"));
    }
    if client_hello[4] != PROTOCOL_VERSION {
        return Err(CryptoError::HandshakeFailed("unsupported protocol version"));
    }

    let mut client_pk_bytes = [0u8; 32];
    client_pk_bytes.copy_from_slice(&client_hello[5..37]);
    let mut client_nonce = [0u8; 32];
    client_nonce.copy_from_slice(&client_hello[37..69]);
    let mut ts_bytes = [0u8; 8];
    ts_bytes.copy_from_slice(&client_hello[69..77]);
    let client_ts_ms = u64::from_be_bytes(ts_bytes);
    let client_tag = &client_hello[77..109];

    let now = now_millis();
    if now.abs_diff(client_ts_ms) > MAX_CLOCK_SKEW_MS {
        return Err(CryptoError::ClockSkew);
    }

    let expected_client_tag = compute_client_auth_tag(
        master_key,
        PROTOCOL_VERSION,
        &client_pk_bytes,
        &client_nonce,
        client_ts_ms,
    );
    if !bool::from(client_tag.ct_eq(&expected_client_tag)) {
        return Err(CryptoError::AuthenticationFailed);
    }

    let server_sk = EphemeralSecret::random_from_rng(OsRng);
    let server_pk = PublicKey::from(&server_sk);
    let server_pk_bytes = *server_pk.as_bytes();

    let mut server_nonce = [0u8; 32];
    OsRng.fill_bytes(&mut server_nonce);

    let transcript_hash = compute_transcript_hash(&client_hello, &server_pk_bytes, &server_nonce);
    let server_tag = compute_server_auth_tag(master_key, &transcript_hash);

    let mut server_hello = [0u8; SERVER_HELLO_LEN];
    server_hello[0..4].copy_from_slice(WIRE_MAGIC);
    server_hello[4] = PROTOCOL_VERSION;
    server_hello[5..37].copy_from_slice(&server_pk_bytes);
    server_hello[37..69].copy_from_slice(&server_nonce);
    server_hello[69..101].copy_from_slice(&server_tag);

    stream.write_all(&server_hello).await?;
    stream.flush().await?;

    let shared = server_sk.diffie_hellman(&PublicKey::from(client_pk_bytes));
    let material = derive_session_material(master_key, shared.as_bytes(), &transcript_hash)?;

    let (read_half, write_half) = stream.into_split();
    let reader = SecureReader {
        stream: read_half,
        cipher: XChaCha20Poly1305::new(Key::from_slice(&material.c2s_key)),
        nonce_base: material.c2s_nonce_base,
        direction: DIR_C2S,
        seq: 0,
    };
    let writer = SecureWriter {
        stream: write_half,
        cipher: XChaCha20Poly1305::new(Key::from_slice(&material.s2c_key)),
        nonce_base: material.s2c_nonce_base,
        direction: DIR_S2C,
        seq: 0,
    };
    Ok((reader, writer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn full_duplex_e2ee_roundtrip() {
        let key = MasterKey::generate();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_key = key.clone();
        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut r, mut w) = server_handshake(stream, &server_key).await.unwrap();
            let msg = r.read_frame().await.unwrap();
            assert_eq!(msg, b"hello from android");
            w.write_frame(b"hello from pc").await.unwrap();
        });

        let client_stream = TcpStream::connect(addr).await.unwrap();
        let (mut cr, mut cw) = client_handshake(client_stream, &key).await.unwrap();
        cw.write_frame(b"hello from android").await.unwrap();
        let reply = cr.read_frame().await.unwrap();
        assert_eq!(reply, b"hello from pc");
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn wrong_key_is_rejected() {
        let server_key = MasterKey::generate();
        let wrong_key = MasterKey::generate();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            server_handshake(stream, &server_key).await
        });

        let client_stream = TcpStream::connect(addr).await.unwrap();
        let client_res = client_handshake(client_stream, &wrong_key).await;
        assert!(client_res.is_err());
        let server_res = server_task.await.unwrap();
        assert!(matches!(server_res, Err(CryptoError::AuthenticationFailed)));
    }

    #[test]
    fn passphrase_derivation_is_deterministic() {
        let k1 = MasterKey::derive_from_passphrase("correct-horse-battery-staple").unwrap();
        let k2 = MasterKey::derive_from_passphrase("correct-horse-battery-staple").unwrap();
        let k3 = MasterKey::derive_from_passphrase("different-secret-passphrase").unwrap();
        assert_eq!(k1.fingerprint(), k2.fingerprint());
        assert_ne!(k1.fingerprint(), k3.fingerprint());
    }
}
