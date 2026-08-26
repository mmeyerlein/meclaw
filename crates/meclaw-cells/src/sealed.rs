//! Sealed-box delivery of a secret over ordinary messaging (R3, GH #421).
//!
//! The message log journals every body, so a credential that travels as a
//! message must travel as a ciphertext. This module is the one place in the
//! workspace where X25519 appears; it is deliberately typeless about WHAT is
//! sealed, so any cell that needs a secret delivered can be a recipient.
//!
//! ```text
//! Recipient (llm/proxy), per request:
//!   r_sk  = 32 random bytes (getrandom)          # StaticSecret
//!   r_pk  = X25519(r_sk, basepoint)              # 32 bytes, hex on the wire
//!   -> sends r_pk with the policy-gated request
//!
//! Sealer (the vault), per answer:
//!   e_sk  = 32 random bytes (getrandom)          # ephemeral, dies in this fn
//!   e_pk  = X25519(e_sk, basepoint)
//!   s     = X25519(e_sk, r_pk)                   # 32-byte shared secret
//!   k     = HMAC-SHA256(key = "meclaw-sealed-box-v1", msg = s || e_pk || r_pk)
//!   n     = 24 random bytes
//!   ct    = XChaCha20-Poly1305(k, n).encrypt(plaintext)
//!   -> SealedBox { epk: hex(e_pk), nonce: hex(n), ciphertext: hex(ct) }
//!
//! Recipient:
//!   s     = X25519(r_sk, e_pk)                   # the same s
//!   k     = HMAC-SHA256(key = "meclaw-sealed-box-v1", msg = s || e_pk || r_pk)
//!   pt    = XChaCha20-Poly1305(k, n).decrypt(ct) # in RAM only
//! ```
//!
//! Why exactly this shape:
//!
//! - **No long-lived sealer key.** The sealer mints `e_sk` per answer and drops
//!   it. There is nothing an attacker could pull out of a database later to
//!   open an old box — forward secrecy falls out for free.
//! - **No authenticity from the crypto.** The box does not say the vault wrote
//!   it. That is deliberate (R3): authenticity is the topology plus the policy
//!   — only the broker may address the vault, and only a valid grant reaches
//!   that far. A signature is addable later without breaking the wire form (one
//!   more field in `SealedBox`).
//! - **`e_pk` and `r_pk` go into the key derivation.** Without that transcript
//!   binding the box would be open to cross-protocol recycling: the same
//!   Diffie-Hellman in another role would produce the same key. Binding both
//!   halves is one line and closes it.
//! - **`hmac` rather than `hkdf`.** The output is exactly 32 bytes, so the
//!   extract step suffices; an expand round would add a step and no statement.
//!   It saves a dependency.
//! - **Hex, not base64.** `vault::crypto::hex`/`unhex` already exist and are
//!   the form in which the vault already puts signatures on the wire.

use crate::vault::crypto::{self, KEY_LEN, NONCE_LEN};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use meclaw_core::serde_json::{Value, json};

/// The label that binds this key derivation to this protocol version.
const SEALED_BOX_LABEL: &[u8] = b"meclaw-sealed-box-v1";

/// What can go wrong on either side of a sealed box.
#[derive(Debug, PartialEq, Eq)]
pub enum SealError {
    /// The recipient's public key was not 32 bytes of lowercase hex.
    BadPublicKey,
    /// A field of the wire form was missing or the wrong length.
    Malformed(&'static str),
    /// The box did not open: wrong recipient, wrong nonce, or tampered
    /// ciphertext. Deliberately one variant — telling them apart is a gift to
    /// an attacker and useless to an honest caller.
    Open,
    /// The system random source failed.
    Random(String),
}

impl std::fmt::Display for SealError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadPublicKey => write!(f, "recipient key is not 32 bytes of hex"),
            Self::Malformed(w) => write!(f, "malformed sealed box field: {w}"),
            Self::Open => write!(f, "sealed box did not open"),
            Self::Random(e) => write!(f, "random source unavailable: {e}"),
        }
    }
}

impl std::error::Error for SealError {}

/// A sealed box as it travels: three lowercase-hex strings and nothing else.
///
/// It carries no sender identity on purpose (R3): who may seal is decided by
/// the topology and the policy in front of the vault, not by a signature. A
/// signature is addable later as a fourth field without breaking this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedBox {
    /// The sealer's ephemeral X25519 public key, hex.
    pub epk: String,
    /// The XChaCha20-Poly1305 nonce, hex.
    pub nonce: String,
    /// The ciphertext including its Poly1305 tag, hex.
    pub ciphertext: String,
}

