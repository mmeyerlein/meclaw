//! GH #612 — the hive boundary, on the DELIVERY side.
//!
//! `docs/meclaw-overview.md` § The hive boundary, requirement 1: the address is
//! the hive; `<hive>/<cell>` is not an address, "including where the substrate
//! still resolves it today for want of a declaration". GH #133 turned that
//! sentence into an enforced contract for one surface, `add_edges`. This module
//! is the other half: what an address means for a message that arrives from
//! OUTSIDE the colony.
//!
//! The rule is the same rule, read from the delivery end. A hive that declared
//! `params.ports` has named the addresses it answers on:
//!
//! - a declared port — the short name of a direct child;
//! - a connect point of a `params.contract` `accepts` lane, and only on that
//!   lane. `config.md` § `params.contract` says it in the declaration's own
//!   words: "This is the one exception a sealed hive pronounces itself.
//!   `ports: []` stays literally true … an `at` entry says: for THIS lane, and
//!   no other, an edge may end here."
//!
//! Everything else inside a sealed hive is refused with `hive_boundary`. A hive
//! that declared nothing is untouched, which is GH #133's opt-in shape one
//! surface over.
//!
//! Hives are not actors here either: this is pure data plus one pure predicate.
//! The table is rebuilt on topology change, next to the slot table and from the
//! same reader (`mutation::port_boundary::collect_sealed_hives`), so seal
//! membership cannot drift between mutation time and delivery time.

use meclaw_core::Path;

/// What one sealed hive declared, in the form the delivery filter reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiveBoundary {
    /// Absolute logical path of the hive (e.g. `/channels/phone`).
    pub path: String,
    /// Canonical short names of the declared ports (`params.ports`).
    pub ports: Vec<String>,
    /// One entry per `params.contract` `accepts[].at` entry: the lane's `route`
    /// paired with the ABSOLUTE address the entry names.
    pub connect_points: Vec<(String, String)>,
}

impl HiveBoundary {
    /// True iff `abs` lies STRICTLY below the hive path. The hive path itself is
    /// an address (hive transit) and is never interior to itself.
    #[must_use]
    fn is_interior(&self, abs: &str) -> bool {
        if self.path == "/" {
            return abs != "/" && abs.starts_with('/');
        }
        abs.starts_with(&format!("{}/", self.path))
    }

    /// True iff this hive declared `abs` as an address — as a port, or as a
    /// connect point of the lane `lane`.
    #[must_use]
    fn declares(&self, abs: &str, lane: Option<&str>) -> bool {
        let prefix = if self.path == "/" {
            "/".to_string()
        } else {
            format!("{}/", self.path)
        };
        if let Some(rest) = abs.strip_prefix(&prefix)
            && !rest.is_empty()
            && !rest.contains('/')
            && self.ports.iter().any(|p| p == rest)
        {
            return true;
        }
        match lane {
            Some(l) => self
                .connect_points
                .iter()
                .any(|(route, address)| route == l && address == abs),
            None => false,
        }
    }
}

/// Every hive that declared a boundary. Rebuilt on topology change only — never
/// per message.
pub type HiveBoundaryTable = Vec<HiveBoundary>;

/// The hive that refuses this address, or `None` when nothing does.
///
/// Every sealed hive that strictly contains the target is asked, and the
/// DEEPEST refusal is the answer (ruling 4 of the design): a connect point "is
/// owed by the hive the endpoint sits IN", so a permission an outer hive granted
/// cannot speak for an inner one, and the boundary a caller stands in front of
/// is the innermost one.
#[must_use]
pub fn boundary_refusal(
    boundaries: &HiveBoundaryTable,
    target: &Path,
    lane: Option<&str>,
) -> Option<String> {
    let abs = target.as_str();
    let mut refusing: Option<&HiveBoundary> = None;
    for b in boundaries {
        if !b.is_interior(abs) || b.declares(abs, lane) {
            continue;
        }
        if refusing.is_none_or(|r| b.path.len() > r.path.len()) {
            refusing = Some(b);
        }
    }
    refusing.map(|b| b.path.clone())
}

