//! Standard-alphabet base64, both ways — what a duplex socket carries audio in.
//!
//! `base64` is not on the tech-stack allow-list (`docs/meclaw-overview.md`
//! § Tech-Stack) and a `Cargo.toml` change is not part of this wave, so the
//! forty lines the protocol actually needs are written out. The encoder is the
//! same shape the OpenAI transcription adapter already carries
//! (`providers/openai_stt.rs`); that one stays where it is rather than being
//! refactored onto this, because a working module is not a reason to touch a
//! working module.
//!
//! What is new here is [`decode`]: a cascade only ever SENDS audio, a duplex
//! session gets it back, and `session.output_audio.delta` is base64 on the way
//! in.

/// The standard alphabet (RFC 4648 § 4), padded. Not the URL-safe one: the
/// payload rides in a JSON string, where `+` and `/` need no escaping.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode bytes into standard-alphabet base64 with padding.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// The value of one base64 character, or `None` for anything outside the
/// alphabet. Padding is handled by the caller, so `=` is not a value here.
fn value_of(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some((c - b'A') as u32),
        b'a'..=b'z' => Some((c - b'a') as u32 + 26),
        b'0'..=b'9' => Some((c - b'0') as u32 + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decode standard-alphabet base64 with padding, or `None` for anything that
/// is not that.
///
/// Strict on purpose: whitespace, the URL-safe alphabet, a length that is not a
/// multiple of four and padding in the middle are all refused rather than
/// repaired. What arrives here is a machine-written frame, and a frame this
/// function had to guess about is a frame that would reach a caller's ear as
/// noise.
pub fn decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    for (i, quad) in b.chunks(4).enumerate() {
        let last = i == b.len() / 4 - 1;
        let pad = if last {
            quad.iter().filter(|c| **c == b'=').count()
        } else {
            0
        };
        if pad > 2 || quad[..4 - pad].contains(&b'=') {
            return None;
        }
        let mut n = 0u32;
        for c in &quad[..4 - pad] {
            n = (n << 6) | value_of(*c)?;
        }
        n <<= 6 * pad;
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every length class round-trips, including the two that need padding.
    #[test]
    fn every_length_round_trips() {
        for len in 0..=64usize {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 7 % 256) as u8).collect();
            let text = encode(&bytes);
            assert_eq!(
                decode(&text).as_deref(),
                Some(bytes.as_slice()),
                "length {len} did not survive the round trip: {text}"
            );
        }
    }

    /// The padding is the standard one, character for character — a peer that
    /// checks the tail must see what RFC 4648 says it will.
    #[test]
    fn the_padding_is_the_standard_one() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(decode("Zg==").as_deref(), Some(&b"f"[..]));
        assert_eq!(decode("Zm8=").as_deref(), Some(&b"fo"[..]));
        // Both non-alphanumeric characters of the standard alphabet.
        assert_eq!(
            decode(&encode(&[251u8, 255, 190])),
            Some(vec![251, 255, 190])
        );
        assert!(encode(&[251u8, 255, 190]).contains('+'));
        assert!(encode(&[255u8, 255, 255]).contains('/'));
    }

    /// Anything that is not standard base64 is refused rather than repaired.
    #[test]
    fn a_frame_that_is_not_base64_is_refused() {
        for bad in [
            "Zg=",      // not a multiple of four
            "Zg=A",     // padding in the middle of the quad
            "Zm9=dm8=", // and of an earlier quad
            "Zm9 v",    // whitespace
            "Zm9-",     // the URL-safe alphabet is a different alphabet
            "Z===",     // three pad characters decode nothing
        ] {
            assert!(decode(bad).is_none(), "`{bad}` is not standard base64");
        }
    }
}
