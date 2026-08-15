//! Conservative light stemmer for the store's FTS5 index (0.2.0 P3, ruling Q6).
//!
//! FTS5 can only prefix-match at the END of a word, so a PLURAL query token
//! never reaches the SINGULAR index term: `"lieblingseditoren"*` scores zero
//! against an index that holds `lieblingseditor`. Fixing that at the root means
//! folding both sides onto one term, which is what this module does — the FTS5
//! tokenizer that wraps it runs on index text and on query text alike.
//!
//! Deliberately NOT a full Snowball stemmer (ruling Q6 asks for a conservative
//! one): only inflectional suffixes are stripped, never derivational ones, and
//! never more than one suffix per family. German compounds stay whole, which is
//! the over-stemming the ruling names.

/// Shortest stem any rule may leave behind, in CHARACTERS.
///
/// Below that the fold stops being morphology and starts being noise: `tee`,
/// `see`, `eis`, `gas` would all lose their last letter and collide with
/// unrelated words. Every rule below is phrased so the stem keeps at least this
/// many characters.
const MIN_STEM_CHARS: usize = 3;

/// Characters an inflectional `-s` may legitimately sit on.
///
/// The guard is what separates a plural (`cats`, `editors`) from a word that
/// simply ends in `s` (`haus`, `atlas`, `bonus`, `gas`): after a vowel or a
/// sibilant the `s` belongs to the stem, not to the inflection. Taken from the
/// well-worn German light-stemmer set, plus `r` and `w` — English agent nouns
/// (`editors`, `servers`) are the single most common plural shape this index
/// sees, and neither language has a `-rs`/`-ws` word the two additions break.
const S_ENDINGS: [char; 12] = ['b', 'd', 'f', 'g', 'h', 'k', 'l', 'm', 'n', 'r', 't', 'w'];

/// Fold one already-tokenized word onto its stem.
///
/// The input is a token as the base tokenizer produced it: case-folded and, for
/// Latin scripts, diacritic-free (`unicode61` strips the diacritic before this
/// function ever sees the text — an a-umlaut arrives as a plain `a`, which is
/// why there is no umlaut handling and no ae-expansion here). The
/// result is always a PREFIX of the input — the stemmer only ever truncates, it
/// never rewrites — so it borrows instead of allocating.
///
/// Two ordered steps, each firing at most once:
///
/// | step | suffix | condition | strips | why |
/// |---|---|---|---|---|
/// | 1 | `s` | > 3 chars, preceding char in [`S_ENDINGS`] | 1 | EN plural / DE genitive: `cats`→`cat`, `editors`→`editor` |
/// | 2 | `ern` | > 5 chars | 3 | DE dative plural: `kindern`→`kind` |
/// | 2 | `em` `en` `er` `es` | > 4 chars | 2 | DE plural + inflection, EN `-es`: `editoren`→`editor`, `kinder`→`kind`, `boxes`→`box` |
/// | 2 | `e` | > 3 chars | 1 | DE plural + EN silent `e`: `tage`→`tag`, `katze`→`katz`, `house`→`hous` |
///
/// **Why two steps and not a fixpoint loop.** One suffix per family is what the
/// languages actually build: `computers` is `computer` + `s`, and `computer` is
/// a lexical stem whose `-er` German morphology would happily eat a second time.
/// Running the two steps in this order makes `computers` and `computer` meet at
/// `comput` while a loop would keep chewing (`elvese` → `elves` → `elv`) and
/// grind proper names down to three letters. A single step would be even more
/// conservative but would break the commonest English pair there is
/// (`servers`/`server`), because only one of the two carries the `-s`.
pub fn stem(token: &str) -> &str {
    strip_inflection(strip_plural_s(token))
}

/// Step 1 — the `-s` of an English plural or a German genitive.
fn strip_plural_s(token: &str) -> &str {
    if char_len(token) > MIN_STEM_CHARS
        && let Some(head) = token.strip_suffix('s')
        && head
            .chars()
            .next_back()
            .is_some_and(|c| S_ENDINGS.contains(&c))
    {
        return head;
    }
    token
}

/// Step 2 — the German inflection endings and the English `-es`/`-e`.
///
/// Longest suffix first, so `kindern` is read as one dative plural rather than
/// as `kinder` plus a stray `n`.
fn strip_inflection(token: &str) -> &str {
    let n = char_len(token);
    if n > MIN_STEM_CHARS + 2
        && let Some(head) = token.strip_suffix("ern")
    {
        return head;
    }
    if n > MIN_STEM_CHARS + 1 {
        for suffix in ["em", "en", "er", "es"] {
            if let Some(head) = token.strip_suffix(suffix) {
                return head;
            }
        }
    }
    if n > MIN_STEM_CHARS
        && let Some(head) = token.strip_suffix('e')
    {
        return head;
    }
    token
}