impl SealedBox {
    /// The wire form.
    pub fn to_json(&self) -> Value {
        json!({"epk": self.epk, "nonce": self.nonce, "ciphertext": self.ciphertext})
    }

    /// Read the wire form back. Every field is required and must be hex.
    pub fn from_json(v: &Value) -> Result<Self, SealError> {
        let field = |k: &'static str| -> Result<String, SealError> {
            let s = v
                .get(k)
                .and_then(|x| x.as_str())
                .ok_or(SealError::Malformed(k))?;
            crypto::unhex(s).ok_or(SealError::Malformed(k))?;
            Ok(s.to_string())
        };
        Ok(Self {
            epk: field("epk")?,
            nonce: field("nonce")?,
            ciphertext: field("ciphertext")?,
        })
    }
}

/// Decode a hex public key into the 32 raw bytes X25519 wants.
fn public_from_hex(hex: &str) -> Result<[u8; 32], SealError> {
    let raw = crypto::unhex(hex).ok_or(SealError::BadPublicKey)?;
    let arr: [u8; 32] = raw.try_into().map_err(|_| SealError::BadPublicKey)?;
    Ok(arr)
}

/// Derive the box key from the shared secret, bound to both public keys.
///
/// One HMAC-SHA256 extract: the output is exactly the 32 bytes XChaCha20 needs,
/// so an expand round would add a step and no statement. Both public keys go
/// into the transcript so the same Diffie-Hellman cannot be recycled by another
/// protocol that happens to reach the same shared secret.
fn box_key(shared: &[u8], epk: &[u8; 32], rpk: &[u8; 32]) -> Result<[u8; KEY_LEN], SealError> {
    let mut transcript = Vec::with_capacity(96);
    transcript.extend_from_slice(shared);
    transcript.extend_from_slice(epk);
    transcript.extend_from_slice(rpk);
    let tag = crypto::mac(SEALED_BOX_LABEL, &transcript).map_err(|_| SealError::Open)?;
    tag.try_into().map_err(|_| SealError::Open)
}

/// Seal `plaintext` to a recipient's ephemeral public key.
///
/// The sealer mints its OWN ephemeral key here and drops it at the end of this
/// function: there is no long-lived vault private key anywhere, so there is
/// nothing an attacker could steal later to open an old box.
pub fn seal_to(recipient_public_hex: &str, plaintext: &[u8]) -> Result<SealedBox, SealError> {
    let rpk = public_from_hex(recipient_public_hex)?;
    let e_sk = x25519_dalek::StaticSecret::from(
        crypto::random_bytes::<32>().map_err(|e| SealError::Random(e.to_string()))?,
    );
    let e_pk = x25519_dalek::PublicKey::from(&e_sk);
    let shared = e_sk.diffie_hellman(&x25519_dalek::PublicKey::from(rpk));
    let key = box_key(shared.as_bytes(), e_pk.as_bytes(), &rpk)?;
    let nonce =
        crypto::random_bytes::<NONCE_LEN>().map_err(|e| SealError::Random(e.to_string()))?;
    let ct = XChaCha20Poly1305::new(Key::from_slice(&key))
        .encrypt(XNonce::from_slice(&nonce), plaintext)
        .map_err(|_| SealError::Open)?;
    Ok(SealedBox {
        epk: crypto::hex(e_pk.as_bytes()),
        nonce: crypto::hex(&nonce),
        ciphertext: crypto::hex(&ct),
    })
}

/// The recipient's ephemeral key pair, minted per request and held in RAM only.
///
/// No `Debug`, no `Clone`, no serialisation: the whole point of this type is
/// that it never leaves the task it was created in.
pub struct RecipientKeypair {
    secret: x25519_dalek::StaticSecret,
    public: [u8; 32],
}

impl RecipientKeypair {
    /// Mint a fresh pair from the system random source.
    pub fn generate() -> Result<Self, SealError> {
        let secret = x25519_dalek::StaticSecret::from(
            crypto::random_bytes::<32>().map_err(|e| SealError::Random(e.to_string()))?,
        );
        let public = *x25519_dalek::PublicKey::from(&secret).as_bytes();
        Ok(Self { secret, public })
    }

    /// The half that travels with the request.
    pub fn public_hex(&self) -> String {
        crypto::hex(&self.public)
    }

