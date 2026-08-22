//! The structured rejection (GH #293).
//!
//! **Safety statement: what is accepted and what is refused does not change.**
//! Same verdicts, complete report (spec `plans/composition-reference/spec.md`
//! § Part 3 — the collecting validator).
//!
//! Today each mutation check returns `Result<(), MutationError>` and the first
//! violation ends validation, so a mutation with eight problems costs eight
//! rounds. This module carries the *report* side of the fix: a
//! [`MutationRejection`] collects every [`Violation`] a stage produced, and
//! [`MutationRejection::render`] folds them back into the single-string form
//! existing callers already read. Nothing here decides a verdict — the checks
//! keep deciding, they just get somewhere to put more than one answer.
//!
//! The pipeline runs the [`Stage`]s in spec order and stops at the first stage
//! that produced any entry. Stopping between stages is deliberate: an
//! unresolved template makes every later endpoint error a consequence rather
//! than a cause, and twenty derived errors hide the one real one.
//!
//! There is deliberately **no** `impl From<MutationError> for Violation`. A
//! stage decides its own stage tag; a blanket conversion would let a check
//! land in the wrong stage silently. The conversion is
//! [`Violation::from_error`], which takes the [`Stage`] as an argument the call
//! site must name.

use crate::mutation::MutationError;

/// The validation stages, in the spec order the pipeline runs them
/// (spec § Part 3, list `1.`–`7.`).
///
/// `PartialOrd`/`Ord` derive from the declaration order, so `DiffSchema <
/// TemplateResolution < … < RequiredDrains` — the comparison IS the pipeline
/// order, and a stage inserted in the wrong place changes it visibly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    /// 1. The diff parses and has the shape the mutation format demands.
    DiffSchema,
    /// 2. Every named template reference resolves, including `ref` chains and
    ///    the rings they can close.
    TemplateResolution,
    /// 3. The `requires` declarations of the reached templates — ctx and env
    ///    keys the mutation must supply.
    Requires,
    /// 4. The addresses the post-state would carry: naming collisions, `match`
    ///    with no hit, unknown cell types, `override_params` keys addressing
    ///    nothing (GH #198).
    PostStateAddresses,
    /// 5. Both endpoints of every edge exist in the post-state, and the edge
    ///    set closes no cycle.
    EdgeEndpoints,
    /// 6. Contract locality (GH #185): hive port boundaries, inbound lanes and
    ///    header contract locality — may this edge be drawn, and what may it
    ///    carry.
    ContractLocality,
    /// 7. `required_drains`: a wired port whose paired drain leads nowhere.
    RequiredDrains,
}

impl Stage {
    /// The stable snake_case token that names this stage in a rendered
    /// rejection line and in a structured reader.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DiffSchema => "diff_schema",
            Self::TemplateResolution => "template_resolution",
            Self::Requires => "requires",
            Self::PostStateAddresses => "post_state_addresses",
            Self::EdgeEndpoints => "edge_endpoints",
            Self::ContractLocality => "contract_locality",
            Self::RequiredDrains => "required_drains",
        }
    }
}

/// One refused thing: which stage found it, which spec `error_code` it carries,
/// which address it concerns, what it says, and — where a contract carries one
/// — the contract's own `because`.
///
/// `code` is the same spec-stable token [`MutationError::error_code`] returns,
/// so a single-violation rejection reports exactly what the `Result` form
/// reported before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The stage that produced this violation.
    pub stage: Stage,
    /// The spec-stable `error_code` token (see `docs/meclaw-overview.md`
    /// § Mutation-Format → Validierung).
    pub code: &'static str,
    /// The path or name the violation concerns, when the check has one in hand.
    pub address: Option<String>,
    /// The human-readable message — the payload the `MutationError` carries.
    pub message: String,
    /// The contract's own reason string, travelling verbatim into the refusal
    /// (`required_drains[].because`, `LaneSpec.because`). Present in the
    /// message too; carried separately so a structured reader need not parse
    /// prose.
    pub because: Option<String>,
}

impl Violation {
    /// Tag an existing [`MutationError`] with the stage that produced it.
    ///
    /// Deliberately not a `From` impl: the stage is an argument the call site
    /// must name, because a check that lands in the wrong stage would reorder
    /// the pipeline's stop-at-first-stage rule without anyone noticing. The
    /// message is the error's own payload string, so the rendered line says
    /// what the single-violation path says today.
    ///
    /// **Never call this with [`MutationError::LiveTreeMutated`].** That
    /// variant is not a refusal at all: it says an atomic rename sequence
    /// failed AFTER its first committed `rename(2)`, so the live tree already
    /// moved. Its call site strict-fails (panics) before any reply, which is
    /// the whole point — turning it into a collected violation would hand the
    /// caller a polite refusal for a tree that is no longer the one it asked
    /// about. Its `error_code()` is a defensive never-reached fallback, and it
    /// must stay never-reached.
    pub fn from_error(stage: Stage, error: &MutationError, address: Option<String>) -> Self {
        Self {
            stage,
            code: error.error_code(),
            address,
            message: error.message(),
            because: None,
        }
    }

    /// The same as [`Violation::from_error`], plus the contract's own reason
    /// string (`required_drains[].because`, `LaneSpec.because`).
    pub fn from_error_because(
        stage: Stage,
        error: &MutationError,
        address: Option<String>,
        because: String,
    ) -> Self {
        Self {
            because: Some(because),
            ..Self::from_error(stage, error, address)
        }
    }

