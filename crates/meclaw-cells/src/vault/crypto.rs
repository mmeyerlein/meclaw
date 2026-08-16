//! The vault's cryptography: derive a key, seal a secret, use a secret.
//!
//! Three jobs, one primitive each, no choices left to the caller:
//!
//! - **derive** — argon2id turns a passphrase plus a per-store salt into the
//!   32-byte master key. The parameters are pinned here, not configurable: a
//!   vault whose KDF cost can be lowered by whoever writes the config is not a
//!   vault.
//! - **seal / open** — XChaCha20-Poly1305 with a random 24-byte nonce per
//!   secret. The extended nonce is the reason for the X variant: at 24 bytes a
//!   random nonce cannot realistically repeat, so nothing has to track a
//!   counter across restarts.
//! - **mac** — HMAC-SHA256. This is what `vault.use` does *with* a secret. The
//!   vault never returns key material; it returns what the key produced.
//!
//! Nothing in this module logs, and nothing returns a plaintext secret to a
//! caller outside the vault cell.

use argon2::Argon2;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Length of the master key and of the salt, in bytes.
pub const KEY_LEN: usize = 32;
/// Length of the per-store salt, in bytes.
pub const SALT_LEN: usize = 16;
/// Length of an XChaCha20-Poly1305 nonce, in bytes.
pub const NONCE_LEN: usize = 24;

/// What can go wrong below the vault's route surface.
#[derive(Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// The key could not be derived from the passphrase.
    Derive(String),
    /// Sealing failed — practically only an allocation failure.
    Seal,
    /// Opening failed: wrong key, wrong nonce, or a tampered ciphertext.
    /// Deliberately one variant — telling the three apart is a gift to an
    /// attacker and useless to an honest caller.
    Open,
    /// A stored field did not have the length its format requires.
    Malformed(&'static str),
    /// The system random source failed.
    Random(String),
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Derive(e) => write!(f, "key derivation failed: {e}"),
            Self::Seal => write!(f, "seal failed"),
            Self::Open => write!(f, "open failed: wrong key or tampered ciphertext"),
            Self::Malformed(what) => write!(f, "malformed stored field: {what}"),
            Self::Random(e) => write!(f, "random source unavailable: {e}"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// The master key, held in memory only while the vault is unlocked.
///
/// Zeroed on drop. That is a best-effort measure, honestly labelled: it closes
/// the window where a freed page is handed to the next allocation, not the
/// window where a process with the same rights reads live memory. The design
/// answer to the second one is placement (own process/user), not this type.
pub struct MasterKey([u8; KEY_LEN]);

impl MasterKey {
    /// Wrap raw key bytes — used by key sources that deliver a key directly
    /// rather than a passphrase.
    pub fn from_bytes(raw: [u8; KEY_LEN]) -> Self {
        Self(raw)
    }

    /// Derive the master key from a passphrase and the store's salt (argon2id).
    ///
    /// Cost parameters are the argon2 crate's defaults for the id variant,
    /// pinned by this call site rather than by config.
    pub fn derive(passphrase: &[u8], salt: &[u8]) -> Result<Self, CryptoError> {
        if salt.len() != SALT_LEN {
            return Err(CryptoError::Malformed("salt"));
        }
        let mut out = [0u8; KEY_LEN];
        Argon2::default()
            .hash_password_into(passphrase, salt, &mut out)
            .map_err(|e| CryptoError::Derive(e.to_string()))?;
        Ok(Self(out))
    }

    /// Seal a plaintext. Returns `(nonce, ciphertext)`; both are stored, only
    /// the key is secret.
    pub fn seal(&self, plaintext: &[u8]) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&self.0));
        let nonce_bytes = random_bytes::<NONCE_LEN>()?;
        let nonce = XNonce::from_slice(&nonce_bytes);
        let ct = cipher
            .encrypt(nonce, plaintext)
            .map_err(|_| CryptoError::Seal)?;
        Ok((nonce_bytes.to_vec(), ct))
    }

    /// Open a sealed secret. Any failure is one error — see [`CryptoError::Open`].
    pub fn open(&self, nonce: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if nonce.len() != NONCE_LEN {
            return Err(CryptoError::Malformed("nonce"));
        }
        let cipher = XChaCha20Poly1305::new(Key::from_slice(&self.0));
        cipher
            .decrypt(XNonce::from_slice(nonce), ciphertext)
            .map_err(|_| CryptoError::Open)
    }
}

