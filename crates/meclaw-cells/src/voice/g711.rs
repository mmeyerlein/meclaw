//! G.711 companding, both laws, both directions — sample by sample.
//!
//! A telephone line carries G.711 at 8 kHz, and a live model takes it raw
//! (`{"type": "audio/pcmu", "rate": 8000}` and `audio/pcma`, the OpenAI
//! "WebSocket audio formats" guide). The edge in front of this cell does not:
//! FreeSWITCH's `mod_audio_stream` streams L16 only, at 8k or 16k
//! (`templates/freeswitch/README.md`, OR-LV-5). So the `gpt_live` adapter
//! compands each frame on its way through: L16 8 kHz from the client becomes
//! one G.711 byte per sample on the wire, and what comes back becomes L16
//! again. Companding is a table per sample, not a conversion between rates —
//! the cell still never resamples (R-V2, R-LS-9).
//!
//! The algorithms are the ITU-T G.711 reference ones (the classic segment
//! search of the public-domain Sun `g711.c`), written out because there is no
//! codec crate on the allow-list and a `Cargo.toml` change is not part of this
//! strand. A frame is little-endian PCM16; a trailing odd byte is not a sample
//! and is left out (the connection's parity check refuses such frames before
//! they get here, R-V6').

/// Which of the two laws a session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Law {
    /// μ-law (North America, Japan; `audio/pcmu`).
    Mu,
    /// A-law (Europe; `audio/pcma`).
    A,
}

/// The bias μ-law adds before the segment search.
const MU_BIAS: i32 = 0x84;
/// The largest magnitude μ-law encodes without clipping.
const MU_CLIP: i32 = 32_635;

/// One PCM16 sample to μ-law.
pub fn mu_encode(sample: i16) -> u8 {
    let mut s = i32::from(sample);
    let sign: u8 = if s < 0 { 0x80 } else { 0 };
    if s < 0 {
        s = -s;
    }
    s = s.min(MU_CLIP) + MU_BIAS;
    let mut exponent: u8 = 7;
    let mut mask: i32 = 0x4000;
    while s & mask == 0 && exponent > 0 {
        exponent -= 1;
        mask >>= 1;
    }
    let mantissa = ((s >> (exponent + 3)) & 0x0F) as u8;
    !(sign | (exponent << 4) | mantissa)
}

/// One μ-law byte to PCM16.
pub fn mu_decode(byte: u8) -> i16 {
    let u = !byte;
    let exponent = (u >> 4) & 0x07;
    let mantissa = i32::from(u & 0x0F);
    let magnitude = (((mantissa << 3) + MU_BIAS) << exponent) - MU_BIAS;
    if u & 0x80 != 0 {
        -magnitude as i16
    } else {
        magnitude as i16
    }
}

/// The upper ends of the eight A-law segments, on the 13-bit scale.
const A_SEG_END: [i32; 8] = [0x1F, 0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF];

/// One PCM16 sample to A-law.
pub fn a_encode(sample: i16) -> u8 {
    let mut pcm = i32::from(sample) >> 3;
    let mask: u8 = if pcm >= 0 {
        0xD5
    } else {
        pcm = -pcm - 1;
        0x55
    };
    let Some(seg) = A_SEG_END.iter().position(|end| pcm <= *end) else {
        return 0x7F ^ mask;
    };
    let mut aval = (seg as u8) << 4;
    aval |= if seg < 2 {
        ((pcm >> 1) & 0x0F) as u8
    } else {
        ((pcm >> seg) & 0x0F) as u8
    };
    aval ^ mask
}

/// One A-law byte to PCM16.
pub fn a_decode(byte: u8) -> i16 {
    let a = byte ^ 0x55;
    let mut t = i32::from(a & 0x0F) << 4;
    let seg = (a & 0x70) >> 4;
    match seg {
        0 => t += 8,
        1 => t += 0x108,
        _ => {
            t += 0x108;
            t <<= seg - 1;
        }
    }
    if a & 0x80 != 0 { t as i16 } else { -t as i16 }
}

/// A frame of little-endian PCM16 to one G.711 byte per sample.
pub fn compress(law: Law, pcm: &[u8]) -> Vec<u8> {
    pcm.as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let s = i16::from_le_bytes(*pair);
            match law {
                Law::Mu => mu_encode(s),
                Law::A => a_encode(s),
            }
        })
        .collect()
}

/// G.711 bytes to a frame of little-endian PCM16, two bytes per sample.
pub fn expand(law: Law, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for b in bytes {
        let s = match law {
            Law::Mu => mu_decode(*b),
            Law::A => a_decode(*b),
        };
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference points of the μ-law table: silence, and both full scales.
    #[test]
    fn mu_law_meets_the_reference_points() {
        assert_eq!(mu_encode(0), 0xFF, "digital silence is 0xFF");
        assert_eq!(mu_decode(0xFF), 0);
        assert_eq!(mu_decode(0x80), 32_124, "positive full scale");
        assert_eq!(mu_decode(0x00), -32_124, "negative full scale");
        assert_eq!(mu_encode(i16::MAX), 0x80, "clipped, not wrapped");
        assert_eq!(mu_encode(i16::MIN), 0x00);
    }

    /// Every code survives decode then encode — except 0x7F, μ-law's second
    /// zero (negative zero), which encodes back as the first one.
    #[test]
    fn every_mu_law_code_round_trips() {
        for b in 0u8..=255 {
            if b == 0x7F {
                assert_eq!(mu_encode(mu_decode(b)), 0xFF);
                continue;
            }
            assert_eq!(mu_encode(mu_decode(b)), b, "code {b:#04x}");
        }
    }

    /// The A-law reference points, and every code round-trips (A-law has
    /// no second zero).
    #[test]
    fn every_a_law_code_round_trips() {
        assert_eq!(a_encode(0), 0xD5, "digital silence is 0xD5");
        assert_eq!(a_decode(0xD5), 8);
        assert_eq!(a_decode(0xAA), 32_256, "positive full scale");
        assert_eq!(a_decode(0x2A), -32_256, "negative full scale");
        for b in 0u8..=255 {
            assert_eq!(a_encode(a_decode(b)), b, "code {b:#04x}");
        }
    }

    /// A frame halves on the way out and doubles on the way in.
    #[test]
    fn a_frame_halves_and_doubles() {
        let pcm: Vec<u8> = [0i16, 1000, -1000, 8000]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        for law in [Law::Mu, Law::A] {
            let wire = compress(law, &pcm);
            assert_eq!(wire.len(), pcm.len() / 2);
            assert_eq!(expand(law, &wire).len(), pcm.len());
        }
    }
}