    /// Render this violation as one line:
    /// `"<stage>/<code> <address>: <message> — <because>"`. The address and
    /// `because` parts are omitted when absent.
    pub fn render(&self) -> String {
        let mut line = format!("{}/{}", self.stage.as_str(), self.code);
        if let Some(address) = &self.address {
            line.push_str(&format!(" {address}:"));
        }
        line.push(' ');
        line.push_str(&self.message);
        if let Some(because) = &self.because {
            line.push_str(&format!(" — {because}"));
        }
        line
    }
}

/// Every violation one stage produced, in push order.
///
/// Empty means the stage passed. The pipeline runs stage by stage and stops at
/// the first stage whose rejection is non-empty; the caller then gets ONE
/// refusal carrying [`MutationRejection::error_code`] and
/// [`MutationRejection::render`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MutationRejection {
    entries: Vec<Violation>,
}

impl MutationRejection {
    /// A rejection with nothing in it — the state a passing stage leaves.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one violation. Order is preserved: the first pushed violation is
    /// the one whose code the refusal carries, which is exactly the error the
    /// `Result` form returned before.
    pub fn push(&mut self, violation: Violation) {
        self.entries.push(violation);
    }

    /// Whether no violation was collected — the stage passed.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The collected violations, in push order.
    pub fn entries(&self) -> &[Violation] {
        &self.entries
    }

    /// All violations, one per line, in push order — the text form that keeps
    /// callers reading a single message working. Empty when nothing was
    /// collected.
    pub fn render(&self) -> String {
        self.entries
            .iter()
            .map(Violation::render)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The spec `error_code` this refusal reports: the **first** entry's code.
    ///
    /// The first entry is the violation the single-violation pipeline would
    /// have returned, so a caller keying on `error_code` sees no change.
    ///
    /// `None` on an empty rejection, deliberately — an empty rejection is not a
    /// refusal and has no code of its own. A fallback token here would let a
    /// forgotten [`MutationRejection::is_empty`] check ship a refusal carrying
    /// a plausible-looking code and empty details: exactly the reasonless
    /// refusal this module exists to abolish. With `Option`, that omission is a
    /// compile error at the call site instead. Panicking is not the
    /// alternative — the colony hot path must not panic (CLAUDE.md § Aktive
    /// Invarianten), so the type carries the case.
    pub fn error_code(&self) -> Option<&'static str> {
        self.entries.first().map(|entry| entry.code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutation::MutationError;

    #[test]
    fn a_single_violation_renders_exactly_what_the_error_says_today() {
        let err = MutationError::EdgeSchema("from='/main/a' unknown".to_string());
        let mut rejection = MutationRejection::new();
        rejection.push(Violation::from_error(Stage::EdgeEndpoints, &err, None));

        let rendered = rejection.render();
        assert_eq!(rendered.lines().count(), 1, "rendered: {rendered}");
        assert!(
            rendered.contains("from='/main/a' unknown"),
            "rendered: {rendered}"
        );
        assert_eq!(rejection.error_code(), Some("edge_schema"));
    }

    #[test]
    fn five_violations_render_five_lines_in_push_order() {
        let mut rejection = MutationRejection::new();
        for name in ["a", "b", "c", "d", "e"] {
            rejection.push(Violation::from_error(
                Stage::PostStateAddresses,
                &MutationError::NamingCollision(format!("name '{name}' already taken")),
                Some(format!("/main/{name}")),
            ));
        }

        let rendered = rejection.render();
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 5, "rendered: {rendered}");
        for (line, name) in lines.iter().zip(["a", "b", "c", "d", "e"]) {
            assert!(
                line.contains(&format!("/main/{name}")),
                "line {line} should name /main/{name}"
            );
        }
        assert_eq!(rejection.entries().len(), 5);
    }

    #[test]
    fn the_error_code_is_the_first_entrys_code_not_the_last() {
        let mut rejection = MutationRejection::new();
        rejection.push(Violation::from_error(
            Stage::PostStateAddresses,
            &MutationError::NamingCollision("first".to_string()),
            None,
        ));
        rejection.push(Violation::from_error(
            Stage::PostStateAddresses,
            &MutationError::UnknownCellType("last".to_string()),
            None,
        ));

        assert_eq!(rejection.error_code(), Some("naming_collision"));
    }

    #[test]
    fn an_empty_rejection_has_no_error_code_at_all() {
        // No fallback token: a forgotten `is_empty()` check must not be able to
        // ship a refusal with a plausible code and nothing behind it.
        assert_eq!(MutationRejection::new().error_code(), None);
    }

    #[test]
    fn stage_order_is_the_spec_order() {
        let spec_order = [
            Stage::DiffSchema,
            Stage::TemplateResolution,
            Stage::Requires,
            Stage::PostStateAddresses,
            Stage::EdgeEndpoints,
            Stage::ContractLocality,
            Stage::RequiredDrains,
        ];
        for pair in spec_order.windows(2) {
            assert!(
                pair[0] < pair[1],
                "{:?} must precede {:?}",
                pair[0],
                pair[1]
            );
        }
        assert_eq!(spec_order[0].as_str(), "diff_schema");
        assert_eq!(spec_order[6].as_str(), "required_drains");
    }

    #[test]
    fn a_full_violation_renders_stage_code_address_message_because_in_that_order() {
        let violation = Violation::from_error_because(
            Stage::RequiredDrains,
            &MutationError::RequiredDrainMissing("reject egress is not consumed".to_string()),
            Some("/main/hive".to_string()),
            "the caller never learns the work was not done".to_string(),
        );

        assert_eq!(
            violation.render(),
            "required_drains/required_drain_missing /main/hive: reject egress is not consumed \
             — the caller never learns the work was not done"
        );
    }

    #[test]
    fn an_empty_rejection_is_empty_and_renders_nothing() {
        let rejection = MutationRejection::new();
        assert!(rejection.is_empty());
        assert!(rejection.render().is_empty());
    }
}
