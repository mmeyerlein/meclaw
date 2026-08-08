//! Hamming distance over binarized embedding vectors, registered as a SQLite
//! scalar function (P4, memory-spec A.2.5).
//!
//! The store only ever *stores and compares* vectors — generating them is a
//! topology concern (memory-spec B.1.1). Comparison never crosses embedding
//! generations: that rule belongs to the caller, and this module makes a breach
//! loud instead of silently mis-ranking.

/// Register `hamming(a, b)` on a `store` `cell.db` connection.
///
/// Called at every point where such a connection comes into existence — the
/// factory's `WakeFn` and `RespawnFn` — so the function survives wake, re-wake
/// and respawn. It is bound to the connection, not to the cell.
///
/// Both arguments may be a BLOB or a strict base64 TEXT; `NULL` yields `NULL`.
/// Anything else — a length mismatch, broken base64, a numeric argument — raises
/// a SQLite error, which the op layer reports as a regular `sql_error` outcome.
pub fn register(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    use rusqlite::functions::FunctionFlags;
    conn.create_scalar_function(
        "hamming",
        2,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        |ctx| {
            let (Some(a), Some(b)) = (arg_bytes(ctx, 0)?, arg_bytes(ctx, 1)?) else {
                return Ok(None);
            };
            hamming_bytes(&a, &b)
                .map(Some)
                .map_err(|e| rusqlite::Error::UserFunctionError(e.into()))
        },
    )
}

/// Normalize one function argument to bytes: BLOB as-is, TEXT as strict base64,
/// `NULL` as `None`. Any other storage class is a caller error.
fn arg_bytes(
    ctx: &rusqlite::functions::Context<'_>,
    idx: usize,
) -> rusqlite::Result<Option<Vec<u8>>> {
    use rusqlite::types::ValueRef;
    let err = |e: String| rusqlite::Error::UserFunctionError(e.into());
    match ctx.get_raw(idx) {
        ValueRef::Null => Ok(None),
        ValueRef::Blob(b) => Ok(Some(b.to_vec())),
        ValueRef::Text(t) => {
            let s = std::str::from_utf8(t)
                .map_err(|_| err("hamming: argument is not valid UTF-8".into()))?;
            decode_base64(s).map(Some).map_err(err)
        }
        _ => Err(err(format!(
            "hamming: argument {idx} must be a base64 text or a blob"
        ))),
    }
}

/// Decode standard Base64 (RFC 4648 §4) strictly.
///
/// Strict means: standard alphabet only (no URL-safe `-_`), length a multiple
/// of four, padding only at the very end and at most two characters, no
/// whitespace and no other slack anywhere. Every deviation is an error — this
/// decoder sees caller text and row content, so a best-effort rescue would be a
/// silent reinterpretation of hostile input.
pub fn decode_base64(s: &str) -> Result<Vec<u8>, String> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(4) {
        return Err(format!("base64: length {} is not a multiple of 4", b.len()));
    }
    let pad = b.iter().rev().take_while(|c| **c == b'=').count();
    if pad > 2 || (pad > 0 && b.len() == pad) {
        return Err(format!("base64: invalid padding ({pad} characters)"));
    }
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    let mut acc: u32 = 0;
    for (i, c) in b[..b.len() - pad].iter().enumerate() {
        let v = sextet(*c).ok_or_else(|| format!("base64: invalid character {:?}", *c as char))?;
        acc = (acc << 6) | u32::from(v);
        if i % 4 == 3 {
            out.extend_from_slice(&[(acc >> 16) as u8, (acc >> 8) as u8, acc as u8]);
            acc = 0;
        }
    }
    // Tail: three payload characters carry two bytes, two carry one.
    if pad == 1 {
        out.push((acc >> 10) as u8);
        out.push((acc >> 2) as u8);
    } else if pad == 2 {
        out.push((acc >> 4) as u8);
    }
    Ok(out)
}

