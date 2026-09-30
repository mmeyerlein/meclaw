//! Whether a request came from the holder of the secret.
//!
//! A pure function of the raw body and one header value, so the whole verdict
//! is a table test. The raw body is what is signed: a sender signs the bytes it
//! sent, and re-serialising parsed JSON (key order, whitespace, escapes) breaks
//! the signature of a request that was genuine.
//!
//! Both comparisons take constant time over the compared bytes: the HMAC one
//! through `Mac::verify_slice`, the token one through `subtle`. A length that
//! cannot be right is refused at once — the length of a signature is public
//! (32 bytes for SHA-256), and that of the token is what any sender knows.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use super::params::VerifyKind;

/// The one `error_code` of a request that did not prove itself.
pub const WEBHOOK_UNVERIFIED: &str = "webhook_unverified";

/// Why a request did not prove itself. Each reason is a fixed text: none of
/// them repeats the header value, the signature or the secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unverified {
    /// The header that carries the proof is absent.
    MissingHeader,
    /// The header value does not start with the declared prefix.
    PrefixMismatch,
    /// The signature is no 64-digit hex string.
    MalformedSignature,
    /// The signature or the token does not match.
    Mismatch,
    /// There is no secret to compare against (a blank variable).
    NoSecret,
}

impl Unverified {
    /// The detail a receipt carries.
    pub fn detail(self) -> &'static str {
        match self {
            Unverified::MissingHeader => "the request carries no proof header",
            Unverified::PrefixMismatch => {
                "the proof header does not start with the declared prefix"
            }
            Unverified::MalformedSignature => "the signature is not a 64-digit hex string",
            Unverified::Mismatch => "the proof does not match",
            Unverified::NoSecret => "this mount holds no secret to verify against",
        }
    }
}

/// The verdict on one request. `header_value` is the raw value of the
/// declared header, `None` when the request has none; `prefix` is `""` when
/// none is declared. `raw` is the body exactly as received.
pub fn verify(
    kind: VerifyKind,
    secret: &str,
    header_value: Option<&[u8]>,
    prefix: &str,
    raw: &[u8],
) -> Result<(), Unverified> {
    if kind == VerifyKind::None {
        return Ok(());
    }
    // Fail closed before anything is compared: an empty key would make every
    // HMAC computable by anyone, and an empty token would match an empty header.
    if secret.is_empty() {
        return Err(Unverified::NoSecret);
    }
    let value = header_value.ok_or(Unverified::MissingHeader)?;
    let proof = value
        .strip_prefix(prefix.as_bytes())
        .ok_or(Unverified::PrefixMismatch)?;
    match kind {
        VerifyKind::HmacSha256 => {
            let sig = decode_hex_32(proof).ok_or(Unverified::MalformedSignature)?;
            let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes())
                .map_err(|_| Unverified::NoSecret)?;
            mac.update(raw);
            mac.verify_slice(&sig).map_err(|_| Unverified::Mismatch)
        }
        VerifyKind::Token => {
            if bool::from(proof.ct_eq(secret.as_bytes())) {
                Ok(())
            } else {
                Err(Unverified::Mismatch)
            }
        }
        VerifyKind::None => Ok(()),
    }
}

