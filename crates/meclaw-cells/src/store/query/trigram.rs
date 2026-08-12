//! Trigram similarity for the entity candidate feed (0.2.0 P4, ruling Q5).
//!
//! The score exists to RANK suspicion, never to decide identity: ruling Q5 puts
//! every fuzzy judgement at the nightly GC with a top-tier model and full context
//! of both nodes. A number here that is a little too high or too low costs the GC
//! one look, while an automatic merge on a threshold costs a fact — thresholds
//! lie on short names, sibling names and one-letter place-name differences.
//!
//! Hand-built on purpose: `rusqlite` runs bundled, no extensions (no `spellfix`,
//! no `pg_trgm`), and the tech stack is a closed allow-list. A set-based Dice
//! coefficient over padded character trigrams is a dozen lines and needs nothing.

use std::collections::BTreeSet;

/// The padded character trigrams of one value.
///
/// Two leading and one trailing pad character make the first and last letters
/// count as strongly as the middle ones — without padding, `helix` and `felix`
/// would look more alike than they read.
fn trigrams(value: &str) -> BTreeSet<(char, char, char)> {
    let chars: Vec<char> = std::iter::repeat_n(' ', 2)
        .chain(value.chars())
        .chain(std::iter::once(' '))
        .collect();
    chars.windows(3).map(|w| (w[0], w[1], w[2])).collect()
}

/// Dice coefficient over the trigram SETS of two values: `0.0` (nothing in
/// common) to `1.0` (identical trigram sets).
///
/// Sets rather than multisets: a repeated trigram says little about a name and
/// would let `aaaa` and `aaaaaaaa` score as near-identical.
pub fn similarity(left: &str, right: &str) -> f64 {
    let (a, b) = (trigrams(left), trigrams(right));
    let total = a.len() + b.len();
    if total == 0 {
        return 0.0;
    }
    let shared = a.intersection(&b).count();
    2.0 * shared as f64 / total as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_values_score_one_and_disjoint_ones_score_low() {
        assert!((similarity("elvese", "elvese") - 1.0).abs() < 1e-9);
        assert!(similarity("elvese", "zzzzzz") < 0.2);
    }

    /// The case the candidate feed exists for: a one-character typo stays close,
    /// while two different short names do not — the ORDER is what the GC reads.
    #[test]
    fn a_typo_ranks_above_two_different_names() {
        let typo = similarity("elvese", "elvse");
        let different = similarity("leroy", "leon");
        assert!(typo > 0.5, "typo pair scored {typo}");
        assert!(different < typo, "different={different} typo={typo}");
    }

    /// Symmetric and total: the feed sorts on this number, so an asymmetry would
    /// make the output depend on which side of the pair came first.
    #[test]
    fn the_score_is_symmetric() {
        assert_eq!(similarity("helix", "felix"), similarity("felix", "helix"));
        assert_eq!(similarity("", ""), 1.0);
    }
}