/// Read the declarations of every registered hive scope from the filesystem.
///
/// Synchronous filesystem work — one `config.json` per sealed hive — which is
/// why it runs on topology change only, exactly like
/// [`crate::rebuild_slot_table`]'s reader. The seal itself comes from
/// [`crate::mutation::port_boundary::collect_sealed_hives`], so the delivery
/// filter and the mutation gate always agree about WHICH hives declared a
/// boundary.
#[must_use]
pub fn rebuild_hive_boundaries<'a>(
    root: &std::path::Path,
    hive_paths: impl Iterator<Item = &'a Path>,
) -> HiveBoundaryTable {
    let sealed = crate::mutation::port_boundary::collect_sealed_hives(root, hive_paths);
    sealed
        .into_iter()
        .map(|h| {
            let connect_points = crate::mutation::port_boundary::lane_connect_points(root, &h.path);
            HiveBoundary {
                path: h.path,
                ports: h.ports,
                connect_points,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sealed() -> HiveBoundaryTable {
        vec![HiveBoundary {
            path: "/channels/phone".into(),
            ports: vec![],
            connect_points: vec![("tool".into(), "/channels/phone/dial".into())],
        }]
    }

    /// The hive path itself is an address and always was — hive transit runs
    /// through it.
    #[test]
    fn the_hive_path_is_not_interior() {
        assert_eq!(
            boundary_refusal(&sealed(), &Path::new("/channels/phone"), None),
            None
        );
    }

    /// An undeclared interior path is refused, and the refusal names the hive.
    #[test]
    fn an_undeclared_interior_path_is_refused() {
        assert_eq!(
            boundary_refusal(&sealed(), &Path::new("/channels/phone/voice"), None),
            Some("/channels/phone".to_string())
        );
    }

    /// A declared port is an address, on every lane and on none.
    #[test]
    fn a_declared_port_is_an_address() {
        let mut t = sealed();
        t[0].ports = vec!["voice".into()];
        assert_eq!(
            boundary_refusal(&t, &Path::new("/channels/phone/voice"), None),
            None
        );
    }

    /// A connect point answers on its own lane and on no other.
    #[test]
    fn a_connect_point_answers_on_its_own_lane_only() {
        assert_eq!(
            boundary_refusal(&sealed(), &Path::new("/channels/phone/dial"), Some("tool")),
            None
        );
        assert_eq!(
            boundary_refusal(
                &sealed(),
                &Path::new("/channels/phone/dial"),
                Some("in_speak")
            ),
            Some("/channels/phone".to_string())
        );
    }

    /// Nested seals are asked from the inside out and any refusal is the answer:
    /// the outer hive's port cannot speak for the inner hive.
    #[test]
    fn the_deepest_refusal_is_the_one_reported() {
        let t = vec![
            HiveBoundary {
                path: "/outer".into(),
                ports: vec!["inner".into()],
                connect_points: vec![],
            },
            HiveBoundary {
                path: "/outer/inner".into(),
                ports: vec![],
                connect_points: vec![],
            },
        ];
        assert_eq!(
            boundary_refusal(&t, &Path::new("/outer/inner/c"), None),
            Some("/outer/inner".to_string())
        );
    }

    /// A path outside every sealed hive is nobody's business here.
    #[test]
    fn an_unsealed_path_is_untouched() {
        assert_eq!(
            boundary_refusal(&sealed(), &Path::new("/channels/voice"), None),
            None
        );
    }

    /// A connect point may sit deeper than one segment (`"./inner/talky"`), and
    /// the port rule may not: a port is the short name of a DIRECT child.
    #[test]
    fn a_connect_point_may_be_deep_and_a_port_may_not() {
        let t = vec![HiveBoundary {
            path: "/h".into(),
            ports: vec!["p".into()],
            connect_points: vec![("in_turn".into(), "/h/inner/talky".into())],
        }];
        assert_eq!(
            boundary_refusal(&t, &Path::new("/h/inner/talky"), Some("in_turn")),
            None
        );
        assert_eq!(
            boundary_refusal(&t, &Path::new("/h/inner/p"), None),
            Some("/h".to_string())
        );
    }
}