/// 64 hex digits of either case into 32 bytes; anything else is `None`.
fn decode_hex_32(s: &[u8]) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, pair) in s.chunks(2).enumerate() {
        out[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(out)
}

fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4231, test case 2: key "Jefe".
    const RFC_KEY: &str = "Jefe";
    const RFC_DATA: &[u8] = b"what do ya want for nothing?";
    const RFC_MAC: &str = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";

    fn hmac(value: Option<&str>, prefix: &str, raw: &[u8]) -> Result<(), Unverified> {
        verify(
            VerifyKind::HmacSha256,
            RFC_KEY,
            value.map(str::as_bytes),
            prefix,
            raw,
        )
    }

    /// One row: name, proof header, prefix, raw body, expected verdict.
    type Case<'a> = (
        &'a str,
        Option<&'a str>,
        &'a str,
        &'a [u8],
        Result<(), Unverified>,
    );

    #[test]
    fn hmac_table() {
        let upper = RFC_MAC.to_uppercase();
        let prefixed = format!("sha256={RFC_MAC}");
        let flipped = format!("{}4", &RFC_MAC[..63]);
        let upper_prefix = format!("SHA256={RFC_MAC}");
        let not_hex = format!("{}zz", &RFC_MAC[..62]);
        let cases: Vec<Case<'_>> = vec![
            ("rfc vector", Some(RFC_MAC), "", RFC_DATA, Ok(())),
            ("upper-case hex", Some(upper.as_str()), "", RFC_DATA, Ok(())),
            (
                "with its prefix",
                Some(prefixed.as_str()),
                "sha256=",
                RFC_DATA,
                Ok(()),
            ),
            (
                "prefix declared, not sent",
                Some(RFC_MAC),
                "sha256=",
                RFC_DATA,
                Err(Unverified::PrefixMismatch),
            ),
            (
                "prefix case differs",
                Some(upper_prefix.as_str()),
                "sha256=",
                RFC_DATA,
                Err(Unverified::PrefixMismatch),
            ),
            (
                "prefix sent, not declared",
                Some(prefixed.as_str()),
                "",
                RFC_DATA,
                Err(Unverified::MalformedSignature),
            ),
            (
                "one digit off",
                Some(flipped.as_str()),
                "",
                RFC_DATA,
                Err(Unverified::Mismatch),
            ),
            (
                "body changed by one byte",
                Some(RFC_MAC),
                "",
                &b"what do ya want for nothing!"[..],
                Err(Unverified::Mismatch),
            ),
            (
                "too short",
                Some(&RFC_MAC[..62]),
                "",
                RFC_DATA,
                Err(Unverified::MalformedSignature),
            ),
            (
                "not hex",
                Some(not_hex.as_str()),
                "",
                RFC_DATA,
                Err(Unverified::MalformedSignature),
            ),
            (
                "no header",
                None,
                "",
                RFC_DATA,
                Err(Unverified::MissingHeader),
            ),
            (
                "empty header",
                Some(""),
                "",
                RFC_DATA,
                Err(Unverified::MalformedSignature),
            ),
        ];
        for (name, value, prefix, raw, want) in cases {
            assert_eq!(hmac(value, prefix, raw), want, "{name}");
        }
    }

    /// The HMAC is over the raw bytes: the same JSON re-serialised is refused.
    #[test]
    fn the_raw_body_is_what_is_signed() {
        let raw = br#"{"b": 1,  "a": 2}"#;
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(b"k").expect("key");
        mac.update(raw);
        let sig: String = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let ok = verify(VerifyKind::HmacSha256, "k", Some(sig.as_bytes()), "", raw);
        assert_eq!(ok, Ok(()));
        let reserialised =
            serde_json::to_vec(&serde_json::from_slice::<serde_json::Value>(raw).expect("json"))
                .expect("ser");
        let again = verify(
            VerifyKind::HmacSha256,
            "k",
            Some(sig.as_bytes()),
            "",
            &reserialised,
        );
        assert_eq!(again, Err(Unverified::Mismatch));
    }

    #[test]
    fn token_table() {
        let t = |value: Option<&str>, prefix: &str| {
            verify(
                VerifyKind::Token,
                "tok-921",
                value.map(str::as_bytes),
                prefix,
                b"ignored",
            )
        };
        assert_eq!(t(Some("tok-921"), ""), Ok(()));
        assert_eq!(t(Some("Bearer tok-921"), "Bearer "), Ok(()));
        assert_eq!(
            t(Some("tok-921"), "Bearer "),
            Err(Unverified::PrefixMismatch)
        );
        assert_eq!(t(Some("tok-92"), ""), Err(Unverified::Mismatch));
        assert_eq!(t(Some("tok-9211"), ""), Err(Unverified::Mismatch));
        assert_eq!(t(Some("TOK-921"), ""), Err(Unverified::Mismatch));
        assert_eq!(t(Some(""), ""), Err(Unverified::Mismatch));
        assert_eq!(t(None, ""), Err(Unverified::MissingHeader));
    }

    #[test]
    fn an_empty_secret_refuses_everything_and_none_takes_everything() {
        for kind in [VerifyKind::HmacSha256, VerifyKind::Token] {
            assert_eq!(
                verify(kind, "", Some(b""), "", b""),
                Err(Unverified::NoSecret),
                "{kind:?}"
            );
        }
        assert_eq!(verify(VerifyKind::None, "", None, "", b"x"), Ok(()));
    }

    #[test]
    fn no_detail_carries_a_value() {
        for u in [
            Unverified::MissingHeader,
            Unverified::PrefixMismatch,
            Unverified::MalformedSignature,
            Unverified::Mismatch,
            Unverified::NoSecret,
        ] {
            assert!(!u.detail().contains(RFC_KEY));
            assert!(!u.detail().contains(RFC_MAC));
        }
    }
}