impl Drop for MasterKey {
    fn drop(&mut self) {
        // Overwrite through a volatile write so the compiler may not elide it
        // as a dead store into memory nobody reads again.
        for b in self.0.iter_mut() {
            unsafe { std::ptr::write_volatile(b, 0) };
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

/// HMAC-SHA256 over `payload` with `secret` — what `vault.use` does instead of
/// handing the secret out. Returns the raw tag; the cell hex-encodes it.
pub fn mac(secret: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut m =
        <Hmac<Sha256> as Mac>::new_from_slice(secret).expect("HMAC accepts a key of any length");
    m.update(payload);
    m.finalize().into_bytes().to_vec()
}

/// N random bytes from the system source.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).map_err(|e| CryptoError::Random(e.to_string()))?;
    Ok(buf)
}

/// A fresh per-store salt.
pub fn new_salt() -> Result<[u8; SALT_LEN], CryptoError> {
    random_bytes::<SALT_LEN>()
}

/// Lowercase hex, for tags and key ids on the wire.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Decode lowercase hex back to bytes; `None` on any malformed input.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// A short, non-secret fingerprint of the master key, for `vault.status`.
///
/// It is the MAC of a fixed label under the key, truncated — enough to tell
/// "the vault is holding the key you think it is" apart from "some other key",
/// and not enough to be worth attacking.
pub fn key_id(key: &MasterKey) -> String {
    hex(&mac(&key.0, b"meclaw-vault-key-id")[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sealed_secret_comes_back_under_the_same_key() {
        let salt = new_salt().unwrap();
        let key = MasterKey::derive(b"correct horse", &salt).unwrap();
        let (nonce, ct) = key.seal(b"hunter2").unwrap();
        assert_eq!(key.open(&nonce, &ct).unwrap(), b"hunter2");
        // The ciphertext must not contain the plaintext.
        assert!(!ct.windows(7).any(|w| w == b"hunter2"));
    }

    #[test]
    fn another_passphrase_does_not_open_it() {
        let salt = new_salt().unwrap();
        let (nonce, ct) = MasterKey::derive(b"correct horse", &salt)
            .unwrap()
            .seal(b"hunter2")
            .unwrap();
        let wrong = MasterKey::derive(b"incorrect horse", &salt).unwrap();
        assert!(matches!(wrong.open(&nonce, &ct), Err(CryptoError::Open)));
    }

    #[test]
    fn a_tampered_ciphertext_is_refused_rather_than_returned_garbled() {
        let salt = new_salt().unwrap();
        let key = MasterKey::derive(b"pw", &salt).unwrap();
        let (nonce, mut ct) = key.seal(b"hunter2").unwrap();
        ct[0] ^= 0x01;
        assert!(matches!(key.open(&nonce, &ct), Err(CryptoError::Open)));
    }

    #[test]
    fn the_same_plaintext_seals_differently_every_time() {
        let salt = new_salt().unwrap();
        let key = MasterKey::derive(b"pw", &salt).unwrap();
        let (n1, c1) = key.seal(b"same").unwrap();
        let (n2, c2) = key.seal(b"same").unwrap();
        assert_ne!(n1, n2, "a repeated nonce would leak the plaintext relation");
        assert_ne!(c1, c2);
    }

    #[test]
    fn the_same_salt_and_passphrase_derive_the_same_key() {
        let salt = new_salt().unwrap();
        let a = MasterKey::derive(b"pw", &salt).unwrap();
        let b = MasterKey::derive(b"pw", &salt).unwrap();
        // Proven through the derived id rather than by exposing the bytes.
        assert_eq!(key_id(&a), key_id(&b));
        let other = MasterKey::derive(b"pw", &new_salt().unwrap()).unwrap();
        assert_ne!(key_id(&a), key_id(&other), "the salt must matter");
    }

    #[test]
    fn a_wrong_salt_length_is_a_named_error_not_a_silent_truncation() {
        assert!(matches!(
            MasterKey::derive(b"pw", &[0u8; 4]),
            Err(CryptoError::Malformed("salt"))
        ));
    }

    #[test]
    fn mac_is_stable_per_secret_and_changes_with_it() {
        assert_eq!(mac(b"k", b"payload"), mac(b"k", b"payload"));
        assert_ne!(mac(b"k", b"payload"), mac(b"k2", b"payload"));
        assert_ne!(mac(b"k", b"payload"), mac(b"k", b"payload2"));
    }

    #[test]
    fn hex_round_trips() {
        let raw = random_bytes::<16>().unwrap();
        assert_eq!(unhex(&hex(&raw)).unwrap(), raw.to_vec());
        assert!(unhex("abc").is_none(), "odd length");
        assert!(unhex("zz").is_none(), "not hex");
    }
}