/// Length in characters — the suffixes are ASCII, but the token need not be.
fn char_len(token: &str) -> usize {
    token.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The issue-#14 case itself: the plural a question is asked in and the
    /// singular a fact was minted in have to meet on one term. Everything else
    /// in this package only carries this pair into the index.
    #[test]
    fn the_plural_of_the_issue_meets_its_singular() {
        assert_eq!(stem("lieblingseditoren"), "lieblingseditor");
        assert_eq!(stem("lieblingseditor"), "lieblingseditor");
    }

    /// Pairs, not absolute stems: what the index needs is that two forms of one
    /// word arrive at the SAME term. The term itself is an implementation
    /// detail and never leaves the index.
    fn same_stem(a: &str, b: &str) {
        assert_eq!(
            stem(a),
            stem(b),
            "{a:?} and {b:?} must fold onto one index term"
        );
    }

    #[test]
    fn german_plurals_meet_their_singular() {
        same_stem("editoren", "editor"); // -en on a foreign-origin noun
        same_stem("katzen", "katze"); // -en against the -e singular
        same_stem("tage", "tag"); // -e plural on a bare stem
        same_stem("kinder", "kind"); // -er plural
        same_stem("kindern", "kind"); // -ern dative plural
        same_stem("hunde", "hund");
        same_stem("hunden", "hund");
        same_stem("jahres", "jahr"); // -es genitive
        same_stem("tages", "tag");
    }

    #[test]
    fn english_plurals_meet_their_singular() {
        same_stem("cats", "cat");
        same_stem("editors", "editor");
        same_stem("boxes", "box"); // -es after a sibilant
        same_stem("houses", "house"); // -es against a silent-e singular
        same_stem("servers", "server"); // the -s + -er shape the two steps exist for
        same_stem("computers", "computer");
    }

    /// The counter-direction of the whole package: a short word must survive
    /// whole. Every rule leaves at least [`MIN_STEM_CHARS`], so the tokens a
    /// three-letter query is made of are never touched.
    #[test]
    fn short_words_are_left_alone() {
        for word in [
            "tag", "see", "tee", "eis", "gas", "das", "die", "ist", "war",
        ] {
            assert_eq!(stem(word), word, "{word:?} is too short to fold");
        }
    }

    /// The `-s` guard: after a vowel or a sibilant the `s` belongs to the word.
    /// Without this, `haus` would collide with `hau` and `atlas` with `atla`.
    #[test]
    fn a_word_that_merely_ends_in_s_keeps_it() {
        for word in ["haus", "maus", "atlas", "bonus", "kurios", "campus"] {
            assert_eq!(stem(word), word, "{word:?} does not carry a plural -s");
        }
    }

    /// Entity-fidelity neighbourhood (ruling Q2): a proper name that carries no
    /// inflection suffix is never folded. `Sonnenhof Gartenbau` is the C2
    /// scenario's invented company, `Elvese` its invented village — the class of
    /// name the epic promises to keep byte-faithful.
    #[test]
    fn a_proper_name_without_an_inflection_suffix_is_untouched() {
        for name in [
            "sonnenhof",
            "gartenbau",
            "berlin",
            "helix",
            "vscodium",
            "meclaw",
        ] {
            assert_eq!(stem(name), name, "{name:?} must survive whole");
        }
    }

    /// The honest half of the same rule: a name that LOOKS inflected is folded,
    /// because no stemmer can tell a village from a plural. What saves it is
    /// that the fold is symmetric — the query goes through the very same
    /// function, so the name stays findable — and that the stemmer only ever
    /// touches the INDEX; the stored row keeps its bytes (pinned at store level
    /// in `crates/meclaw-cells/tests/p3_fts_stemming.rs`).
    #[test]
    fn a_name_that_looks_inflected_folds_symmetrically_and_stays_findable() {
        assert_eq!(stem("elvese"), "elves", "one step, never a loop");
        same_stem("elvese", "elvese");
        // and the fold is a truncation, so the name is still a prefix match of
        // itself — which is the shape the recall lane queries in.
        assert!("elvese".starts_with(stem("elvese")));
    }

    /// The result is always a prefix of the input: the stemmer truncates, it
    /// never rewrites. That is what lets the tokenizer hand FTS5 the original
    /// byte offsets and what keeps the fold explainable in a receipt.
    #[test]
    fn every_stem_is_a_prefix_of_its_token() {
        for word in [
            "lieblingseditoren",
            "kindern",
            "computers",
            "houses",
            "tage",
            "gas",
            "",
        ] {
            assert!(word.starts_with(stem(word)), "{word:?} was rewritten");
            assert!(stem(word).len() <= word.len());
        }
    }

    /// One application is the whole fold, and that is a decision, not an
    /// accident: index side and query side each run [`stem`] exactly once, so
    /// what correctness needs is that two forms of one word MEET — not that the
    /// meeting point survives a further pass.
    ///
    /// The words the index is actually made of do reach a fixed point after one
    /// application. The exception is the reason the loop is absent: a second
    /// pass reads the `-es` of `elves` as an inflection and grinds an invented
    /// village down to three letters. Measured here so that a later "let's just
    /// loop until nothing changes" has to argue with a number.
    #[test]
    fn one_pass_is_the_whole_fold() {
        for word in [
            "lieblingseditoren",
            "kindern",
            "computers",
            "houses",
            "editoren",
            "katzen",
        ] {
            let once = stem(word);
            assert_eq!(stem(once), once, "{word:?} moves on a second pass");
        }
        assert_eq!(stem("elvese"), "elves");
        assert_eq!(stem("elves"), "elv", "what a fixpoint loop would cost");
    }

    /// Non-Latin tokens reach the stemmer as whole UTF-8 words. The rules are
    /// ASCII, so they must not fire — and above all must not slice a multi-byte
    /// character in half.
    #[test]
    fn a_non_latin_token_is_never_sliced() {
        for word in ["\u{43a}\u{43d}\u{438}\u{433}\u{438}", "\u{6771}\u{4eac}"] {
            assert_eq!(stem(word), word);
        }
    }
}
