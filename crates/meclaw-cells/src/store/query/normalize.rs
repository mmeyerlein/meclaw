//! Entity-name normalisation for canonical bindings (0.2.0 P4, ruling Q5).
//!
//! Ruling Q5 splits the identity work in two: *mechanics automatic, judgement at
//! the GC*. This module is the whole automatic half — the ONLY merge the store
//! performs on its own. Two spellings that are equal after normalisation are the
//! same entity by construction, which is provable rather than judged; everything
//! beyond that (edit distance, trigram overlap) only ever produces CANDIDATES for
//! the nightly GC (`alias_candidates`), never a merge.
//!
//! Three effects, in this order:
//! 1. **case fold** — `str::to_lowercase`, full Unicode;
//! 2. **composition** — a combining mark that follows a base letter is folded
//!    into the precomposed character (the NFD → NFC direction);
//! 3. **whitespace collapse** — trim, and every run of whitespace becomes one
//!    ASCII space.
//!
//! Composition covers the Latin-1 Supplement, deliberately: that table is a
//! constant of this file (there is no Unicode database in the tech stack, and
//! adding one is a dependency question, not an implementation detail). The
//! failure direction is what makes the limit acceptable — an unrecognised mark
//! leaves the two spellings DIFFERENT, so the miss costs a merge and can never
//! cause a wrong one. That is the same conservatism ruling Q5 asks for and the
//! same shape P3's light stemmer already ships.

/// The precomposed lowercase Latin-1 letters, as `(base, combining mark,
/// composed)`.
///
/// Lowercase only: [`normalize`] case-folds BEFORE it composes, so an uppercase
/// base can never reach this table. Marks are the seven that actually have
/// precomposed forms over ASCII letters in the Latin-1 Supplement — grave,
/// acute, circumflex, tilde, diaeresis, ring above, cedilla.
const COMPOSITIONS: &[(char, char, char)] = &[
    ('a', '\u{0300}', '\u{00E0}'),
    ('e', '\u{0300}', '\u{00E8}'),
    ('i', '\u{0300}', '\u{00EC}'),
    ('o', '\u{0300}', '\u{00F2}'),
    ('u', '\u{0300}', '\u{00F9}'),
    ('a', '\u{0301}', '\u{00E1}'),
    ('e', '\u{0301}', '\u{00E9}'),
    ('i', '\u{0301}', '\u{00ED}'),
    ('o', '\u{0301}', '\u{00F3}'),
    ('u', '\u{0301}', '\u{00FA}'),
    ('y', '\u{0301}', '\u{00FD}'),
    ('a', '\u{0302}', '\u{00E2}'),
    ('e', '\u{0302}', '\u{00EA}'),
    ('i', '\u{0302}', '\u{00EE}'),
    ('o', '\u{0302}', '\u{00F4}'),
    ('u', '\u{0302}', '\u{00FB}'),
    ('a', '\u{0303}', '\u{00E3}'),
    ('n', '\u{0303}', '\u{00F1}'),
    ('o', '\u{0303}', '\u{00F5}'),
    ('a', '\u{0308}', '\u{00E4}'),
    ('e', '\u{0308}', '\u{00EB}'),
    ('i', '\u{0308}', '\u{00EF}'),
    ('o', '\u{0308}', '\u{00F6}'),
    ('u', '\u{0308}', '\u{00FC}'),
    ('y', '\u{0308}', '\u{00FF}'),
    ('a', '\u{030A}', '\u{00E5}'),
    ('c', '\u{0327}', '\u{00E7}'),
];

/// The normal form of one entity spelling — the store's only automatic statement
/// about identity (ruling Q5).
///
/// `NULL`-like input (an empty string) normalises to an empty string; the callers
/// treat that as "no usable source value" and derive nothing, so an empty
/// identity is never invented.
pub fn normalize(value: &str) -> String {
    let folded = value.to_lowercase();
    let composed = compose(&folded);
    collapse_whitespace(&composed)
}

/// Fold combining marks into their precomposed letters (NFD → NFC, Latin-1).
fn compose(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        let composed = out.chars().next_back().and_then(|base| {
            COMPOSITIONS
                .iter()
                .find(|(b, m, _)| *b == base && *m == c)
                .map(|(_, _, composed)| *composed)
        });
        match composed {
            Some(composed) => {
                out.pop();
                out.push(composed);
            }
            None => out.push(c),
        }
    }
    out
}

