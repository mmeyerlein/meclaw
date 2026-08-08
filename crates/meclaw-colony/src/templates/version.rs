//! Own parser for `<major>.<minor>.<patch>`. Ranges (^, ~) are post-roadmap.
//! Spec: docs/meclaw-overview.md § Template system, R3 (no semver crate).

/// A parsed semantic version triple `(major, minor, patch)`.
pub type SimpleVersion = (u64, u64, u64);

/// Error returned when a version string cannot be parsed as `major.minor.patch`.
#[derive(Debug, thiserror::Error)]
pub enum VersionError {
    #[error("invalid version {0:?}: {1}")]
    Invalid(String, &'static str),
}

/// Parse a strict `major.minor.patch` version string.
///
/// Rejects anything that is not exactly three dot-separated non-empty
/// ASCII-digit segments: two-part versions, pre-release suffixes (`-rc1`),
/// build metadata (`+build`), non-numeric segments, and empty segments all
/// return [`VersionError::Invalid`].
pub fn parse_simple_version(s: &str) -> Result<SimpleVersion, VersionError> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 3 {
        return Err(VersionError::Invalid(
            s.into(),
            "need exactly major.minor.patch",
        ));
    }
    let parse = |p: &str| -> Result<u64, VersionError> {
        if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()) {
            return Err(VersionError::Invalid(s.into(), "non-numeric segment"));
        }
        p.parse::<u64>()
            .map_err(|_| VersionError::Invalid(s.into(), "overflow"))
    };
    Ok((parse(parts[0])?, parse(parts[1])?, parse(parts[2])?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_triple() {
        assert_eq!(parse_simple_version("1.2.3").unwrap(), (1, 2, 3));
        assert_eq!(parse_simple_version("0.0.0").unwrap(), (0, 0, 0));
        assert_eq!(parse_simple_version("10.20.30").unwrap(), (10, 20, 30));
    }

    #[test]
    fn rejects_two_parts() {
        let err = parse_simple_version("1.2").unwrap_err();
        assert!(matches!(err, VersionError::Invalid(_, _)));
    }

    #[test]
    fn rejects_non_numeric() {
        assert!(parse_simple_version("1.2.x").is_err());
        assert!(parse_simple_version("a.b.c").is_err());
    }

    #[test]
    fn rejects_prerelease_or_build_metadata() {
        // SemVer-Ranges post-roadmap (overview Z.1157).
        assert!(parse_simple_version("1.2.3-rc1").is_err());
        assert!(parse_simple_version("1.2.3+build").is_err());
    }

    #[test]
    fn tuple_ord_orders_versions() {
        let a = parse_simple_version("1.0.0").unwrap();
        let b = parse_simple_version("1.0.1").unwrap();
        let c = parse_simple_version("2.0.0").unwrap();
        assert!(a < b);
        assert!(b < c);
    }
}