    /// Open a box addressed to this pair. The plaintext exists only in the
    /// `Vec` this returns — nothing here writes, logs or emits it.
    pub fn open(&self, sealed: &SealedBox) -> Result<Vec<u8>, SealError> {
        let epk = public_from_hex(&sealed.epk).map_err(|_| SealError::Malformed("epk"))?;
        let nonce = crypto::unhex(&sealed.nonce).ok_or(SealError::Malformed("nonce"))?;
        if nonce.len() != NONCE_LEN {
            return Err(SealError::Malformed("nonce"));
        }
        let ct = crypto::unhex(&sealed.ciphertext).ok_or(SealError::Malformed("ciphertext"))?;
        let shared = self
            .secret
            .diffie_hellman(&x25519_dalek::PublicKey::from(epk));
        let key = box_key(shared.as_bytes(), &epk, &self.public)?;
        XChaCha20Poly1305::new(Key::from_slice(&key))
            .decrypt(XNonce::from_slice(&nonce), ct.as_slice())
            .map_err(|_| SealError::Open)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"sk-or-v1-THE-ACTUAL-CREDENTIAL";

    #[test]
    fn a_box_sealed_to_a_recipient_opens_only_for_that_recipient() {
        let me = RecipientKeypair::generate().expect("keypair");
        let sealed = seal_to(&me.public_hex(), SECRET).expect("seal");
        assert_eq!(me.open(&sealed).expect("open"), SECRET.to_vec());

        let somebody_else = RecipientKeypair::generate().expect("keypair");
        assert!(
            matches!(somebody_else.open(&sealed), Err(SealError::Open)),
            "a second recipient must learn nothing from the box"
        );
    }

    #[test]
    fn the_wire_form_never_contains_the_plaintext() {
        let me = RecipientKeypair::generate().expect("keypair");
        let sealed = seal_to(&me.public_hex(), SECRET).expect("seal");
        let wire = sealed.to_json().to_string();
        assert!(!wire.contains("sk-or-v1"), "{wire}");
        assert!(!wire.contains("CREDENTIAL"), "{wire}");
    }

    #[test]
    fn the_same_secret_seals_differently_every_time() {
        let me = RecipientKeypair::generate().expect("keypair");
        let a = seal_to(&me.public_hex(), SECRET).expect("seal");
        let b = seal_to(&me.public_hex(), SECRET).expect("seal");
        assert_ne!(a.epk, b.epk, "the sealer's ephemeral key must not repeat");
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn a_tampered_box_is_refused_rather_than_returned_garbled() {
        let me = RecipientKeypair::generate().expect("keypair");
        let mut sealed = seal_to(&me.public_hex(), SECRET).expect("seal");
        sealed.ciphertext.replace_range(0..2, "ff");
        assert!(matches!(me.open(&sealed), Err(SealError::Open)));
    }

    #[test]
    fn a_recipient_key_that_is_not_a_key_is_a_named_error() {
        assert!(matches!(
            seal_to("not-hex", SECRET),
            Err(SealError::BadPublicKey)
        ));
        assert!(matches!(
            seal_to("abcd", SECRET),
            Err(SealError::BadPublicKey)
        ));
        assert!(matches!(seal_to("", SECRET), Err(SealError::BadPublicKey)));
    }

    #[test]
    fn the_wire_form_round_trips_and_refuses_a_half_one() {
        let me = RecipientKeypair::generate().expect("keypair");
        let sealed = seal_to(&me.public_hex(), SECRET).expect("seal");
        assert_eq!(
            SealedBox::from_json(&sealed.to_json()).expect("parse"),
            sealed
        );
        let mut half = sealed.to_json();
        half.as_object_mut().expect("object").remove("nonce");
        assert!(matches!(
            SealedBox::from_json(&half),
            Err(SealError::Malformed("nonce"))
        ));
    }

    /// The dependency probe. X25519 is the one primitive the workspace did not
    /// have; everything else this module needs is already a direct dependency.
    #[test]
    fn the_curve_is_available_and_agrees_with_itself() {
        let a = x25519_dalek::StaticSecret::from([7u8; 32]);
        let b = x25519_dalek::StaticSecret::from([9u8; 32]);
        let (a_pub, b_pub) = (
            x25519_dalek::PublicKey::from(&a),
            x25519_dalek::PublicKey::from(&b),
        );
        assert_eq!(
            a.diffie_hellman(&b_pub).as_bytes(),
            b.diffie_hellman(&a_pub).as_bytes(),
            "X25519 must agree from both ends"
        );
    }
}
