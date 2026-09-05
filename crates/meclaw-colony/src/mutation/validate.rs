//! Mutation validation (phase 6 + phase 11). Single-stage colony validation
//! against the post_state graph; all checks pure, without FS or DB.

use crate::CellFactoryRegistry;
use crate::mutation::MutationError;
use meclaw_core::JsonValue;

/// Every key a mutation `diff` may carry, in the order the door executes them.
///
/// This list IS the diff vocabulary. It is not a convenience copy of one: the
/// door reads the diff key by key (`diff.get("add_nodes")`, …), so a key that
/// appears here and nowhere else would be accepted and never executed, and a
/// key executed somewhere without appearing here would be refused. Adding an
/// operation means adding its key here in the same change.
pub const DIFF_OPERATIONS: [&str; 8] = [
    "add_templates",
    "add_nodes",
    "remove_nodes",
    "swap_nodes",
    "move_nodes",
    "add_edges",
    "remove_edges",
    "seed_rows",
];

/// A `diff` key no operation reads is refused, not ignored.
///
/// The door used to read the keys it knew and let everything else fall through
/// every arm untouched — and then answer `committed`. The shape that made it
/// indefensible: a colony on an OLDER binary is handed an `add_templates`
/// declaration, has no arm for the key, registers nothing, and replies
/// "applied". The same hole swallows a typo (`add_node`), a key from a newer
/// schema, and a hand-written declaration whose author guessed the vocabulary.
/// In every case the receipt claims work that did not happen.
///
/// Reporting success without effect is the defect, so an unreadable key is a
/// refusal under the token a broken body form has always carried — `schema`,
/// never a new `error_code` (README § Stability). The message names the key it
/// could not read AND the ones it can, because an operator who mistyped one
/// word needs the vocabulary, not a verdict.
///
/// Pre-destructive by position: this runs on the RAW diff, before substitution
/// and before a single byte is staged, spawned, wired or registered. A manifest
/// inherits it entry by entry — the entries before the offending one stay
/// committed, the ones after it are never read — and `--apply` inherits it as
/// the single form.
///
/// A `diff` that is not an object is NOT this function's verdict: that is
/// [`validate_post_state`]'s long-standing "diff is not an object", and moving
/// it here would change which refusal an old caller sees.
pub fn refuse_unknown_diff_keys(diff: &JsonValue) -> Result<(), MutationError> {
    let Some(obj) = diff.as_object() else {
        return Ok(());
    };
    let unknown: Vec<&str> = obj
        .keys()
        .map(String::as_str)
        .filter(|k| !DIFF_OPERATIONS.contains(k))
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    Err(MutationError::Schema(format!(
        "diff carries {} no operation reads: {}. \
         The diff keys this colony executes are: {}. \
         Refused rather than ignored — a key nothing reads would have committed \
         without effect.",
        if unknown.len() == 1 { "a key" } else { "keys" },
        unknown.join(", "),
        DIFF_OPERATIONS.join(", "),
    )))
}

/// GH #581 — the single-declaration door refuses a MANIFEST body by name.
///
/// `handle_mutation` reads its work from `payload["diff"]` and treats an absent
/// key as the EMPTY diff. A manifest body — `{"manifest": [ … ]}` — carries no
/// `diff`, so every step below had nothing to do and the door answered
/// `committed` for a colony it never changed. `refuse_unknown_diff_keys` could
/// not catch it: that check looks INSIDE the diff and never sees a top-level
/// key.
///
/// The discriminator is the same one [`crate::mutation::ManifestBody::detect`]
/// uses on the other side of the same wall — the presence of `manifest`, and
/// nothing else. Any other top-level key stays ignored exactly as it always was
/// (pinned by `gh422_the_single_mutation_body_does_not_move`); only `manifest`
/// discriminates, here as there.
///
/// A body carrying `manifest` BESIDE `diff`/`scope` is refused too, and for the
/// reason [`crate::mutation::ManifestError::BothForms`] already gives it: two
/// intentions in one document, where guessing which one wins is how a mutation
/// lands somewhere nobody asked for. The single door used to apply the `diff`
/// half and drop the other without a word.
///
/// The token is `schema` — never a new `error_code` (README § Stability). A
/// body form a door will not apply is what `schema` has always meant, and
/// `ManifestError::error_code` says the same from the other side.
///
/// Pre-destructive by position AND spurless: the caller runs this BEFORE the
/// mutation id is minted, so the refusal opens no mutation-log row.
pub fn refuse_manifest_at_the_single_door(payload: &JsonValue) -> Result<(), MutationError> {
    if payload.get("manifest").is_none() {
        return Ok(());
    }
    Err(MutationError::Schema(
        "this body carries a top-level `manifest` key, and this is the \
         single-declaration door, which applies one `diff` and would have \
         applied none of the manifest's entries. Send a manifest to the \
         mutation door (`/colony/mutations`), or drop the key and send one \
         declaration. Refused rather than ignored — a body nothing reads would \
         have committed without effect."
            .into(),
    ))
}

/// T10 — schema check + template existence.
///
/// T11 extends this with match patterns and naming uniqueness; T11b adds
/// cycle and edge schema (see phase-6 plan, decision 7).
pub fn validate_post_state(
    diff_substituted: &JsonValue,
    factories: &CellFactoryRegistry,
) -> Result<(), MutationError> {
    let obj = diff_substituted
        .as_object()
        .ok_or_else(|| MutationError::Schema("diff is not an object".into()))?;
    if let Some(adds) = obj.get("add_nodes").and_then(|v| v.as_array()) {
        for n in adds {
            let template = n
                .get("template")
                .and_then(|v| v.as_str())
                .ok_or_else(|| MutationError::Schema("add_nodes[].template missing".into()))?;
            if !factories.contains_key(template) {
                return Err(MutationError::TemplateMissing(template.into()));
            }
        }
    }
    Ok(())
}

/// T11 — extends T10's checks with Match-Pattern hit and Naming-Eindeutigkeit.
///
/// `registry_names`: current cell-name-set in the scope (last path segment).
/// Phase-6-MVP: post_state = (registry_names \ remove_matches) ∪ add_names.
/// T11b adds cycle + edge_schema on top.
pub fn validate_post_state_full(
    diff: &JsonValue,
    factories: &CellFactoryRegistry,
    registry_names: &[String],
) -> Result<(), MutationError> {
    validate_post_state(diff, factories)?;
    let obj = diff
        .as_object()
        .expect("validate_post_state covered schema");
    // GH #179: the identity checks live in ONE place. This entry point carries no
    // scope and no depth information (its callers are the pre-R12 wrappers), so
    // it passes root scope and empty depth sets — the short-name behaviour it
    // always had, minus the second copy of the rules that drifted from it.
    validate_naming_and_match(obj, registry_names, &[], "/", &[], &[])
}

/// T11b — extends T11's checks with edge_schema (endpoints exist in post_state)
/// and cycle-freeness via hand-rolled DFS.
///
/// `existing_edges` is the current (from, to) edge-list, as Phase-6-MVP passes
/// the in-memory `EdgeTable` snapshot when calling from the Mutation arm (T13).
///
/// post_state-node-set: (registry_names \ remove_matches) ∪ add_names ∪ hive_endpoint_names.
/// post_state-edges: existing_edges ++ add_edges.
///
/// `hive_endpoint_names`: hive short-names (last path segment) that are also
/// valid edge endpoints — Cell ∪ Hive symmetry (Phase 13.5 step-6).
pub fn validate_post_state_with_edges(
    diff: &JsonValue,
    factories: &CellFactoryRegistry,
    registry_names: &[String],
    existing_edges: &[(String, String)],
    hive_endpoint_names: &[String],
) -> Result<(), MutationError> {
    // Thin wrapper: single-cell mutations contribute no subtree nodes/edges, so
    // delegate with empty subtree slices — byte-behavior-unchanged.
    validate_post_state_with_edges_and_subtree(
        diff,
        factories,
        registry_names,
        existing_edges,
        hive_endpoint_names,
        &[],
        &[],
    )
}

/// T7 (Phase 13.5 a5-subtree) — extends [`validate_post_state_with_edges`] so a
/// SUBTREE instantiation can be validated in a single pass.
///
/// In addition to the single-cell inputs, the caller contributes the subtree's
/// own nodes and internal edges (empty for single-cell mutations):
///
/// - `subtree_node_endpoints`: every subtree cell + hive path, expressed in the
///   validator's endpoint representation. They are added to the valid-endpoint
///   node-set so internal edges pointing at a nested node validate.
/// - `subtree_internal_edges`: the subtree's resolved internal `(from, to)` edges
///   (from each hive's `params.graph`), in the same representation. They join the
///   post_state edge-set BEFORE the endpoint-existence check and the cycle check,
///   so an internal edge that forms a cycle (alone or combined with existing/diff
///   edges) is rejected.
///
/// # Representation contract
/// The validator is representation-agnostic: it only does string-set membership.
/// The caller MUST express `subtree_node_endpoints` and `subtree_internal_edges`
/// in the SAME representation as each other (the subtree resolver uses absolute
/// logical paths, e.g. `/main/m1/inner_a`). The existing single-cell inputs use
/// scope-relative short-names; the two namespaces do not collide, which is
/// intended — a subtree-internal edge endpoint only matches a subtree node.
#[allow(clippy::too_many_arguments)]
pub fn validate_post_state_with_edges_and_subtree(
    diff: &JsonValue,
    factories: &CellFactoryRegistry,
    registry_names: &[String],
    existing_edges: &[(String, String)],
    hive_endpoint_names: &[String],
    subtree_node_endpoints: &[String],
    subtree_internal_edges: &[(String, String)],
) -> Result<(), MutationError> {
    validate_post_state_full(diff, factories, registry_names)?;
    let obj = diff.as_object().expect("validated above");
    // Phase 13.5-A1 T4: edge-schema + cycle + CEL-parse-validate share one
    // implementation (`validate_edges_and_cycle`). Scope-agnostic helper:
    // depth endpoints resolve against root, pre-state depth paths are not
    // known here (R12 callers use `validate_post_state_with_templates_scoped`).
    validate_edges_and_cycle(
        obj,
        registry_names,
        existing_edges,
        hive_endpoint_names,
        subtree_node_endpoints,
        subtree_internal_edges,
        "/",
        &[],
        // GH #285: this scope-agnostic face has no hive declarations in hand —
        // no slots, so the endpoint universe is the one it always had.
        &std::collections::HashSet::new(),
    )
}

/// GH #166 / #179 — the ONE namespace every check that asks "does this path
/// already exist" has to resolve a diff name in.
///
/// A diff name is a SHORT name only while it carries no `/` once the canonical
/// `./` prefix is stripped (Befund 6: `./a` and `a` denote the same node). Then
/// it lives in the scope's own namespace, where names are unique per scope
/// (spec Z.265). A name that still carries a `/` addresses a PATH — the
/// containment guard resolves multi-segment names against the scope and only
/// refuses `..` segments and absolute names, so `unit/q` is the sanctioned way
/// to name a node one level below the scope — and a path means nothing until it
/// is resolved. Comparing it against a set of short names matches nothing, which
/// is silence, not a verdict.
///
/// Splitting this decision per check is how the two halves of the same defect
/// arose: the endpoint check (#166) and the identity checks (#179) each read the
/// name in one namespace and looked it up in another, in opposite directions.
///
/// GH #487 — and `.` is a THIRD reading of the same string, the one the boot
/// path always had: it names the scope root itself. See [`scoped_name`].
pub(crate) enum ScopedName<'a> {
    /// Tested in the scope's short-name namespace (`registry_names` & co).
    Short(&'a str),
    /// Tested in the absolute-path namespace (`deep_*_paths` & co).
    Deep(String),
}

/// Classify `name` into the namespace it is to be tested in — see [`ScopedName`].
///
/// GH #487 — `.` (and the `./` that strips to nothing) is the SELF-reference:
/// it names the declaration's own `scope`, which is an absolute path and
/// therefore [`ScopedName::Deep`]. That is not a new rule, it is the one
/// [`meclaw_core::Path::resolve`] has always applied — `trimmed.is_empty() ||
/// trimmed == "."` ⇒ "stay at the sender" — and the one the boot path resolves
/// a hive's `params.graph` with, which is why `{"from": "./firewall", "to": "."}`
/// means a lane out of the level in 31 of the shipped templates. Only this
/// classifier disagreed: it read `.` as a short NAME, no node is called `.`, and
/// the endpoint check answered `edge_schema: to='.' unknown` for the single
/// spelling the whole catalogue teaches.
///
/// Resolving it here is deliberately the whole fix. The apply arm never needed
/// one — `resolve_scoped_path` IS `Path::resolve`, so `add_edges`,
/// `remove_edges`, the header-view mirror, the port-boundary and the hive
/// contract have all resolved `.` correctly the entire time; validate was the
/// one reader in the other namespace. And it is resolved at the point of USE,
/// never written back into the diff: a manifest is parked under the sha256 of
/// its own bytes and submitted by that digest, so canonicalising the spelling
/// before the digest would make the digest describe a document nobody was shown
/// and would move every digest already parked. Two spellings of one node have
/// two digests — which `./a` and `a` have had since Befund 6.
///
/// A scope whose root is no node is unaffected: `.` at scope `/` resolves to
/// `/`, which is the colony's top-level scope and carries no registry row, so
/// the membership test refuses it exactly as before. This widens the
/// vocabulary; it invents no node.
pub(crate) fn scoped_name<'a>(scope: &str, name: &'a str) -> ScopedName<'a> {
    let stripped = name.strip_prefix("./").unwrap_or(name);
    if stripped.is_empty() || stripped == "." {
        return ScopedName::Deep(
            crate::mutation::resolve_scoped_path(scope, stripped)
                .as_str()
                .to_string(),
        );
    }
    if stripped.contains('/') {
        ScopedName::Deep(
            crate::mutation::resolve_scoped_path(scope, stripped)
                .as_str()
                .to_string(),
        )
    } else {
        ScopedName::Short(stripped)
    }
}

/// Whether `name` already names a node, tested in whichever namespace the name
/// itself selects (see [`ScopedName`]). `short_names` and `deep_paths` are the
/// two spellings of the SAME pre-state set; a caller that has no depth
/// information passes an empty `deep_paths` and keeps the short-name behaviour.
pub(crate) fn name_is_taken(
    scope: &str,
    name: &str,
    short_names: &[String],
    deep_paths: &[String],
) -> bool {
    match scoped_name(scope, name) {
        ScopedName::Short(s) => short_names.iter().any(|n| n == s),
        ScopedName::Deep(p) => deep_paths.contains(&p),
    }
}

/// One entry of a diff that will put a node AT a path — see [`diff_path_claims`].
struct PathClaim {
    /// The name exactly as the operator wrote it. The post-state view needs it
    /// unresolved because it is consulted in BOTH namespaces and only
    /// `scoped_name` decides which one a given spelling asks.
    name: String,
    /// `name` resolved against the scope: the single namespace two claims are
    /// compared in, since the apply side renames onto the resolved path.
    path: String,
    /// `add_nodes[2].name 'q'` — what a duplicate-claim message points at.
    entry: String,
}

/// GH #195 — every path this diff CLAIMS, in the order the operator wrote them,
/// resolved against the scope and labelled with the entry that claims it.
///
/// A claim is an entry that will put a node at a path: `add_nodes[].name`, the
/// INSTANTIATE form of `swap_nodes[].with` (`template` + `name`, which stages a
/// fresh template tree), and `move_nodes[].to`. The existing-node form of
/// `swap_nodes[].with` carries no `template` — it references a node that is
/// there or that an `add_nodes` in the same diff is creating, and referencing is
/// not claiming.
///
/// `add_nodes` resume targets are in the set. A resume is deliberately NOT a
/// collision against the registry — the same node keeps its identity — but it is
/// still this diff saying "that path is mine", and a second entry aiming there
/// is the thing this catches.
///
/// Malformed entries are skipped rather than reported: their own checks raise
/// the `Schema` errors, and reordering those would change what a broken diff is
/// told.
///
/// GH #198 — this is also the set of nodes the diff makes ADDRESSABLE, so
/// [`validate_edges_and_cycle`] builds the insert side of its post-state view
/// from it. "Which entries put a node at a path" is one question; answering it
/// in two places is how #166 came to cover `add_nodes` and neither of the other
/// two, and how a `move_nodes` could not be wired in the diff that performed it.
fn diff_path_claims(
    scope: &str,
    obj: &meclaw_core::serde_json::Map<String, JsonValue>,
) -> Vec<PathClaim> {
    let mut claims: Vec<PathClaim> = Vec::new();
    let mut push = |name: &str, entry: String| {
        claims.push(PathClaim {
            name: name.to_string(),
            path: crate::mutation::resolve_scoped_path(scope, name)
                .as_str()
                .to_string(),
            entry,
        });
    };
    if let Some(adds) = obj.get("add_nodes").and_then(|v| v.as_array()) {
        for (i, n) in adds.iter().enumerate() {
            if let Some(name) = n.get("name").and_then(|v| v.as_str()) {
                push(name, format!("add_nodes[{i}].name '{name}'"));
            }
        }
    }
    if let Some(swaps) = obj.get("swap_nodes").and_then(|v| v.as_array()) {
        for (i, s) in swaps.iter().enumerate() {
            let Some(with) = s.get("with").and_then(|v| v.as_object()) else {
                continue;
            };
            // Instantiate form only — see the note above.
            if !with.contains_key("template") {
                continue;
            }
            if let Some(name) = with.get("name").and_then(|v| v.as_str()) {
                push(name, format!("swap_nodes[{i}].with.name '{name}'"));
            }
        }
    }
    if let Some(moves) = obj.get("move_nodes").and_then(|v| v.as_array()) {
        for (i, m) in moves.iter().enumerate() {
            if let Some(to) = m.get("to").and_then(|v| v.as_str()) {
                push(to, format!("move_nodes[{i}].to '{to}'"));
            }
        }
    }
    claims
}

/// GH #195 — every duplicated claim in a diff: two entries that name one path.
///
/// It REPORTS, it does not refuse (the name it carried until GH #293 said
/// otherwise, and a name that promises a verdict is worth correcting): the
/// caller — [`addressed_naming_and_match`] — decides what happens to the claims
/// handed back, and its own `Result` face turns the first of them into the
/// refusal this used to return directly.
///
/// The pre-state sets this used to be left to cannot answer it. They arrive
/// resume-filtered, so an `add_nodes` at an existing path is taken out of them
/// on purpose, and the only in-diff bookkeeping there was tracked `add_nodes`
/// among themselves. A claim from any other entry was therefore invisible: a
/// `swap_nodes[].with` at a resume target got the generic occupied-path message
/// (advice for a leftover directory, wrong here), and at a FRESH name nothing
/// refused it at all — two trees staged onto one path and the second apply
/// failed halfway, which is `LiveTreeMutated` and strict-fails the whole colony
/// task rather than the mutation.
///
/// Claims are compared as RESOLVED paths, the one namespace decision
/// `scoped_name` makes everywhere else on this surface (#179): the apply side
/// renames onto the resolved path, so `unit/n1` and `./unit/n1` are one target.
///
/// GH #293 — collecting: every duplicated claim is reported, in claim order.
/// The FIRST one is the error the `Result` form returned before, so no verdict
/// moves; the ones after it used to cost a round trip apiece.
fn collect_duplicate_claims(
    scope: &str,
    obj: &meclaw_core::serde_json::Map<String, JsonValue>,
) -> Vec<(MutationError, Option<String>)> {
    let claims = diff_path_claims(scope, obj);
    let mut seen: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    let mut violations = Vec::new();
    for PathClaim { path, entry, .. } in &claims {
        if let Some(first) = seen.insert(path.as_str(), entry.as_str()) {
            violations.push((
                MutationError::NamingCollision(format!(
                    "{entry} and {first} both claim {path} in this diff. One path holds \
                     one node, so whichever entry is applied second lands on what the \
                     first one just put there. Nothing was written — give them \
                     different names, or drop one of the two entries."
                )),
                Some(path.clone()),
            ));
        }
    }
    violations
}

/// Inner helper: naming-collision + match-no-hit checks, shared between
/// `validate_post_state_full` and `validate_post_state_with_templates`.
/// Does NOT check `add_nodes[].template` against factories (that is caller's job).
///
/// `scope` + `deep_registry_paths` / `deep_hive_paths` (GH #179) are the
/// absolute-path twins of `registry_names` / `hive_match_names`: the same
/// pre-state, spelled the way a multi-segment name has to be looked up (see
/// [`name_is_taken`]). Colony-global is safe for both — `validate_scope_containment`
/// runs BEFORE this and rejects `..`/absolute names, so a resolved deep name is
/// scope-contained by construction and cannot borrow a node from a foreign scope
/// the way an un-filtered SHORT name could. `deep_registry_paths` MUST have the
/// caller's resume targets removed, exactly as `registry_names` has its resume
/// short-names removed: an `add_nodes` at an existing path is a Resume, not a
/// duplicate, at depth as at level one.
///
/// `hive_match_names`: SCOPE-FILTERED hive short-names (parent path == guard_scope),
/// mirroring `registry_names`. For `swap_nodes`, a match.name that refers to a HIVE
/// (which has no registry entry but IS in the pre-state via `hive_scopes`) must also
/// pass the match check — hives are valid swap sources (they carry external edges
/// just like cells). Per spec Z.265 ("Names are unique per scope") this set MUST
/// be scope-filtered so a hive in a FOREIGN scope cannot satisfy a short-name match
/// (apply-side `resolve_scoped_path` is scope-correct, so a global set here would be a
/// validate-side false-positive — Paket-5 companion finding Paket-2-b'). Pass an empty
/// slice for callers that do not have this information (e.g. `validate_post_state_full`,
/// which does not know hive names; existing swap-of-cell tests are unaffected).
///
/// GH #293 — this is the thin `Result` face of [`collect_naming_and_match`]: the
/// FIRST violation the collecting core produces is, by construction, the one
/// this function returned before, so every verdict it ever gave is byte-identical.
fn validate_naming_and_match(
    obj: &meclaw_core::serde_json::Map<String, JsonValue>,
    registry_names: &[String],
    hive_match_names: &[String],
    scope: &str,
    deep_registry_paths: &[String],
    deep_hive_paths: &[String],
) -> Result<(), MutationError> {
    collect_naming_and_match(
        obj,
        registry_names,
        hive_match_names,
        scope,
        deep_registry_paths,
        deep_hive_paths,
    )
    .into_iter()
    .next()
    .map_or(Ok(()), Err)
}

/// GH #293 — the collecting core of [`validate_naming_and_match`]: every naming
/// collision and every `match` that hits nothing, in the order the checks used
/// to abort at.
///
/// The addresses are dropped here; [`collect_post_state_addresses`] takes the
/// addressed form so a rejection can say WHICH name each violation is about
/// without a reader parsing the prose back apart.
fn collect_naming_and_match(
    obj: &meclaw_core::serde_json::Map<String, JsonValue>,
    registry_names: &[String],
    hive_match_names: &[String],
    scope: &str,
    deep_registry_paths: &[String],
    deep_hive_paths: &[String],
) -> Vec<MutationError> {
    addressed_naming_and_match(
        obj,
        registry_names,
        hive_match_names,
        scope,
        deep_registry_paths,
        deep_hive_paths,
    )
    .into_iter()
    .map(|(error, _)| error)
    .collect()
}

/// [`collect_naming_and_match`] with the address each check already had in hand
/// — the name or path the violation is about.
///
/// The per-item bodies are the ones [`validate_naming_and_match`] used to
/// `return` from; they push and carry on instead. A malformed entry pushes its
/// `Schema` error and the loop moves to the next one, which is the only way a
/// second violation of the same kind can ever be seen.
fn addressed_naming_and_match(
    obj: &meclaw_core::serde_json::Map<String, JsonValue>,
    registry_names: &[String],
    hive_match_names: &[String],
    scope: &str,
    deep_registry_paths: &[String],
    deep_hive_paths: &[String],
) -> Vec<(MutationError, Option<String>)> {
    // GH #195: the diff against ITSELF, before it is measured against the
    // pre-state — a path claimed twice by one diff is a different problem from a
    // path that was already occupied, and the pre-state check cannot name it.
    let mut violations = collect_duplicate_claims(scope, obj);
    {
        let mut refuse = |error: MutationError, address: String| {
            violations.push((error, Some(address)));
        };
        if let Some(adds) = obj.get("add_nodes").and_then(|v| v.as_array()) {
            for (i, n) in adds.iter().enumerate() {
                let Some(name) = n.get("name").and_then(|v| v.as_str()) else {
                    refuse(
                        MutationError::Schema("add_nodes[].name missing".into()),
                        format!("add_nodes[{i}]"),
                    );
                    continue;
                };
                if name_is_taken(scope, name, registry_names, deep_registry_paths) {
                    refuse(
                        MutationError::NamingCollision(name.into()),
                        name.to_string(),
                    );
                }
            }
        }
        if let Some(rems) = obj.get("remove_nodes").and_then(|v| v.as_array()) {
            for (i, r) in rems.iter().enumerate() {
                let Some(pat_name) = r
                    .get("match")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                else {
                    refuse(
                        MutationError::Schema("remove_nodes[].match.name missing".into()),
                        format!("remove_nodes[{i}]"),
                    );
                    continue;
                };
                if !name_is_taken(scope, pat_name, registry_names, deep_registry_paths) {
                    refuse(
                        MutationError::MatchNoHit(pat_name.into()),
                        pat_name.to_string(),
                    );
                }
            }
        }
        if let Some(swaps) = obj.get("swap_nodes").and_then(|v| v.as_array()) {
            for (i, s) in swaps.iter().enumerate() {
                let Some(pat_name) = s
                    .get("match")
                    .and_then(|v| v.get("name"))
                    .and_then(|v| v.as_str())
                else {
                    refuse(
                        MutationError::Schema("swap_nodes[].match.name missing".into()),
                        format!("swap_nodes[{i}]"),
                    );
                    continue;
                };
                // PRE-STATE check: cell registry OR hive scope (hive is a valid swap source
                // because it carries external edges). Both sets are scope-filtered by the
                // caller — a hive in a foreign scope must NOT satisfy this short-name match.
                let in_registry =
                    name_is_taken(scope, pat_name, registry_names, deep_registry_paths);
                let in_hives = name_is_taken(scope, pat_name, hive_match_names, deep_hive_paths);
                if !in_registry && !in_hives {
                    refuse(
                        MutationError::MatchNoHit(pat_name.into()),
                        pat_name.to_string(),
                    );
                }
            }
        }
    }
    violations
}

/// Take `name` out of BOTH spellings of the post-state view (GH #194).
///
/// The view an endpoint is looked up in is two sets — `nodes` for short names,
/// `deep` for absolute paths — and `scoped_name` decides which one a given
/// endpoint asks. A diff entry that takes a node out therefore has to reach
/// both, or the endpoint check answers from the half the removal never touched:
/// that is precisely how a deep `remove_nodes` and an `add_edges` naming the
/// same node ended up committing a lane onto a disconnected cell.
///
/// The short branch subtracts the resolved path as well. No spelling reaches a
/// short-named node through the deep namespace today (containment refuses the
/// `..` that would be needed), but leaving one half standing is exactly the
/// asymmetry this whole family of defects is made of.
fn vacate(
    scope: &str,
    name: &str,
    nodes: &mut std::collections::HashSet<String>,
    deep: &mut std::collections::HashSet<String>,
) {
    match scoped_name(scope, name) {
        ScopedName::Short(s) => {
            nodes.remove(s);
            deep.remove(crate::mutation::resolve_scoped_path(scope, s).as_str());
        }
        ScopedName::Deep(abs) => {
            nodes.remove(abs.as_str());
            deep.remove(abs.as_str());
        }
    }
}

/// Put `name` into BOTH spellings of the post-state view (GH #198).
///
/// The mirror image of [`vacate`], and deliberately its exact shape: the view is
/// two sets, `scoped_name` decides which one an endpoint asks, and an entry that
/// reaches only one half leaves the other half answering from a state that never
/// existed. That asymmetry is what this whole family of defects is made of — it
/// was the corrupting direction in #194 and the obstructing one here.
///
/// The short branch contributes the resolved path as well. No single-segment
/// endpoint is looked up in the deep namespace today, so it changes no verdict;
/// leaving one half of the view standing is the thing that keeps costing an
/// issue apiece.
fn occupy(
    scope: &str,
    name: &str,
    nodes: &mut std::collections::HashSet<String>,
    deep: &mut std::collections::HashSet<String>,
) {
    match scoped_name(scope, name) {
        ScopedName::Short(s) => {
            nodes.insert(s.to_string());
            deep.insert(
                crate::mutation::resolve_scoped_path(scope, s)
                    .as_str()
                    .to_string(),
            );
        }
        ScopedName::Deep(abs) => {
            nodes.insert(abs.clone());
            deep.insert(abs);
        }
    }
}

/// Inner helper: edge-schema + cycle checks over the post-state graph.
/// Extracted to be shared between `validate_post_state_with_edges` and
/// `validate_post_state_with_templates`.
///
/// R12 depth resolution: `scope` is the mutation's guard scope and
/// `deep_endpoint_paths` the absolute paths of ALL pre-state nodes (registry ∪
/// hive scopes, any depth). A multi-segment `add_edges` endpoint (contains `/`
/// after the `./`-strip) resolves against `scope` — the SAME normalisation the
/// apply side uses (`resolve_scoped_path`) — and is membership-tested in the
/// ABSOLUTE-path namespace (`deep_endpoint_paths` ∪ `subtree_node_endpoints`).
/// Single-segment endpoints keep the unchanged short-name test. Containment is
/// NOT relaxed: `validate_scope_containment` ran before this and already
/// rejected `..`/absolute endpoints (`scope_out_of_bounds`), so a resolved
/// depth path is within scope by construction.
///
/// GH #194 — `deep_endpoint_paths` is the PRE-state and the caller has no way to
/// know what this diff does to it, so the subtraction happens here, next to the
/// short-name one, through the same [`vacate`]. Three entries take a path out of
/// the post-state view and all three go through it: `remove_nodes` (disconnect),
/// `swap_nodes[].match` (the node being replaced — its edges are swung onto the
/// target, but the swing runs over the PRE-diff edges, so a lane this diff adds
/// onto it is not carried along), and `move_nodes[].match` (an address the
/// mutation vacates by `rename(2)`).
///
/// GH #293 — this is the thin `Result` face of [`addressed_edges_and_cycle`]:
/// the FIRST violation the collecting core produces is, by construction, the one
/// this function returned before, so every verdict it ever gave is byte-identical.
///
/// GH #285 — `slot_endpoints` is the second known-set: the absolute addresses
/// hives declared as SLOTS, an empty set for every caller that has none.
#[allow(clippy::too_many_arguments)]
fn validate_edges_and_cycle(
    obj: &meclaw_core::serde_json::Map<String, JsonValue>,
    registry_names: &[String],
    existing_edges: &[(String, String)],
    hive_endpoint_names: &[String],
    subtree_node_endpoints: &[String],
    subtree_internal_edges: &[(String, String)],
    scope: &str,
    deep_endpoint_paths: &[String],
    slot_endpoints: &std::collections::HashSet<String>,
) -> Result<(), MutationError> {
    addressed_edges_and_cycle(
        obj,
        registry_names,
        existing_edges,
        hive_endpoint_names,
        subtree_node_endpoints,
        subtree_internal_edges,
        scope,
        deep_endpoint_paths,
        slot_endpoints,
    )
    .into_iter()
    .next()
    .map_or(Ok(()), |(error, _)| Err(error))
}