/// Trim, and turn every run of whitespace into a single ASCII space.
fn collapse_whitespace(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for word in value.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Register `meclaw_norm(x)` on a `store` `cell.db` connection.
///
/// The SQL twin of [`normalize`], and it exists for one reason: the canonical
/// column is re-derived by a single `UPDATE` (`canonicalize`, and the backfill at
/// wake), so the normal form has to be computable inside that statement. Without
/// it the store would have to read every row into Rust and write it back, which
/// is the same truth by a slower and less atomic road.
///
/// `NULL` yields `NULL`, so a row without a source value keeps a `NULL` identity
/// instead of an invented empty one. Deterministic and innocuous: it is a pure
/// function of its argument and touches nothing.
pub fn register(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    use rusqlite::functions::FunctionFlags;
    conn.create_scalar_function(
        "meclaw_norm",
        1,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        |ctx| {
            use rusqlite::types::ValueRef;
            match ctx.get_raw(0) {
                ValueRef::Null => Ok(None),
                ValueRef::Text(t) => {
                    let s = std::str::from_utf8(t).map_err(|_| {
                        rusqlite::Error::UserFunctionError(
                            "meclaw_norm: argument is not valid UTF-8".into(),
                        )
                    })?;
                    Ok(Some(normalize(s)))
                }
                other => Err(rusqlite::Error::UserFunctionError(
                    format!("meclaw_norm: argument must be text or NULL, got {other:?}").into(),
                )),
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three effects ruling Q5 names, each on its own and all at once.
    #[test]
    fn normalisation_folds_case_whitespace_and_composition() {
        assert_eq!(normalize("Helix"), "helix");
        assert_eq!(normalize("  helix  "), "helix");
        assert_eq!(normalize("sonnen  hof"), "sonnen hof");
        assert_eq!(normalize("Sonnen\tHof\n"), "sonnen hof");
        // NFD (e + combining acute) becomes NFC (precomposed e-acute)
        assert_eq!(normalize("elve\u{0301}se"), "elv\u{00E9}se");
        assert_eq!(normalize("ELVE\u{0301}SE"), normalize("elv\u{00E9}se"));
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("   "), "");
    }

    /// The protection direction: normalisation is not transliteration. Two names
    /// that merely LOOK alike stay different, which is what keeps the automatic
    /// half of ruling Q5 provable instead of judged.
    #[test]
    fn normalisation_never_merges_two_different_names() {
        assert_ne!(normalize("Alvaro"), normalize("Alvin"));
        assert_ne!(normalize("elvese"), normalize("elvse"));
        // a diacritic is part of the name, never stripped
        assert_ne!(normalize("elv\u{00E9}se"), normalize("elvese"));
        assert_ne!(normalize("user"), normalize("user:alpha"));
    }

    /// A mark outside the table leaves the string alone rather than guessing —
    /// the miss costs a merge, never causes one.
    #[test]
    fn an_uncovered_mark_is_left_untouched() {
        // combining macron over `e` has no Latin-1 precomposed form
        assert_eq!(normalize("e\u{0304}x"), "e\u{0304}x");
        assert_eq!(normalize("\u{0301}x"), "\u{0301}x");
    }

    /// Idempotent: normalising a normal form changes nothing. The alias table is
    /// keyed on the normal form, so a second pass must not move the key.
    #[test]
    fn normalisation_is_idempotent() {
        for s in ["Helix", "  Sonnen  Hof ", "elve\u{0301}se", "USER:U1"] {
            let once = normalize(s);
            assert_eq!(normalize(&once), once, "{s:?} is not stable");
        }
    }

    #[test]
    fn the_sql_function_matches_the_rust_one_and_passes_null_through() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        register(&conn).unwrap();
        let got: Option<String> = conn
            .query_row("SELECT meclaw_norm('  Sonnen  HOF ')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(got.as_deref(), Some(normalize("  Sonnen  HOF ").as_str()));
        let null: Option<String> = conn
            .query_row("SELECT meclaw_norm(NULL)", [], |r| r.get(0))
            .unwrap();
        assert_eq!(null, None);
    }
}