/// Map one Base64 character to its 6-bit value — the closed alphabet.
fn sextet(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Hamming distance between two equally long byte strings: the number of
/// differing bits.
///
/// Unequal lengths are an error, never a value. Two binarized vectors of
/// different length come from different embedding generations, and comparing
/// them at all is the bug — see the module docs.
pub fn hamming_bytes(a: &[u8], b: &[u8]) -> Result<i64, String> {
    if a.len() != b.len() {
        return Err(format!(
            "hamming: length mismatch ({} vs {} bytes)",
            a.len(),
            b.len()
        ));
    }
    Ok(a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x ^ y).count_ones() as i64)
        .sum())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tripwire: `create_scalar_function` requires the rusqlite feature
    /// `functions`. Without it this test does not compile — which is exactly the
    /// receipt that the (sanctioned) feature line is load-bearing, not cosmetic.
    #[test]
    fn scalar_functions_are_available() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        c.create_scalar_function(
            "p4probe",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            |_| Ok(1i64),
        )
        .unwrap();
        let n: i64 = c.query_row("SELECT p4probe()", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    /// The SQL-visible contract of the registered function.
    #[test]
    fn registered_hamming_is_callable_from_sql() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        register(&c).unwrap();
        let d: i64 = c
            .query_row("SELECT hamming('/w==', 'AA==')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(d, 8);
        let n: Option<i64> = c
            .query_row("SELECT hamming(NULL, 'AA==')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            n, None,
            "NULL in -> NULL out; the column side is prefiltered"
        );
        assert!(
            c.query_row::<i64, _, _>("SELECT hamming('AAAA','AA==')", [], |r| r.get(0))
                .is_err(),
            "length mismatch must surface as a SQLite error, never as a value"
        );
        assert!(
            c.query_row::<i64, _, _>("SELECT hamming('not b64','AA==')", [], |r| r.get(0))
                .is_err(),
            "broken base64 must surface as a SQLite error"
        );
        assert!(
            c.query_row::<i64, _, _>("SELECT hamming(7, 'AA==')", [], |r| r.get(0))
                .is_err(),
            "a numeric argument is not a vector"
        );
    }

    /// Blobs work too, so the op keeps working the day a real blob write path
    /// exists (memory-spec A.2.5 speaks of BLOB columns; today's producer writes
    /// base64 text — see plan § 3).
    #[test]
    fn registered_hamming_accepts_blobs_and_mixes_them_with_base64() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        register(&c).unwrap();
        let d: i64 = c
            .query_row("SELECT hamming(x'FF', 'AA==')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(d, 8);
    }

    /// If a future connection path forgets `register`, this is the shape of the
    /// failure — a plain `sql_error`, named, not a wrong number.
    #[test]
    fn unregistered_hamming_is_a_plain_sql_error() {
        let c = rusqlite::Connection::open_in_memory().unwrap();
        let e = c
            .query_row::<i64, _, _>("SELECT hamming('AA==','AA==')", [], |r| r.get(0))
            .unwrap_err();
        assert!(format!("{e}").contains("no such function"), "got {e}");
    }

    #[test]
    fn hamming_counts_differing_bits() {
        assert_eq!(hamming_bytes(&[0x00], &[0x00]).unwrap(), 0);
        assert_eq!(hamming_bytes(&[0xFF], &[0x00]).unwrap(), 8);
        assert_eq!(
            hamming_bytes(&[0b1010_1010, 0x00], &[0b1010_0000, 0x01]).unwrap(),
            3
        );
        assert_eq!(hamming_bytes(&[], &[]).unwrap(), 0);
    }

    // ---- decoder test net (plan R7 requirement) -------------------------
    // The decoder reads caller text (`vector`) and row content, so it is part
    // of the injection surface, not a helper. Four failure classes, each pinned
    // on its own.

    #[test]
    fn base64_decodes_valid_input_of_every_residue_class() {
        assert_eq!(decode_base64("").unwrap(), Vec::<u8>::new());
        assert_eq!(decode_base64("AAAA").unwrap(), vec![0, 0, 0]);
        assert_eq!(decode_base64("/w==").unwrap(), vec![0xFF]);
        assert_eq!(decode_base64("//8=").unwrap(), vec![0xFF, 0xFF]);
        assert_eq!(decode_base64("AAAA/w==").unwrap(), vec![0, 0, 0, 0xFF]);
        // full alphabet incl. the last two symbols and every sextet value
        assert_eq!(
            decode_base64("Ab+/").unwrap(),
            vec![0b0000_0001, 0b1011_1111, 0b1011_1111]
        );
    }

    /// The URL-safe alphabet is a different encoding, not a tolerated spelling.
    #[test]
    fn base64_rejects_foreign_alphabet_characters() {
        for bad in ["_w==", "-w==", "AA*A", "AA$A", "ÄAA", "AAA\u{0}", "AA A"] {
            assert!(decode_base64(bad).is_err(), "must reject {bad:?}");
        }
    }

    /// Padding is structural: at most two, only at the very end.
    #[test]
    fn base64_rejects_broken_padding() {
        for bad in [
            "AA=A", "A=AA", "=AAA", "AAA==", "AA===", "=", "==", "AAAA=", "====",
        ] {
            assert!(decode_base64(bad).is_err(), "must reject {bad:?}");
        }
    }

    /// Length is a multiple of four; whitespace is never skipped.
    #[test]
    fn base64_rejects_bad_length_and_any_whitespace() {
        for bad in [
            "A",
            "AA",
            "AAA",
            "AAAAA",
            "AAAA A===",
            " AAAA",
            "AAAA ",
            "AA AA",
            "AAAA\n",
        ] {
            assert!(decode_base64(bad).is_err(), "must reject {bad:?}");
        }
    }

    /// The producer's shape (memory-hive `embed` cell, BIN_VERSION v1):
    /// 1024 float dims -> 128 packed bytes -> 172 base64 characters.
    #[test]
    fn decodes_the_producers_vector_width() {
        let b64 = "A".repeat(170) + "==";
        assert_eq!(b64.len(), 172);
        assert_eq!(decode_base64(&b64).unwrap().len(), 127);
    }

    /// A mismatch is almost always a breach of the embedding-generation
    /// discipline (memory-spec B.1.1). It must be loud: a NULL result would sort
    /// FIRST under `ORDER BY distance ASC` and silently poison the ranking.
    #[test]
    fn hamming_rejects_length_mismatch_loudly() {
        let e = hamming_bytes(&[0x00], &[0x00, 0x00]).unwrap_err();
        assert!(e.contains("length mismatch"), "got {e}");
        assert!(
            e.contains('1') && e.contains('2'),
            "must name both lengths: {e}"
        );
    }
}