/// GH #293 — the collecting core of [`validate_edges_and_cycle`], with the
/// address each check already had in hand: every endpoint that no post-state
/// node answers to and every malformed `add_edges` entry, in the order the
/// checks used to abort at.
///
/// The per-item bodies are the ones [`validate_edges_and_cycle`] used to
/// `return` from; they push and carry on instead. An edge whose `from` is
/// missing is skipped rather than measured — without endpoints there is nothing
/// left to say about it — but the NEXT edge is still checked, which is the only
/// way a second dangling endpoint can ever be seen.
///
/// **On the cycle half of the name:** there is no cycle entry to collect. The
/// topological cycle gate was removed by Befund 2 (see the closing comment
/// below) and the spec is explicit that meclaw-core does not reject cycles in
/// general. Were it ever reinstated it would contribute AT MOST ONE entry, and
/// after the endpoint entries: a cycle is a property of the whole edge set, not
/// of one edge, so it cannot be attributed to an endpoint and it cannot be
/// counted twice.
///
/// **On `slot_endpoints` (GH #285):** the absolute addresses hives declared as
/// SLOTS — an address that exists and may stand EMPTY, so an `add_edges` onto it
/// is the edge the declaration invited rather than a lane into nothing. It is a
/// set of its own and is consulted by the `known` endpoint closure alone:
/// folding it into `nodes` would make a slot a legal `swap_nodes[].match` or
/// `remove_nodes` target, i.e. would turn a hive's promise about an empty
/// address into a node this colony claims to have registered. Empty for every
/// caller that declares no slots, which leaves every verdict where it was.
///
/// It is also, deliberately, the one endpoint term the [`vacate`] subtractions
/// above do NOT reach: a diff that removes the node filling a slot and wires the
/// address in the same breath COMMITS, where GH #194 gave `edge_schema` for
/// every other node on its way out. The reason is what a slot is — the
/// declaration outlives whatever stood behind it, so after the removal the
/// address is exactly the empty declared slot the hive announced, and an edge
/// onto it is the edge that declaration invited. Pinned in
/// `gh285_a_slot_is_a_declared_empty_address.rs`.
#[allow(clippy::too_many_arguments)]
fn addressed_edges_and_cycle(
    obj: &meclaw_core::serde_json::Map<String, JsonValue>,
    registry_names: &[String],
    existing_edges: &[(String, String)],
    hive_endpoint_names: &[String],
    subtree_node_endpoints: &[String],
    subtree_internal_edges: &[(String, String)],
    scope: &str,
    deep_endpoint_paths: &[String],
    slot_endpoints: &std::collections::HashSet<String>,
) -> Vec<(MutationError, Option<String>)> {
    use std::collections::HashSet;
    let mut violations: Vec<(MutationError, Option<String>)> = Vec::new();
    let mut nodes: HashSet<String> = registry_names.iter().cloned().collect();
    // Phase 13.5 step-6: hives are valid edge endpoints too (Cell ∪ Hive). Add
    // their short-names before the add_edges endpoint check below.
    nodes.extend(hive_endpoint_names.iter().cloned());
    // Phase 13.5 a5-subtree T7: a subtree instantiation contributes its own
    // nested cell + hive paths as valid edge endpoints (same representation as
    // its internal edges below). Empty for single-cell mutations.
    nodes.extend(subtree_node_endpoints.iter().cloned());
    // GH #163: the colony's read-only topology endpoint is a valid edge TARGET at
    // every scope (containment lets it through for the same reason). It exists
    // for the whole lifetime of the colony, so "unknown endpoint" would be a
    // false negative — the boot path has always resolved `/colony/*` this way.
    nodes.extend(
        crate::mutation::MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS
            .iter()
            .map(|s| (*s).to_string()),
    );
    // GH #194: the pre-state absolute-path half of the same view. It used to be
    // consulted straight from the caller's slice, which is why every subtraction
    // below missed it — for a node that already existed at depth, this is the
    // set the endpoint check actually reads.
    let mut deep: HashSet<String> = deep_endpoint_paths.iter().cloned().collect();
    // GH #193/#194: every entry that takes a node out of the post-state view
    // goes through `vacate`, which canonicalises through the same `scoped_name`
    // the endpoint check asks and reaches BOTH spellings of the view. Taken as
    // written (#193) or subtracted from one half only (#194), a node on its way
    // out stayed addressable and an `add_edges` in the same diff wired a lane
    // onto it — the one thing this check exists to prevent. That is the LENIENT
    // direction, which is why it outlives its neighbours every time: it accepts
    // what it should refuse, and nothing complains at the time.
    if let Some(rems) = obj.get("remove_nodes").and_then(|v| v.as_array()) {
        for r in rems {
            if let Some(name) = r
                .get("match")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
            {
                vacate(scope, name, &mut nodes, &mut deep);
            }
        }
    }
    // GH #194: a `swap_nodes[].match.name` is a node on its way out too. Its
    // edges are swung onto the target — but `plan_edge_swing` runs over the
    // edges that were there BEFORE the diff, so a lane this same diff adds onto
    // the replaced node is not swung with them and is left naming a cell that
    // has been disconnected.
    if let Some(swaps) = obj.get("swap_nodes").and_then(|v| v.as_array()) {
        for s in swaps {
            if let Some(name) = s
                .get("match")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
            {
                vacate(scope, name, &mut nodes, &mut deep);
            }
        }
    }
    // GH #194: a `move_nodes[].match.name` is an ADDRESS the mutation vacates.
    // The directory leaves by `rename(2)` and the registry row is re-addressed,
    // so after the move there is nothing at the old path at all — an edge naming
    // it is not merely dead, it points at ground the colony no longer knows.
    if let Some(moves) = obj.get("move_nodes").and_then(|v| v.as_array()) {
        for m in moves {
            if let Some(name) = m
                .get("match")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
            {
                vacate(scope, name, &mut nodes, &mut deep);
            }
        }
    }
    // GH #166/#189/#198: and every entry that PUTS a node at a path goes into it,
    // through the one enumeration that already answers "which entries do that" —
    // `diff_path_claims`, which #195 wrote to compare a diff against itself.
    //
    // #166 gave `add_nodes` this treatment so an `add_edges` may point at a node
    // arriving in the same diff, and named the reason: splitting into two
    // mutations means choosing between a window where a lane is wired twice and
    // one where it is not wired at all. The two other creating entries were left
    // out. `move_nodes` is the operation shipped (#169) to have no such window,
    // and a caller could not give the relocated cell one extra lane in the same
    // breath; a `swap_nodes[].with` instantiate stages a fresh tree and the
    // swing only carries the lanes that were there BEFORE the diff, so a new one
    // had to be expressible here or not at all.
    //
    // Driving both sides of the view from one enumeration is the point: the
    // second list is what drifts. The existing-node form of `swap_nodes[].with`
    // stays out of it for the same reason it is no claim (#195) — it references
    // a node that the pre-state or an `add_nodes` of this diff already puts in
    // the view, and it creates nothing of its own.
    for claim in diff_path_claims(scope, obj) {
        occupy(scope, &claim.name, &mut nodes, &mut deep);
    }
    // Existing edges are part of the post_state graph, but Befund 2 removed the
    // topological cycle gate — they no longer need to be accumulated here. The
    // endpoint-existence check below is purely a node-set membership test.
    let _ = existing_edges;
    // Phase 13.5 a5-subtree T7: subtree-internal edges' endpoints must each be a
    // known post_state node (subtree node, registry, hive, or an add_node from
    // this diff). Endpoint-existence only — no cycle accumulation (Befund 2).
    for (from, to) in subtree_internal_edges {
        if !nodes.contains(from.as_str()) {
            violations.push((
                MutationError::EdgeSchema(format!("subtree internal edge from='{from}' unknown")),
                Some(from.clone()),
            ));
        }
        if !nodes.contains(to.as_str()) {
            violations.push((
                MutationError::EdgeSchema(format!("subtree internal edge to='{to}' unknown")),
                Some(to.clone()),
            ));
        }
    }
    if let Some(adds) = obj.get("add_edges").and_then(|v| v.as_array()) {
        for (i, e) in adds.iter().enumerate() {
            let Some(from) = e.get("from").and_then(|v| v.as_str()) else {
                violations.push((
                    MutationError::Schema("add_edges[].from missing".into()),
                    Some(format!("add_edges[{i}]")),
                ));
                continue;
            };
            let Some(to) = e.get("to").and_then(|v| v.as_str()) else {
                violations.push((
                    MutationError::Schema("add_edges[].to missing".into()),
                    Some(format!("add_edges[{i}]")),
                ));
                continue;
            };
            // Befund 6: endpoints are scope-relative; the canonical mutation
            // form is `./name` (overview § Variable substitution example). Both
            // `./name` and the bare `name` denote the same scope-local node, so
            // strip the `./` before the short-name membership test — otherwise a
            // `./`-prefixed endpoint never matched an `add_nodes`/registry
            // short-name and rejected as `edge_schema` ("from='./a' unknown").
            // (Apply mirrors this: `resolve_scoped_path` now normalises `./`.)
            //
            // R12: a multi-segment endpoint (`./unit/dispatch`) addresses a node
            // DEEPER than level 1 within the scope (spec Z.227 has no depth
            // restriction). Resolve it against `scope` and test in the
            // absolute-path namespace: `deep_endpoint_paths` (pre-state at any
            // depth) ∪ `nodes` (which carries the absolute
            // `subtree_node_endpoints` and — GH #166 — the scope-resolved
            // multi-segment `add_nodes` names: diff-new nodes, Befund-6
            // semantics at depth). Befund-6 short-name semantics for single
            // segments are byte-unchanged.
            // GH #179: the namespace decision itself is `scoped_name` now — the
            // same one the identity checks use, so the endpoint check and the
            // "does this path already exist" checks can no longer drift apart
            // the way they did between #166 and #179.
            // GH #194: `deep` rather than the caller's `deep_endpoint_paths`,
            // because the pre-state is not the post-state — the diff's own
            // removals have been subtracted from it above, exactly as they are
            // from `nodes`. One view, both spellings.
            // GH #285: and the second known-set, the only term that resolves an
            // address with NO node behind it — a hive declared this address a
            // SLOT, which says it exists and may stand empty, so wiring it
            // before anything fills it is the whole point of saying so. Slot
            // addresses are absolute, so both spellings ask them in the
            // absolute namespace: `./h/gen` from the parent scope and `./gen`
            // from inside the hive name the same node and must answer alike.
            // Deliberately NOT merged into `nodes` — see the doc comment.
            // A short name reaches the slot set only through the scope: the set
            // speaks absolute addresses, and `./gen` at scope `/h` is the node
            // `/h/gen` that `./h/gen` at scope `/` also names. The emptiness
            // guard keeps the resolution out of the hot path of every colony
            // that declares no slot at all — which is every colony today.
            let short_is_slot = |short: &str| -> bool {
                !slot_endpoints.is_empty()
                    && slot_endpoints
                        .contains(crate::mutation::resolve_scoped_path(scope, short).as_str())
            };
            let known = |endpoint: &str| -> bool {
                match scoped_name(scope, endpoint) {
                    ScopedName::Short(s) => nodes.contains(s) || short_is_slot(s),
                    ScopedName::Deep(abs) => {
                        deep.contains(&abs)
                            || nodes.contains(abs.as_str())
                            || slot_endpoints.contains(abs.as_str())
                    }
                }
            };
            if !known(from) {
                violations.push((
                    MutationError::EdgeSchema(format!("from='{from}' unknown")),
                    Some(from.to_string()),
                ));
            }
            if !known(to) {
                violations.push((
                    MutationError::EdgeSchema(format!("to='{to}' unknown")),
                    Some(to.to_string()),
                ));
            }
            // Phase 13.5-A1 T4 (Slice 3): CEL parse-validate for condition +
            // modifier.set_context.* / set_hop.*. Parse-fail → MutationError::
            // EdgeSchema (error_code "edge_schema", per spec § Mutation-Format
            // Z.263).
            if let Some(cond_str) = e.get("condition").and_then(|v| v.as_str())
                && let Err(p) = crate::cel_eval::parse_condition(cond_str)
            {
                violations.push((
                    MutationError::EdgeSchema(format!("add_edges[].condition invalid cel: {p}")),
                    Some(format!("add_edges[{i}]")),
                ));
            }
            // GH #283 — the fifth top-level edge key, and the only one this
            // loop reads for its TYPE rather than its content. The loop rejects
            // unknown `modifier` keys but has never rejected unknown TOP-LEVEL
            // ones, so before this read a `"default": true` was accepted and
            // then dropped on the floor: the apply arm built its candidate with
            // a literal `is_default: false`, and the caller got `Committed` for
            // an edge that is spelled the way the template DSL spells it and
            // routes as an ordinary always-edge — beside the regular edges
            // instead of behind them.
            //
            // Absent = `false` (an edge is regular unless it says otherwise),
            // any non-boolean = `edge_schema`. That is the code the neighbouring
            // edge checks already carry, so this adds no new `error_code` and no
            // documentation obligation on the README stability surface.
            match e.get("default") {
                None => {}
                Some(v) if v.as_bool().is_some() => {}
                Some(_) => {
                    violations.push((
                        MutationError::EdgeSchema("add_edges[].default must be boolean".into()),
                        Some(format!("add_edges[{i}]")),
                    ));
                }
            }
            // GH #559: `lane` is the v-lane declaration, and it names a lane.
            // Same discipline and same code as `default` above: absent is an
            // ordinary edge, anything but a non-empty string is `edge_schema`.
            // An empty string would be a lane nothing can ever declare, which
            // is GH #196's silence in a new costume.
            match e.get("lane") {
                None => {}
                Some(v) if v.as_str().is_some_and(|s| !s.is_empty()) => {}
                Some(_) => {
                    violations.push((
                        MutationError::EdgeSchema(
                            "add_edges[].lane must be a non-empty string".into(),
                        ),
                        Some(format!("add_edges[{i}]")),
                    ));
                }
            }
            // GH #283 (ruling Q1 2026-08-21): one advisory per UNGUARDED
            // default, in the SAME words `bootstrap.rs` puts into
            // `BootstrapPlan::advisories` — the declaration paths must not
            // describe the same topology in two different sentences. It is a
            // hint and nothing more: the mutation commits, and no channel of a
            // mutation carries findings, so `warn` is the whole of it.
            if e.get("default").and_then(|v| v.as_bool()) == Some(true)
                && e.get("condition").is_none()
            {
                tracing::warn!(
                    "edge {} -> {} is an unguarded default: it consumes everything that would \
                     otherwise dead-letter as no_route from {}; a condition narrows it to the \
                     traffic you mean",
                    from,
                    to,
                    from
                );
            }
            if let Some(modif) = e.get("modifier") {
                // Befund-6-Folge: the modifier must match the
                // `{set_context, delete_context, set_hop, delete_hop}` schema
                // (spec § Validation l.277). Reject any unknown key — the old
                // flat `{"headers.X": ...}` map form was previously ignored at
                // apply and committed silently (builder foot-gun).
                if let Some(modif_obj) = modif.as_object() {
                    for k in modif_obj.keys() {
                        if !matches!(
                            k.as_str(),
                            "set_context"
                                | "delete_context"
                                | "set_hop"
                                | "delete_hop"
                                | "restore_ttl"
                        ) {
                            violations.push((
                                MutationError::EdgeSchema(format!(
                                    "add_edges[].modifier unknown key '{k}' (valid: set_context, \
                                     delete_context, set_hop, delete_hop, restore_ttl)"
                                )),
                                Some(format!("add_edges[{i}]")),
                            ));
                        }
                    }
                } else {
                    violations.push((
                        MutationError::EdgeSchema("add_edges[].modifier must be an object".into()),
                        Some(format!("add_edges[{i}]")),
                    ));
                }
                for set_key in ["set_context", "set_hop"] {
                    if let Some(set_obj) = modif.get(set_key).and_then(|v| v.as_object()) {
                        for (k, v) in set_obj {
                            let Some(expr_str) = v.as_str() else {
                                violations.push((
                                    MutationError::EdgeSchema(format!(
                                        "add_edges[].modifier.{set_key}.{k} must be string"
                                    )),
                                    Some(format!("add_edges[{i}]")),
                                ));
                                continue;
                            };
                            if let Err(p) = crate::cel_eval::parse_condition(expr_str) {
                                violations.push((
                                    MutationError::EdgeSchema(format!(
                                        "add_edges[].modifier.{set_key}.{k} invalid cel: {p}"
                                    )),
                                    Some(format!("add_edges[{i}]")),
                                ));
                            }
                        }
                    }
                }
                for del_key in ["delete_context", "delete_hop"] {
                    if let Some(del) = modif.get(del_key)
                        && del.as_array().is_none()
                    {
                        violations.push((
                            MutationError::EdgeSchema(format!(
                                "add_edges[].modifier.{del_key} must be array"
                            )),
                            Some(format!("add_edges[{i}]")),
                        ));
                    }
                }
                // GH #82 (ruling 2026-08-13): `restore_ttl` is a boolean
                // declaration, not an expression — and a restoring edge opts its
                // cycle out of the TTL loop guard, so it must carry a bound of its
                // own. The mutation path enforces the same minimum as config load
                // (`BootstrapError::EdgeTtlRestoreUnconditional`): a restoring edge
                // without a `condition` is rejected.
                if let Some(rt) = modif.get("restore_ttl") {
                    let Some(rt) = rt.as_bool() else {
                        violations.push((
                            MutationError::EdgeSchema(
                                "add_edges[].modifier.restore_ttl must be boolean".into(),
                            ),
                            Some(format!("add_edges[{i}]")),
                        ));
                        continue;
                    };
                    if rt && e.get("condition").and_then(|v| v.as_str()).is_none() {
                        violations.push((
                            MutationError::EdgeSchema(format!(
                                "add_edges[] {from}->{to}: modifier.restore_ttl needs a condition \
                                 — a ttl-restoring edge is exempt from the TTL loop guard, so it \
                                 must be bounded by its own iteration condition (e.g. \
                                 \"int(context.iter) < 12\")"
                            )),
                            Some(format!("add_edges[{i}]")),
                        ));
                    }
                }
            }
        }
    }
    // Finding 2 — NO general cycle reject. Spec overview § Validation:
    // "cycle-freedom … insofar as the application forbids cycles; meclaw-core
    // does not reject cycles in general". Tool/reply loops are legitimately
    // instantiable per mutation (and boot fine from the filesystem); the
    // runtime TTL loop-guard bounds any traversal cycle. Edge endpoints were
    // verified above (node-set membership); the topological cycle gate is gone.
    //
    // GH #293: if it ever came back it would be appended HERE, after the loop —
    // one entry at most, and after every endpoint entry, because a cycle is a
    // property of the whole edge set rather than of any single edge.
    violations
}

/// GH #293 — stage 5 ([`Stage::EdgeEndpoints`]) as a COLLECTING check: every
/// endpoint of every `add_edges` entry that no post-state node answers to, plus
/// every malformed edge entry, in one refusal.
///
/// **This changes no verdict.** The body is the collecting core the `Result`
/// form now calls ([`addressed_edges_and_cycle`]), so the messages are the same
/// strings and the first entry is the error the single-violation path returned.
///
/// Same parameters as [`validate_edges_and_cycle`], except that the diff arrives
/// as the whole [`JsonValue`] — a diff that is not an object is a stage-1
/// (`diff_schema`) matter and contributes nothing here.
///
/// [`Stage::EdgeEndpoints`]: crate::mutation::rejection::Stage::EdgeEndpoints
#[allow(clippy::too_many_arguments)]
pub fn collect_edge_endpoints(
    diff: &JsonValue,
    registry_names: &[String],
    existing_edges: &[(String, String)],
    hive_endpoint_names: &[String],
    subtree_node_endpoints: &[String],
    subtree_internal_edges: &[(String, String)],
    scope: &str,
    deep_endpoint_paths: &[String],
    slot_endpoints: &std::collections::HashSet<String>,
    into: &mut crate::mutation::rejection::MutationRejection,
) {
    use crate::mutation::rejection::{Stage, Violation};

    let Some(obj) = diff.as_object() else {
        return;
    };
    for (error, address) in addressed_edges_and_cycle(
        obj,
        registry_names,
        existing_edges,
        hive_endpoint_names,
        subtree_node_endpoints,
        subtree_internal_edges,
        scope,
        deep_endpoint_paths,
        slot_endpoints,
    ) {
        into.push(Violation::from_error(Stage::EdgeEndpoints, &error, address));
    }
}

/// Phase-11 T14 — additive validation for template-based mutations.
///
/// Ebene 0: schema, naming-collision, match-no-hit (sans factory-check for add_nodes).
/// Ebene 1: every `add_nodes[].template` must resolve in `templates`.
/// Ebene 2: the resolved template's `cell.type` (via `template_to_cell_type`) must be in `factories`.
/// Edge-Schema + Cycle: delegated to the inner helper.
///
/// `hive_endpoint_names`: hive short-names accepted as edge endpoints
/// (Cell ∪ Hive symmetry, Phase 13.5 step-6). COLONY-GLOBAL by design — `add_edges`
/// endpoints are scope-relative and a hive defining a transit scope may legitimately
/// be referenced from anywhere.
///
/// `hive_match_names` (Paket-5 T4, P10b companion): SCOPE-FILTERED hive short-names
/// (parent path == guard_scope), used ONLY for the `swap_nodes` `match.name` existence
/// check. Distinct from `hive_endpoint_names`: a `match.name` is scope-bound (spec
/// Z.265), so a hive in a foreign scope must not satisfy it (validate-side mirror of
/// the scope-correct apply-side `resolve_scoped_path`).
///
/// `subtree_node_endpoints` / `subtree_internal_edges` (Phase 13.5 a5-subtree
/// T8b-2): a SUBTREE `add_nodes` contributes its nested cell + hive paths as valid
/// edge endpoints AND its resolved internal edges, so the subtree's internal graph
/// participates in the endpoint-existence + cycle check up front (before staging).
/// Empty for single-cell mutations → byte-behavior-unchanged. Same absolute-path
/// representation as [`validate_post_state_with_edges_and_subtree`].
#[allow(clippy::too_many_arguments)]
pub fn validate_post_state_with_templates(
    diff: &JsonValue,
    templates: &crate::templates::TemplatesRegistry,
    factories: &CellFactoryRegistry,
    registry_names: &[String],
    existing_edges: &[(String, String)],
    template_to_cell_type: &[(String, String)],
    hive_endpoint_names: &[String],
    hive_match_names: &[String],
    subtree_node_endpoints: &[String],
    subtree_internal_edges: &[(String, String)],
) -> Result<(), MutationError> {
    // Thin wrapper (same pattern as `validate_post_state_with_edges`):
    // root scope + no pre-state depth paths — byte-behavior-unchanged for
    // callers without R12 depth-endpoint needs.
    validate_post_state_with_templates_scoped(
        diff,
        templates,
        factories,
        registry_names,
        existing_edges,
        template_to_cell_type,
        hive_endpoint_names,
        hive_match_names,
        subtree_node_endpoints,
        subtree_internal_edges,
        "/",
        &[],
        &[],
        &[],
        // GH #285: no hive declarations in hand at this face — no slots.
        &std::collections::HashSet::new(),
    )
}

/// R12 — full form of [`validate_post_state_with_templates`] with depth-endpoint
/// resolution: `scope` is the mutation's guard scope, `deep_endpoint_paths` the
/// absolute paths of ALL pre-state nodes (registry ∪ hive scopes, any depth).
/// A multi-segment `add_edges` endpoint (`./unit/dispatch`) resolves against
/// `scope` and is membership-tested in the absolute-path namespace
/// (`deep_endpoint_paths` ∪ `subtree_node_endpoints`) — spec Z.227 declares
/// edge paths scope-relative WITHOUT a depth restriction. Containment is not
/// relaxed: `validate_scope_containment` runs before this (caller order) and
/// rejects `..`/absolute endpoints as `scope_out_of_bounds`.
///
/// GH #179 — `deep_registry_paths` / `deep_hive_paths` do the same for the NODE
/// IDENTITY checks (`add_nodes[].name`, `remove_nodes`/`swap_nodes` `match.name`,
/// `swap_nodes[].with.name`): the absolute-path twins of `registry_names` /
/// `hive_match_names`, so a multi-segment name is compared against the paths
/// that exist instead of against a set of short names it can never equal.
/// `deep_registry_paths` MUST come resume-filtered from the caller (see
/// `validate_naming_and_match`). Distinct from `deep_endpoint_paths`, which is
/// the union of both and NOT resume-filtered — an edge may address a node that
/// is being resumed, an instantiation may not collide with it.
///
/// GH #285 — `slot_endpoints` is the absolute addresses this colony's hives
/// declared as SLOTS. It reaches the edge half only, and it is the one endpoint
/// term that is not a node: a hive said the address exists and may stand empty.
#[allow(clippy::too_many_arguments)]
pub fn validate_post_state_with_templates_scoped(
    diff: &JsonValue,
    templates: &crate::templates::TemplatesRegistry,
    factories: &CellFactoryRegistry,
    registry_names: &[String],
    existing_edges: &[(String, String)],
    template_to_cell_type: &[(String, String)],
    hive_endpoint_names: &[String],
    hive_match_names: &[String],
    subtree_node_endpoints: &[String],
    subtree_internal_edges: &[(String, String)],
    scope: &str,
    deep_endpoint_paths: &[String],
    deep_registry_paths: &[String],
    deep_hive_paths: &[String],
    slot_endpoints: &std::collections::HashSet<String>,
) -> Result<(), MutationError> {
    // Ebene 0a: diff must be an object.
    let obj = diff
        .as_object()
        .ok_or_else(|| MutationError::Schema("diff is not an object".into()))?;

    // Ebene 0b: naming-collision + match-no-hit (no factory-check here).
    // Pass the SCOPE-FILTERED `hive_match_names` so a swap_nodes match.name can resolve
    // against a hive in pre-state ONLY within the mutation scope (spec Z.265) — a hive
    // in a foreign scope must not produce a short-name false-positive (Paket-5 T4).
    validate_naming_and_match(
        obj,
        registry_names,
        hive_match_names,
        scope,
        deep_registry_paths,
        deep_hive_paths,
    )?;

    // Build lookup map for template → cell_type.
    let ct_map: std::collections::HashMap<&str, &str> = template_to_cell_type
        .iter()
        .map(|(t, c)| (t.as_str(), c.as_str()))
        .collect();

    if let Some(adds) = obj.get("add_nodes").and_then(|v| v.as_array()) {
        for (i, n) in adds.iter().enumerate() {
            // GH #437: the birth state is a closed vocabulary and it is grammar,
            // so it is refused PRE-DESTRUCTIVELY — before the adopt branch and
            // before any template lookup, because an `adopt` entry instantiates
            // a cell at an address too and that instantiation has an activity
            // like any other.
            crate::mutation::Birth::parse(n, &format!("add_nodes[{i}]"))?;
            // A5b 2b (Phase-16 W1b, Ruling 2026-06-12): an `adopt` entry
            // instantiates from an EXISTING on-disk node, not a template. Grammar
            // (pure, here): `adopt` is an object declaring the expected identity
            // with a mandatory `type`; `template` is mutually exclusive; a bare
            // `adopt: true` / an `adopt` without `type` is a `schema` reject (NO
            // blind adoption — ruling 2026-06-12). The FS/registry-dependent
            // checks (path exists, unregistered, on-disk type/version match) live
            // in `colony::handle_mutation` Step 1a. Skip the template-existence /
            // factory checks below for an adopt entry.
            if n.get("adopt").is_some() {
                if let Some(error) = adopt_grammar(n) {
                    return Err(error);
                }
                continue;
            }
            let template = n
                .get("template")
                .and_then(|v| v.as_str())
                .ok_or_else(|| MutationError::Schema("add_nodes[].template missing".into()))?;

            // Ebene 1: template must exist in templates registry.
            let entry = templates
                .resolve(template)
                .map_err(|_| MutationError::TemplateMissing(template.into()))?;

            // Ebene 1b/2 — `override_params` addressing and cell type. Shared
            // with [`collect_post_state_addresses`] so the two cannot drift
            // (GH #293): the collecting core decides, this call site takes its
            // first answer, which is the one this loop returned before.
            let mut errors = Vec::new();
            collect_add_node_addresses(
                entry,
                n,
                template,
                templates,
                factories,
                &ct_map,
                &mut errors,
            );
            if let Some(error) = errors.into_iter().next() {
                return Err(error);
            }
        }
    }

    // swap_nodes: validate `with` per new re-dedicated shape (paket-2 T1).
    // T5 Part 1: collect add_names (scope-bound, from add_nodes in this same diff)
    // so that `with.name` (existing form) can forward-reference a node being added
    // in the same composite diff. The post-state set = registry_names ∪ add_names.
    // Both spellings and the GH #199 canonicalisation live in `add_name_claims`.
    let (add_names, add_paths) = add_name_claims(obj, scope);
    if let Some(swaps) = obj.get("swap_nodes").and_then(|v| v.as_array()) {
        for s in swaps {
            let match_name = s
                .get("match")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| MutationError::Schema("swap_nodes[].match.name missing".into()))?;
            let with_val = s
                .get("with")
                .ok_or_else(|| MutationError::Schema("swap_nodes[].with missing".into()))?;
            validate_swap_with_entry_full(
                with_val,
                match_name,
                registry_names,
                &add_names,
                templates,
                scope,
                deep_registry_paths,
                &add_paths,
                &ct_map,
            )?;
        }
    }

    // Edge-Schema + Cycle. Subtree contributions (T8b-2) flow through so a
    // subtree's internal graph participates in the endpoint + cycle check;
    // scope + pre-state depth paths enable R12 depth-endpoint resolution.
    validate_edges_and_cycle(
        obj,
        registry_names,
        existing_edges,
        hive_endpoint_names,
        subtree_node_endpoints,
        subtree_internal_edges,
        scope,
        deep_endpoint_paths,
        slot_endpoints,
    )?;
    Ok(())
}

/// The pure `adopt` grammar of one `add_nodes` entry, or `None` when it holds
/// (A5b 2b, ruling 2026-06-12).
///
/// `adopt` is an object declaring the expected identity with a mandatory
/// non-empty `type`, and `template` is mutually exclusive with it. A bare
/// `adopt: true` or an `adopt` without `type` is a `schema` reject: no blind
/// adoption. The FS/registry-dependent half (the path exists, is unregistered,
/// and carries the declared identity) lives in `colony::handle_mutation`
/// Step 1a.
///
/// Called by the `Result` form and by [`collect_post_state_addresses`] alike
/// (GH #293, W3 T21) — the collecting stage 4 must refuse exactly what the
/// sequential one refused, and a second copy of three message strings is how
/// that stops being true.
fn adopt_grammar(n: &JsonValue) -> Option<MutationError> {
    let adopt = n.get("adopt")?;
    if n.get("template").is_some() {
        return Some(MutationError::Schema(
            "add_nodes[].adopt and .template are mutually exclusive".into(),
        ));
    }
    let Some(adopt_obj) = adopt.as_object() else {
        return Some(MutationError::Schema(
            "add_nodes[].adopt must be an object declaring the expected `type` \
             (no blind adoption)"
                .into(),
        ));
    };
    if adopt_obj
        .get("type")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .is_none()
    {
        return Some(MutationError::Schema(
            "add_nodes[].adopt.type (non-empty string) is required — no blind adoption".into(),
        ));
    }
    None
}

/// The names this diff's own `add_nodes` claim, in both spellings: the
/// short-name namespace (`.0`, deep names kept resolved) and the absolute-path
/// one (`.1`).
///
/// A `swap_nodes[].with.name` may forward-reference a node the same diff adds
/// (T5 Part 1), so the post-state set a `with` is measured against is
/// `registry_names ∪ add_names`. Extracted so the `Result` form and
/// [`collect_post_state_addresses`] build the same set (GH #293, W3 T21).
///
/// GH #199 / GH #179: canonicalised through [`scoped_name`], not collected as
/// written — `name_is_taken` strips the canonical `./` prefix before comparing,
/// so a raw `./successor` sat in a set that is only ever queried with
/// `successor`.
fn add_name_claims(
    obj: &meclaw_core::serde_json::Map<String, JsonValue>,
    scope: &str,
) -> (Vec<String>, Vec<String>) {
    let add_names: Vec<String> = obj
        .get("add_nodes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|n| n.get("name").and_then(|v| v.as_str()))
                .map(|name| match scoped_name(scope, name) {
                    ScopedName::Short(s) => s.to_string(),
                    // A deep name is never queried in the short namespace; it is
                    // answered from `add_paths` below. Keeping the resolved form
                    // here rather than the raw one means no entry of this set is
                    // a spelling.
                    ScopedName::Deep(abs) => abs,
                })
                .collect()
        })
        .unwrap_or_default();
    // GH #179: the same forward-reference set spelled as absolute paths, for a
    // multi-segment `with.name` (see `name_is_taken`). This half was always
    // correct — `resolve_scoped_path` normalises the prefix — which is exactly
    // why the defect was depth-invisible and short-name-only.
    let add_paths: Vec<String> = add_names
        .iter()
        .map(|n| {
            crate::mutation::resolve_scoped_path(scope, n)
                .as_str()
                .to_string()
        })
        .collect();
    (add_names, add_paths)
}

/// The post-state ADDRESS half of one `add_nodes` entry, collecting: which
/// cells and params its `override_params` addresses, and whether the colony has
/// a factory for its cell type.
///
/// GH #140 (supersedes the R10 blanket reject of 2026-06-11): on a SUBTREE
/// template, `override_params` is ADDRESSED — its keys are the cells' paths
/// inside the template, `""` being the subtree root. R10's complaint was that
/// the flat form committed as a silent no-op; addressing removes the cause
/// rather than the feature. What R10 protected is kept exactly: a key that names
/// no cell is refused pre-destructively and told what the template actually
/// contains, so nothing can be "set" into the void again.
///
/// GH #294 (ruling Q6, 2026-08-21) adds the PARAM half one nesting level down:
/// an addressed key that names no param of the cell it reached is refused the
/// same way, for the same reason — a typo inside the entry committed and the
/// cell spawned with its default. Both forms go through
/// [`crate::mutation::subtree::check_override_params`]: the ADDRESSED form of a
/// subtree template and the FLAT form of a single-cell template, whose merge
/// lives in `stage::patch_and_substitute_config`. Putting the flat form's check
/// HERE rather than in staging is what keeps the two from drifting apart.
///
/// GH #293 — collecting, and for the same reason the checks above live in one
/// place: [`validate_post_state_with_templates_scoped`] takes the first pushed
/// error (byte-identical verdict, since that is the one its loop returned) and
/// [`collect_post_state_addresses`] takes all of them. A diff whose override
/// misspells four keys names four keys.
fn collect_add_node_addresses(
    entry: &crate::templates::TemplateEntry,
    n: &JsonValue,
    template: &str,
    templates: &crate::templates::TemplatesRegistry,
    factories: &CellFactoryRegistry,
    ct_map: &std::collections::HashMap<&str, &str>,
    out: &mut Vec<MutationError>,
) {
    if let Some(over) = n.get("override_params") {
        match crate::mutation::subtree::parse_subtree(&entry.filesystem_path, templates) {
            // A template that does not parse is a stage-2 matter
            // ([`collect_template_resolution`]); it is reported here too because
            // the `Result` form raised it here and the first-error identity must
            // hold. The pipeline stops at stage 2 long before stage 4 sees it.
            Err(error) => out.push(error),
            Ok(parsed) => {
                if parsed.cells.len() > 1 {
                    match over.as_object() {
                        None => out.push(MutationError::Schema(format!(
                            "override_params on the subtree template '{template}' must be an \
                             object keyed by the cells' paths inside the template (\"\" is the \
                             subtree root)"
                        ))),
                        Some(obj) => {
                            let known: Vec<&str> =
                                parsed.cells.iter().map(|c| c.rel_path.as_str()).collect();
                            for (key, params) in obj {
                                let Some(cell) = parsed.cells.iter().find(|c| &c.rel_path == key)
                                else {
                                    out.push(MutationError::Schema(format!(
                                        "override_params['{key}'] names no cell of the subtree \
                                         template '{template}'. Its cells are: {}",
                                        crate::mutation::subtree::render_cell_list(&known)
                                    )));
                                    continue;
                                };
                                if !params.is_object() {
                                    out.push(MutationError::Schema(format!(
                                        "override_params['{key}'] must be a params object"
                                    )));
                                    continue;
                                }
                                if let Err(error) = crate::mutation::subtree::check_override_params(
                                    cell,
                                    Some(key),
                                    template,
                                    params,
                                ) {
                                    out.push(error);
                                }
                            }
                        }
                    }
                } else if let Some(cell) = parsed.cells.first() {
                    // GH #436: the two notations of `override_params` look alike
                    // and the wrong one used to be refused for the wrong reason.
                    // On a single-cell template the object IS the params — a
                    // `""` key is the path-keyed form, which belongs to a ref
                    // marker or a subtree, and asking about a param called ""
                    // answers a question nobody asked. Checked HERE and not in
                    // `check_override_params`, because only the caller knows how
                    // many cells the template has.
                    if let Some(nested) = over.get("").and_then(|v| v.as_object()) {
                        out.push(MutationError::Schema(format!(
                            "override_params[''] on '{template}': this is a single-cell \
                             template — override_params is a flat params object here \
                             ({{\"{}\": …}}), not keyed by the paths of cells inside a \
                             template. The path-keyed form ({{\"\": …}}) applies to a ref \
                             marker and to a subtree template, and this template has \
                             nothing to address.",
                            nested.keys().next().map(|k| k.as_str()).unwrap_or("param"),
                        )));
                    } else if let Err(error) =
                        crate::mutation::subtree::check_override_params(cell, None, template, over)
                    {
                        out.push(error);
                    }
                }
            }
        }
    }

    // Ebene 2: cell.type for resolved template must be in factories.
    // Use entry.name (resolved name, e.g. "echo") not raw template string
    // (e.g. "echo@1.0.0") as ct_map key — fixes R3 versioned-ref mismatch.
    let Some(cell_type) = ct_map.get(entry.name.as_str()) else {
        // Stage-2-shaped (`template_missing`) but raised here because this is
        // where the `Result` form raises it; unreachable through the pipeline,
        // which stops at stage 2 whenever a reference does not resolve.
        out.push(MutationError::TemplateMissing(template.into()));
        return;
    };
    // Phase-13.5 a5-subtree T8b-1: a SUBTREE template's ROOT cell.type is
    // `hive` — a scope marker, never an actor, so it has NO factory by
    // design (CONTRIBUTING.md: "a hive is not an actor"). Skip the level-2
    // factory check for a hive root; the spawnable nested cells are
    // staged + registered by `stage_subtree` (their own cell-types are
    // validated by bootstrap-side factory presence at spawn time).
    if *cell_type != "hive" && !factories.contains_key(*cell_type) {
        out.push(MutationError::UnknownCellType((*cell_type).into()));
    }
    // GH #572: the exception above is for a SUBTREE root. A hive-rooted
    // template with nothing under it is not a subtree — `parse_subtree` says
    // one cell, so the staging door takes the single-cell path and the apply
    // arm looks up a factory for `hive` that by design does not exist. Same
    // fact, same code, the other door.
    if let Err(error) = reject_if_single_cell_hive_template(entry, template, templates, ct_map) {
        out.push(error);
    }
}

/// GH #293 — stage 4 ([`Stage::PostStateAddresses`]) as a COLLECTING check:
/// every address the post-state would carry that does not hold up, in one
/// refusal.
///
/// Everything [`validate_post_state_with_templates_scoped`] decides except its
/// edge half (that is stage 5, [`collect_edge_endpoints`]), walked in that same
/// order:
///
/// 1. naming collisions — a diff claiming a path twice, or a name the pre-state
///    already holds,
/// 2. a `remove_nodes` / `swap_nodes` `match` that hits nothing,
/// 3. the `adopt` grammar of an `add_nodes` entry ([`adopt_grammar`]),
/// 4. an `add_nodes` entry with neither `adopt` nor `template`,
/// 5. an `override_params` key addressing no cell of the template, or no param
///    of the cell it reached (GH #140 + GH #294),
/// 6. a cell type no factory serves, and
/// 7. the shape and the installed name of a `swap_nodes[].with`
///    ([`validate_swap_with_entry_full`]).
///
/// **This changes no verdict.** The shared halves are the collecting cores the
/// `Result` form now calls ([`addressed_naming_and_match`],
/// [`collect_add_node_addresses`]) and the shared helpers it now uses
/// ([`adopt_grammar`], [`add_name_claims`]), so the messages are the same
/// strings and the first entry is the error the single-violation path returned.
/// Points 3, 4 and 7 are here because nothing else refuses them: dropping them
/// from the pipeline would turn refused mutations into applied ones.
///
/// An `add_nodes` entry whose template does not resolve is SKIPPED, not
/// reported: that is stage 2's verdict, and the pipeline stops there. Reporting
/// it again here is exactly the derived-error noise GH #293 exists to remove.
/// (The `Result` form raises `template_missing` at that point instead — the one
/// place where the two deliberately differ, and it is unreachable through the
/// pipeline.)
///
/// [`Stage::PostStateAddresses`]: crate::mutation::rejection::Stage::PostStateAddresses
#[allow(clippy::too_many_arguments)]
pub fn collect_post_state_addresses(
    diff: &JsonValue,
    templates: &crate::templates::TemplatesRegistry,
    factories: &CellFactoryRegistry,
    registry_names: &[String],
    template_to_cell_type: &[(String, String)],
    hive_match_names: &[String],
    scope: &str,
    deep_registry_paths: &[String],
    deep_hive_paths: &[String],
    into: &mut crate::mutation::rejection::MutationRejection,
) {
    use crate::mutation::rejection::{Stage, Violation};

    let mut refuse = |error: &MutationError, address: Option<String>| {
        into.push(Violation::from_error(
            Stage::PostStateAddresses,
            error,
            address,
        ));
    };

    let Some(obj) = diff.as_object() else {
        // A diff that is not an object is shaped like stage 1, but this is the
        // check that actually decides it — there is no earlier one, and dropping
        // it here would turn a refused mutation into an applied one. Kept where
        // the verdict is made rather than moved to a stage that does not run it.
        refuse(&MutationError::Schema("diff is not an object".into()), None);
        return;
    };
    for (error, address) in addressed_naming_and_match(
        obj,
        registry_names,
        hive_match_names,
        scope,
        deep_registry_paths,
        deep_hive_paths,
    ) {
        refuse(&error, address);
    }

    let ct_map: std::collections::HashMap<&str, &str> = template_to_cell_type
        .iter()
        .map(|(t, c)| (t.as_str(), c.as_str()))
        .collect();
    if let Some(adds) = obj.get("add_nodes").and_then(|v| v.as_array()) {
        for (i, n) in adds.iter().enumerate() {
            // The address is the node this entry puts at a path — the one thing
            // its checks are about. The message names the key or the type.
            let address = n
                .get("name")
                .and_then(|v| v.as_str())
                .or_else(|| n.get("template").and_then(|v| v.as_str()))
                .map(str::to_string)
                .unwrap_or_else(|| format!("add_nodes[{i}]"));
            // GH #437: the collecting face asks the SAME grammar as the
            // `Result` face above — one question, two answers, never two
            // questions (GH #293).
            if let Err(error) = crate::mutation::Birth::parse(n, &format!("add_nodes[{i}]")) {
                refuse(&error, Some(address.clone()));
            }
            if n.get("adopt").is_some() {
                if let Some(error) = adopt_grammar(n) {
                    refuse(&error, Some(address));
                }
                continue;
            }
            let Some(template) = n.get("template").and_then(|v| v.as_str()) else {
                refuse(
                    &MutationError::Schema("add_nodes[].template missing".into()),
                    Some(address),
                );
                continue;
            };
            let Ok(entry) = templates.resolve(template) else {
                // Stage 2's verdict ([`collect_template_resolution`]), and the
                // pipeline stops there — reporting it again here is exactly the
                // derived-error noise GH #293 exists to remove. The `Result`
                // form raises `template_missing` at this point instead; it never
                // reaches stage 4 through the pipeline.
                continue;
            };
            let mut errors = Vec::new();
            collect_add_node_addresses(
                entry,
                n,
                template,
                templates,
                factories,
                &ct_map,
                &mut errors,
            );
            for error in errors {
                refuse(&error, Some(address.clone()));
            }
        }
    }

    // `swap_nodes[].with`: the shape of the replacement, and whether the name it
    // installs is free in the post-state. Its three refusals
    // (`NamingCollision` / `MatchNoHit` / `TemplateMissing`, plus the two
    // `Schema` shapes above them) are stage-4 addresses like every other, and
    // they are checked here for the reason the collector exists: a diff that
    // swaps four nodes onto four taken names should say so once.
    let (add_names, add_paths) = add_name_claims(obj, scope);
    if let Some(swaps) = obj.get("swap_nodes").and_then(|v| v.as_array()) {
        for (i, s) in swaps.iter().enumerate() {
            let address = format!("swap_nodes[{i}]");
            let Some(match_name) = s
                .get("match")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
            else {
                refuse(
                    &MutationError::Schema("swap_nodes[].match.name missing".into()),
                    Some(address),
                );
                continue;
            };
            let Some(with_val) = s.get("with") else {
                refuse(
                    &MutationError::Schema("swap_nodes[].with missing".into()),
                    Some(match_name.to_string()),
                );
                continue;
            };
            if let Err(error) = validate_swap_with_entry_full(
                with_val,
                match_name,
                registry_names,
                &add_names,
                templates,
                scope,
                deep_registry_paths,
                &add_paths,
                &ct_map,
            ) {
                refuse(&error, Some(match_name.to_string()));
            }
        }
    }
}

/// GH #293 — stage 2 ([`Stage::TemplateResolution`]) as a COLLECTING check:
/// EVERY `add_nodes` entry whose template reference does not resolve is named,
/// not only the first one.
///
/// **This changes no verdict.** It is additive: the `Result` forms above keep
/// their signatures and answer exactly what they answered before, and the first
/// violation pushed here is the one they returned. What changes is the report —
/// a diff naming three unresolvable templates costs one round trip instead of
/// three.
///
/// Four failure shapes land in this stage, and the last two are why the check
/// has to walk the template rather than merely look it up:
///
/// - the reference names no template at all ([`MutationError::TemplateMissing`],
///   the same payload — the raw reference string — the single-violation path
///   builds),
/// - the reference names a malformed `@<version>` (same variant, same payload),
/// - a `cell.type: "ref"` sub-unit points at a template the registry does not
///   hold, and
/// - the refs close a ring ([`MutationError::TemplateRefCycle`]).
///
/// The last two only exist once the subtree is parsed, so
/// [`crate::mutation::subtree::parse_subtree`] runs here for every entry — the
/// same call, hence the same messages, as the `override_params` path in
/// [`validate_post_state_with_templates_scoped`], which reaches them only when
/// an override happens to be present.
///
/// An `adopt` entry instantiates from an existing on-disk node and names no
/// template, and an entry with no `template` key is a stage-1 (`diff_schema`)
/// matter; both are skipped here rather than reported twice.
///
/// # Why a Resume is NOT exempt here, unlike at stage 3
///
/// [`validate_requires`] takes a `resumed_names` list and skips those entries,
/// because a Reconnect instantiates nothing and would otherwise be refused for
/// a contract it never consumes — a real accept→refuse flip (Task 15). The
/// mirror-image exemption does not belong here, and the reason is one line of
/// `stage.rs`: `build_staging_tree_from_templates` calls
/// [`crate::mutation::subtree::parse_subtree`] with a `?` **before** its
/// single-cell existence-skip (the subtree-dispatch, deliberately ahead of the
/// skip so a partially-existing subtree still merge-stages). A Resume onto a
/// template with a broken `ref` was therefore always refused with the same
/// `template_missing` — at staging rather than at validation. There is no
/// acceptance to preserve; exempting the walk here would only move that refusal
/// back to where it costs a `.staging` directory and a `failed` audit row
/// instead of a clean pre-destructive `rejected` one (GH #276's whole
/// direction). Pinned by
/// `a_broken_ref_is_refused_for_a_resume_and_a_fresh_instantiation_alike`.
///
/// [`Stage::TemplateResolution`]: crate::mutation::rejection::Stage::TemplateResolution
pub fn collect_template_resolution(
    diff: &JsonValue,
    templates: &crate::templates::TemplatesRegistry,
    into: &mut crate::mutation::rejection::MutationRejection,
) {
    use crate::mutation::rejection::{Stage, Violation};

    let Some(adds) = diff
        .as_object()
        .and_then(|obj| obj.get("add_nodes"))
        .and_then(|v| v.as_array())
    else {
        return;
    };
    for n in adds {
        if n.get("adopt").is_some() {
            continue;
        }
        let Some(template) = n.get("template").and_then(|v| v.as_str()) else {
            continue;
        };
        let mut refuse = |error: &MutationError| {
            into.push(Violation::from_error(
                Stage::TemplateResolution,
                error,
                Some(template.to_string()),
            ));
        };
        let Ok(entry) = templates.resolve(template) else {
            refuse(&MutationError::TemplateMissing(template.into()));
            continue;
        };
        if let Err(error) =
            crate::mutation::subtree::parse_subtree(&entry.filesystem_path, templates)
        {
            refuse(&error);
        }
    }
}

/// GH #572 (ruling O-0904-1) — may this template be instantiated at an address?
///
/// A hive is a scope marker in the filesystem, not an actor: it owns no task,
/// no mailbox and no `cell.db`, and there is no `CellFactory` registered for
/// the type `"hive"` — by design. Every door that stages a SINGLE cell ends at
/// that fact, and until this predicate it ended there LATE: the diff validated
/// clean, a directory was staged, and the apply arm answered
/// `spawn: factory missing for hive` — an unnamed refusal from the half of the
/// mutation that is past deciding.
///
/// A hive with cells under it is a different thing entirely: the subtree door
/// stages the whole unit, registers its hive scope, and spawns the cells. So
/// the refusal is not "no hives" but "no hive ALONE" — a scope marking nothing,
/// which is why the message names the shape that works.
///
/// One predicate serves both instantiating doors (`add_nodes[].template` and
/// the instantiate form of `swap_nodes[].with`) because it is one question, and
/// two codes for one fact would be two public surfaces. Stage 4
/// ([`crate::mutation::rejection::Stage::PostStateAddresses`]): what class the
/// post-state's address would carry, decided where the door already decides
/// existence, collision and template form.
///
/// `ct_map` is keyed by the RESOLVED template name (`entry.name`), the same key
/// the level-2 factory check uses. "Alone" is measured the way the staging door
/// measures it — [`crate::mutation::subtree::parse_subtree`] with `ref` markers
/// resolved — so the predicate and the machinery it protects cannot disagree
/// about what a subtree is.
pub(crate) fn reject_if_single_cell_hive_template(
    entry: &crate::templates::TemplateEntry,
    tpl_ref: &str,
    templates: &crate::templates::TemplatesRegistry,
    ct_map: &std::collections::HashMap<&str, &str>,
) -> Result<(), MutationError> {
    if ct_map.get(entry.name.as_str()).copied() != Some("hive") {
        return Ok(());
    }
    if crate::mutation::subtree::parse_subtree(&entry.filesystem_path, templates)?
        .cells
        .len()
        > 1
    {
        return Ok(()); // a subtree: the hive enters as its root, which is the way in
    }
    Err(MutationError::HiveTemplateSingleCell(format!(
        "template '{tpl_ref}': its root is a hive and it has no cells under it, so nothing \
         about it can be spawned — a hive is a scope marker, not an actor, and no factory \
         serves the type 'hive'. A hive enters the world as the ROOT of a multi-cell subtree: \
         put the cells the scope is for into the template and grow it with an `add_nodes` \
         entry, which stages the subtree and registers its hive scope (a generation change \
         then names it in `swap_nodes[].with: {{\"name\": …}}`, GH #256). Nothing was staged."
    )))
}

/// Paket-2 T1 — Validate a `swap_nodes[].with` object.
///
/// Discriminator: presence of the `template` key.
///
/// **Instantiate form (new, with `name`):** `{"template": "<tmpl>", "name": "<t3>", "params": {...}}`
///   Unknown keys → Schema. `name` required → Schema.
///   t3 already in registry → NamingCollision.
///   Unknown `template` → TemplateMissing. Subtree template → Schema.
///
/// **Instantiate form without `name`:** `{"template": "<tmpl>"}` → Schema error.
///   The graph-swap instantiate form REQUIRES `name` (t3 needs an own name to
///   swing edges onto). The legacy no-name form was retired with the old
///   transplant apply-arm (paket-2 T4).
///
/// **Existing-node form:** `{"name": "<t3>"}` — only `name`, NO `template`.
///   Unknown keys → Schema (typo guard). t3 NOT in registry → MatchNoHit.
///
/// `registry_names` MUST be scope-filtered by the caller (colony.rs passes
/// names from paths within `guard_scope` only — A2 scope-binding).
///
/// T5 Part 1 — Full form for validating `swap_nodes[].with`; also accepts a
/// `with.name` (existing-node form) that refers to a node being ADDED in the same
/// diff via `add_nodes` (forward reference / post-state resolution).
///
/// `add_names`: short-names of nodes introduced by `add_nodes` in the same diff
/// (scope-bound). The effective post-state set for the existing-form check is:
/// `registry_names ∪ add_names`.
///
/// GH #179 — `scope`, `deep_registry_paths` and `add_paths` are the absolute-path
/// twins of `registry_names` and `add_names`; a `with.name` carrying a `/` is a
/// path and is looked up as one (see `name_is_taken`).
///
/// # Invariant
///
/// `match.name` (t2 in the swap source) is still checked against PRE-STATE only
/// (via `validate_naming_and_match` / `validate_post_state_full`). Only the
/// `with.name` TARGET gains forward-reference resolution here.
#[allow(clippy::too_many_arguments)]
fn validate_swap_with_entry_full(
    with_val: &JsonValue,
    match_name: &str,
    registry_names: &[String],
    add_names: &[String],
    templates: &crate::templates::TemplatesRegistry,
    scope: &str,
    deep_registry_paths: &[String],
    add_paths: &[String],
    ct_map: &std::collections::HashMap<&str, &str>,
) -> Result<(), MutationError> {
    let with_obj = with_val
        .as_object()
        .ok_or_else(|| MutationError::Schema("swap_nodes[].with must be an object".into()))?;

    let has_template = with_obj.contains_key("template");
    let has_name = with_obj.contains_key("name");

    if has_template && has_name {
        // ── New instantiate form: allowed keys = {template, name, params} ─
        for key in with_obj.keys() {
            if !matches!(key.as_str(), "template" | "name" | "params") {
                return Err(MutationError::Schema(format!(
                    "swap_nodes[].with unknown key '{key}' in instantiate form"
                )));
            }
        }

        // `name` is guaranteed present (has_name is true); `.as_str()` fails only
        // for a non-string value such as `{"name": 42}` — that is a real schema
        // reject, not dead code.
        let name = with_obj
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("swap_nodes[].with.name missing".into()))?;

        // Rule 4 (Strict-Regel b): target name collision. GH #179: the with-side
        // is where this check carries the most weight — staging has no
        // existence-skip here (unlike an `add_nodes` Resume), so the apply
        // reaches the live directory and overwrites its `config.json`, re-minting
        // a `cell.id` that is assigned exactly once per path. A multi-segment
        // name has to be looked up as the path it is.
        if name_is_taken(scope, name, registry_names, deep_registry_paths) {
            return Err(MutationError::NamingCollision(name.into()));
        }

        let template = with_obj
            .get("template")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("swap_nodes[].with.template missing".into()))?;

        // Rule 5a: template must exist.
        let entry = templates
            .resolve(template)
            .map_err(|_| MutationError::TemplateMissing(template.into()))?;

        // Rule 5b (A6): subtree templates not allowed.
        crate::mutation::stage::reject_if_subtree_template(&entry.filesystem_path, template)?;

        // GH #572: and what is left after that guard may still be a hive with
        // nothing under it. BEHIND the subtree reject on purpose — a template
        // whose root is a hive AND which has nested cells is refused here as
        // `schema` by the line above, and that verdict does not change.
        reject_if_single_cell_hive_template(entry, template, templates, ct_map)?;
    } else if has_template {
        // ── `template` without `name` ────────────────────────────────────
        // The instantiate form REQUIRES `name` (paket-2 graph-swap: t3 needs an
        // own name to swing edges onto). A `with` carrying `template` but no
        // `name` is a schema error (the legacy no-name transplant form was
        // retired with the old apply-arm in paket-2 T4).
        return Err(MutationError::Schema(
            "swap_nodes[].with: instantiate form requires 'name'".into(),
        ));
    } else {
        // ── Existing-node form: allowed keys = {name} ────────────────────
        for key in with_obj.keys() {
            if key.as_str() != "name" {
                return Err(MutationError::Schema(format!(
                    "swap_nodes[].with unknown key '{key}' in existing-node form"
                )));
            }
        }

        let name = with_obj
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("swap_nodes[].with.name missing".into()))?;

        // T13: identity-swap guard — with.name == match.name (same node, same scope)
        // would drop every external edge of t2 (all swung edges become t3→t3 self-loops
        // and are silently dropped). Loud reject instead of silent edge-loss.
        // GH #179: compared as PATHS — `unit/q` and `./unit/q` are the same node,
        // and the guard exists to stop exactly that node from being swapped with
        // itself.
        if crate::mutation::resolve_scoped_path(scope, name)
            == crate::mutation::resolve_scoped_path(scope, match_name)
        {
            return Err(MutationError::Schema(
                "swap_nodes[].with.name must differ from match.name".into(),
            ));
        }

        // Rule 3 (Strict-Regel a): existing target must resolve within POST-STATE scope.
        // T5 Part 1: post-state = registry_names (pre-state) ∪ add_names (nodes being
        // added in the same diff). A `with.name` that forward-references a node from
        // `add_nodes` in the same composite diff is VALID — the node will exist at
        // apply time. Cross-scope rejection still applies (a name absent from BOTH
        // sets is rejected as MatchNoHit, preserving A2 scope-binding).
        let in_pre_state = name_is_taken(scope, name, registry_names, deep_registry_paths);
        let in_add_names = name_is_taken(scope, name, add_names, add_paths);
        if !in_pre_state && !in_add_names {
            return Err(MutationError::MatchNoHit(name.into()));
        }
    }

    Ok(())
}

/// A pre-state edge reduced to the fields needed for `remove_edges` match
/// equality (Paket-5 T1/T2 / D-031).
///
/// The endpoints are ABSOLUTE logical paths (as stored on the live
/// [`crate::edge_table::Edge`]). `condition_source` mirrors
/// `edge.condition.source` (the original CEL string) and `modifier_source` is
/// the serde-JSON value of `edge.modifier.source` — exactly the two stored
/// representations the apply-time F6 match compares against
/// (`colony.rs` remove_edges arm). Keeping this view here lets validate and
/// apply share ONE equality definition (`remove_edges_pattern_hits`).
#[derive(Debug, Clone)]
pub struct EdgeMatchView {
    /// Absolute `from` path of the edge.
    pub from: String,
    /// Absolute `to` path of the edge.
    pub to: String,
    /// `edge.condition.source` (the raw CEL string), or `None` for an
    /// unconditional edge.
    pub condition_source: Option<String>,
    /// serde-JSON value of `edge.modifier.source`, or `None` for an edge
    /// without a modifier.
    pub modifier_source: Option<JsonValue>,
    /// GH #283 — the edge's routing phase (`edge.is_default`). Part of the
    /// edge's identity, so a pattern can name the default edge that sits
    /// beside a regular one with the same four other terms. Unlike the two
    /// fields above this is NOT an `Option`: every edge has a phase; the
    /// PATTERN side is what may leave it unconstrained.
    pub is_default: bool,
}

/// Builds an [`EdgeMatchView`] from a live [`crate::edge_table::Edge`] — the
/// SINGLE mapping of a stored edge into the F6 match-view, shared by both the
/// validate-time pre-check and the apply-time remove_edges arm in `colony.rs`.
/// Keeping this conversion here (next to the predicate it feeds) removes the
/// last drift risk between validate and apply: both call sites map identically.
impl From<&crate::edge_table::Edge> for EdgeMatchView {
    fn from(e: &crate::edge_table::Edge) -> Self {
        EdgeMatchView {
            from: e.from.as_str().to_string(),
            to: e.to.as_str().to_string(),
            condition_source: e.condition.as_ref().map(|c| c.source.clone()),
            modifier_source: e
                .modifier
                .as_ref()
                .and_then(|m| meclaw_core::serde_json::to_value(&m.source).ok()),
            is_default: e.is_default,
        }
    }
}

/// Shared `remove_edges` match predicate — the SINGLE source of truth for the
/// F6 equality, used by both validate-time (Paket-5 T1/T2) and apply-time
/// (`colony.rs` remove_edges arm).
///
/// An edge matches a `remove_edges[].match` pattern iff:
/// - `edge.from == from_path` AND `edge.to == to_path` (mandatory, absolute
///   paths — the caller resolves the scope-relative pattern names via
///   [`crate::mutation::resolve_scoped_path`] before calling), AND
/// - if `pat_condition` is `Some`, the edge's `condition_source` equals it
///   byte-for-byte (string equality); if `None`, condition is unconstrained,
///   AND
/// - if `pat_modifier` is `Some`, the edge's `modifier_source` JSON equals it;
///   if `None`, modifier is unconstrained, AND
/// - if `pat_default` is `Some`, the edge's `is_default` equals it; if `None`,
///   the routing phase is unconstrained and the pattern hits BOTH phases
///   (GH #283 — same convention as the two optional fields above).
pub fn remove_edges_pattern_hits(
    edge: &EdgeMatchView,
    from_path: &str,
    to_path: &str,
    pat_condition: Option<&str>,
    pat_modifier: Option<&JsonValue>,
    pat_default: Option<bool>,
) -> bool {
    if edge.from != from_path || edge.to != to_path {
        return false;
    }
    // F6: condition-source string-equality (not semantic). Pattern absent =>
    // any edge passes; pattern present => stored source must equal it exactly.
    if let Some(pc) = pat_condition
        && edge.condition_source.as_deref() != Some(pc)
    {
        return false;
    }
    // F6: modifier serde-JSON equality on the stored `ModifierSpec` source.
    if let Some(pm) = pat_modifier
        && edge.modifier_source.as_ref() != Some(pm)
    {
        return false;
    }
    // GH #283: routing-phase equality. Pattern absent => both phases pass;
    // pattern present => the edge must run in exactly that phase.
    if let Some(pd) = pat_default
        && edge.is_default != pd
    {
        return false;
    }
    true
}

/// GH #559 — the ids of the edges a diff's `remove_edges` would take away.
///
/// `existing` pairs each live edge's id with its F6 view. The patterns are read
/// with [`remove_edges_pattern_hits`], the same predicate validate and apply
/// already share, so this list is exactly the set the apply arm will remove.
///
/// It exists because "an identical edge already exists" has to mean "…and will
/// still exist afterwards". Taking the blank edge away in the SAME diff is the
/// documented way to migrate a hand-through lane onto a v-lane (R-V3), and a
/// check that counted the doomed edge would refuse precisely the diff it is
/// telling people to write.
#[must_use]
pub fn remove_edges_targets(
    diff: &JsonValue,
    scope: &str,
    existing: &[(meclaw_core::Uuid, EdgeMatchView)],
) -> std::collections::HashSet<meclaw_core::Uuid> {
    let mut out = std::collections::HashSet::new();
    let Some(removes) = diff.get("remove_edges").and_then(|v| v.as_array()) else {
        return out;
    };
    for r in removes {
        let m = r.get("match");
        let (Some(from), Some(to)) = (
            m.and_then(|v| v.get("from")).and_then(|v| v.as_str()),
            m.and_then(|v| v.get("to")).and_then(|v| v.as_str()),
        ) else {
            continue; // a malformed pattern is `validate_remove_edges`' verdict
        };
        let from_abs = crate::mutation::resolve_scoped_path(scope, from);
        let to_abs = crate::mutation::resolve_scoped_path(scope, to);
        for (id, view) in existing {
            if remove_edges_pattern_hits(
                view,
                from_abs.as_str(),
                to_abs.as_str(),
                m.and_then(|v| v.get("condition")).and_then(|v| v.as_str()),
                m.and_then(|v| v.get("modifier")),
                m.and_then(|v| v.get("default")).and_then(|v| v.as_bool()),
            ) {
                out.insert(*id);
            }
        }
    }
    out
}

/// GH #574 — the five routing terms of ONE `add_edges[]` diff entry, read
/// exactly once.
///
/// Every door that has to decide whether two edges are "the same edge to the
/// table" reads the same five terms off the same diff entry: the two endpoints
/// resolved against the mutation scope, the raw condition string, the modifier
/// spec reconstructed by [`crate::mutation::modifier_spec_from_add_entry`] and
/// serialised the way the stored edge carries it, and the routing phase. Before
/// this function each door spelled that reading out by hand, so a change to one
/// term (the phase joined the identity in GH #283) had to be made in every copy
/// or the doors quietly disagreed.
///
/// `None` means the entry has no usable `from`/`to` pair — a shape earlier
/// stages refuse — and is what the callers used to express as `continue`.
#[must_use]
pub fn add_entry_match_view(scope: &str, entry: &JsonValue) -> Option<EdgeMatchView> {
    let from = entry.get("from").and_then(|v| v.as_str())?;
    let to = entry.get("to").and_then(|v| v.as_str())?;
    Some(EdgeMatchView {
        from: crate::mutation::resolve_scoped_path(scope, from)
            .as_str()
            .to_string(),
        to: crate::mutation::resolve_scoped_path(scope, to)
            .as_str()
            .to_string(),
        condition_source: entry
            .get("condition")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string),
        modifier_source: crate::mutation::modifier_spec_from_add_entry(entry)
            .and_then(|spec| meclaw_core::serde_json::to_value(&spec).ok()),
        is_default: entry
            .get("default")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

/// GH #574 — the sentence fragment with which every lane refusal names what a
/// side declared.
///
/// Both Stage-6 lane checks build the same half-sentence, and the wording is
/// asserted verbatim by `gh559_a_v_lane_is_a_declared_deep_edge`: a caller
/// cannot see from the outside which of two identical-looking edges carries
/// which lane, so the refusal says it for both sides in the same words.
#[must_use]
pub fn lane_says(lane: Option<&str>) -> String {
    lane.map_or_else(
        || "declares no lane".to_string(),
        |l| format!("declares lane '{l}'"),
    )
}

/// GH #574 — [`edge_identity_equal`] with both sides already read into a view.
///
/// The six-argument form exists because one side is usually a live edge and the
/// other a set of loose terms. Where both sides are views — a diff entry against
/// a standing edge, or two entries of the same diff — this is the same predicate
/// without the caller having to unpack one of them term by term, which is
/// precisely where a term used to get dropped.
#[must_use]
pub fn edge_identity_equal_views(a: &EdgeMatchView, b: &EdgeMatchView) -> bool {
    edge_identity_equal(
        a,
        b.from.as_str(),
        b.to.as_str(),
        b.condition_source.as_deref(),
        b.modifier_source.as_ref(),
        b.is_default,
    )
}

/// GH #559 — the same F6 comparison with EVERY term constrained: the identity
/// [`crate::edge_table::EdgeTable::contains_equal`] dedups on, expressed on the
/// same source forms [`remove_edges_pattern_hits`] compares.
///
/// The two differ in exactly one word, and it is worth saying out loud: a
/// `remove_edges` PATTERN may leave a term unconstrained (`None` = "any"), an
/// IDENTITY may not (`None` = "this edge has no condition"). Reusing the
/// pattern predicate here would have made an unconditional entry equal to every
/// conditional edge between the same two nodes.
#[must_use]
pub fn edge_identity_equal(
    edge: &EdgeMatchView,
    from_path: &str,
    to_path: &str,
    condition: Option<&str>,
    modifier: Option<&JsonValue>,
    is_default: bool,
) -> bool {
    edge.from == from_path
        && edge.to == to_path
        && edge.condition_source.as_deref() == condition
        && edge.modifier_source.as_ref() == modifier
        && edge.is_default == is_default
}

/// Paket-5 T1/T2 (P10a / D-031) — validate-time reject for malformed or no-hit
/// `remove_edges` match patterns, reaching parity with `remove_nodes` /
/// `swap_nodes` (spec Z.272: every `remove_*` pattern must hit ≥1 element of the
/// pre_state; Z.279: the whole diff is rejected, no partial commit).
///
/// Before this check, a `remove_edges` entry was validated ONLY at apply-time,
/// where a missing `match.from`/`match.to` was silently skipped and a no-hit
/// pattern was a silent no-op. This makes both loud and PRE-destructive.
///
/// - missing `match.from` → `Schema("remove_edges[].match.from missing")`
/// - missing `match.to`   → `Schema("remove_edges[].match.to missing")`
/// - pattern that matches zero edges → `MatchNoHit(...)`
///
/// `scope` is the mutation scope; the pattern's `from`/`to` are scope-relative
/// names resolved with [`crate::mutation::resolve_scoped_path`] before being
/// compared against the absolute endpoints in `existing_edges` — mirroring
/// apply EXACTLY so validate and apply agree. The optional `condition` /
/// `modifier` / `default` keys follow the same F6 semantics via
/// [`remove_edges_pattern_hits`] — absent means unconstrained (GH #283).
pub fn validate_remove_edges(
    diff: &JsonValue,
    scope: &str,
    existing_edges: &[EdgeMatchView],
) -> Result<(), MutationError> {
    let Some(obj) = diff.as_object() else {
        return Err(MutationError::Schema("diff is not an object".into()));
    };
    let Some(rems) = obj.get("remove_edges").and_then(|v| v.as_array()) else {
        return Ok(());
    };
    for r in rems {
        let m = r.get("match");
        let from_name = m
            .and_then(|v| v.get("from"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("remove_edges[].match.from missing".into()))?;
        let to_name = m
            .and_then(|v| v.get("to"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| MutationError::Schema("remove_edges[].match.to missing".into()))?;
        let pat_condition = m.and_then(|v| v.get("condition")).and_then(|v| v.as_str());
        let pat_modifier = m.and_then(|v| v.get("modifier"));
        let pat_default = m.and_then(|v| v.get("default")).and_then(|v| v.as_bool());
        let from_path = crate::mutation::resolve_scoped_path(scope, from_name);
        let to_path = crate::mutation::resolve_scoped_path(scope, to_name);
        let hit = existing_edges.iter().any(|e| {
            remove_edges_pattern_hits(
                e,
                from_path.as_str(),
                to_path.as_str(),
                pat_condition,
                pat_modifier,
                pat_default,
            )
        });
        if !hit {
            return Err(MutationError::MatchNoHit(format!("{from_name}->{to_name}")));
        }
    }
    Ok(())
}

/// Befund 22 — scope-containment guard. Every scoped name in a mutation diff
/// resolves (apply-side [`crate::mutation::resolve_scoped_path`], a pure
/// string-join WITHOUT `..`-normalisation) to an absolute path that MUST stay
/// within the mutation's declared `guard_scope` prefix. A name that walks out of
/// its scope (e.g. via `..` segments) would otherwise let a *scoped* mutation
/// touch the registry / filesystem OUTSIDE its hive scope — a confinement breach.
///
/// Two layers (Slice-0 security hotfix added the first):
///
/// 1. **Raw traversal reject**: a name containing a `..` segment or starting
///    with `/` is rejected outright. The apply-side FS joins
///    (`resolve_cell_dir`, `staging_root.join(name)`) do NOT root-clamp `..`
///    and honour absolute names as join-replacement, so such a name diverges
///    from the clamped logical normalisation below — at root scope the clamp
///    made `/../escape` look contained while the filesystem realisation
///    walked OUTSIDE `{root}`.
/// 2. **Prefix containment**: we normalise the resolved path (root-clamped
///    `..` pop) and reject if the result escapes the (normalised) scope prefix.
///
/// Both reject with [`MutationError::ScopeOutOfBounds`]. Runs BEFORE any
/// FS/registry mutation, parallel to the `naming_collision` / `match_no_hit`
/// validate-time checks.
///
/// Only TOP-LEVEL diff names are checked (`add_nodes[].name`,
/// `remove_nodes[].match.name`, `swap_nodes[].match.name`/`.with.name`,
/// `add_edges[].from`/`.to`, `remove_edges[].match.from`/`.to`). Subtree-internal
/// `params.graph` edges (which legitimately use `./` and `../` relative
/// addressing, resolved by the subtree resolver) are NOT in scope here.
pub fn validate_scope_containment(
    diff: &JsonValue,
    guard_scope: &str,
) -> Result<(), MutationError> {
    let Some(obj) = diff.as_object() else {
        // Shape errors are surfaced by schema validation; nothing to contain.
        return Ok(());
    };
    let root = meclaw_core::Path::new("/");
    let scope_norm = meclaw_core::Path::resolve(&root, guard_scope);
    let check = |name: &str| -> Result<(), MutationError> {
        let resolved = crate::mutation::resolve_scoped_path(guard_scope, name);
        // FS-realisation guard (Slice-0 security hotfix): the apply-side joins
        // (`resolve_cell_dir`, `staging_root.join(name)`) do NOT root-clamp
        // `..`, and `PathBuf::join` REPLACES the base for an absolute name —
        // both diverge from the clamped logical normalisation below (worst
        // case: a directory OUTSIDE `{root}`). A raw `..` segment or an
        // absolute name has no legitimate top-level-diff producer (the
        // legitimate `./`/`../` relative addressing lives in subtree-internal
        // `params.graph`, which is NOT checked here) → reject outright,
        // reporting the UN-normalised resolved path (the normalised one would
        // look in-bounds at root scope and hide the escape).
        if name.starts_with('/') || name.split('/').any(|seg| seg == "..") {
            return Err(MutationError::ScopeOutOfBounds { path: resolved });
        }
        let normalized = meclaw_core::Path::resolve(&root, resolved.as_str());
        if path_within(&scope_norm, &normalized) {
            Ok(())
        } else {
            Err(MutationError::ScopeOutOfBounds { path: normalized })
        }
    };
    if let Some(adds) = obj.get("add_nodes").and_then(|v| v.as_array()) {
        for n in adds {
            if let Some(name) = n.get("name").and_then(|v| v.as_str()) {
                check(name)?;
            }
        }
    }
    if let Some(rems) = obj.get("remove_nodes").and_then(|v| v.as_array()) {
        for r in rems {
            if let Some(name) = r
                .get("match")
                .and_then(|m| m.get("name"))
                .and_then(|v| v.as_str())
            {
                check(name)?;
            }
        }
    }
    if let Some(swaps) = obj.get("swap_nodes").and_then(|v| v.as_array()) {
        for s in swaps {
            if let Some(name) = s
                .get("match")
                .and_then(|m| m.get("name"))
                .and_then(|v| v.as_str())
            {
                check(name)?;
            }
            if let Some(name) = s
                .get("with")
                .and_then(|m| m.get("name"))
                .and_then(|v| v.as_str())
            {
                check(name)?;
            }
        }
    }
    // GH #169: a `move_nodes` addresses two paths and both are the mutation's
    // business — the cell it takes and the ground it puts it on. A target
    // outside the scope would let a mutation scoped to one hive relocate a cell
    // into a hive it has no authority over, which is exactly the reach this
    // guard exists to deny.
    if let Some(ms) = obj.get("move_nodes").and_then(|v| v.as_array()) {
        for m in ms {
            if let Some(name) = m
                .get("match")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
            {
                check(name)?;
            }
            if let Some(to) = m.get("to").and_then(|v| v.as_str()) {
                check(to)?;
            }
        }
    }
    if let Some(es) = obj.get("add_edges").and_then(|v| v.as_array()) {
        for e in es {
            if let Some(f) = e.get("from").and_then(|v| v.as_str()) {
                check(f)?;
            }
            if let Some(t) = e.get("to").and_then(|v| v.as_str()) {
                // GH #163, GH #267: the colony's three read-only endpoints —
                // `/colony/graph` (topology), `/colony/registry` (its own
                // bookkeeping about its own cells) and `/colony/ledger`
                // (counts) —
                // are the absolute targets that are in bounds at every scope;
                // they are the authority's own endpoints, not cells (see
                // `crate::mutation::MUTATION_DRAWABLE_VIRTUAL_ENDPOINTS`).
                if crate::mutation::is_mutation_drawable_virtual_target(t) {
                    continue;
                }
                check(t)?;
            }
        }
    }
    if let Some(es) = obj.get("remove_edges").and_then(|v| v.as_array()) {
        for e in es {
            let m = e.get("match");
            if let Some(f) = m.and_then(|v| v.get("from")).and_then(|v| v.as_str()) {
                check(f)?;
            }
            if let Some(t) = m.and_then(|v| v.get("to")).and_then(|v| v.as_str()) {
                check(t)?;
            }
        }
    }
    // GH #456: a `seed_rows` addresses one cell and writes into it, which is
    // exactly the reach this guard exists to bound — a mutation scoped to one
    // hive must not drop a policy row into a store it has no authority over.
    if let Some(es) = obj.get("seed_rows").and_then(|v| v.as_array()) {
        for e in es {
            if let Some(t) = e.get("target").and_then(|v| v.as_str()) {
                check(t)?;
            }
        }
    }
    Ok(())
}

/// Segment-aware containment: `inner` is within `scope` iff it equals `scope` or
/// is a strict path-descendant (`scope`/...). Root scope (`/`) contains every
/// absolute path. Both inputs are expected pre-normalised.
fn path_within(scope: &meclaw_core::Path, inner: &meclaw_core::Path) -> bool {
    let s = scope.as_str();
    if s == "/" {
        return true;
    }
    let i = inner.as_str();
    i == s || i.starts_with(&format!("{s}/"))
}

/// Standard-Header-Konvention (spec § Standard-Header-Konvention): the context
/// keys that CAN exist by virtue of ingress-at-birth.
///
/// GH #185 — this is the OUTER BOUND of what a cell may claim, not a grant.
/// Being born at ingress is something a cell declares
/// (`contract.ingress.context`, [`HeaderNodeView::ingress_context`]); the list
/// here limits the claim to the standard header convention. Before #185 the
/// whole list was handed to any node that happened to have no incoming edge —
/// an inference about the graph's shape standing in for a statement about the
/// cell, which took the branch away from the connectors that really are
/// entries and handed it to any island.
pub const INGRESS_CONTEXT_KEYS: &[&str] =
    &["turn_id", "session_id", "user_id", "chat_id", "locale"];

/// Per-node contract projection used by [`validate_header_contract_locality`].
/// Carries only the key-sets the two header rules need — the caller projects a
/// `ContractBlock` (or in-memory `ConsumesBlock`/`EmitsBlock`) into this shape.
#[derive(Debug, Clone, Default)]
pub struct HeaderNodeView {
    /// `emits.hop` keys this node writes (the hop keys it provides downstream).
    pub emits_hop: std::collections::BTreeSet<String>,
    /// `consumes.context` keys this node declares `required: true`.
    pub required_context: std::collections::BTreeSet<String>,
    /// `consumes.hop` keys this node declares `required: true`.
    pub required_hop: std::collections::BTreeSet<String>,
    /// GH #185 — `contract.ingress.context`: the context keys this node
    /// declares it MINTS at birth, because messages enter the colony here.
    /// Empty ⇒ this node is not an ingress. Bounded by
    /// [`INGRESS_CONTEXT_KEYS`]; a claim outside it is refused.
    pub ingress_context: std::collections::BTreeSet<String>,
}

/// Project a parsed `contract` block into the [`HeaderNodeView`] the
/// 14-B locality check consumes. Only `required: true` consume keys carry a
/// build-time obligation; non-required keys are omitted. Shared by the
/// bootstrap walk, the mutation validate step and the staging path.
pub fn header_view_from_contract(block: &crate::config::ContractBlock) -> HeaderNodeView {
    let required_keys = |m: &std::collections::BTreeMap<String, meclaw_core::ConsumeSpec>| {
        m.iter()
            .filter(|(_, s)| s.required)
            .map(|(k, _)| k.clone())
            .collect::<std::collections::BTreeSet<String>>()
    };
    // Absent `consumes` (Slice 4: presence-detectable Option) projects like
    // an empty block — absent ⇒ empty ⇒ vacuous, semantics unchanged.
    let default_consumes = meclaw_core::ConsumesBlock::default();
    let consumes = block.consumes.as_ref().unwrap_or(&default_consumes);
    HeaderNodeView {
        emits_hop: block.emits.hop.keys().cloned().collect(),
        required_context: required_keys(&consumes.context),
        required_hop: required_keys(&consumes.hop),
        ingress_context: block.ingress.context.clone(),
    }
}

/// Per-edge modifier projection used by [`validate_header_contract_locality`].
/// `from`/`to` are node names in the SAME namespace as the `node_contracts`
/// keys (the caller MUST use one consistent representation, e.g. absolute
/// meclaw paths). The four key-sets mirror `ModifierSpec`'s four fields.
#[derive(Debug, Clone, Default)]
pub struct HeaderEdgeView {
    /// Source node name.
    pub from: String,
    /// Destination node name.
    pub to: String,
    /// `modifier.set_context` keys promoted on this edge.
    pub set_context: std::collections::BTreeSet<String>,
    /// `modifier.delete_context` keys removed on this edge.
    pub delete_context: std::collections::BTreeSet<String>,
    /// `modifier.set_hop` keys written on this edge.
    pub set_hop: std::collections::BTreeSet<String>,
    /// `modifier.delete_hop` keys removed on this edge.
    pub delete_hop: std::collections::BTreeSet<String>,
    /// The constant lane this edge STATES — the `set_hop.route` value when
    /// that expression is a constant (`'in_query'` ⇒ `Some("in_query")`).
    /// `None` when the edge states no route at all, or states an expression
    /// whose value is not knowable without a message (`hop.upstream_route`):
    /// unknown, therefore unjudged. Filled by
    /// `crate::mutation::hive_contract::constant_route` at BOTH projection
    /// sites (mutation and boot), so the check can never see a different graph
    /// than the colony runs.
    pub states_route: Option<String>,
}

/// Build-time header-contract check (Phase-14-B locality). PURE — the caller
/// supplies the already-loaded post_state node contracts + the edge modifier
/// key-sets; this function does NO FS/DB access. Two rules:
///
/// - **hop locality (strict, fan-in intersection):** for every node `N` with a
///   required `consumes.hop` key `k`, `k` MUST lie in the INTERSECTION over ALL
///   incoming edges `e: from → N` of the keys `e` contributes. A CELL `from`
///   contributes `(emits.hop(from) ∪ set_hop(e)) − delete_hop(e)`. A HIVE
///   `from` (`hives` member) is a transit pass-through (F1 fix, K-H1): the
///   hive forwards the hop compartment unchanged (spec § Hive-Pfade als
///   Target — hop decays only at a CELL emission), so `e` contributes
///   `(set_hop(e) ∪ INTERSECTION over the contributions of ALL inbound edges
///   of the hive) − delete_hop(e)` — recursively across multi-level transits,
///   mirroring the runtime key walk. Cycles are legal (tool-/refine-loops): an
///   edge already on the walk stack imposes no additional obligation (every
///   runtime delivery is rooted at a finite cell-emission path — greatest-
///   fixpoint reading), and a hive with NO inbound edge can never deliver a
///   message via `e`, so it is vacuously providing. A node with a required hop
///   key but NO incoming edge is rejected. A key not delivered by EVERY
///   incoming edge is rejected (`EdgeSchema`, message names node + key +
///   "14-B locality / fan-in intersection"). This is the collector safeguard.
/// - **ingress declaration (GH #185):** every key a node names in
///   `contract.ingress.context` MUST lie in [`INGRESS_CONTEXT_KEYS`]. A cell
///   may narrow the standard header set it mints at birth; it may not invent a
///   key that reaches `context` any other way than through an edge
///   `set_context`. Violation → `EdgeSchema`, naming node + key.
/// - **context REACHABILITY (presence, not freshness):** for every required
///   `consumes.context` key `k` at `N`, a path must exist backwards over
///   incoming edges to a context SETTER with NO intervening `delete_context: k`
///   on that path. Two setter roots count: an edge carrying `set_context: k`
///   (source-cell promotion), OR — GH #185 — a node on that path which DECLARES
///   `contract.ingress.context: k`, i.e. says messages are born at it carrying
///   `k`. `N` itself counts, which is what makes the ordinary connector shape
///   legal: a proxy that mints `chat_id` and also receives the replies routed
///   back to it satisfies its own requirement. No such path → `EdgeSchema`
///   ("context presence not reachable"), and the message names the declaration
///   an author would have to add. This is honestly PRESENCE/reachability, NOT
///   freshness — a key proven reachable may still be stale.
///
/// Empty `consumes` (no required keys) makes the check vacuously true, so
/// existing topologies that declare no `consumes` never break.
///
/// GH #293 — this is the thin `Result` face of
/// [`addressed_header_contract_locality`]: the FIRST violation the collecting
/// core produces is, by construction, the one this function returned before, so
/// every verdict it ever gave is byte-identical.
pub fn validate_header_contract_locality(
    node_contracts: &std::collections::BTreeMap<String, HeaderNodeView>,
    edges: &[HeaderEdgeView],
    hives: &std::collections::BTreeSet<String>,
) -> Result<(), MutationError> {
    addressed_header_contract_locality(node_contracts, edges, hives)
        .into_iter()
        .next()
        .map_or(Ok(()), |(error, _)| Err(error))
}

/// GH #293 — stage 6 ([`Stage::ContractLocality`]) as a COLLECTING check, the
/// header-locality third of it: EVERY node whose header contract cannot be
/// honoured by the post-state topology is named, not only the first one.
///
/// **This changes no verdict** — see [`validate_header_contract_locality`],
/// which is now the first-error face of the same core.
///
/// [`Stage::ContractLocality`]: crate::mutation::rejection::Stage::ContractLocality
pub fn collect_header_contract_locality(
    node_contracts: &std::collections::BTreeMap<String, HeaderNodeView>,
    edges: &[HeaderEdgeView],
    hives: &std::collections::BTreeSet<String>,
    into: &mut crate::mutation::rejection::MutationRejection,
) {
    use crate::mutation::rejection::{Stage, Violation};

    for (error, address) in addressed_header_contract_locality(node_contracts, edges, hives) {
        into.push(Violation::from_error(
            Stage::ContractLocality,
            &error,
            Some(address),
        ));
    }
}

/// The collecting core of [`validate_header_contract_locality`], with the node
/// each violation concerns as its address.
///
/// The per-node bodies are the ones the `Result` form used to `return` from;
/// they push and carry on to the NEXT node instead. One deliberate exception
/// inside a node: once Rule 0 has refused an ingress claim, that node's rules 1
/// and 2 are skipped — exactly the reason Rule 0 runs first (a nonsensical claim
/// would otherwise resurface as an unreachable key somewhere downstream, which
/// is a derived error, and derived errors are what GH #293 exists to keep out of
/// a refusal).
fn addressed_header_contract_locality(
    node_contracts: &std::collections::BTreeMap<String, HeaderNodeView>,
    edges: &[HeaderEdgeView],
    hives: &std::collections::BTreeSet<String>,
) -> Vec<(MutationError, String)> {
    use std::collections::HashMap;

    let mut violations: Vec<(MutationError, String)> = Vec::new();

    // Index incoming edges per node (by `to`), as indices into `edges` — the
    // transit walk needs a cycle guard, and the index is the edge identity.
    let mut incoming: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, e) in edges.iter().enumerate() {
        incoming.entry(e.to.as_str()).or_default().push(i);
    }

    for (node, view) in node_contracts {
        // ── Rule 0 (GH #185): the ingress claim is bounded ──────────────────
        // A cell may narrow the standard header set it mints at birth, never
        // widen it. Checked before the two rules below so a nonsensical claim
        // is named as such instead of surfacing as an unreachable key somewhere
        // downstream.
        let ingress_violations = violations.len();
        for key in &view.ingress_context {
            if !INGRESS_CONTEXT_KEYS.contains(&key.as_str()) {
                violations.push((
                    MutationError::EdgeSchema(format!(
                        "node '{node}' declares contract.ingress.context '{key}', which is not a \
                         standard header key born at ingress (allowed: {}) — a key outside that \
                         set reaches context through an edge modifier.set_context",
                        INGRESS_CONTEXT_KEYS.join(", ")
                    )),
                    node.clone(),
                ));
            }
        }
        if violations.len() > ingress_violations {
            // The claim this node makes about itself is nonsense; everything
            // rules 1 and 2 would say about it follows from that.
            continue;
        }

        // ── Rule 1: hop locality (fan-in intersection) ──────────────────────
        if !view.required_hop.is_empty() {
            // A node with required hop keys but no incoming edge can never have
            // those keys delivered → reject. Its per-key verdicts would all be
            // the same sentence in a longer form, so the node is named ONCE and
            // rule 2 — a statement about a different compartment — still runs.
            match incoming.get(node.as_str()) {
                None => {
                    let key = view.required_hop.iter().next().cloned().unwrap_or_default();
                    violations.push((
                        MutationError::EdgeSchema(format!(
                            "node '{node}' requires consumes.hop '{key}' but has no incoming edge \
                             (14-B locality / fan-in intersection)"
                        )),
                        node.clone(),
                    ));
                }
                Some(in_edges) => {
                    for key in &view.required_hop {
                        // The key must be provided by EVERY incoming edge (transit
                        // edges recurse into the hive's inbound fan-in — F1 fix).
                        let provided_by_all = in_edges.iter().all(|&ei| {
                            let mut walk = std::collections::HashSet::new();
                            edge_provides_hop_key(
                                ei,
                                key,
                                edges,
                                node_contracts,
                                hives,
                                &incoming,
                                &mut walk,
                            )
                        });
                        if !provided_by_all {
                            violations.push((
                                MutationError::EdgeSchema(format!(
                                    "node '{node}' requires consumes.hop '{key}' not in the \
                                     fan-in intersection of all incoming edges \
                                     (14-B locality / fan-in intersection)"
                                )),
                                node.clone(),
                            ));
                        }
                    }
                }
            }
        }

        // ── Rule 2: context reachability (presence, not freshness) ──────────
        for key in &view.required_context {
            if !context_key_reachable(node, key, edges, &incoming, node_contracts) {
                let hint = if INGRESS_CONTEXT_KEYS.contains(&key.as_str()) {
                    format!(
                        " — no edge promotes it and no cell on the way declares \
                         contract.ingress.context '{key}'"
                    )
                } else {
                    String::new()
                };
                violations.push((
                    MutationError::EdgeSchema(format!(
                        "node '{node}' requires consumes.context '{key}' but context presence \
                         not reachable from any setter{hint}"
                    )),
                    node.clone(),
                ));
            }
        }
    }
    violations
}

/// Contribution test for one incoming edge in Rule 1 of
/// [`validate_header_contract_locality`]: does the edge at `edge_idx`
/// guarantee hop key `key` on every message it delivers? PURE.
///
/// - `delete_hop: key` on the edge severs the key (delete wins over set).
/// - `set_hop: key` on the edge provides it.
/// - CELL `from`: the key must be in the cell's `emits.hop` view.
/// - HIVE `from` (F1 fix, K-H1): the hive is a transit pass-through — the
///   hop compartment survives (it decays only at a CELL emission), so the
///   edge contributes the INTERSECTION over the contributions of ALL inbound
///   edges of the hive, recursively across multi-level transits (the same
///   key walk the runtime performs). An edge already on the `walk` stack is
///   a loop back-edge and imposes no additional obligation: every runtime
///   delivery is rooted at a finite cell-emission path, so the looping
///   continuation is covered by the acyclic prefixes (greatest-fixpoint
///   reading — returning `false` here would falsely empty the intersection
///   at legal tool-/refine-loops). A hive with NO inbound edge can never
///   deliver a message via this edge → vacuously providing (zero runtime
///   paths impose zero obligations; the wiring mutation that later connects
///   the hive re-validates the full post-state).
#[allow(clippy::too_many_arguments)]
fn edge_provides_hop_key(
    edge_idx: usize,
    key: &str,
    edges: &[HeaderEdgeView],
    node_contracts: &std::collections::BTreeMap<String, HeaderNodeView>,
    hives: &std::collections::BTreeSet<String>,
    incoming: &std::collections::HashMap<&str, Vec<usize>>,
    walk: &mut std::collections::HashSet<usize>,
) -> bool {
    let e = &edges[edge_idx];
    if e.delete_hop.contains(key) {
        return false;
    }
    if e.set_hop.contains(key) {
        return true;
    }
    if !hives.contains(e.from.as_str()) {
        // Cell (or unknown) source: only its emits view counts.
        return node_contracts
            .get(e.from.as_str())
            .map(|fv| fv.emits_hop.contains(key))
            .unwrap_or(false);
    }
    // Hive source: recurse into the hive's inbound fan-in (cycle-guarded).
    if !walk.insert(edge_idx) {
        return true;
    }
    let provided = incoming.get(e.from.as_str()).is_none_or(|ins| {
        ins.iter()
            .all(|&g| edge_provides_hop_key(g, key, edges, node_contracts, hives, incoming, walk))
    });
    walk.remove(&edge_idx);
    provided
}

/// Backwards reachability for one required `consumes.context` key (Rule 2 of
/// [`validate_header_contract_locality`]). PURE. Walks incoming edges from
/// `node` looking for a setter root, pruning any path that crosses a
/// `delete_context: key`. Returns `true` iff such a path exists.
///
/// Setter roots: an edge with `set_context: key` (promotion), OR — GH #185 — a
/// node that DECLARES `contract.ingress.context: key`, i.e. one that says
/// messages are born at it carrying that key. `node` itself is on the walk, so
/// an ingress satisfies its own requirement.
///
/// It used to be the second root that was inferred: a node with no incoming
/// edge was read as the graph entry and handed every key in
/// [`INGRESS_CONTEXT_KEYS`]. In-degree is not a property of a cell — a proxy
/// that also receives replies has an incoming edge and lost the branch, while
/// an unconnected island gained it — so the answer changed when an unrelated
/// edge was added. Now nothing about the shape of the graph decides it.
fn context_key_reachable(
    node: &str,
    key: &str,
    edges: &[HeaderEdgeView],
    incoming: &std::collections::HashMap<&str, Vec<usize>>,
    node_contracts: &std::collections::BTreeMap<String, HeaderNodeView>,
) -> bool {
    use std::collections::HashSet;
    // BFS backwards over nodes; the frontier holds nodes whose incoming edges we
    // still need to inspect. We start at `node` itself.
    let mut visited: HashSet<&str> = HashSet::new();
    let mut frontier: Vec<&str> = vec![node];
    while let Some(current) = frontier.pop() {
        if !visited.insert(current) {
            continue;
        }
        // GH #185 — a declared ingress is a setter root, whatever its in-degree.
        if node_contracts
            .get(current)
            .is_some_and(|v| v.ingress_context.contains(key))
        {
            return true;
        }
        match incoming.get(current) {
            None => {}
            Some(in_edges) => {
                for &ei in in_edges {
                    let e = &edges[ei];
                    // A delete on this edge severs the key on THIS path; do not
                    // traverse it.
                    if e.delete_context.contains(key) {
                        continue;
                    }
                    // A promotion on this edge is a setter root → reachable.
                    if e.set_context.contains(key) {
                        return true;
                    }
                    frontier.push(e.from.as_str());
                }
            }
        }
    }
    false
}

// ──────────────────────────────────────────────────────────────────────────────
// GH #291 — a hive lane's `context` requirement, answered by the same walk
// ──────────────────────────────────────────────────────────────────────────────

/// One lane of one hive contract, as the lane-context check receives it.
///
/// The check needs four things and no more: which hive PATH the lane belongs to
/// (a contract is a statement about the path, never about a cell inside), the
/// `hop.route` that IS the lane, the `context` keys a caller must have promoted
/// by the time a message enters on it, and the hive's own sentence about the
/// lane — which travels verbatim into the refusal, the way every other
/// `because` in this pipeline does.
///
/// Projected from `crate::mutation::hive_contract::HiveContract` at the call
/// site (one requirement per `accepts[]` entry), so this module stays PURE and
/// keeps knowing nothing about `config.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HiveLaneRequirement {
    /// Absolute logical path of the hive scope the lane belongs to.
    pub hive_path: String,
    /// The `hop.route` value that IS this lane.
    pub route: String,
    /// `context` keys a caller must have promoted by the time a message enters
    /// on this lane. Empty ⇒ this lane requires nothing and is never judged.
    pub context: Vec<String>,
    /// The hive's own sentence about the lane, quoted in the refusal.
    /// `None` when the lane declares none.
    pub because: Option<String>,
}

/// Build-time lane-context check (GH #291). PURE — same inputs as
/// [`validate_header_contract_locality`] plus the lane requirements.
///
/// `LaneSpec.context` used to say "declared, not enforced", and the reason it
/// gave was that a promotion three edges upstream is indistinguishable from a
/// missing one *to anything that reads a single edge*. That is true of a check
/// that reads a single edge; it stopped being true when GH #185 built the
/// backwards reachability walk ([`context_key_reachable`]) the header rule uses
/// for `consumes.context`. The lane requirement is answered with that same walk,
/// started at the CALLER's side of the hive path — so the answer here and the
/// answer for a `consumes.context` key can never disagree.
///
/// Judged: an edge whose `to` IS the hive path and which STATES a constant
/// `hop.route` naming the lane ([`HeaderEdgeView::states_route`]). Each of the
/// lane's keys must then be either promoted on that edge itself
/// (`set_context`) or reachable backwards from its `from`.
///
/// Two things are deliberately NOT judged, both for reasons the rest of
/// `hive_contract` already runs on:
///
/// - an edge that states no constant route — which lane it means is knowable
///   only once a message exists, and a check that cannot say which lane an edge
///   means must never reject it;
/// - an edge whose `from` is a HIVE path with NO inbound edge — nothing can be
///   delivered through it, so the requirement is dormant rather than broken
///   (the same island reading as `hive_contract::hive_path_is_wired`, and what
///   keeps a freshly instantiated composite installable). One inbound edge
///   lifts it.
///
/// GH #293 — this is the thin `Result` face of [`addressed_hive_lane_context`]:
/// the FIRST violation the collecting core produces is the one this function
/// returns.
pub fn validate_hive_lane_context(
    lanes: &[HiveLaneRequirement],
    edges: &[HeaderEdgeView],
    node_contracts: &std::collections::BTreeMap<String, HeaderNodeView>,
    hives: &std::collections::BTreeSet<String>,
) -> Result<(), MutationError> {
    addressed_hive_lane_context(lanes, edges, node_contracts, hives)
        .into_iter()
        .next()
        .map_or(Ok(()), |(error, _, _)| Err(error))
}

/// GH #291 + #293 — the lane-context check as a COLLECTING one: EVERY edge that
/// sends a declared lane into a hive without the context the lane requires is
/// named, not only the first one.
///
/// **This changes no verdict** — see [`validate_hive_lane_context`], which is
/// the first-error face of the same core.
///
/// Tagged [`Stage::ContractLocality`] because that is what it decides: whether
/// the context a hive's lane promises is local to the wiring that uses it.
///
/// [`Stage::ContractLocality`]: crate::mutation::rejection::Stage::ContractLocality
pub fn collect_hive_lane_context(
    lanes: &[HiveLaneRequirement],
    edges: &[HeaderEdgeView],
    node_contracts: &std::collections::BTreeMap<String, HeaderNodeView>,
    hives: &std::collections::BTreeSet<String>,
    into: &mut crate::mutation::rejection::MutationRejection,
) {
    use crate::mutation::rejection::{Stage, Violation};

    for (error, address, because) in
        addressed_hive_lane_context(lanes, edges, node_contracts, hives)
    {
        into.push(match because {
            Some(because) => Violation::from_error_because(
                Stage::ContractLocality,
                &error,
                Some(address),
                because,
            ),
            None => Violation::from_error(Stage::ContractLocality, &error, Some(address)),
        });
    }
}

/// The collecting core: one entry per violated (edge, key), addressed by the
/// judged edge rendered `"<from> -> <to>"` — the edge is what an author has to
/// go and change, so the edge is the address — and carrying the lane's own
/// `because`.
fn addressed_hive_lane_context(
    lanes: &[HiveLaneRequirement],
    edges: &[HeaderEdgeView],
    node_contracts: &std::collections::BTreeMap<String, HeaderNodeView>,
    hives: &std::collections::BTreeSet<String>,
) -> Vec<(MutationError, String, Option<String>)> {
    use std::collections::HashMap;

    let mut violations: Vec<(MutationError, String, Option<String>)> = Vec::new();
    if lanes.is_empty() {
        return violations;
    }

    // Same index the locality core builds, and used for the same two purposes:
    // the reachability walk, and the in-degree question the dormancy rule asks.
    let mut incoming: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, e) in edges.iter().enumerate() {
        incoming.entry(e.to.as_str()).or_default().push(i);
    }

    for lane in lanes {
        if lane.context.is_empty() {
            continue;
        }
        for e in edges {
            if e.to != lane.hive_path || e.states_route.as_deref() != Some(lane.route.as_str()) {
                continue;
            }
            // Dormant: the caller side is a hive path nothing addresses, so no
            // message can travel this edge at all.
            if hives.contains(e.from.as_str()) && !incoming.contains_key(e.from.as_str()) {
                continue;
            }
            for key in &lane.context {
                if e.set_context.contains(key)
                    || context_key_reachable(&e.from, key, edges, &incoming, node_contracts)
                {
                    continue;
                }
                // The lane's own sentence is NOT interpolated here. It travels
                // in the third tuple slot into `Violation::because`, and
                // `Violation::render` appends it once at the end of the line —
                // so an author reads it exactly once. Quoting it inline as well
                // put a ~1.4 kB `because` twice into every rendered refusal
                // (memory-hive's `in_query` sentence is that long), which
                // buries the one thing the line is FOR: the key that is
                // missing.
                violations.push((
                    MutationError::HiveContract(format!(
                        "edge '{from} -> {hive}' states hop.route='{route}' into hive '{hive}', \
                         whose lane '{route}' requires context '{key}', but nothing \
                         promotes it: neither this edge's modifier.set_context nor any setter \
                         reachable upstream of '{from}'. Promote the key on the edge, or wire the \
                         caller behind something that does.",
                        from = e.from,
                        hive = lane.hive_path,
                        route = lane.route,
                    )),
                    format!("{} -> {}", e.from, e.to),
                    lane.because.clone(),
                ));
            }
        }
    }
    violations
}

// ──────────────────────────────────────────────────────────────────────────────
// GH #292 — a template's `requires` declaration, enforced before staging
// ──────────────────────────────────────────────────────────────────────────────

/// Render the refusal for one missing key.
///
/// `{ref}: {class} key {key:?} is required — {because}`; the `— {because}` half
/// is omitted when the declaration carries none, because an empty dash is worse
/// than no dash. `class` is `ctx` or `env` — the two placeholder classes stay
/// apart in the message for the same reason they stay apart in the declaration:
/// a `ctx` key is supplied by the mutation, an `env` key by the colony's `.env`,
/// and the reader has to know which of the two to go and fix.
///
/// The `because` stays INLINE here, unlike
/// [`addressed_hive_lane_context`]'s, and the two are consistent rather than in
/// disagreement: the rule is that the sentence appears exactly ONCE in a
/// rendered line, and which half carries it follows from whether a structured
/// [`Violation::because`] is filled. Stage 3's violations are built with
/// [`Violation::from_error`] (see [`collect_declared_requirements`]), so
/// nothing would ever append the sentence for them — inline is the only place
/// it can live. The lane check fills the field via
/// [`Violation::from_error_because`], so its message must stay clean.
///
/// [`Violation::because`]: crate::mutation::rejection::Violation::because
/// [`Violation::from_error`]: crate::mutation::rejection::Violation::from_error
/// [`Violation::from_error_because`]: crate::mutation::rejection::Violation::from_error_because
fn requirement_missing(
    reference: &str,
    class: &str,
    key: &str,
    decl: &crate::templates::RequiredKey,
) -> MutationError {
    let head = format!("{reference}: {class} key {key:?} is required");
    MutationError::RequirementMissing(match &decl.because {
        Some(because) => format!("{head} — {because}"),
        None => head,
    })
}

/// Check one template directory's own declaration against `ctx` and `env`.
///
/// `reference` is how the template was named (the mutation's `template` string
/// for the outer one, `name@version` for a template reached through a `ref`) —
/// it is what the refusal leads with, so a composite's refusal says WHICH unit
/// asked for the key.
///
/// GH #293 — collecting: a template missing three keys names three keys. The
/// caller takes the first entry when it wants the old single answer, and that
/// first entry is unchanged (both maps are ordered and `ctx` is still read
/// before `env`). The malformed-declaration case is the one that still returns
/// alone: once the file does not parse there is nothing further to read out of
/// it.
fn check_declared_requirements(
    reference: &str,
    template_dir: &std::path::Path,
    ctx: &std::collections::HashMap<String, String>,
    env: &std::collections::HashMap<String, String>,
    out: &mut Vec<MutationError>,
) {
    let req = match crate::templates::read_requires(template_dir) {
        Ok(req) => req,
        Err(e) => {
            out.push(MutationError::Schema(e.to_string()));
            return;
        }
    };
    // Both maps are ordered, and `ctx` is checked before `env`, so a template
    // missing several keys always names the same one first.
    for (key, decl) in &req.ctx {
        if decl.required && !ctx.contains_key(key) {
            out.push(requirement_missing(reference, "ctx", key, decl));
        }
    }
    for (key, decl) in &req.env {
        if decl.required && !env.contains_key(key) {
            out.push(requirement_missing(reference, "env", key, decl));
        }
    }
}

/// Where the live tree is, for the per-node Resume derivation (GH #347 gap 2).
///
/// A Resume is classified against the filesystem, so the requirements check
/// needs the same two coordinates the staging side resolves final paths from:
/// the colony root and the mutation's logical scope. They travel together
/// because they are only ever meaningful together — and as one parameter
/// because the check's argument list is long enough already.
#[derive(Debug, Clone, Copy)]
pub struct LiveTree<'a> {
    /// The colony root directory (`{root}`), the tree the mutation runs against.
    pub root: &'a std::path::Path,
    /// The mutation's logical scope (`/`, `/sub`, …), as the payload spells it.
    pub scope: &'a str,
}

/// GH #292 — refuse an instantiation whose template declares a key the mutation
/// does not supply, BEFORE anything is staged.
///
/// A template says what it needs (`requires.ctx` / `requires.env`, see
/// `templates/requires.rs`). Until this check existed the declaration was
/// documentation: a missing `${ctx.X}` only surfaced while the already-copied
/// tree was substituted (`ctx_key_missing`, after the copy), and a missing
/// environment variable not at all — the instance was born and failed at run
/// time. The declaration is a contract, so it is enforced where a contract
/// belongs: before the first byte is written.
///
/// **The union spans the refs.** A composite is refused for what its parts
/// need: every distinct `(name, version)` hop recorded in the parsed
/// [`crate::mutation::subtree::CellNode::ref_chain`] is resolved back to its
/// registry entry and read too. A template that names a ref therefore inherits
/// the ref's requirements without restating them — restating them is exactly
/// the drift this declaration exists to remove.
///
/// Pure with respect to colony state: it reads `template.json` files and the
/// two supplied maps, and touches neither registry nor filesystem tree.
///
/// # Errors
/// - [`MutationError::RequirementMissing`] naming the template, the class, the
///   key and the template's own `because`.
/// - [`MutationError::Schema`] if a `requires` block is malformed (the message
///   names the file), plus whatever
///   [`crate::mutation::subtree::parse_subtree`] reports for a broken ref — the
///   same errors staging would raise, only earlier.
///
/// **Both instantiating operations are read** (GH #347 gap 1): an `add_nodes`
/// entry, and the INSTANTIATE form of a `swap_nodes[].with` — the one that
/// carries a `template` and therefore performs the same copy-and-substitute.
/// The existing-node form of `with` references a cell that is already there and
/// stages nothing, so it owes nothing here.
///
/// **A Resume is exempt — per NODE** (GH #347 gap 2). `resumed_names` carries
/// the `add_nodes[].name` of every entry the caller identified as a
/// Reconnect/Resume (`colony.rs` Step 1a: the target directory exists). A
/// Resume instantiates nothing — it stages nothing, substitutes no `${ctx.X}`
/// and rewrites no `config.json` (overview Z.170-180, A1) — so it never
/// consumes the declared keys, and requiring them would refuse a reconnect that
/// was legal before the declaration existed. The requirement belongs to the
/// instantiation, not to the address. It applies to `add_nodes` only: a swap
/// always stages.
///
/// For a COMPOSITE template that premise holds only for the nodes that are
/// actually there. A subtree whose root exists but whose children do not is a
/// MERGE resume: the merge path stages the missing children, substituting their
/// `${ctx.X}` exactly like a fresh instantiation. So the exemption is decided
/// per node, and by the SAME derivation the staging side uses —
/// [`crate::mutation::subtree::classify_subtree_nodes`], the pure classifier
/// `stage_subtree_merge` itself calls. [`LiveTree`] carries what that
/// classifier needs to resolve each template node's final directory; it is the
/// only reason this function touches the filesystem tree at all, and it only
/// reads directory existence.
///
/// An entry without a `template` (an `adopt`, an existing-node swap, or a
/// schema error the schema check names) and a `template` the registry does not
/// hold are both skipped: neither is this check's finding.
///
/// GH #293 — this is the thin `Result` face of [`addressed_requires`]: the
/// FIRST violation the collecting core produces is, by construction, the one
/// this function returned before, so every verdict it ever gave is
/// byte-identical.
pub fn validate_requires(
    diff: &JsonValue,
    templates: &crate::templates::TemplatesRegistry,
    ctx: &std::collections::HashMap<String, String>,
    env: &std::collections::HashMap<String, String>,
    resumed_names: &[String],
    live: LiveTree<'_>,
) -> Result<(), MutationError> {
    addressed_requires(diff, templates, ctx, env, resumed_names, live)
        .into_iter()
        .next()
        .map_or(Ok(()), |(error, _)| Err(error))
}

/// GH #293 — stage 3 ([`Stage::Requires`]) as a COLLECTING check: every
/// instantiating entry (`add_nodes`, and a `swap_nodes[].with` that names a
/// template) whose template declares a key the mutation does not supply, in one
/// refusal.
///
/// **This changes no verdict** — see [`validate_requires`], which is now the
/// first-error face of the same core.
///
/// [`Stage::Requires`]: crate::mutation::rejection::Stage::Requires
pub fn collect_requires(
    diff: &JsonValue,
    templates: &crate::templates::TemplatesRegistry,
    ctx: &std::collections::HashMap<String, String>,
    env: &std::collections::HashMap<String, String>,
    resumed_names: &[String],
    live: LiveTree<'_>,
    into: &mut crate::mutation::rejection::MutationRejection,
) {
    use crate::mutation::rejection::{Stage, Violation};

    for (error, address) in addressed_requires(diff, templates, ctx, env, resumed_names, live) {
        into.push(Violation::from_error(Stage::Requires, &error, address));
    }
}

/// The collecting core of [`validate_requires`], with the template reference
/// each violation concerns.
///
/// One entry per unmet requirement — a template naming three keys the mutation
/// supplies none of says all three, and the refusal is one round trip instead
/// of three. Nothing short-circuits: a `parse_subtree` failure (which used to
/// end the whole walk) is now one more entry, and the next entry is read all
/// the same. Only the malformed-declaration case stops within its own template
/// — once the file does not parse there is nothing further to read out of it.
///
/// **Two instantiation paths, one walk** (GH #347 gap 1). Both operations that
/// copy a template are read here: an `add_nodes` entry, and the INSTANTIATE
/// form of a `swap_nodes[].with` (the one that carries a `template`). The swap
/// performs the same copy-and-substitute, so it owes the same declared keys —
/// until GH #347 it was accepted and broke later, during the staging
/// substitution, as `ctx_key_missing`. The existing-node form of `with` (no
/// `template`) references a cell that is already there, instantiates nothing
/// and is therefore not this check's business.
///
/// `resumed_names` applies to `add_nodes` only: a swap always stages, so there
/// is no address at which it could be a Reconnect. What a resumed entry still
/// stages is decided per node by [`resume_staged_nodes`].
fn addressed_requires(
    diff: &JsonValue,
    templates: &crate::templates::TemplatesRegistry,
    ctx: &std::collections::HashMap<String, String>,
    env: &std::collections::HashMap<String, String>,
    resumed_names: &[String],
    live: LiveTree<'_>,
) -> Vec<(MutationError, Option<String>)> {
    let mut violations: Vec<(MutationError, Option<String>)> = Vec::new();
    if let Some(adds) = diff.get("add_nodes").and_then(|v| v.as_array()) {
        for n in adds {
            // Spelled as the diff spells it — `resumed_names` is filled from the
            // same field of the same entry, so the two always agree.
            let name = n.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            let staged = if resumed_names.iter().any(|r| r == name) {
                resume_staged_nodes(n.get("template"), templates, live, name)
            } else {
                StagedNodes::All
            };
            if matches!(staged, StagedNodes::None) {
                continue; // a true resume: nothing is staged, nothing is owed.
            }
            violations.extend(requires_for_reference(
                n.get("template"),
                templates,
                ctx,
                env,
                &staged,
            ));
        }
    }
    if let Some(swaps) = diff.get("swap_nodes").and_then(|v| v.as_array()) {
        for n in swaps {
            // Only the instantiate form names a template; the existing-node
            // form carries a `name` alone and stages nothing.
            violations.extend(requires_for_reference(
                n.get("with").and_then(|w| w.get("template")),
                templates,
                ctx,
                env,
                &StagedNodes::All,
            ));
        }
    }
    violations
}

/// Which of a template's nodes this diff entry actually instantiates.
///
/// A fresh entry stages every node ([`Self::All`]); a resume over a fully
/// existing tree stages none ([`Self::None`]); a MERGE resume over a partially
/// existing composite stages exactly the nodes whose final directory is absent
/// ([`Self::Only`], addressed by template rel-path — `""` is the subtree root).
enum StagedNodes {
    All,
    Only(Vec<String>),
    None,
}

impl StagedNodes {
    /// Is the template node at `rel_path` one this entry instantiates?
    fn stages(&self, rel_path: &str) -> bool {
        match self {
            Self::All => true,
            Self::Only(rels) => rels.iter().any(|r| r == rel_path),
            Self::None => false,
        }
    }
}

/// What a Reconnect/Resume still stages, derived the way the staging side
/// derives it (GH #347 gap 2).
///
/// The exemption used to be decided per diff ENTRY, on the existence of the
/// entry's root directory alone. For a composite that is only half the truth:
/// `stage.rs` dispatches a multi-cell template to
/// [`crate::mutation::subtree::stage_subtree_merge`] BEFORE its single-cell
/// existence-skip, and that merge stages every node whose final directory is
/// absent — a missing node has no existing descendants, so the nodes it stages
/// are exactly [`crate::mutation::subtree::SubtreePartition::missing`] plus
/// `missing_hives`.
///
/// So this calls the very same classifier `stage_subtree_merge` calls —
/// [`crate::mutation::subtree::classify_subtree_nodes`], which is pure and
/// resolves each node's final path through `path_truth::resolve_cell_dir`, the
/// shared `logical → fs` truth. One helper, one answer to "what is a resume":
/// a second existence check here would be a second opinion, and the two would
/// drift the first time either side learned something new.
///
/// A single-cell template classifies to one node at the entry's own path, which
/// a resume found existing by definition → [`StagedNodes::None`], the old
/// behaviour unchanged.
///
/// A template that does not resolve, or whose tree cannot be parsed, yields
/// [`StagedNodes::None`] — the old exemption. Stage 2
/// ([`collect_template_resolution`]) has already refused a broken reference
/// before this stage runs, so declining to guess here can hide nothing; it only
/// keeps a classification failure from inventing a requirement violation.
fn resume_staged_nodes(
    reference: Option<&JsonValue>,
    templates: &crate::templates::TemplatesRegistry,
    live: LiveTree<'_>,
    name: &str,
) -> StagedNodes {
    let Some(reference) = reference.and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else {
        return StagedNodes::None; // an `adopt` — never a resume, never an instantiation.
    };
    let Ok(entry) = templates.resolve(reference) else {
        return StagedNodes::None;
    };
    let Ok(partition) = crate::mutation::subtree::classify_subtree_nodes(
        live.root,
        live.scope,
        name,
        &entry.filesystem_path,
        templates,
    ) else {
        return StagedNodes::None;
    };
    let staged: Vec<String> = partition
        .missing
        .iter()
        .chain(partition.missing_hives.iter())
        .map(|node| node.rel_path.clone())
        .collect();
    if staged.is_empty() {
        StagedNodes::None
    } else {
        StagedNodes::Only(staged)
    }
}

/// One template reference's unmet requirements — its own declaration plus, over
/// its `ref`s, every referenced template's.
///
/// The shared body of the two instantiating operations (`add_nodes[].template`
/// and the instantiate form of `swap_nodes[].with.template`), so that the two
/// cannot grow a second opinion about what a template requires (GH #347).
///
/// `reference` is the raw JSON value of the `template` key, as the diff spells
/// it; an absent, non-string or empty one is not an instantiation and yields
/// nothing (an `adopt`, an existing-node swap, or a schema error the schema
/// check names). A `template` the registry does not hold yields nothing either:
/// `template_missing` is the schema validator's finding, not this check's.
///
/// `staged` says which of the template's nodes this operation instantiates
/// (GH #347 gap 2). It filters the REF walk: a `ref` hop belongs to the node it
/// hangs under, so a merge resume that leaves that node alone consumes none of
/// the referenced template's keys and is not asked for them. The named
/// template's OWN declaration is not attributable to a single node — it is the
/// composite's, made once for the whole tree — so it is owed as soon as the
/// operation stages anything at all. `StagedNodes::None` never reaches here:
/// the caller drops such an entry whole.
fn requires_for_reference(
    reference: Option<&JsonValue>,
    templates: &crate::templates::TemplatesRegistry,
    ctx: &std::collections::HashMap<String, String>,
    env: &std::collections::HashMap<String, String>,
    staged: &StagedNodes,
) -> Vec<(MutationError, Option<String>)> {
    let Some(reference) = reference.and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    let Ok(entry) = templates.resolve(reference) else {
        return Vec::new(); // `template_missing` is the schema validator's finding.
    };
    let mut errors = Vec::new();
    check_declared_requirements(reference, &entry.filesystem_path, ctx, env, &mut errors);

    // The refs: parse once, walk every distinct hop of every node this
    // operation stages, read each one's own declaration. `parse_subtree` is
    // what staging runs anyway, so a ref-free template costs one directory walk
    // and nothing else.
    match crate::mutation::subtree::parse_subtree(&entry.filesystem_path, templates) {
        Err(error) => errors.push(error),
        Ok(parsed) => {
            let mut seen: Vec<(String, Option<String>)> = Vec::new();
            for cell in &parsed.cells {
                if !staged.stages(&cell.rel_path) {
                    continue;
                }
                for hop in &cell.ref_chain {
                    if seen.contains(hop) {
                        continue;
                    }
                    seen.push(hop.clone());
                    let hop_ref = match &hop.1 {
                        Some(version) => format!("{}@{}", hop.0, version),
                        None => hop.0.clone(),
                    };
                    let Ok(hop_entry) = templates.resolve(&hop_ref) else {
                        continue; // resolved once already during parsing.
                    };
                    check_declared_requirements(
                        &hop_ref,
                        &hop_entry.filesystem_path,
                        ctx,
                        env,
                        &mut errors,
                    );
                }
            }
        }
    }
    errors
        .into_iter()
        .map(|e| (e, Some(reference.to_string())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;
    use std::sync::Arc;

    fn factories_with(types: &[&str]) -> CellFactoryRegistry {
        struct Noop;
        impl crate::CellFactory for Noop {
            fn validate_params(&self, _: &JsonValue) -> Result<(), String> {
                Ok(())
            }
            fn spawn_cell(
                self: Arc<Self>,
                _: meclaw_core::Path,
                _: JsonValue,
                _: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
                _cell_dir: std::path::PathBuf,
                _contract: crate::ContractView,
                _colony_inbox_tx: tokio::sync::mpsc::Sender<crate::ColonyMsg>,
                _idle_timeout: Option<std::time::Duration>,
                _cell_timeout: i64,
                _message_timeout: Option<std::time::Duration>,
                _blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
                _mailbox_capacity: usize,
            ) -> Result<crate::SpawnedCellKind, String> {
                unimplemented!()
            }
        }
        let mut m = CellFactoryRegistry::new();
        for t in types {
            m.insert((*t).into(), Arc::new(Noop) as Arc<dyn crate::CellFactory>);
        }
        m
    }

    #[test]
    fn schema_check_rejects_non_object_diff() {
        let factories = factories_with(&["echo"]);
        let err = validate_post_state(&json!([]), &factories).unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    #[test]
    fn template_missing_when_add_node_uses_unknown_type() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"add_nodes": [{"name": "foo", "template": "unknown"}]});
        let err = validate_post_state(&diff, &factories).unwrap_err();
        assert_eq!(err.error_code(), "template_missing");
    }

    #[test]
    fn valid_minimal_diff_passes() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"add_nodes": [{"name": "foo", "template": "echo"}]});
        assert!(validate_post_state(&diff, &factories).is_ok());
    }

    #[test]
    fn match_no_hit_when_remove_pattern_misses_registry() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"remove_nodes": [{"match": {"name": "absent"}}]});
        let registry_names: Vec<String> = vec!["present".into()];
        let err = validate_post_state_full(&diff, &factories, &registry_names).unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    #[test]
    fn naming_collision_when_add_node_duplicates_existing_name() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"add_nodes": [{"name": "x", "template": "echo"}]});
        let registry_names: Vec<String> = vec!["x".into()];
        let err = validate_post_state_full(&diff, &factories, &registry_names).unwrap_err();
        assert_eq!(err.error_code(), "naming_collision");
    }

    /// Substrate-fix finding 7 — two `add_nodes` with the SAME name in one diff
    /// form a post_state duplicate (spec § Naming collisions: "a node name that
    /// occurs twice within the same scope in the post_state"). This is
    /// `naming_collision` and must be caught in VALIDATE — before staging — so
    /// it never reaches the rename step, where the second `rename(2)` failed
    /// with a `schema` (rename-IO) token AND left the first node's directory
    /// stranded in `{root}` (no spurless reject).
    #[test]
    fn in_diff_duplicate_add_node_name_is_naming_collision() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"add_nodes": [
            {"name": "dup", "template": "echo"},
            {"name": "dup", "template": "echo"}
        ]});
        let registry_names: Vec<String> = vec![];
        let err = validate_post_state_full(&diff, &factories, &registry_names).unwrap_err();
        assert_eq!(err.error_code(), "naming_collision");
    }

    /// Substrate fix, finding 2 — spec overview § Validation: "cycle-freedom …
    /// insofar as the application forbids cycles; meclaw-core does not reject
    /// cycles in general". A self-edge `a → a` (a tool/reply loop's degenerate
    /// form) is instantiable per mutation; meclaw-core does NOT reject it. The runtime
    /// TTL loop-guard (`hive_cycle_terminates_with_ttl_expired`) bounds it.
    #[test]
    fn self_edge_cycle_is_tolerated() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({"add_edges": [{"from": "a", "to": "a"}]});
        assert!(
            validate_post_state_with_edges(
                &diff,
                &factories,
                &registry_names,
                &existing_edges,
                &[],
            )
            .is_ok(),
            "meclaw-Core does not reject cycles (Befund 2)"
        );
    }

    /// Befund 2 — a 2-node reply loop (`a ⇄ b`, the bot-basic reply-leg shape)
    /// must validate; the identical shape boots fine from the filesystem, so the
    /// mutation path matches that tolerance.
    #[test]
    fn two_node_loop_cycle_is_tolerated() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![("a".into(), "b".into())];
        let diff = json!({"add_edges": [{"from": "b", "to": "a"}]});
        assert!(
            validate_post_state_with_edges(
                &diff,
                &factories,
                &registry_names,
                &existing_edges,
                &[],
            )
            .is_ok(),
            "meclaw-Core does not reject cycles (Befund 2)"
        );
    }

    #[test]
    fn dag_passes_cycle_check() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let existing_edges: Vec<(String, String)> = vec![("a".into(), "b".into())];
        let diff = json!({"add_edges": [{"from": "b", "to": "c"}]});
        assert!(
            validate_post_state_with_edges(
                &diff,
                &factories,
                &registry_names,
                &existing_edges,
                &[],
            )
            .is_ok()
        );
    }

    /// Substrat-Fix Befund 6 — an `add_edges` edge may reference a node added in
    /// the SAME diff (spec § Mutation-Format: post_state validation). The
    /// canonical mutation endpoint form is `./name` (overview § Variablen-
    /// Substitution example). Both `./a`/`./b` and the bare `a`/`b` must resolve
    /// against the diff's `add_nodes` short-names — `./`-prefixed endpoints were
    /// rejected as `edge_schema` ("from='./a' unknown") because the membership
    /// check compared the raw string against bare short-names.
    #[test]
    fn add_edges_dot_slash_endpoints_resolve_against_same_diff_add_nodes() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec![];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_nodes": [
                {"name": "a", "template": "echo"},
                {"name": "b", "template": "echo"}
            ],
            "add_edges": [{"from": "./a", "to": "./b"}]
        });
        assert!(
            validate_post_state_with_edges(
                &diff,
                &factories,
                &registry_names,
                &existing_edges,
                &[],
            )
            .is_ok(),
            "`./a`/`./b` must resolve against same-diff add_nodes (Befund 6)"
        );
    }

    /// Befund 6 control: a `./ghost` endpoint with no matching node (neither in
    /// registry nor add_nodes) still rejects — the fix normalises the form, it
    /// does not weaken the existence check.
    #[test]
    fn add_edges_dot_slash_endpoint_to_unknown_node_still_rejects() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({"add_edges": [{"from": "./a", "to": "./ghost"}]});
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "edge_schema");
    }

    #[test]
    fn edge_schema_rejected_when_endpoint_unknown() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({"add_edges": [{"from": "a", "to": "ghost"}]});
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "edge_schema");
    }

    /// Phase 13.5-A1 T4: malformed CEL `condition` → EdgeSchema(... cel ...).
    #[test]
    fn validate_with_edges_rejects_malformed_cel_condition() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["x".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{"from": "x", "to": "x", "condition": "hop.foo ==="}]
        });
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("cel"), "msg should mention cel: {msg}");
                assert!(
                    msg.contains("condition"),
                    "msg should mention condition: {msg}"
                );
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// Phase 13.5-A1 T4: valid CEL condition is not rejected on cel-grounds.
    /// (self-edge `x → x` still trips the cycle check, but NOT EdgeSchema(cel).)
    #[test]
    fn validate_with_edges_accepts_valid_cel_condition() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["x".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{"from": "x", "to": "x", "condition": "hop.foo == 'bar'"}]
        });
        let result = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        );
        if let Err(MutationError::EdgeSchema(msg)) = &result {
            assert!(
                !msg.contains("cel"),
                "valid CEL must not be rejected: {msg}"
            );
        }
    }

    /// Phase 13.5-A1 T4 (Slice 3): malformed CEL `modifier.set_hop.*` → EdgeSchema.
    #[test]
    fn validate_with_edges_rejects_malformed_cel_modifier_set() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "modifier": {"set_hop": {"tier": "hop.priority ==="}}
            }]
        });
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("cel"), "msg should mention cel: {msg}");
                assert!(
                    msg.contains("modifier.set_hop"),
                    "msg should mention modifier.set_hop: {msg}"
                );
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// Phase 13.5-A1 T4 (Slice 3): `modifier.delete_hop` must be an array if present.
    #[test]
    fn validate_with_edges_rejects_non_array_modifier_delete() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "modifier": {"delete_hop": "not_an_array"}
            }]
        });
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("delete"), "msg should mention delete: {msg}")
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// GH #82 (ruling 2026-08-13): the mutation path enforces the same minimum
    /// as config load — a `modifier.restore_ttl` edge is exempt from the TTL
    /// loop guard, so it must carry a bound of its own. No `condition` → reject.
    #[test]
    fn validate_with_edges_rejects_ttl_restoring_edge_without_condition() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "modifier": {"restore_ttl": true}
            }]
        });
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(
                    msg.contains("restore_ttl") && msg.contains("condition"),
                    "msg must name the field and the missing bound: {msg}"
                );
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// The counterpart: a restoring edge bounded by an iteration condition is a
    /// legal `add_edges` entry — `restore_ttl` is a known modifier key, not an
    /// unknown one.
    #[test]
    fn validate_with_edges_accepts_bounded_ttl_restoring_edge() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "condition": "int(context.iter) < 12",
                "modifier": {
                    "set_context": {"iter": "int(context.iter) + 1"},
                    "restore_ttl": true
                }
            }]
        });
        validate_post_state_with_edges(&diff, &factories, &registry_names, &existing_edges, &[])
            .expect("a restoring edge bounded by an iteration condition must validate");
    }

    /// `restore_ttl` is a declaration, not an expression: a non-boolean value is
    /// a schema error rather than a silently ignored key.
    #[test]
    fn validate_with_edges_rejects_non_boolean_restore_ttl() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "condition": "int(context.iter) < 12",
                "modifier": {"restore_ttl": "yes"}
            }]
        });
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(
                    msg.contains("restore_ttl") && msg.contains("boolean"),
                    "msg must say restore_ttl is boolean: {msg}"
                );
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// Substrate fix, modifier schema (post-finding-6): a `modifier` key outside
    /// the `{set_context, delete_context, set_hop, delete_hop}` schema (spec
    /// overview § Validation l.277: "modifier (if set) matches the
    /// {set?, delete?} schema") must reject as `edge_schema`. The old flat
    /// `{"headers.X": ...}` map form used to commit silently (the unknown key
    /// was ignored at apply) — a builder foot-gun: silent no-op instead of
    /// schema feedback.
    #[test]
    fn validate_with_edges_rejects_unknown_modifier_key() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "modifier": {"headers.msg_type": "'old_form'"}
            }]
        });
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(
                    msg.contains("headers.msg_type"),
                    "msg should name the unknown key: {msg}"
                );
                assert!(
                    msg.contains("modifier"),
                    "msg should mention modifier: {msg}"
                );
            }
            other => panic!("expected EdgeSchema for unknown modifier key, got {other:?}"),
        }
    }

    /// modifier-Schema control: a modifier that uses ONLY the four valid keys is
    /// not rejected on schema grounds.
    #[test]
    fn validate_with_edges_accepts_all_four_valid_modifier_keys() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "modifier": {
                    "set_context": {"k": "hop.k"},
                    "delete_context": ["old"],
                    "set_hop": {"h": "hop.h"},
                    "delete_hop": ["gone"]
                }
            }]
        });
        assert!(
            validate_post_state_with_edges(
                &diff,
                &factories,
                &registry_names,
                &existing_edges,
                &[],
            )
            .is_ok(),
            "the four canonical modifier keys must pass the schema check"
        );
    }

    /// Slice 4: malformed CEL `modifier.set_context.<k>` → EdgeSchema, message
    /// names `set_context.iter` (schema-compat coverage for the context path).
    #[test]
    fn validate_with_edges_rejects_malformed_cel_modifier_set_context() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "modifier": {"set_context": {"iter": "=="}}
            }]
        });
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("cel"), "msg should mention cel: {msg}");
                assert!(
                    msg.contains("set_context.iter"),
                    "msg should mention set_context.iter: {msg}"
                );
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// Slice 4: a well-formed four-field modifier (all of `set_context`,
    /// `delete_context`, `set_hop`, `delete_hop` valid) is not rejected on
    /// modifier-schema grounds (no `EdgeSchema` from the modifier-parse path).
    #[test]
    fn validate_with_edges_accepts_well_formed_four_field_modifier() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({
            "add_edges": [{
                "from": "a", "to": "b",
                "modifier": {
                    "set_context": {"tag": "'gold'"},
                    "delete_context": ["stale"],
                    "set_hop": {"tier": "hop.priority"},
                    "delete_hop": ["transient"]
                }
            }]
        });
        let result = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        );
        assert!(
            result.is_ok(),
            "well-formed four-field modifier must be accepted: {result:?}"
        );
    }

    /// Phase 13.5 step-6: an `add_edges` endpoint that names a hive (by its
    /// short-name) is accepted when `hive_endpoint_names` carries it — symmetric
    /// to the cell case.
    #[test]
    fn edge_schema_accepts_hive_endpoint_via_hive_names() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let hive_names: Vec<String> = vec!["pool".into()];
        // `from=a` (cell) → `to=pool` (hive short-name): no EdgeSchema error.
        let diff = json!({"add_edges": [{"from": "a", "to": "pool"}]});
        let result = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &hive_names,
        );
        assert!(result.is_ok(), "hive endpoint must be accepted: {result:?}");
    }

    /// Phase 13.5 step-6: a truly unknown endpoint still trips EdgeSchema, even
    /// with `hive_endpoint_names` populated (the unknown name is in neither set).
    #[test]
    fn edge_schema_still_rejects_unknown_with_hive_names_present() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let hive_names: Vec<String> = vec!["pool".into()];
        let diff = json!({"add_edges": [{"from": "a", "to": "ghost"}]});
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &hive_names,
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "edge_schema");
    }

    #[test]
    fn edge_schema_accepts_endpoint_added_in_same_diff() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        // "newcomer" is born via add_nodes in the same diff — edge endpoint ok.
        let diff = json!({
            "add_nodes": [{"name": "newcomer", "template": "echo"}],
            "add_edges": [{"from": "a", "to": "newcomer"}]
        });
        assert!(
            validate_post_state_with_edges(
                &diff,
                &factories,
                &registry_names,
                &existing_edges,
                &[],
            )
            .is_ok()
        );
    }

    // ── T14: validate_post_state_with_templates ──────────────────────────────

    struct StubFactory;
    impl crate::CellFactory for StubFactory {
        fn validate_params(&self, _: &JsonValue) -> Result<(), String> {
            Ok(())
        }
        fn spawn_cell(
            self: Arc<Self>,
            _: meclaw_core::Path,
            _: JsonValue,
            _: tokio::sync::mpsc::Sender<meclaw_core::CellEmission>,
            _cell_dir: std::path::PathBuf,
            _contract: crate::ContractView,
            _colony_inbox_tx: tokio::sync::mpsc::Sender<crate::ColonyMsg>,
            _idle_timeout: Option<std::time::Duration>,
            _cell_timeout: i64,
            _message_timeout: Option<std::time::Duration>,
            _blob_store: Option<std::sync::Arc<crate::DiskBlobStore>>,
            _mailbox_capacity: usize,
        ) -> Result<crate::SpawnedCellKind, String> {
            unimplemented!()
        }
    }

    // ── GH #437: `add_nodes[].birth` ─────────────────────────────────────

    /// GH #437: the birth state is a CLOSED vocabulary, and an unknown value is
    /// refused PRE-DESTRUCTIVELY — before anything is staged, like every other
    /// grammar refusal of this stage. The message names the site, quotes the
    /// offending value and lists the values that DO exist, because a closed
    /// vocabulary that does not say what it contains is a riddle.
    #[test]
    fn an_unknown_birth_value_is_refused_with_schema_and_names_the_alternatives() {
        let diff = json!({"add_nodes": [
            {"name": "p", "template": "poller", "birth": "asleep"}
        ]});
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .expect_err("an unknown birth value must be refused");
        assert_eq!(err.error_code(), "schema");
        let msg = err.message();
        assert!(
            msg.contains("add_nodes[0].birth"),
            "must name the site: {msg}"
        );
        assert!(
            msg.contains("'asleep'"),
            "must quote the offending value: {msg}"
        );
        assert!(
            msg.contains("active") && msg.contains("inactive"),
            "must list the values that DO exist: {msg}"
        );
    }

    /// The refusal comes BEFORE the template lookup: a diff whose birth value is
    /// wrong is refused for that, not for a template that was never reached.
    /// That is what "pre-destructive" means here.
    #[test]
    fn the_birth_refusal_precedes_the_template_lookup() {
        let diff = json!({"add_nodes": [
            {"name": "p", "template": "does-not-exist", "birth": "dormant"}
        ]});
        let err = validate_post_state_with_templates(
            &diff,
            &crate::templates::TemplatesRegistry::default(),
            &CellFactoryRegistry::new(),
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .expect_err("must be refused");
        assert_eq!(err.error_code(), "schema", "not template_missing: {err:?}");
    }

    /// The collecting validator asks the SAME grammar as the `Result` form —
    /// GH #293 discipline, so the two faces cannot drift apart.
    #[test]
    fn the_collecting_validator_sees_the_same_birth_violation() {
        let diff = json!({"add_nodes": [
            {"name": "a", "template": "poller", "birth": "asleep"}
        ]});
        let mut rejection = crate::mutation::rejection::MutationRejection::new();
        collect_post_state_addresses(
            &diff,
            &crate::templates::TemplatesRegistry::default(),
            &CellFactoryRegistry::new(),
            &[],
            &[],
            &[],
            "/",
            &[],
            &[],
            &mut rejection,
        );
        assert!(
            rejection
                .entries()
                .iter()
                .any(|v| v.message.contains("add_nodes[0].birth")),
            "the collecting face must report the birth violation too: {:?}",
            rejection.entries()
        );
    }

    /// An `adopt` entry declares its birth state too: adopting a directory
    /// instantiates a cell at an address, and that instantiation has an
    /// activity like any other. So the grammar is checked BEFORE the adopt
    /// branch, and a wrong value there is refused as well.
    #[test]
    fn an_adopt_entry_is_held_to_the_same_birth_grammar() {
        let diff = json!({"add_nodes": [
            {"name": "a", "adopt": {"type": "echo"}, "birth": "asleep"}
        ]});
        let err = validate_post_state_with_templates(
            &diff,
            &crate::templates::TemplatesRegistry::default(),
            &CellFactoryRegistry::new(),
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .expect_err("must be refused");
        assert_eq!(err.error_code(), "schema");
        assert!(err.message().contains("birth"), "{}", err.message());
        // …and the legal value passes the grammar (the entry then fails, or
        // not, for its own reasons — never for the declaration).
        let ok = json!({"add_nodes": [
            {"name": "a", "adopt": {"type": "echo"}, "birth": "inactive"}
        ]});
        assert!(
            validate_post_state_with_templates(
                &ok,
                &crate::templates::TemplatesRegistry::default(),
                &CellFactoryRegistry::new(),
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
            )
            .is_ok(),
            "an adopt entry may declare `birth: inactive`"
        );
    }

    /// Absent = the shipped default. A wrong TYPE is a grammar error, not a
    /// silently ignored key.
    #[test]
    fn birth_defaults_to_active_and_a_non_string_is_a_grammar_error() {
        use crate::mutation::Birth;
        assert_eq!(
            Birth::parse(&json!({"name": "a"}), "add_nodes[0]").unwrap(),
            Birth::Active
        );
        assert_eq!(
            Birth::parse(&json!({"birth": "inactive"}), "add_nodes[0]").unwrap(),
            Birth::Inactive
        );
        assert_eq!(
            Birth::parse(&json!({"birth": "active"}), "add_nodes[0]").unwrap(),
            Birth::Active
        );
        assert!(Birth::parse(&json!({"birth": true}), "add_nodes[0]").is_err());
        assert!(Birth::parse(&json!({"birth": null}), "add_nodes[0]").is_err());
    }

    #[test]
    fn validate_rejects_unknown_template() {
        let diff = json!({"add_nodes": [{"name":"x","template":"unknown"}]});
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, MutationError::TemplateMissing(_)));
    }

    #[test]
    fn swap_nodes_rejects_unknown_with_template() {
        // match.name hits "existing" in the registry — rejection must be about
        // the unknown with.template (template_missing), not a match_no_hit.
        // with.name is required (paket-2 T1); target "newnode" is fresh (no collision).
        let diff = json!({
            "swap_nodes": [{"match": {"name": "existing"}, "with": {"template": "unknown", "name": "newnode"}}]
        });
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let registry_names: Vec<String> = vec!["existing".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "template_missing");
    }

    #[test]
    fn validate_rejects_unknown_cell_type_even_if_template_exists() {
        use std::path::PathBuf;
        let diff = json!({"add_nodes": [{"name":"x","template":"echo"}]});
        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "t1".into(),
                name: "echo".into(),
                version: None,
                filesystem_path: PathBuf::from("/tmp/echo"),
            },
        ]);
        let factories = CellFactoryRegistry::new(); // empty → cell.type lookup fails
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &[],
            &[],
            // simulate: the template directory carries a config.json with cell.type = "ghost"
            &[("echo".into(), "ghost".into())],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert!(matches!(err, MutationError::UnknownCellType(_)));
    }

    #[test]
    fn validate_passes_when_template_and_cell_type_both_known() {
        use std::path::PathBuf;
        let diff = json!({"add_nodes": [{"name":"x","template":"echo"}]});
        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "t1".into(),
                name: "echo".into(),
                version: None,
                filesystem_path: PathBuf::from("/tmp/echo"),
            },
        ]);
        let mut factories = CellFactoryRegistry::new();
        factories.insert(
            "echo_type".into(),
            Arc::new(StubFactory) as Arc<dyn crate::CellFactory>,
        );
        let result = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &[],
            &[],
            &[("echo".into(), "echo_type".into())],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(result.is_ok());
    }

    /// T17.fix — R3 conformance: versioned template refs must be resolved via entry.name.
    ///
    /// Bug: the ct_map lookup used the raw `template` string ("echo@1.0.0") instead of
    /// `entry.name` ("echo").
    /// Fix: resolve() returns a TemplateEntry; the ct_map lookup uses entry.name.
    #[test]
    fn validate_resolves_versioned_template_ref() {
        use std::path::PathBuf;
        let diff = json!({"add_nodes": [{"name":"x","template":"echo@1.0.0"}]});
        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "t1".into(),
                name: "echo".into(),
                version: Some("1.0.0".into()),
                filesystem_path: PathBuf::from("/tmp/echo"),
            },
        ]);
        let mut factories = CellFactoryRegistry::new();
        factories.insert(
            "echo_type".into(),
            Arc::new(StubFactory) as Arc<dyn crate::CellFactory>,
        );
        let result = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &[],
            &[],
            &[("echo".into(), "echo_type".into())],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(
            result.is_ok(),
            "versioned template ref must resolve via entry.name"
        );
    }

    // ── Phase 13.5 Slice 4 T1: swap_nodes validate arm ──────────────────────

    /// T1 (RED): swap_nodes with a match.name that hits no registry cell must be
    /// rejected with error_code == "match_no_hit".
    #[test]
    fn swap_nodes_match_no_hit_is_rejected() {
        let factories = factories_with(&["echo"]);
        let diff =
            json!({"swap_nodes": [{"match": {"name": "absent"}, "with": {"template": "echo"}}]});
        let registry_names: Vec<String> = vec!["present".into()];
        let err = validate_post_state_full(&diff, &factories, &registry_names).unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    // ── T7: validate_post_state_with_edges_and_subtree ──────────────────────

    /// T7 (RED): a subtree-internal edge whose target is a subtree-nested node
    /// validates — the nested node counts as a valid edge endpoint.
    #[test]
    fn subtree_internal_edge_to_nested_node_validates() {
        let factories = factories_with(&["echo"]);
        // No diff add_edges; the subtree contributes its own nodes + internal edge.
        let diff = json!({});
        let subtree_nodes: Vec<String> = vec![
            "/main/m1".into(),
            "/main/m1/inner_a".into(),
            "/main/m1/inner_b".into(),
        ];
        let subtree_edges: Vec<(String, String)> =
            vec![("/main/m1/inner_a".into(), "/main/m1/inner_b".into())];
        let result = validate_post_state_with_edges_and_subtree(
            &diff,
            &factories,
            &[],
            &[],
            &[],
            &subtree_nodes,
            &subtree_edges,
        );
        assert!(
            result.is_ok(),
            "nested subtree edge must validate: {result:?}"
        );
    }

    /// Substrat-Fix Befund 2: a subtree-internal edge-set that forms a cycle
    /// (a→b, b→a) is TOLERATED — meclaw-Core does not reject cycles generally
    /// (spec overview § Validation). Endpoint-existence still applies (both
    /// endpoints are in `subtree_nodes`), so this validates clean.
    #[test]
    fn subtree_internal_edge_cycle_is_tolerated() {
        let factories = factories_with(&["echo"]);
        let diff = json!({});
        let subtree_nodes: Vec<String> = vec!["/main/m1/a".into(), "/main/m1/b".into()];
        let subtree_edges: Vec<(String, String)> = vec![
            ("/main/m1/a".into(), "/main/m1/b".into()),
            ("/main/m1/b".into(), "/main/m1/a".into()),
        ];
        assert!(
            validate_post_state_with_edges_and_subtree(
                &diff,
                &factories,
                &[],
                &[],
                &[],
                &subtree_nodes,
                &subtree_edges,
            )
            .is_ok(),
            "cyclic subtree-internal edges are tolerated (Befund 2)"
        );
    }

    /// T7 (RED): a subtree-internal edge whose endpoint is absent from the
    /// node-set is rejected with the edge-schema error.
    #[test]
    fn subtree_internal_edge_endpoint_not_in_nodeset_is_rejected() {
        let factories = factories_with(&["echo"]);
        let diff = json!({});
        let subtree_nodes: Vec<String> = vec!["/main/m1/a".into()];
        // edge points at /main/m1/ghost which is in neither node-set.
        let subtree_edges: Vec<(String, String)> =
            vec![("/main/m1/a".into(), "/main/m1/ghost".into())];
        let err = validate_post_state_with_edges_and_subtree(
            &diff,
            &factories,
            &[],
            &[],
            &[],
            &subtree_nodes,
            &subtree_edges,
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "edge_schema");
    }

    /// T7 (RED): the OLD wrapper on a single-cell diff is byte-behavior-unchanged
    /// — a valid single-cell diff still returns `Ok(())` (guards the delegation).
    #[test]
    fn existing_validate_post_state_with_edges_unchanged_for_single_cell() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        let existing_edges: Vec<(String, String)> = vec![("a".into(), "b".into())];
        let diff = json!({"add_edges": [{"from": "b", "to": "c"}]});
        assert!(
            validate_post_state_with_edges(
                &diff,
                &factories,
                &registry_names,
                &existing_edges,
                &[],
            )
            .is_ok()
        );
    }

    /// Pin: `swap_nodes` is **name-keyed** → a single swap entry resolves to
    /// EXACTLY ONE target (`resolve_scoped_path(scope, match.name)`). A match
    /// WITHOUT `name` (e.g. a `template`-only match that could otherwise hit >1
    /// cell) is rejected at validation with `error_code == "schema"` — it is NEVER
    /// silently applied to multiple cells. This nails the single-target invariant
    /// behind the deferred "Multi-Target-swap atomicity" item (only a swap_nodes
    /// ARRAY with multiple entries is multi-target, and that is applied per-entry
    /// after the whole diff has validated).
    #[test]
    fn swap_nodes_match_without_name_is_rejected_schema() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"swap_nodes": [
            {"match": {"template": "echo"}, "with": {"template": "echo"}}
        ]});
        let registry_names: Vec<String> = vec!["present".into()];
        let err = validate_post_state_full(&diff, &factories, &registry_names).unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    // ── paket-2 T1: swap_nodes with-schema + strict rejects + scope-binding ──

    /// Rule 1 (Strict-Regel c): `with` missing entirely → schema error.
    #[test]
    fn swap_nodes_with_missing_is_schema() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let diff = json!({"swap_nodes": [{"match": {"name": "t2"}}]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Rule 1 (Strict-Regel c): instantiate form with unknown key → schema error.
    /// Typo `"tempalte"` must NOT be silently treated as existing-node form.
    #[test]
    fn swap_nodes_instantiate_unknown_key_is_schema() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // "tempalte" is a typo — has no "template" key, has unknown key → schema.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"tempalte": "echo", "name": "t3"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Rule 1 (Strict-Regel c): existing-node form with unknown key → schema error.
    #[test]
    fn swap_nodes_existing_unknown_key_is_schema() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // No "template" key, but "extra" key is unknown → schema.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"name": "t2", "extra": "bad"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Rule 1: new instantiate form (`template` + `name` + unknown extra key) → schema error.
    /// Legacy form `{template}` (no `name`) is backward-compatible and NOT rejected here.
    #[test]
    fn swap_nodes_new_instantiate_extra_key_is_schema() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // Has both template AND name → new form. Extra key "foo" → schema.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"template": "echo", "name": "t3", "foo": "bad"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Rule 1: `with.name` missing in existing-node form (empty `with`) → schema error.
    #[test]
    fn swap_nodes_existing_name_missing_is_schema() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // Empty `with` object — no name → schema.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Rule 3 (Strict-Regel a): existing-node form where t3 is not in registry →
    /// MatchNoHit (scope-bound rejection, loud).
    #[test]
    fn swap_nodes_existing_target_not_found_is_match_no_hit() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"name": "t3_ghost"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()]; // t3_ghost not present
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    /// Rule 3: existing-node form where t3 IS in registry → passes validation.
    #[test]
    fn swap_nodes_existing_target_found_passes() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"name": "t3"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into(), "t3".into()];
        let result = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(
            result.is_ok(),
            "existing-form with known t3 must pass: {result:?}"
        );
    }

    /// Rule 4 (Strict-Regel b): instantiate form where t3 is already in registry →
    /// NamingCollision.
    #[test]
    fn swap_nodes_instantiate_target_collision_is_naming_collision() {
        use std::path::PathBuf;
        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "t1".into(),
                name: "echo".into(),
                version: None,
                filesystem_path: PathBuf::from("/tmp/echo_single"),
            },
        ]);
        let mut factories = CellFactoryRegistry::new();
        factories.insert(
            "echo_type".into(),
            Arc::new(StubFactory) as Arc<dyn crate::CellFactory>,
        );
        // t3 = "already_there" is already in registry → NamingCollision.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"template": "echo", "name": "already_there"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into(), "already_there".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[("echo".into(), "echo_type".into())],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "naming_collision");
    }

    /// Rule 4: instantiate form where t3 is NOT in registry → no collision, passes.
    /// Uses a real temp dir so `reject_if_subtree_template` can scan it.
    #[test]
    fn swap_nodes_instantiate_fresh_target_passes() {
        // Build a minimal single-cell template dir (no nested config.json → not a subtree).
        let tmp = tempfile::tempdir().unwrap();
        let tpl_dir = tmp.path().to_path_buf();
        std::fs::write(tpl_dir.join("template.json"), r#"{"name":"echo"}"#).unwrap();
        std::fs::write(
            tpl_dir.join("config.json"),
            r#"{"cell":{"type":"echo_type"}}"#,
        )
        .unwrap();

        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "t1".into(),
                name: "echo".into(),
                version: None,
                filesystem_path: tpl_dir,
            },
        ]);
        let mut factories = CellFactoryRegistry::new();
        factories.insert(
            "echo_type".into(),
            Arc::new(StubFactory) as Arc<dyn crate::CellFactory>,
        );
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"template": "echo", "name": "t3_fresh"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let result = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[("echo".into(), "echo_type".into())],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(
            result.is_ok(),
            "instantiate-form with fresh t3 must pass: {result:?}"
        );
    }

    /// GH #572 (ruling O-0904-1): instantiate form whose template is a hive
    /// with NOTHING under it → refused by name, at stage 4.
    ///
    /// The template dir carries a hive root and no nested cell directory, so
    /// the subtree guard one line above lets it through: this is the shape that
    /// used to validate clean and die in the apply arm as
    /// `spawn: factory missing for hive`.
    #[test]
    fn swap_nodes_instantiate_hive_template_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let tpl_dir = tmp.path().to_path_buf();
        std::fs::write(tpl_dir.join("template.json"), r#"{"name":"hive_tpl"}"#).unwrap();
        std::fs::write(
            tpl_dir.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{}}"#,
        )
        .unwrap();

        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "h1".into(),
                name: "hive_tpl".into(),
                version: None,
                filesystem_path: tpl_dir,
            },
        ]);
        let factories = CellFactoryRegistry::new();
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"template": "hive_tpl", "name": "t3_fresh"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[("hive_tpl".into(), "hive".into())],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(
            err.error_code(),
            "hive_template_single_cell",
            "a hive alone has no factory and no way in, got {err:?}"
        );
    }

    /// Rule 5 (A6 subtree-guard): instantiate form whose template resolves to a
    /// SUBTREE template → schema error.
    ///
    /// Uses a real temp dir with a nested `config.json` to trigger the subtree
    /// guard in `reject_if_subtree_template`.
    #[test]
    fn swap_nodes_instantiate_subtree_template_is_schema() {
        use std::path::PathBuf;
        // Build a temp dir that looks like a subtree template (has nested dir with config.json).
        let tmp = tempfile::tempdir().unwrap();
        let tpl_dir = tmp.path();
        std::fs::write(tpl_dir.join("template.json"), r#"{"name":"subtree_tpl"}"#).unwrap();
        std::fs::write(tpl_dir.join("config.json"), r#"{"cell":{"type":"hive"}}"#).unwrap();
        let nested = tpl_dir.join("inner_cell");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("config.json"), r#"{"cell":{"type":"echo"}}"#).unwrap();

        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "st1".into(),
                name: "subtree_tpl".into(),
                version: None,
                filesystem_path: PathBuf::from(tpl_dir),
            },
        ]);
        let factories = CellFactoryRegistry::new();
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"template": "subtree_tpl", "name": "t3_fresh"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(
            err.error_code(),
            "schema",
            "subtree template must reject as schema, got {err:?}"
        );
    }

    /// Instantiate form where `name` is a non-string (e.g. `{"name": 42}`) →
    /// the `.as_str()` conversion fails and the `ok_or_else(Schema)` fires.
    /// This is an intentional schema reject, not dead code.
    #[test]
    fn swap_nodes_instantiate_name_non_string_is_schema() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // Both template + name present (instantiate form), but name is an integer.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"template": "echo", "name": 42}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(
            err.error_code(),
            "schema",
            "non-string name must be a schema error, got {err:?}"
        );
    }

    // ── T5 Part 1: with.name post-state resolution (forward reference) ─────────

    /// T5-P1 direct unit test: validate_swap_with_entry must accept a `with.name`
    /// that is in `add_names` (post-state) but NOT in `registry_names` (pre-state).
    /// And it must still reject a `with.name` that is in neither.
    #[test]
    fn validate_swap_with_entry_accepts_add_names_forward_reference() {
        let templates = crate::templates::TemplatesRegistry::default();

        // Existing-form `with: {name: "t3"}`. t3 NOT in pre-state registry_names,
        // but IS in add_names (post-state contribution from add_nodes in same diff).
        let with_val = json!({"name": "t3"});
        let registry_names: Vec<String> = vec!["t2".into()];
        let add_names: Vec<String> = vec!["t3".into()]; // t3 added by add_nodes in same diff

        // Should PASS: t3 is reachable via post-state (registry ∪ add_names).
        // match_name = "t2" (the source being swapped); with.name = "t3" ≠ "t2".
        let result = validate_swap_with_entry_full(
            &with_val,
            "t2",
            &registry_names,
            &add_names,
            &templates,
            "/",
            &[],
            &[],
            &std::collections::HashMap::new(),
        );
        assert!(
            result.is_ok(),
            "with.name forward ref from add_nodes must pass: {result:?}"
        );

        // Ghost: in neither pre-state nor add_names → still match_no_hit.
        let with_ghost = json!({"name": "ghost"});
        let err = validate_swap_with_entry_full(
            &with_ghost,
            "t2",
            &registry_names,
            &add_names,
            &templates,
            "/",
            &[],
            &[],
            &std::collections::HashMap::new(),
        )
        .unwrap_err();
        assert_eq!(
            err.error_code(),
            "match_no_hit",
            "ghost name must still reject as match_no_hit: {err:?}"
        );
    }

    /// T13 (identity-swap reject): existing-node form where with.name == match.name
    /// (same node, same scope) must be rejected with error_code == "schema".
    /// Swapping a node onto itself drops all edges (every swung edge becomes a
    /// self-loop and is dropped) — that is a loud spec violation, not a silent no-op.
    #[test]
    fn swap_nodes_existing_identity_swap_is_schema() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // with.name == match.name → identity swap → must be rejected.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"name": "t2"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(
            err.error_code(),
            "schema",
            "identity swap must be schema error, got {err:?}"
        );
    }

    /// T13 guard: existing-node form with.name != match.name must still pass.
    #[test]
    fn swap_nodes_existing_non_identity_swap_still_passes() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"name": "t3"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into(), "t3".into()];
        let result = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(
            result.is_ok(),
            "normal existing-form swap must pass: {result:?}"
        );
    }

    /// Rule 6 (A2 scope-binding): `with.name` (existing form) naming a node that
    /// lives in a DIFFERENT scope is rejected as MatchNoHit because the caller
    /// (colony.rs) filters `registry_names` to scope-local names only.
    /// Here we model the filtered set: the cross-scope name is absent.
    #[test]
    fn swap_nodes_existing_target_cross_scope_is_match_no_hit() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // The diff names t3 = "other_scope_node".
        // registry_names is filtered to current scope — "other_scope_node" absent.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "t2"}, "with": {"name": "other_scope_node"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()]; // scope-filtered: no "other_scope_node"
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    /// Paket-5 T4 (P10b companion, finding Paket-2-b'): a `swap_nodes` `match.name`
    /// naming a HIVE that lives in a FOREIGN scope must be rejected as MatchNoHit.
    /// The caller scope-filters `hive_match_names` (parent path == guard_scope), so a
    /// hive in another scope never contributes its short-name here — the global
    /// `hive_endpoint_names` (8th→7th arg) MUST NOT short-circuit this. Modelled by
    /// passing the foreign hive ONLY in the global endpoint set, NOT in the scoped
    /// match set. Before the split this was a false-positive pass.
    #[test]
    fn swap_nodes_match_name_foreign_scope_hive_is_match_no_hit() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let diff = json!({"swap_nodes": [
            {"match": {"name": "pool"}, "with": {"name": "t3"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into()];
        // Global endpoint set carries the foreign hive (it lives elsewhere in the
        // colony); the SCOPED match set does NOT — so match.name must miss.
        let hive_endpoint_names: Vec<String> = vec!["pool".into()];
        let hive_match_names: Vec<String> = vec![];
        let err = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &hive_endpoint_names,
            &hive_match_names,
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    /// Paket-5 T4 (P10b companion): a `swap_nodes` `match.name` naming a HIVE in the
    /// CORRECT scope still passes — the scoped `hive_match_names` carries it, so no
    /// false-negative. Mirror of `edge_schema_accepts_hive_endpoint_via_hive_names`
    /// but for the match.name existence check.
    #[test]
    fn swap_nodes_match_name_in_scope_hive_passes() {
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        // match.name = "pool" (the in-scope hive). `with` is the existing-node form
        // naming a real registry target so the swap's `with` check also passes — that
        // isolates the assertion on the match.name step succeeding for the hive.
        let diff = json!({"swap_nodes": [
            {"match": {"name": "pool"}, "with": {"name": "target_cell"}}
        ]});
        let registry_names: Vec<String> = vec!["t2".into(), "target_cell".into()];
        let hive_endpoint_names: Vec<String> = vec!["pool".into()];
        let hive_match_names: Vec<String> = vec!["pool".into()]; // scope-local hive
        let result = validate_post_state_with_templates(
            &diff,
            &templates,
            &factories,
            &registry_names,
            &[],
            &[],
            &hive_endpoint_names,
            &hive_match_names,
            &[],
            &[],
        );
        // match.name "pool" is satisfied by the in-scope hive → no match_no_hit.
        // (The `with` here is an existing-node form with a fresh, non-colliding name,
        // which is accepted, so the overall result is Ok.)
        assert!(
            result.is_ok(),
            "in-scope hive match.name must pass, got: {result:?}"
        );
    }

    // ── Paket-5 T1/T2: remove_edges validate-time reject (P10a / D-031) ──────

    fn edge_view(from: &str, to: &str) -> EdgeMatchView {
        EdgeMatchView {
            from: from.into(),
            to: to.into(),
            condition_source: None,
            modifier_source: None,
            is_default: false,
        }
    }

    /// GH #283 (W4 T4): `match.default` is the fifth, OPTIONAL constraint. A
    /// pattern that names the phase hits only edges running in it; a pattern
    /// that omits it stays unconstrained and hits BOTH — the same convention
    /// `condition` and `modifier` already follow, asserted here so nobody has
    /// to infer it from the absence of a check.
    #[test]
    fn remove_edges_pattern_hits_constrains_on_the_default_phase() {
        let regular = edge_view("/main/a", "/main/b");
        let default = EdgeMatchView {
            is_default: true,
            ..regular.clone()
        };
        let hits = |e: &EdgeMatchView, pat: Option<bool>| {
            remove_edges_pattern_hits(e, "/main/a", "/main/b", None, None, pat)
        };

        // Pattern names the default phase → only the default edge.
        assert!(hits(&default, Some(true)), "default edge, default pattern");
        assert!(
            !hits(&regular, Some(true)),
            "a `default: true` pattern must NOT take the regular edge beside it"
        );
        // Pattern names the regular phase → only the regular edge.
        assert!(hits(&regular, Some(false)), "regular edge, regular pattern");
        assert!(
            !hits(&default, Some(false)),
            "a `default: false` pattern must NOT take the default edge beside it"
        );
        // Pattern omits the key → unconstrained, hits BOTH phases.
        assert!(
            hits(&regular, None) && hits(&default, None),
            "no `default` key => unconstrained: the pattern takes both phases"
        );
    }

    /// GH #283 (W4 T4): the constraint reaches `validate_remove_edges`, so a
    /// pattern naming a phase no live edge runs in is a loud `match_no_hit`
    /// instead of a silently widened removal.
    #[test]
    fn remove_edges_default_constraint_matches_and_misses() {
        let edges = vec![EdgeMatchView {
            is_default: true,
            ..edge_view("/main/a", "/main/b")
        }];
        let hit = json!({"remove_edges": [
            {"match": {"from": "a", "to": "b", "default": true}}
        ]});
        assert!(validate_remove_edges(&hit, "/main", &edges).is_ok());
        let miss = json!({"remove_edges": [
            {"match": {"from": "a", "to": "b", "default": false}}
        ]});
        let err = validate_remove_edges(&miss, "/main", &edges).unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    /// T1 (RED): a `remove_edges` entry with a missing `match.from` →
    /// Schema("remove_edges[].match.from missing").
    #[test]
    fn remove_edges_missing_from_is_schema() {
        let diff = json!({"remove_edges": [{"match": {"to": "b"}}]});
        let edges = vec![edge_view("/main/a", "/main/b")];
        let err = validate_remove_edges(&diff, "/main", &edges).unwrap_err();
        assert_eq!(err.error_code(), "schema");
        match err {
            MutationError::Schema(msg) => assert!(
                msg.contains("match.from"),
                "msg should mention match.from: {msg}"
            ),
            other => panic!("expected Schema, got {other:?}"),
        }
    }

    /// T1 (RED): a `remove_edges` entry with a missing `match.to` →
    /// Schema("remove_edges[].match.to missing").
    #[test]
    fn remove_edges_missing_to_is_schema() {
        let diff = json!({"remove_edges": [{"match": {"from": "a"}}]});
        let edges = vec![edge_view("/main/a", "/main/b")];
        let err = validate_remove_edges(&diff, "/main", &edges).unwrap_err();
        assert_eq!(err.error_code(), "schema");
        match err {
            MutationError::Schema(msg) => {
                assert!(
                    msg.contains("match.to"),
                    "msg should mention match.to: {msg}"
                )
            }
            other => panic!("expected Schema, got {other:?}"),
        }
    }

    /// T2 (RED): a `remove_edges` pattern matching ZERO existing edges →
    /// MatchNoHit (loud reject, NOT a silent no-op).
    #[test]
    fn remove_edges_no_hit_is_match_no_hit() {
        let diff = json!({"remove_edges": [{"match": {"from": "a", "to": "ghost"}}]});
        let edges = vec![edge_view("/main/a", "/main/b")];
        let err = validate_remove_edges(&diff, "/main", &edges).unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    /// T2 (positive): a `remove_edges` pattern hitting an existing edge (from/to
    /// only, scope-resolved) → Ok.
    #[test]
    fn remove_edges_hit_passes() {
        let diff = json!({"remove_edges": [{"match": {"from": "a", "to": "b"}}]});
        let edges = vec![edge_view("/main/a", "/main/b")];
        assert!(validate_remove_edges(&diff, "/main", &edges).is_ok());
    }

    /// T2 (positive, F6 condition): pattern with a `condition` matching the
    /// edge's stored condition source → Ok; a non-matching condition → no-hit.
    #[test]
    fn remove_edges_condition_constraint_matches_and_misses() {
        let edges = vec![EdgeMatchView {
            from: "/main/a".into(),
            to: "/main/b".into(),
            condition_source: Some("hop.x == 'y'".into()),
            modifier_source: None,
            is_default: false,
        }];
        // Matching condition → Ok.
        let hit = json!({"remove_edges": [
            {"match": {"from": "a", "to": "b", "condition": "hop.x == 'y'"}}
        ]});
        assert!(validate_remove_edges(&hit, "/main", &edges).is_ok());
        // Non-matching condition → MatchNoHit (constraint narrows the match).
        let miss = json!({"remove_edges": [
            {"match": {"from": "a", "to": "b", "condition": "hop.x == 'z'"}}
        ]});
        let err = validate_remove_edges(&miss, "/main", &edges).unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    /// T2 (positive, F6 modifier): pattern with a `modifier` matching the edge's
    /// stored modifier JSON → Ok.
    #[test]
    fn remove_edges_modifier_constraint_matches() {
        let modifier = json!({"set_hop": {"tier": "hop.priority"}});
        let edges = vec![EdgeMatchView {
            from: "/main/a".into(),
            to: "/main/b".into(),
            condition_source: None,
            modifier_source: Some(modifier.clone()),
            is_default: false,
        }];
        let diff = json!({"remove_edges": [
            {"match": {"from": "a", "to": "b", "modifier": modifier}}
        ]});
        assert!(validate_remove_edges(&diff, "/main", &edges).is_ok());
    }

    /// T2 (F6 modifier-miss): pattern with a `modifier` that does NOT equal the
    /// edge's stored modifier JSON → MatchNoHit (mirrors the condition-miss
    /// case; the modifier constraint narrows the match away).
    #[test]
    fn remove_edges_modifier_constraint_misses() {
        let edges = vec![EdgeMatchView {
            from: "/main/a".into(),
            to: "/main/b".into(),
            condition_source: None,
            modifier_source: Some(json!({"set_hop": {"tier": "hop.priority"}})),
            is_default: false,
        }];
        let miss = json!({"remove_edges": [
            {"match": {"from": "a", "to": "b", "modifier": {"set_hop": {"tier": "hop.other"}}}}
        ]});
        let err = validate_remove_edges(&miss, "/main", &edges).unwrap_err();
        assert_eq!(err.error_code(), "match_no_hit");
    }

    /// No `remove_edges` key at all → Ok (nothing to check).
    #[test]
    fn remove_edges_absent_passes() {
        let diff = json!({"add_nodes": []});
        assert!(validate_remove_edges(&diff, "/main", &[]).is_ok());
    }

    // ── Paket-5 T3: regression-lock pin-tests for already-loud diff arms ─────
    // These pin the CURRENT loud behavior (malformed / no-hit) of the other
    // diff arms so future refactors cannot silently regress it. They are
    // expected to PASS immediately; a failure here is a real gap, not a fix.

    /// Pin: `remove_nodes` with a `match` that omits `name` →
    /// Schema("remove_nodes[].match.name missing"). The single-target invariant
    /// requires a `name`; a nameless match is never silently applied.
    #[test]
    fn remove_nodes_match_without_name_is_schema() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"remove_nodes": [{"match": {}}]});
        let registry_names: Vec<String> = vec!["present".into()];
        let err = validate_post_state_full(&diff, &factories, &registry_names).unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Pin: `add_edges` entry missing `from` →
    /// Schema("add_edges[].from missing"). A malformed edge is rejected loudly,
    /// never silently dropped.
    #[test]
    fn add_edges_missing_from_is_schema() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({"add_edges": [{"to": "b"}]});
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Pin: `add_edges` entry missing `to` →
    /// Schema("add_edges[].to missing").
    #[test]
    fn add_edges_missing_to_is_schema() {
        let factories = factories_with(&["echo"]);
        let registry_names: Vec<String> = vec!["a".into(), "b".into()];
        let existing_edges: Vec<(String, String)> = vec![];
        let diff = json!({"add_edges": [{"from": "a"}]});
        let err = validate_post_state_with_edges(
            &diff,
            &factories,
            &registry_names,
            &existing_edges,
            &[],
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    /// Pin: `add_nodes` entry missing `name` →
    /// Schema("add_nodes[].name missing"). Checked by `validate_post_state_full`
    /// (the naming-collision pass needs a name to compare against the registry).
    #[test]
    fn add_nodes_missing_name_is_schema() {
        let factories = factories_with(&["echo"]);
        let diff = json!({"add_nodes": [{"template": "echo"}]});
        let registry_names: Vec<String> = vec![];
        let err = validate_post_state_full(&diff, &factories, &registry_names).unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    // ── Slice 6: header-contract locality (Phase-14-B as build-time error) ───

    use std::collections::{BTreeMap, BTreeSet};

    fn keys(ks: &[&str]) -> BTreeSet<String> {
        ks.iter().map(|s| s.to_string()).collect()
    }

    /// Build a [`HeaderNodeView`] from emits.hop / required-context / required-hop.
    fn node(emits_hop: &[&str], req_ctx: &[&str], req_hop: &[&str]) -> HeaderNodeView {
        HeaderNodeView {
            emits_hop: keys(emits_hop),
            required_context: keys(req_ctx),
            required_hop: keys(req_hop),
            ..Default::default()
        }
    }

    /// GH #185 — build a [`HeaderNodeView`] that DECLARES it is an ingress
    /// minting `ingress_ctx` at birth, and nothing else.
    fn ingress_node(ingress_ctx: &[&str]) -> HeaderNodeView {
        HeaderNodeView {
            ingress_context: keys(ingress_ctx),
            ..Default::default()
        }
    }

    /// Build a bare [`HeaderEdgeView`] from `from`/`to` with no modifier keys.
    fn edge(from: &str, to: &str) -> HeaderEdgeView {
        HeaderEdgeView {
            from: from.into(),
            to: to.into(),
            ..Default::default()
        }
    }

    /// (a) Collector requires `consumes.hop.operation`; its single predecessor
    /// neither emits `operation` (emits.hop) nor sets it on the edge → reject.
    #[test]
    fn rejects_required_hop_consume_not_set_by_immediate_predecessor() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("src".into(), node(&["other"], &[], &[]));
        contracts.insert("collector".into(), node(&[], &[], &["operation"]));
        let edges = vec![edge("src", "collector")];
        let err = validate_header_contract_locality(&contracts, &edges, &keys(&[])).unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("operation"), "names key: {msg}");
                assert!(msg.contains("hop"), "names hop: {msg}");
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// (b) Two incoming edges; one delivers the required hop key, the other does
    /// not → reject (fan-in intersection, Korrektur #2).
    #[test]
    fn rejects_required_hop_consume_missing_in_one_fan_in_edge() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("a".into(), node(&["operation"], &[], &[]));
        contracts.insert("b".into(), node(&[], &[], &[])); // does NOT emit operation
        contracts.insert("collector".into(), node(&[], &[], &["operation"]));
        let edges = vec![edge("a", "collector"), edge("b", "collector")];
        let err = validate_header_contract_locality(&contracts, &edges, &keys(&[])).unwrap_err();
        assert_eq!(err.error_code(), "edge_schema");
        if let MutationError::EdgeSchema(msg) = err {
            assert!(msg.contains("operation"), "names key: {msg}");
            assert!(
                msg.contains("intersection"),
                "names fan-in intersection: {msg}"
            );
        }
    }

    /// (c) Node requires a NON-ingress `consumes.context` key with no
    /// `set_context` setter anywhere upstream → reject.
    #[test]
    fn rejects_required_context_consume_with_no_setter_path() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("src".into(), node(&[], &[], &[]));
        contracts.insert("sink".into(), node(&[], &["custom_key"], &[]));
        let edges = vec![edge("src", "sink")];
        let err = validate_header_contract_locality(&contracts, &edges, &keys(&[])).unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("custom_key"), "names key: {msg}");
                assert!(msg.contains("context"), "names context: {msg}");
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// (d) Node requires an INGRESS context key (`turn_id`), reachable from a
    /// cell that DECLARES it mints the key at birth (no intervening delete)
    /// → accepted.
    ///
    /// GH #185 re-cut: `src` used to qualify by having no incoming edge. That
    /// inference is gone — it now qualifies by saying so.
    #[test]
    fn accepts_required_context_consume_for_ingress_key() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("src".into(), ingress_node(&["turn_id"]));
        contracts.insert("sink".into(), node(&[], &["turn_id"], &[]));
        let edges = vec![edge("src", "sink")];
        assert!(validate_header_contract_locality(&contracts, &edges, &keys(&[])).is_ok());
    }

    /// (d′) GH #185, the counter-pin: the same topology WITHOUT the declaration
    /// is refused, and the refusal names the field to add.
    #[test]
    fn rejects_required_ingress_key_when_no_cell_declares_the_ingress() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("src".into(), node(&[], &[], &[]));
        contracts.insert("sink".into(), node(&[], &["turn_id"], &[]));
        let edges = vec![edge("src", "sink")];
        let err = validate_header_contract_locality(&contracts, &edges, &keys(&[]))
            .expect_err("no in-degree inference grants turn_id any more");
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("turn_id"), "names key: {msg}");
                assert!(
                    msg.contains("contract.ingress.context"),
                    "names the declaration to add: {msg}"
                );
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// (e) Two incoming edges, BOTH deliver the required hop key → accepted.
    #[test]
    fn accepts_hop_consume_in_fan_in_intersection() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("a".into(), node(&["operation"], &[], &[]));
        contracts.insert("b".into(), node(&["operation"], &[], &[]));
        contracts.insert("collector".into(), node(&[], &[], &["operation"]));
        let edges = vec![edge("a", "collector"), edge("b", "collector")];
        assert!(validate_header_contract_locality(&contracts, &edges, &keys(&[])).is_ok());
    }

    /// (f) Regression: the backwards context-reachability BFS must TERMINATE on a
    /// CYCLIC edge graph (meclaw graphs may legitimately contain tool-loops). The
    /// topology `a → b`, `b → a`, `a → sink` cycles between `a` and `b`; `sink`
    /// requires a NON-ingress `consumes.context` key (`custom_key`) with no
    /// `set_context` setter anywhere → the key is unreachable. The test proves
    /// termination purely by RETURNING (rather than hanging) with the expected
    /// unreachable `Err(EdgeSchema)`; the `visited`-set guards the cycle.
    #[test]
    fn context_reachability_terminates_on_cyclic_graph() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("a".into(), node(&[], &[], &[]));
        contracts.insert("b".into(), node(&[], &[], &[]));
        contracts.insert("sink".into(), node(&[], &["custom_key"], &[]));
        // Cycle: a ⇄ b, plus a → sink (the consuming node).
        let edges = vec![edge("a", "b"), edge("b", "a"), edge("a", "sink")];
        let err = validate_header_contract_locality(&contracts, &edges, &keys(&[])).unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("custom_key"), "names key: {msg}");
                assert!(msg.contains("context"), "names context: {msg}");
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// (g) Regression: a SELF-LOOP `n → n` must also terminate the BFS. `n`
    /// requires a NON-ingress `consumes.context` key with no setter → unreachable,
    /// returns `Err(EdgeSchema)` instead of looping on its own incoming edge.
    #[test]
    fn context_reachability_terminates_on_self_loop() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("n".into(), node(&[], &["custom_key"], &[]));
        let edges = vec![edge("n", "n")];
        let err = validate_header_contract_locality(&contracts, &edges, &keys(&[])).unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("custom_key"), "names key: {msg}");
                assert!(msg.contains("context"), "names context: {msg}");
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    // ── F1 fix (K-H1): hive-transit participation in the fan-in intersection ─

    /// (h) F1 / K-H1 shape: a hop key set on the edge INTO a hive must credit
    /// the consumer behind the transit — the hive passes the hop compartment
    /// through unchanged (spec § Hive-Pfade als Target — Transit-Auswertung;
    /// hop decays only at a CELL emission). Honest contract
    /// (`consumes.hop.hmark required:true` behind the transit) must validate.
    #[test]
    fn accepts_required_hop_set_on_edge_into_hive_transit() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("/entry".into(), node(&[], &[], &[]));
        contracts.insert("/sub/cellA".into(), node(&[], &[], &["hmark"]));
        let mut into_hive = edge("/entry", "/sub");
        into_hive.set_hop = keys(&["hmark"]);
        let edges = vec![into_hive, edge("/sub", "/sub/cellA")];
        assert!(
            validate_header_contract_locality(&contracts, &edges, &keys(&["/sub"])).is_ok(),
            "set_hop on the edge into the hive must satisfy the consumer behind the transit"
        );
    }

    /// (i) Multi-level transit chain (K-H4 needs depth 3): the source cell's
    /// `emits.hop` key walks across THREE chained hives unchanged; consumers
    /// behind depth 2 AND depth 3 both validate.
    #[test]
    fn accepts_required_hop_across_multi_level_transit_chain() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("/src".into(), node(&["k"], &[], &[]));
        contracts.insert("/mid".into(), node(&[], &[], &["k"])); // behind depth 2
        contracts.insert("/sink".into(), node(&[], &[], &["k"])); // behind depth 3
        let edges = vec![
            edge("/src", "/h1"),
            edge("/h1", "/h2"),
            edge("/h2", "/mid"),
            edge("/h2", "/h3"),
            edge("/h3", "/sink"),
        ];
        assert!(
            validate_header_contract_locality(&contracts, &edges, &keys(&["/h1", "/h2", "/h3"]))
                .is_ok(),
            "emits.hop key must walk across chained transits (depth 2 and 3)"
        );
    }

    /// (j) NEGATIVE (non-vacuity guard, mandatory point TDD b): a key NOT
    /// guaranteed on EVERY path into the hive must still reject — a second
    /// inbound edge without the key empties the transit contribution.
    #[test]
    fn rejects_required_hop_missing_on_one_path_into_hive_transit() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("/a".into(), node(&[], &[], &[]));
        contracts.insert("/b".into(), node(&[], &[], &[]));
        contracts.insert("/sink".into(), node(&[], &[], &["k"]));
        let mut a_in = edge("/a", "/h");
        a_in.set_hop = keys(&["k"]);
        let edges = vec![a_in, edge("/b", "/h"), edge("/h", "/sink")];
        let err =
            validate_header_contract_locality(&contracts, &edges, &keys(&["/h"])).unwrap_err();
        match err {
            MutationError::EdgeSchema(msg) => {
                assert!(msg.contains("'k'"), "names key: {msg}");
                assert!(msg.contains("intersection"), "names intersection: {msg}");
            }
            other => panic!("expected EdgeSchema, got {other:?}"),
        }
    }

    /// (k) `delete_hop` on the transit edge severs the key even when the
    /// hive's inbound fan-in provides it (delete wins over the pass-through).
    #[test]
    fn rejects_required_hop_deleted_on_transit_edge() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("/a".into(), node(&[], &[], &[]));
        contracts.insert("/sink".into(), node(&[], &[], &["k"]));
        let mut a_in = edge("/a", "/h");
        a_in.set_hop = keys(&["k"]);
        let mut transit = edge("/h", "/sink");
        transit.delete_hop = keys(&["k"]);
        let edges = vec![a_in, transit];
        assert!(
            validate_header_contract_locality(&contracts, &edges, &keys(&["/h"])).is_err(),
            "delete_hop on the transit edge must sever the key"
        );
    }

    /// (l) Loops are legal (tool-/refine-loops, mandatory point 2): a hive⇄hive
    /// cycle on the walk must TERMINATE (proven by returning) and must NOT
    /// falsely empty the intersection — the loop back-edge imposes no
    /// additional obligation (greatest-fixpoint reading).
    #[test]
    fn transit_walk_terminates_and_provides_across_hive_cycle() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("/src".into(), node(&[], &[], &[]));
        contracts.insert("/sink".into(), node(&[], &[], &["k"]));
        let mut src_in = edge("/src", "/h1");
        src_in.set_hop = keys(&["k"]);
        let edges = vec![
            src_in,
            edge("/h1", "/h2"),
            edge("/h2", "/h1"), // cycle back-edge
            edge("/h1", "/sink"),
        ];
        assert!(
            validate_header_contract_locality(&contracts, &edges, &keys(&["/h1", "/h2"])).is_ok(),
            "hive cycle must not falsely empty the fan-in intersection"
        );
    }

    /// (m) The cycle guard must NOT make loops vacuously green: a
    /// `delete_hop` on the loop edge severs the key on every looped path —
    /// the intersection at the cycling hive empties → reject.
    #[test]
    fn rejects_required_hop_deleted_on_loop_edge_of_hive_cycle() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("/src".into(), node(&[], &[], &[]));
        contracts.insert("/sink".into(), node(&[], &[], &["k"]));
        let mut src_in = edge("/src", "/h1");
        src_in.set_hop = keys(&["k"]);
        let mut loop_out = edge("/h1", "/h2");
        loop_out.delete_hop = keys(&["k"]);
        let edges = vec![
            src_in,
            loop_out,
            edge("/h2", "/h1"), // looped path arrives at /h1 without k
            edge("/h1", "/sink"),
        ];
        assert!(
            validate_header_contract_locality(&contracts, &edges, &keys(&["/h1", "/h2"])).is_err(),
            "a looped path that lost the key must empty the intersection"
        );
    }

    /// (n) A hive with NO inbound edge can never deliver via its out-edge:
    /// zero runtime paths impose zero obligations (vacuous), and the wiring
    /// mutation that later connects the hive re-validates the full
    /// post-state.
    #[test]
    fn accepts_required_hop_behind_unconnected_hive_vacuously() {
        let mut contracts: BTreeMap<String, HeaderNodeView> = BTreeMap::new();
        contracts.insert("/sink".into(), node(&[], &[], &["k"]));
        let edges = vec![edge("/h", "/sink")];
        assert!(
            validate_header_contract_locality(&contracts, &edges, &keys(&["/h"])).is_ok(),
            "unconnected hive's out-edge can never fire — vacuously providing"
        );
    }

    /// Hardening Slice 1: the pure `ContractBlock` → [`HeaderNodeView`]
    /// projection keeps only `required: true` consume keys and all `emits.hop`
    /// keys; `required` defaults to true (config.md Z.107).
    #[test]
    fn header_view_from_contract_projects_required_keys_only() {
        let block: crate::config::ContractBlock =
            meclaw_core::serde_json::from_value(meclaw_core::serde_json::json!({
                "emits": {"hop": {"k1": {"type": "string"}}},
                "consumes": {
                    "context": {"c1": {"type": "string", "required": true},
                                 "c2": {"type": "string", "required": false}},
                    "hop": {"h1": {"type": "string"}}
                }
            }))
            .unwrap();
        let view = header_view_from_contract(&block);
        assert_eq!(view.emits_hop.iter().collect::<Vec<_>>(), vec!["k1"]);
        assert_eq!(view.required_context.iter().collect::<Vec<_>>(), vec!["c1"]);
        // required defaultet auf true (config.md Z.107)
        assert_eq!(view.required_hop.iter().collect::<Vec<_>>(), vec!["h1"]);
    }

    /// Substrat-Fix Slice 0 (security hotfix) — FS-realisation traversal guard.
    /// The apply-side joins (`resolve_cell_dir`, `staging_root.join(name)`) do
    /// NOT root-clamp `..`, while the logical normalisation here does: at root
    /// scope `/../escape` clamps to `/escape` ("within /") and the filesystem
    /// realisation walks OUTSIDE `{root}`. A raw `..` segment in any top-level
    /// diff name must therefore reject as `scope_out_of_bounds` regardless of
    /// where the clamped path lands (spec § Mutation-Format: paths that would
    /// land outside the scope are rejected at validation).
    #[test]
    fn scope_containment_rejects_dotdot_segment_in_every_name_bearing_op_at_root() {
        let diffs = [
            (
                "add_nodes[].name",
                json!({"add_nodes": [{"name": "../escape", "template": "echo"}]}),
            ),
            (
                "remove_nodes[].match.name",
                json!({"remove_nodes": [{"match": {"name": "../escape"}}]}),
            ),
            (
                "swap_nodes[].match.name",
                json!({"swap_nodes": [{"match": {"name": "../escape"}, "with": {"name": "ok"}}]}),
            ),
            (
                "swap_nodes[].with.name",
                json!({"swap_nodes": [{"match": {"name": "ok"}, "with": {"name": "../escape"}}]}),
            ),
            (
                "add_edges[].from",
                json!({"add_edges": [{"from": "../escape", "to": "ok"}]}),
            ),
            (
                "add_edges[].to",
                json!({"add_edges": [{"from": "ok", "to": "../escape"}]}),
            ),
            (
                "remove_edges[].match.from",
                json!({"remove_edges": [{"match": {"from": "../escape", "to": "ok"}}]}),
            ),
            (
                "remove_edges[].match.to",
                json!({"remove_edges": [{"match": {"from": "ok", "to": "../escape"}}]}),
            ),
        ];
        for (field, diff) in diffs {
            let err = validate_scope_containment(&diff, "/")
                .expect_err(&format!("{field}: raw `..` must reject at root scope"));
            assert_eq!(err.error_code(), "scope_out_of_bounds", "{field}");
        }
    }

    /// Absolute names: `PathBuf::join` REPLACES the base when joined with an
    /// absolute path, so an absolute `add_nodes[].name` realises a directory at
    /// an arbitrary host location. No top-level diff op has a legitimate
    /// absolute-name producer (`resolve_scoped_path` string-joins them blindly,
    /// so they were never honoured as absolute logically) → reject.
    #[test]
    fn scope_containment_rejects_absolute_name() {
        let diff = json!({"add_nodes": [{"name": "/abs/escape", "template": "echo"}]});
        let err = validate_scope_containment(&diff, "/").unwrap_err();
        assert_eq!(err.error_code(), "scope_out_of_bounds");
    }

    /// Embedded separators hiding a traversal (`a/../../escape`): the clamped
    /// logical path lands within scope, the FS realisation does not → reject.
    #[test]
    fn scope_containment_rejects_embedded_dotdot_traversal() {
        let diff = json!({"add_nodes": [{"name": "a/../../escape", "template": "echo"}]});
        let err = validate_scope_containment(&diff, "/").unwrap_err();
        assert_eq!(err.error_code(), "scope_out_of_bounds");
    }

    /// Positive control: legitimate name forms (bare name, `./`-prefixed edge
    /// endpoint) stay accepted at root scope — the guard rejects traversal,
    /// not shape (`./a` is an established mutation-diff form, see
    /// workshop/fixtures/negative/edge_schema).
    #[test]
    fn scope_containment_accepts_bare_and_dot_slash_names() {
        let diff = json!({
            "add_nodes": [{"name": "worker", "template": "echo"}],
            "add_edges": [{"from": "./worker", "to": "worker"}]
        });
        assert!(validate_scope_containment(&diff, "/").is_ok());
    }

    // ── R12: add_edges depth-endpoint resolution into sub-scopes ────────────
    //
    // Spec Z.227: edge from/to are "paths relative to the hive scope" — WITHOUT a
    // depth restriction. A `./unit/dispatch` endpoint must resolve against the
    // mutation scope (post_state membership), while containment stays sharp
    // (Form C of the llm-unit RECEIPT matrix is covered by
    // `validate_scope_containment`, unchanged).

    /// R12 Form B (validate level): a depth endpoint matches a node that
    /// already EXISTS in the registry at depth ≥ 2 within the scope, passed
    /// via `deep_endpoint_paths` (absolute representation).
    #[test]
    fn depth_endpoint_resolves_against_deep_registry_paths() {
        let diff = json!({
            "add_edges": [{"from": "./unit/dispatch", "to": "./sink"}]
        });
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let deep: Vec<String> = vec!["/unit/dispatch".into()];
        let result = validate_post_state_with_templates_scoped(
            &diff,
            &templates,
            &factories,
            &["sink".to_string()],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            "/",
            &deep,
            &[],
            &[],
            // GH #285: no hive declares a slot in this fixture.
            &std::collections::HashSet::new(),
        );
        assert!(result.is_ok(), "depth endpoint must validate: {result:?}");
    }

    /// R12 Form A (validate level): a depth endpoint matches a DIFF-NEW node
    /// contributed by a subtree `add_nodes` in the SAME mutation
    /// (`subtree_node_endpoints`, absolute representation — Befund-6 semantics
    /// hold at depth: the post_state includes diff-new nodes).
    #[test]
    fn depth_endpoint_resolves_against_subtree_nodes_in_same_diff() {
        let diff = json!({
            "add_edges": [{"from": "./unit/dispatch", "to": "./sink"}]
        });
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let subtree_nodes: Vec<String> = vec!["/unit".into(), "/unit/dispatch".into()];
        let result = validate_post_state_with_templates_scoped(
            &diff,
            &templates,
            &factories,
            &["sink".to_string()],
            &[],
            &[],
            &[],
            &[],
            &subtree_nodes,
            &[],
            "/",
            &[],
            &[],
            &[],
            // GH #285: no hive declares a slot in this fixture.
            &std::collections::HashSet::new(),
        );
        assert!(
            result.is_ok(),
            "depth endpoint into same-diff subtree must validate: {result:?}"
        );
    }

    /// R12 negative: a depth path to a NON-existent node rejects as
    /// `edge_schema` (post_state membership miss — not scope_out_of_bounds,
    /// the path IS contained).
    #[test]
    fn depth_endpoint_unknown_rejects_edge_schema() {
        let diff = json!({
            "add_edges": [{"from": "./unit/nonexistent", "to": "./sink"}]
        });
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let err = validate_post_state_with_templates_scoped(
            &diff,
            &templates,
            &factories,
            &["sink".to_string()],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            "/",
            &[],
            &[],
            &[],
            // GH #285: no hive declares a slot in this fixture.
            &std::collections::HashSet::new(),
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "edge_schema");
    }

    /// R12 scope-relativity: the depth endpoint resolves against the MUTATION
    /// scope (not root). `./unit/dispatch` at scope `/main` is
    /// `/main/unit/dispatch`; the same node under a FOREIGN scope must not
    /// satisfy the membership test.
    #[test]
    fn depth_endpoint_resolves_relative_to_mutation_scope() {
        let diff = json!({
            "add_edges": [{"from": "./unit/dispatch", "to": "./sink"}]
        });
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let deep_in_scope: Vec<String> = vec!["/main/unit/dispatch".into()];
        let result = validate_post_state_with_templates_scoped(
            &diff,
            &templates,
            &factories,
            &["sink".to_string()],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            "/main",
            &deep_in_scope,
            &[],
            &[],
            // GH #285: no hive declares a slot in this fixture.
            &std::collections::HashSet::new(),
        );
        assert!(result.is_ok(), "in-scope depth node must match: {result:?}");

        let deep_foreign: Vec<String> = vec!["/other/unit/dispatch".into()];
        let err = validate_post_state_with_templates_scoped(
            &diff,
            &templates,
            &factories,
            &["sink".to_string()],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            "/main",
            &deep_foreign,
            &[],
            &[],
            // GH #285: no hive declares a slot in this fixture.
            &std::collections::HashSet::new(),
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "edge_schema");
    }

    // ── GH #487: `.` names the scope root ───────────────────────────────────
    //
    // The template loader has always resolved `.` to the level (`Path::resolve`
    // → "stay at the sender"), which is why 277 of the 561 edges the shipped
    // templates declare are spelled that way. `scoped_name` read it as a short
    // NAME, no node is called `.`, and the endpoint check answered
    // `edge_schema: to='.' unknown` for the catalogue's own idiom.

    /// One endpoint, both self-spellings, both sides of the arrow: `.` and the
    /// `./` that strips to nothing resolve to the scope root, which is a node
    /// like any other once it is looked up in the namespace it lives in.
    #[test]
    fn a_dot_endpoint_resolves_to_the_scope_root() {
        for dot in [".", "./"] {
            for diff in [
                json!({"add_edges": [{"from": "./q", "to": dot}]}),
                json!({"add_edges": [{"from": dot, "to": "./q"}]}),
            ] {
                let templates = crate::templates::TemplatesRegistry::default();
                let factories = CellFactoryRegistry::new();
                // The scope root is a hive, so it is in the pre-state absolute
                // view — exactly what the colony hands the validator.
                let deep: Vec<String> = vec!["/unit".into(), "/unit/q".into()];
                let result = validate_post_state_with_templates_scoped(
                    &diff,
                    &templates,
                    &factories,
                    &["q".to_string()],
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    "/unit",
                    &deep,
                    &[],
                    &[],
                    &std::collections::HashSet::new(),
                );
                assert!(
                    result.is_ok(),
                    "`{dot}` at scope /unit must resolve to /unit: {result:?}"
                );
            }
        }
    }

    /// The boundary that keeps this a widening of the vocabulary rather than an
    /// invented node: a scope whose root NOTHING answers to still refuses. Not
    /// a statement about the root scope — where a hive-scope marker sits at `/`,
    /// `.` names it like any other node, and that is the lane a marked answer
    /// leaves by (GH #163). This fixture simply hands the validator a view in
    /// which the scope root is absent.
    #[test]
    fn a_dot_endpoint_at_a_scope_root_nothing_answers_to_still_rejects() {
        let diff = json!({"add_edges": [{"from": "./q", "to": "."}]});
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let err = validate_post_state_with_templates_scoped(
            &diff,
            &templates,
            &factories,
            &["q".to_string()],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            "/",
            &["/q".to_string()],
            &[],
            &[],
            &std::collections::HashSet::new(),
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "edge_schema");
    }

    /// `.` is contained in its own scope by definition — the guard resolves it
    /// to the scope root and a scope contains itself. Both spellings, both
    /// sides, and `remove_edges` too, because the guard walks that entry as
    /// well and a one-way grammar is the defect one level down.
    #[test]
    fn a_dot_endpoint_is_never_out_of_its_own_scope() {
        for dot in [".", "./"] {
            for diff in [
                json!({"add_edges": [{"from": "./q", "to": dot}]}),
                json!({"add_edges": [{"from": dot, "to": "./q"}]}),
                json!({"remove_edges": [{"match": {"from": "./q", "to": dot}}]}),
            ] {
                assert!(
                    validate_scope_containment(&diff, "/os/orgs/acme/members/alex").is_ok(),
                    "`{dot}` addresses the scope root, which is in bounds: {diff}"
                );
            }
        }
    }

    /// And the resolution is scope-relative, not root-relative: the same `.`
    /// under two scopes names two different nodes. A validator that resolved it
    /// against `/` would accept a lane onto a level the declaration has no
    /// authority over.
    #[test]
    fn a_dot_endpoint_names_the_scope_it_was_declared_at() {
        let diff = json!({"add_edges": [{"from": "./q", "to": "."}]});
        let templates = crate::templates::TemplatesRegistry::default();
        let factories = CellFactoryRegistry::new();
        let foreign: Vec<String> = vec!["/other".into(), "/unit/q".into()];
        let err = validate_post_state_with_templates_scoped(
            &diff,
            &templates,
            &factories,
            &["q".to_string()],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            "/unit",
            &foreign,
            &[],
            &[],
            &std::collections::HashSet::new(),
        )
        .unwrap_err();
        assert_eq!(
            err.error_code(),
            "edge_schema",
            "a node at /other must not satisfy a `.` declared at /unit"
        );
    }

    /// GH #293 — the collecting core and the `Result` form must answer the same
    /// thing, fixture by fixture.
    ///
    /// Every single-violation shape `validate_naming_and_match` can produce is
    /// listed here, and for each one the FIRST element the collecting core
    /// pushes is compared against the error the `Result` form returns. That
    /// equality is the whole safety statement of the refactor: the report grew,
    /// the verdict did not. A future edit that reorders a check, or that pushes
    /// a differently-worded error before the one it used to return, turns this
    /// test red rather than silently changing what an operator is told.
    #[test]
    fn the_collecting_core_answers_first_what_the_result_form_answers() {
        // (label, diff, registry_names, hive_match_names)
        let fixtures: Vec<(&str, JsonValue, Vec<String>, Vec<String>)> = vec![
            (
                "two entries claim one path",
                json!({"add_nodes":[
                    {"name":"n1","template":"t"},
                    {"name":"n1","template":"t"}
                ]}),
                vec![],
                vec![],
            ),
            (
                "add_nodes name already taken",
                json!({"add_nodes":[{"name":"a","template":"t"}]}),
                vec!["a".to_string()],
                vec![],
            ),
            (
                "add_nodes without a name",
                json!({"add_nodes":[{"template":"t"}]}),
                vec![],
                vec![],
            ),
            (
                "remove_nodes match hits nothing",
                json!({"remove_nodes":[{"match":{"name":"gone"}}]}),
                vec![],
                vec![],
            ),
            (
                "remove_nodes without a match name",
                json!({"remove_nodes":[{"match":{}}]}),
                vec![],
                vec![],
            ),
            (
                "swap_nodes match hits neither a cell nor a hive",
                json!({"swap_nodes":[{"match":{"name":"gone"},"with":{"name":"x"}}]}),
                vec!["other".to_string()],
                vec!["some_hive".to_string()],
            ),
            (
                "swap_nodes without a match name",
                json!({"swap_nodes":[{"with":{"name":"x"}}]}),
                vec![],
                vec![],
            ),
        ];

        for (label, diff, registry_names, hive_match_names) in fixtures {
            let obj = diff.as_object().expect("fixture is an object");
            let collected =
                collect_naming_and_match(obj, &registry_names, &hive_match_names, "/", &[], &[]);
            let returned =
                validate_naming_and_match(obj, &registry_names, &hive_match_names, "/", &[], &[])
                    .expect_err(&format!("fixture '{label}' must be refused"));
            assert_eq!(
                collected.len(),
                1,
                "fixture '{label}' is a SINGLE-violation fixture: {collected:?}"
            );
            assert_eq!(
                collected.into_iter().next(),
                Some(returned),
                "fixture '{label}': the collecting core's first element is the \
                 error the Result form returns"
            );
        }
    }

    /// The other direction: a diff with nothing wrong collects nothing, so the
    /// `Result` form's `Ok(())` is not an artefact of taking `.next()` on a list
    /// that happened to be empty for the wrong reason.
    #[test]
    fn a_clean_diff_collects_no_naming_or_match_violation() {
        let diff = json!({
            "add_nodes":[{"name":"fresh","template":"t"}],
            "remove_nodes":[{"match":{"name":"here"}}],
            "swap_nodes":[{"match":{"name":"here"},"with":{"name":"fresh"}}]
        });
        let obj = diff.as_object().unwrap();
        let names = vec!["here".to_string()];
        assert!(collect_naming_and_match(obj, &names, &[], "/", &[], &[]).is_empty());
        assert!(validate_naming_and_match(obj, &names, &[], "/", &[], &[]).is_ok());
    }

    // ── GH #347 gap 1: the `swap_nodes[].with` instantiate form ─────────────

    /// Build a one-cell template whose `template.json` declares
    /// `requires.ctx.model` and whose `config.json` actually substitutes it.
    fn needy_template_registry(
        tmp: &tempfile::TempDir,
    ) -> (crate::templates::TemplatesRegistry, std::path::PathBuf) {
        let tpl_dir = tmp.path().to_path_buf();
        std::fs::write(
            tpl_dir.join("template.json"),
            r#"{"name":"needy","requires":{"ctx":{"model":{"type":"string","required":true,
                 "because":"the model the brain infers with"}}}}"#,
        )
        .unwrap();
        std::fs::write(
            tpl_dir.join("config.json"),
            r#"{"cell":{"type":"echo_type"},"params":{"model":"${ctx.model}"}}"#,
        )
        .unwrap();
        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "needy1".into(),
                name: "needy".into(),
                version: None,
                filesystem_path: tpl_dir.clone(),
            },
        ]);
        (templates, tpl_dir)
    }

    /// GH #347 gap 1 — the instantiate form of `swap_nodes[].with` copies a
    /// template exactly like an `add_nodes` entry does, so the same declared
    /// keys have to be on the table BEFORE anything is staged. Until this test
    /// the `requires` walk read `add_nodes` only, and a swap into a template
    /// declaring `ctx.model` was accepted and broke later, during the staging
    /// substitution, as `ctx_key_missing` — after the copy.
    #[test]
    fn swap_into_template_with_unmet_requires_is_refused_before_staging() {
        let tmp = tempfile::tempdir().unwrap();
        let (templates, _tpl_dir) = needy_template_registry(&tmp);
        let diff = json!({"swap_nodes": [
            {"match": {"name": "old"}, "with": {"template": "needy", "name": "fresh"}}
        ]});
        let env = std::collections::HashMap::new();

        // 1. `ctx` empty → refused, naming template, class, key and `because`.
        let ctx = std::collections::HashMap::new();
        // A swap never consults the live tree — it always stages, so `root`/`scope`
        // are inert here; the template dir doubles as a stand-in root.
        let err = validate_requires(
            &diff,
            &templates,
            &ctx,
            &env,
            &[],
            LiveTree {
                root: tmp.path(),
                scope: "/",
            },
        )
        .expect_err("a swap into a template declaring ctx.model must be refused");
        let MutationError::RequirementMissing(msg) = &err else {
            panic!("expected RequirementMissing, got {err:?}");
        };
        for part in ["needy", "ctx", "model", "the model the brain infers with"] {
            assert!(msg.contains(part), "refusal must name {part:?}: {msg}");
        }

        // 2. `ctx` supplies the key → the same swap passes.
        let mut ctx = std::collections::HashMap::new();
        ctx.insert("model".to_string(), "anthropic/claude".to_string());
        let ok = validate_requires(
            &diff,
            &templates,
            &ctx,
            &env,
            &[],
            LiveTree {
                root: tmp.path(),
                scope: "/",
            },
        );
        assert!(
            ok.is_ok(),
            "a swap that supplies the declared key must pass: {ok:?}"
        );
    }

    /// The existing-node form of `swap_nodes[].with` (no `template`) references
    /// a cell that is already there — it instantiates nothing and therefore
    /// consumes no declared key. It must not be refused for one.
    #[test]
    fn swap_onto_an_existing_node_is_not_a_requirements_case() {
        let tmp = tempfile::tempdir().unwrap();
        let (templates, _tpl_dir) = needy_template_registry(&tmp);
        let diff = json!({"swap_nodes": [
            {"match": {"name": "old"}, "with": {"name": "already_there"}}
        ]});
        let ctx = std::collections::HashMap::new();
        let env = std::collections::HashMap::new();
        assert!(
            validate_requires(
                &diff,
                &templates,
                &ctx,
                &env,
                &[],
                LiveTree {
                    root: tmp.path(),
                    scope: "/"
                }
            )
            .is_ok(),
            "an existing-node swap stages nothing and requires nothing"
        );
    }

    /// A swap always stages, so `resumed_names` — which exempts an `add_nodes`
    /// Reconnect/Resume — must not reach across and exempt a swap that happens
    /// to install the same name.
    #[test]
    fn resumed_names_do_not_exempt_a_swap() {
        let tmp = tempfile::tempdir().unwrap();
        let (templates, _tpl_dir) = needy_template_registry(&tmp);
        let diff = json!({"swap_nodes": [
            {"match": {"name": "old"}, "with": {"template": "needy", "name": "fresh"}}
        ]});
        let ctx = std::collections::HashMap::new();
        let env = std::collections::HashMap::new();
        let resumed = vec!["fresh".to_string(), "old".to_string()];
        assert!(
            validate_requires(
                &diff,
                &templates,
                &ctx,
                &env,
                &resumed,
                LiveTree {
                    root: tmp.path(),
                    scope: "/"
                }
            )
            .is_err(),
            "a swap stages regardless of any resume list"
        );
    }

    // ── GH #347 gap 2: the resume exemption is per NODE, not per entry ──────

    /// Write `body` to `path`, creating the parent directories.
    fn write_file(path: &std::path::Path, body: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).unwrap();
        std::fs::write(path, body).unwrap();
    }

    const ECHO_CFG: &str = r#"{"cell":{"type":"echo_type"},"params":{"model":"${ctx.model}"},
         "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

    /// Colony root + template registry for the merge-resume tests.
    ///
    /// The colony root holds exactly ONE top-level directory carrying a
    /// `config.json` (`main`, the root cell), so `path_truth::resolve_cell_dir`
    /// anchors logical `/m1` at `<colony>/main/m1`. The templates live in a
    /// SIBLING directory on purpose — a template directory under the colony
    /// root would be a second root-cell candidate and move that anchor.
    ///
    /// The template is a composite: a hive root plus one `child` cell. Its
    /// `template.json` declares `requires.ctx.model`, and `child`'s
    /// `config.json` is what actually substitutes it.
    fn merge_resume_fixture(
        tmp: &tempfile::TempDir,
    ) -> (std::path::PathBuf, crate::templates::TemplatesRegistry) {
        let colony = tmp.path().join("colony");
        write_file(
            &colony.join("main/config.json"),
            r#"{"cell":{"type":"hive"}}"#,
        );

        let tpl = tmp.path().join("templates/needysub");
        write_file(
            &tpl.join("template.json"),
            r#"{"name":"needysub","requires":{"ctx":{"model":{"type":"string",
                 "required":true,"because":"the model the brain infers with"}}}}"#,
        );
        write_file(
            &tpl.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_file(&tpl.join("child/config.json"), ECHO_CFG);

        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "needysub1".into(),
                name: "needysub".into(),
                version: None,
                filesystem_path: tpl,
            },
        ]);
        (colony, templates)
    }

    /// GH #347 gap 2 — an `add_nodes` at an existing path is a Resume, and a
    /// Resume is exempt from the requirements check because it stages nothing.
    /// For a COMPOSITE template whose root exists but whose children do not,
    /// that premise is false: the merge path stages the missing children, and
    /// their `${ctx.X}` is substituted like any fresh instantiation. Until this
    /// test the exemption was decided per diff ENTRY — the whole entry was
    /// skipped on the strength of its root directory — and the key missing for
    /// the staged child surfaced late again, during the staging substitution,
    /// as `ctx_key_missing`.
    #[test]
    fn merge_resume_requires_keys_for_the_children_it_stages() {
        let tmp = tempfile::tempdir().unwrap();
        let (colony, templates) = merge_resume_fixture(&tmp);
        // Live tree: the subtree ROOT exists, its `child` does not.
        write_file(
            &colony.join("main/m1/config.json"),
            r#"{"cell":{"type":"hive"}}"#,
        );

        let diff = json!({"add_nodes": [{"name": "m1", "template": "needysub"}]});
        let resumed = vec!["m1".to_string()];
        let env = std::collections::HashMap::new();

        // 1. `ctx` empty → refused, naming template, class, key and `because`.
        let ctx = std::collections::HashMap::new();
        let err = validate_requires(
            &diff,
            &templates,
            &ctx,
            &env,
            &resumed,
            LiveTree {
                root: &colony,
                scope: "/",
            },
        )
        .expect_err("the merge resume stages `child`, so ctx.model is owed");
        let MutationError::RequirementMissing(msg) = &err else {
            panic!("expected RequirementMissing, got {err:?}");
        };
        for part in [
            "needysub",
            "ctx",
            "model",
            "the model the brain infers with",
        ] {
            assert!(msg.contains(part), "refusal must name {part:?}: {msg}");
        }

        // 2. `ctx` supplies the key → the same merge resume passes.
        let mut ctx = std::collections::HashMap::new();
        ctx.insert("model".to_string(), "anthropic/claude".to_string());
        let ok = validate_requires(
            &diff,
            &templates,
            &ctx,
            &env,
            &resumed,
            LiveTree {
                root: &colony,
                scope: "/",
            },
        );
        assert!(
            ok.is_ok(),
            "a merge resume that supplies the declared key must pass: {ok:?}"
        );
    }

    /// The counter-test: a resume over a FULLY existing subtree stages nothing
    /// at all, so it consumes none of the declared keys and stays exempt. The
    /// per-node exemption must not turn every resume into a requirements case.
    #[test]
    fn a_fully_existing_subtree_resume_stays_exempt() {
        let tmp = tempfile::tempdir().unwrap();
        let (colony, templates) = merge_resume_fixture(&tmp);
        // Live tree: root AND child present — a true resume.
        write_file(
            &colony.join("main/m1/config.json"),
            r#"{"cell":{"type":"hive"}}"#,
        );
        write_file(
            &colony.join("main/m1/child/config.json"),
            r#"{"cell":{"type":"echo_type"}}"#,
        );

        let diff = json!({"add_nodes": [{"name": "m1", "template": "needysub"}]});
        let resumed = vec!["m1".to_string()];
        let ctx = std::collections::HashMap::new();
        let env = std::collections::HashMap::new();
        let ok = validate_requires(
            &diff,
            &templates,
            &ctx,
            &env,
            &resumed,
            LiveTree {
                root: &colony,
                scope: "/",
            },
        );
        assert!(
            ok.is_ok(),
            "a resume that stages nothing owes nothing: {ok:?}"
        );
    }

    /// Colony root + templates for the per-node `ref` test: a composite `outer`
    /// that declares nothing itself, with a `kept` node behind a
    /// `cell.type: "ref"` to a `leaf` template that declares `ctx.api_key`, and
    /// a plain `fresh` sibling.
    fn ref_resume_fixture(
        tmp: &tempfile::TempDir,
    ) -> (std::path::PathBuf, crate::templates::TemplatesRegistry) {
        let colony = tmp.path().join("colony");
        write_file(
            &colony.join("main/config.json"),
            r#"{"cell":{"type":"hive"}}"#,
        );

        let leaf = tmp.path().join("templates/leaf");
        write_file(
            &leaf.join("template.json"),
            r#"{"name":"leaf","version":"1.0.0","requires":{"ctx":{"api_key":{"type":"string",
                 "required":true,"because":"der Schluessel, mit dem das Blatt spricht"}}}}"#,
        );
        write_file(&leaf.join("config.json"), ECHO_CFG);

        let outer = tmp.path().join("templates/outer");
        write_file(&outer.join("template.json"), r#"{"name":"outer"}"#);
        write_file(
            &outer.join("config.json"),
            r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
        );
        write_file(
            &outer.join("kept/config.json"),
            r#"{"cell":{"type":"ref","template":"leaf@1.0.0"}}"#,
        );
        write_file(&outer.join("fresh/config.json"), ECHO_CFG);

        let templates = crate::templates::TemplatesRegistry::from_entries(vec![
            crate::templates::TemplateEntry {
                template_id: "leaf1".into(),
                name: "leaf".into(),
                version: Some("1.0.0".into()),
                filesystem_path: leaf,
            },
            crate::templates::TemplateEntry {
                template_id: "outer1".into(),
                name: "outer".into(),
                version: None,
                filesystem_path: outer,
            },
        ]);
        (colony, templates)
    }

    /// Per NODE, not per subtree: the requirements a merge resume owes are the
    /// ones of the nodes it stages, and a `ref` hop belongs to the node it
    /// hangs under. `m1` keeps its `kept` node (present on disk) and grows
    /// `fresh`, so `leaf`'s `ctx.api_key` is not consumed and not owed. `m2`
    /// grows both, so it is.
    #[test]
    fn a_merge_resume_owes_only_the_refs_of_the_nodes_it_stages() {
        let tmp = tempfile::tempdir().unwrap();
        let (colony, templates) = ref_resume_fixture(&tmp);
        let ctx = std::collections::HashMap::new();
        let env = std::collections::HashMap::new();

        // m1: root + `kept` on disk, only `fresh` missing.
        for rel in ["main/m1/config.json", "main/m1/kept/config.json"] {
            write_file(&colony.join(rel), r#"{"cell":{"type":"echo_type"}}"#);
        }
        let diff_m1 = json!({"add_nodes": [{"name": "m1", "template": "outer"}]});
        let ok = validate_requires(
            &diff_m1,
            &templates,
            &ctx,
            &env,
            &["m1".to_string()],
            LiveTree {
                root: &colony,
                scope: "/",
            },
        );
        assert!(
            ok.is_ok(),
            "the ref hangs under a node this resume leaves alone: {ok:?}"
        );

        // m2: only the root on disk — `kept` is staged, so its ref is owed.
        write_file(
            &colony.join("main/m2/config.json"),
            r#"{"cell":{"type":"hive"}}"#,
        );
        let diff_m2 = json!({"add_nodes": [{"name": "m2", "template": "outer"}]});
        let err = validate_requires(
            &diff_m2,
            &templates,
            &ctx,
            &env,
            &["m2".to_string()],
            LiveTree {
                root: &colony,
                scope: "/",
            },
        )
        .expect_err("this resume stages `kept`, so leaf's ctx.api_key is owed");
        let MutationError::RequirementMissing(msg) = &err else {
            panic!("expected RequirementMissing, got {err:?}");
        };
        assert!(msg.contains("api_key"), "refusal must name the key: {msg}");
    }
    /// GH #574 — `add_entry_match_view` reads the five routing terms of an
    /// `add_edges[]` entry exactly like the two Stage-6 sites used to read them
    /// by hand: scope-resolved endpoints, the raw condition string, the
    /// reconstructed modifier spec as JSON, and the routing phase.
    #[test]
    fn add_entry_match_view_reads_the_five_routing_terms() {
        let entry = json!({
            "from": "a",
            "to": "b",
            "condition": "body.kind == 'x'",
            "modifier": {"set_route": "recall"},
            "default": true,
            "lane": "recall"
        });
        let view = add_entry_match_view("/org", &entry).expect("from and to are present");
        assert_eq!(view.from, "/org/a");
        assert_eq!(view.to, "/org/b");
        assert_eq!(view.condition_source.as_deref(), Some("body.kind == 'x'"));
        assert!(view.is_default);
        let expected = crate::mutation::modifier_spec_from_add_entry(&entry)
            .and_then(|spec| meclaw_core::serde_json::to_value(&spec).ok());
        assert_eq!(
            view.modifier_source, expected,
            "the view must carry the SAME modifier reading the apply arm uses"
        );
    }

    /// GH #574 — an entry without `from`/`to` yields `None`, which is what the
    /// two Stage-6 loops used to express as `continue` ("a shape earlier stages
    /// refuse").
    #[test]
    fn add_entry_match_view_is_none_without_endpoints() {
        assert!(add_entry_match_view("/", &json!({"to": "b"})).is_none());
        assert!(add_entry_match_view("/", &json!({"from": "a"})).is_none());
        assert!(add_entry_match_view("/", &json!({"from": 1, "to": "b"})).is_none());
    }

    /// GH #574 — `lane_says` is the ONE wording both Stage-6 refusals speak.
    /// The exact strings are pinned by `gh559_a_v_lane_is_a_declared_deep_edge`.
    #[test]
    fn lane_says_speaks_one_wording() {
        assert_eq!(lane_says(None), "declares no lane");
        assert_eq!(lane_says(Some("recall")), "declares lane 'recall'");
    }

    /// GH #574 — the two-view form of the identity agrees term by term with the
    /// six-argument form it wraps.
    #[test]
    fn edge_identity_equal_views_agrees_with_the_term_form() {
        let a = add_entry_match_view("/", &json!({"from": "a", "to": "b"})).unwrap();
        let same = add_entry_match_view("/", &json!({"from": "a", "to": "b"})).unwrap();
        let other_phase =
            add_entry_match_view("/", &json!({"from": "a", "to": "b", "default": true})).unwrap();
        let other_cond =
            add_entry_match_view("/", &json!({"from": "a", "to": "b", "condition": "x"})).unwrap();
        assert!(edge_identity_equal_views(&a, &same));
        assert!(!edge_identity_equal_views(&a, &other_phase));
        assert!(!edge_identity_equal_views(&a, &other_cond));
        assert_eq!(
            edge_identity_equal_views(&a, &other_cond),
            edge_identity_equal(
                &a,
                other_cond.from.as_str(),
                other_cond.to.as_str(),
                other_cond.condition_source.as_deref(),
                other_cond.modifier_source.as_ref(),
                other_cond.is_default,
            ),
            "the wrapper must not invent a second identity"
        );
    }
}
