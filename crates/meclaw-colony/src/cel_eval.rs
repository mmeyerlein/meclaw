//! CEL-Engine wrapper for edge condition + modifier evaluation.
//!
//! Spec: `docs/meclaw-overview.md` § Edge-Modell (Z.819-845) +
//! § Edge-Expression-Sprache (Z.919-921 — "CEL via the rust crate").
//!
//! Phase 13.5-A1: provides `parse_condition`, `parse_modifier`,
//! `evaluate_condition`. Modifier-Eval (`apply_modifier_set` / `_delete`)
//! arrives in T7/T8.

use std::collections::BTreeMap;
use std::sync::Arc;

use cel::Program;
use meclaw_core::Headers;
use meclaw_core::serde_json::{Map, Value};

use crate::config::ModifierSpec;

/// A pre-compiled CEL condition expression. The source string is kept for
/// match-pattern equality (per spec Z.253 + A1-F6: string-equality, not
/// semantic).
#[derive(Clone, Debug)]
pub struct CompiledCondition {
    /// Original source string (for match-pattern equality).
    pub source: String,
    /// Pre-compiled program (Arc'd because `Program` is not Clone-cheap).
    pub program: Arc<Program>,
}

/// A pre-compiled modifier over the two header compartments: per-key compiled
/// CEL set-expressions + raw delete-lists, per compartment. The source
/// `ModifierSpec` is kept for match-pattern equality (Phase 13.5-A1 F6).
#[derive(Clone, Debug)]
pub struct CompiledModifier {
    /// Original modifier spec (for match-pattern equality).
    pub source: ModifierSpec,
    /// Per-key pre-compiled `context` set-expressions.
    pub set_context: BTreeMap<String, Arc<Program>>,
    /// `context` keys to remove (idempotent for non-existent keys).
    pub delete_context: Vec<String>,
    /// Per-key pre-compiled `hop` set-expressions.
    pub set_hop: BTreeMap<String, Arc<Program>>,
    /// `hop` keys to remove (idempotent for non-existent keys).
    pub delete_hop: Vec<String>,
    /// GH #82: this edge restores the message's routing budget (`ttl`). Carried
    /// verbatim from the spec — there is nothing to compile, the field is a
    /// declaration, not an expression. Applied by the colony (the envelope
    /// setter), never here: [`apply_modifier`] works on headers alone.
    pub restore_ttl: bool,
}

/// Parse a CEL condition source string into a `CompiledCondition`.
///
/// Phase 13.5-A1 T1/T2: parse-only, no evaluation. Used by bootstrap-plan
/// and mutation-validate to fail fast on malformed CEL.
pub fn parse_condition(source: &str) -> Result<CompiledCondition, String> {
    let program = Program::compile(source).map_err(|e| format!("cel parse: {e}"))?;
    Ok(CompiledCondition {
        source: source.to_string(),
        program: Arc::new(program),
    })
}

/// Parse a `ModifierSpec` into a `CompiledModifier` (pre-compile both
/// `set_context` and `set_hop` expression maps). Returns `(key, reason)` on
/// first parse error, so the caller can attribute the failure to a specific
/// key. The returned key is prefixed `set_context.`/`set_hop.` to disambiguate
/// the two compartments.
pub fn parse_modifier(spec: &ModifierSpec) -> Result<CompiledModifier, (String, String)> {
    let mut set_context = BTreeMap::new();
    for (k, expr) in &spec.set_context {
        let prog = Program::compile(expr)
            .map_err(|e| (format!("set_context.{k}"), format!("cel parse: {e}")))?;
        set_context.insert(k.clone(), Arc::new(prog));
    }
    let mut set_hop = BTreeMap::new();
    for (k, expr) in &spec.set_hop {
        let prog = Program::compile(expr)
            .map_err(|e| (format!("set_hop.{k}"), format!("cel parse: {e}")))?;
        set_hop.insert(k.clone(), Arc::new(prog));
    }
    Ok(CompiledModifier {
        source: spec.clone(),
        set_context,
        delete_context: spec.delete_context.clone(),
        set_hop,
        delete_hop: spec.delete_hop.clone(),
        restore_ttl: spec.restore_ttl,
    })
}

/// Convert one JSON value into the CEL value an expression sees (GH #500).
///
/// The serde path this replaced went through `serde_json::Number`'s own
/// `Serialize`, which emits every non-negative integer via `serialize_u64` and
/// therefore bound `200` as CEL `uint`. cel 0.13's runtime equality downcasts
/// to its own type and nothing else (`common/types/int.rs`, `uint.rs`), so
/// `uint(200) == 200` was `false` — no error, no advisory, just an edge that
/// never fired. Ordering was cross-type all along, which is why `> 100` worked
/// and hid the class.
///
/// The mapping here is the one the CEL spec gives a JSON number: `int` when it
/// fits `i64`, `uint` only above `i64::MAX` (where `int` cannot hold it), and
/// `double` otherwise. A `uint` is then reachable exactly where CEL says it is
/// — through a `u`-suffixed literal or the `uint()` cast — and not by accident.
fn json_to_cel(v: &Value) -> cel::Value {
    use cel::Value as Cv;
    use cel::objects::{Key, Map as CelMap};
    match v {
        Value::Null => Cv::Null,
        Value::Bool(b) => Cv::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Cv::Int(i)
            } else if let Some(u) = n.as_u64() {
                // Only reachable above i64::MAX: no `int` can hold it.
                Cv::UInt(u)
            } else if let Some(f) = n.as_f64() {
                Cv::Float(f)
            } else {
                Cv::Null
            }
        }
        Value::String(s) => Cv::String(Arc::new(s.clone())),
        Value::Array(a) => Cv::List(Arc::new(a.iter().map(json_to_cel).collect())),
        Value::Object(o) => Cv::Map(CelMap {
            map: Arc::new(
                o.iter()
                    .map(|(k, val)| (Key::String(Arc::new(k.clone())), json_to_cel(val)))
                    .collect(),
            ),
        }),
    }
}

/// Bind one header compartment as a CEL map value (GH #500).
fn compartment_to_cel(compartment: &Map<String, Value>) -> cel::Value {
    use cel::objects::{Key, Map as CelMap};
    cel::Value::Map(CelMap {
        map: Arc::new(
            compartment
                .iter()
                .map(|(k, v)| (Key::String(Arc::new(k.clone())), json_to_cel(v)))
                .collect(),
        ),
    })
}

/// Bind the two header compartments (`context` and `hop`) as CEL variables.
///
/// Shared by [`evaluate_condition`] and [`apply_modifier`] so both expose the
/// same `context.*`/`hop.*` namespace — and, since GH #500, the same number
/// typing: both go through [`json_to_cel`], so a condition and a modifier read
/// an identical value out of an identical header.
fn bind_ctx<'a>(context: &Map<String, Value>, hop: &Map<String, Value>) -> cel::Context<'a> {
    let mut ctx = cel::Context::default();
    ctx.add_variable_from_value("context", compartment_to_cel(context));
    ctx.add_variable_from_value("hop", compartment_to_cel(hop));
    ctx
}

/// Apply a `CompiledModifier` to the incoming two-compartment headers,
/// returning fresh headers (input is read-only).
///
/// Per spec § Edge-Modell Z.820-833: all `set_*` expressions read the incoming
/// (pre-modifier) `context.*`/`hop.*` as a fixed namespace — never the output
/// of another set-key. Per spec Z.832: per compartment, set runs before delete.
pub fn apply_modifier(m: &CompiledModifier, headers_in: &Headers) -> Result<Headers, String> {
    let mut out = headers_in.clone();
    for (k, prog) in &m.set_context {
        let ctx = bind_ctx(&headers_in.context, &headers_in.hop);
        let v = prog
            .execute(&ctx)
            .map_err(|e| format!("cel eval set_context.{k}: {e}"))?;
        out.context.insert(k.clone(), cel_to_json(v));
    }
    for (k, prog) in &m.set_hop {
        let ctx = bind_ctx(&headers_in.context, &headers_in.hop);
        let v = prog
            .execute(&ctx)
            .map_err(|e| format!("cel eval set_hop.{k}: {e}"))?;
        out.hop.insert(k.clone(), cel_to_json(v));
    }
    for k in &m.delete_context {
        out.context.remove(k);
    }
    for k in &m.delete_hop {
        out.hop.remove(k);
    }
    Ok(out)
}

/// Convert a `cel::Value` back into a `serde_json::Value` for header storage.
///
/// Lossy for cel-only types (Duration / Timestamp / Bytes / Function /
/// Opaque) — stored as Debug-string (Null for Bytes). Phase-13.5-A1 does
/// not expect those in modifier outputs.
fn cel_to_json(v: cel::Value) -> Value {
    use cel::Value as Cv;
    match v {
        Cv::Null => Value::Null,
        Cv::Bool(b) => Value::Bool(b),
        Cv::Int(i) => Value::from(i),
        Cv::UInt(u) => Value::from(u),
        Cv::Float(f) => serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Cv::String(s) => Value::String(s.as_ref().clone()),
        Cv::List(l) => Value::Array(l.iter().cloned().map(cel_to_json).collect()),
        Cv::Map(m) => {
            use cel::objects::Key;
            let obj: Map<String, Value> = m
                .map
                .iter()
                .map(|(k, val)| {
                    let key_str = match k {
                        Key::String(s) => s.as_ref().clone(),
                        Key::Int(i) => i.to_string(),
                        Key::Uint(u) => u.to_string(),
                        Key::Bool(b) => b.to_string(),
                    };
                    (key_str, cel_to_json(val.clone()))
                })
                .collect();
            Value::Object(obj)
        }
        Cv::Bytes(_) => Value::Null,
        other => Value::String(format!("{other:?}")),
    }
}

/// GH #80: why a condition did not produce a boolean.
///
/// The two classes want different operator attention, and only one of them is a
/// defect: a fan-out edge that discriminates on an optional `hop` key errors on
/// every message that does not carry that key, which is most of them. That is
/// the steady state, not a fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CondErrorKind {
    /// A key the expression reads is absent from the bound compartments. CEL
    /// standard semantics: reading it is an error, and per spec F3 the edge is
    /// skipped. Nothing is wrong with the colony.
    ///
    /// Note the honest limit: CEL cannot tell a legitimately absent key from a
    /// mistyped one. `hop.toolname` and an absent `hop.tool_name` are the same
    /// event here. What stays visible is the class below — a typo at the
    /// compartment level (`hopp.tool_name`) or a shape error.
    MissingKey,
    /// Everything else: type mismatch, incomparable values, a reference to a
    /// variable nobody bound, a non-boolean result, recursion limit. These are
    /// builder errors and stay loud.
    Eval,
}

/// A condition that failed to evaluate, with its class ([`CondErrorKind`]).
///
/// `Display` is the message alone, so call sites that only format the error
/// read exactly as they did before the class existed.
#[derive(Debug, Clone)]
pub struct CondError {
    /// Which of the two classes this is.
    pub kind: CondErrorKind,
    /// Human-readable reason, unchanged in wording from before GH #80.
    pub message: String,
}

impl std::fmt::Display for CondError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CondError {}

impl CondError {
    fn eval(message: String) -> Self {
        Self {
            kind: CondErrorKind::Eval,
            message,
        }
    }
}

/// Evaluate a `CompiledCondition` against a headers map.
///
/// Returns `Ok(true)`/`Ok(false)` on successful eval, `Err(CondError)` for
/// everything else (undefined map access, type errors, recursion limit).
/// Callers (e.g. `evaluate_edge`) skip the edge on `Err` per spec F3
/// (CEL-Standard: undefined-header → skip); GH #80 added the class so the log
/// level can follow the class instead of treating every fan-out miss as a fault.
pub fn evaluate_condition(
    cond: &CompiledCondition,
    context: &Map<String, Value>,
    hop: &Map<String, Value>,
) -> Result<bool, CondError> {
    // CEL surface is `context.*`/`hop.*`: bind both compartments as variables.
    // The mapping is explicit (GH #500) rather than serde's: serde re-serialises
    // every non-negative JSON integer via `serialize_u64`, which bound `200` as
    // CEL `uint` and made `hop.http_status == 200` unsatisfiable.
    let ctx = bind_ctx(context, hop);
    let value = cond.program.execute(&ctx).map_err(|e| CondError {
        // The engine's own distinction: `NoSuchKey` is the absent key, every
        // other variant is a defect in the expression or in the values.
        kind: match e {
            cel::ExecutionError::NoSuchKey(_) => CondErrorKind::MissingKey,
            _ => CondErrorKind::Eval,
        },
        message: format!("cel eval: {e}"),
    })?;
    match value {
        cel::Value::Bool(b) => Ok(b),
        other => Err(CondError::eval(format!("cel result not bool: {other:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_condition_simple_eq_succeeds() {
        let c = parse_condition("hop.foo == 'bar'").expect("parse");
        assert_eq!(c.source, "hop.foo == 'bar'");
    }

    #[test]
    fn parse_condition_malformed_returns_err() {
        let r = parse_condition("hop.foo ===");
        assert!(r.is_err(), "expected parse err, got {:?}", r);
    }

    #[test]
    fn parse_modifier_compiles_all_set_expressions() {
        let mut spec = ModifierSpec::default();
        spec.set_hop
            .insert("tier".into(), "hop.priority == 'high'".into());
        spec.set_hop.insert("other".into(), "'literal'".into());
        let m = parse_modifier(&spec).expect("parse");
        assert_eq!(m.set_hop.len(), 2);
        assert!(m.set_hop.contains_key("tier"));
        assert!(m.set_hop.contains_key("other"));
    }

    #[test]
    fn parse_modifier_returns_prefixed_key_on_first_bad_expression() {
        let mut spec = ModifierSpec::default();
        spec.set_hop.insert("bad".into(), "==".into());
        let (k, _reason) = parse_modifier(&spec).unwrap_err();
        assert_eq!(k, "set_hop.bad");
    }

    #[test]
    fn parse_modifier_attributes_set_context_key() {
        let mut spec = ModifierSpec::default();
        spec.set_context.insert("bad".into(), "==".into());
        let (k, _reason) = parse_modifier(&spec).unwrap_err();
        assert_eq!(k, "set_context.bad");
    }

    #[test]
    fn evaluate_condition_eq_string_returns_true() {
        let c = parse_condition("hop.priority == 'high'").unwrap();
        let mut hop = Map::new();
        hop.insert("priority".into(), Value::String("high".into()));
        let result = evaluate_condition(&c, &Map::new(), &hop).expect("eval");
        assert!(result, "expected true for matching condition");
    }

    #[test]
    fn evaluate_condition_eq_string_returns_false_on_mismatch() {
        let c = parse_condition("hop.priority == 'high'").unwrap();
        let mut hop = Map::new();
        hop.insert("priority".into(), Value::String("low".into()));
        let result = evaluate_condition(&c, &Map::new(), &hop).expect("eval");
        assert!(!result, "expected false for non-matching condition");
    }

    /// The two forms a hive-boundary contract needs, checked empirically rather
    /// than assumed: a route tested against a LIST of accepted lanes, and a
    /// prefix test. A hive that says "I accept every `in_*` route" is writing one
    /// of these two into an edge, and cel 0.13's support for them is not
    /// something the overview's function-scope note covers (it names `has()`,
    /// the `in` key-existence form, `contains` and `int()`).
    #[test]
    fn evaluate_condition_supports_the_forms_a_hive_contract_needs() {
        let list =
            parse_condition("has(hop.route) && hop.route in ['in_turn', 'in_tool', 'in_answer']")
                .expect("a value-in-list condition must parse");
        for (route, want) in [("in_tool", true), ("in_turn", true), ("brain", false)] {
            let mut hop = Map::new();
            hop.insert("route".into(), Value::String(route.into()));
            assert_eq!(
                evaluate_condition(&list, &Map::new(), &hop).expect("eval"),
                want,
                "route {route} against the accepted list"
            );
        }

        let prefix = parse_condition("has(hop.route) && hop.route.startsWith('in_')")
            .expect("a startsWith condition must parse");
        for (route, want) in [("in_bundle", true), ("brain", false), ("i", false)] {
            let mut hop = Map::new();
            hop.insert("route".into(), Value::String(route.into()));
            assert_eq!(
                evaluate_condition(&prefix, &Map::new(), &hop).expect("eval"),
                want,
                "route {route} against the in_ prefix"
            );
        }

        // The guard that makes either form safe on a message with no route at
        // all: `has()` short-circuits before the member test runs.
        let mut empty = Map::new();
        empty.insert("other".into(), Value::String("x".into()));
        assert!(!evaluate_condition(&list, &Map::new(), &empty).expect("eval"));
        assert!(!evaluate_condition(&prefix, &Map::new(), &empty).expect("eval"));
    }

    #[test]
    fn evaluate_condition_ternary_returns_correct_branch() {
        let c = parse_condition("hop.finish_reason == 'tool_calls' ? true : false").unwrap();
        let mut hop = Map::new();
        hop.insert("finish_reason".into(), Value::String("tool_calls".into()));
        assert!(evaluate_condition(&c, &Map::new(), &hop).unwrap());

        hop.insert("finish_reason".into(), Value::String("stop".into()));
        assert!(!evaluate_condition(&c, &Map::new(), &hop).unwrap());
    }

    #[test]
    fn evaluate_condition_number_comparison() {
        let c = parse_condition("hop.tokens_prompt > 100").unwrap();
        let mut hop = Map::new();
        hop.insert("tokens_prompt".into(), Value::from(150_i64));
        assert!(evaluate_condition(&c, &Map::new(), &hop).unwrap());
        hop.insert("tokens_prompt".into(), Value::from(50_i64));
        assert!(!evaluate_condition(&c, &Map::new(), &hop).unwrap());
    }

    /// GH #500: the plain equality form on a numeric header key. Before the
    /// explicit binding, `hop.http_status == 200` was `false` on a message
    /// carrying exactly 200 — serde bound the JSON integer as CEL `uint`, and
    /// cel 0.13's runtime equality is same-type only. Ordering was cross-type
    /// all along, which is the asymmetry that made the class expensive to find:
    /// the builder gets no error, only an edge that never fires.
    #[test]
    fn gh500_a_numeric_header_key_binds_as_int_so_plain_equality_holds() {
        let mut hop = Map::new();
        hop.insert("http_status".into(), Value::from(200_i64));

        for (src, want) in [
            ("hop.http_status == 200", true),
            ("hop.http_status == 404", false),
            ("hop.http_status != 404", true),
            ("hop.http_status > 100", true),
            ("hop.http_status >= 200 && hop.http_status < 300", true),
            ("int(hop.http_status) == 200", true),
        ] {
            let c = parse_condition(src).expect("parse");
            assert_eq!(
                evaluate_condition(&c, &Map::new(), &hop).expect("eval"),
                want,
                "GH #500: `{src}` against hop.http_status = 200"
            );
        }

        // Arithmetic on a header integer no longer needs the `int()` cast: the
        // bound value and the literal are the same CEL type. The cast stays
        // valid (it is an identity on an int), which is why every shipped
        // topology written against the old quirk keeps running.
        let c = parse_condition("hop.http_status + 1 == 201").expect("parse");
        assert!(
            evaluate_condition(&c, &Map::new(), &hop).expect("eval"),
            "GH #500: uint + int used to be an unsupported mixed-type op"
        );
    }

    /// GH #500, the half that changed direction: CEL equality across `uint` and
    /// `int` is false in BOTH directions, so a `u`-suffixed literal — the
    /// workaround that used to be the only form that worked — no longer matches
    /// an int-bound header. It is pinned rather than left implicit: this is the
    /// one migration cost of the fix, and no shipped template or example spells
    /// a `u` literal (there is a sweep for that in the colony tests).
    #[test]
    fn gh500_a_uint_literal_no_longer_matches_an_int_bound_header() {
        let mut hop = Map::new();
        hop.insert("http_status".into(), Value::from(200_i64));

        let suffixed = parse_condition("hop.http_status == 200u").expect("parse");
        assert!(
            !evaluate_condition(&suffixed, &Map::new(), &hop).expect("eval"),
            "GH #500: a `u` literal is a uint and no longer equals an int header"
        );

        // The spelling that survives, for anyone who wants unsigned semantics
        // explicitly: cast the header, not the literal.
        let cast = parse_condition("uint(hop.http_status) == 200u").expect("parse");
        assert!(
            evaluate_condition(&cast, &Map::new(), &hop).expect("eval"),
            "GH #500: uint() on the header restores the unsigned comparison"
        );
    }

    /// GH #500: the three number shapes the binding has to keep apart. `int`
    /// for everything that fits `i64` (negatives included — those were already
    /// `Int` under serde, which is why the defect only ever showed on the
    /// non-negative side), `uint` only above `i64::MAX`, `double` for the rest.
    #[test]
    fn gh500_number_binding_covers_int_uint_and_double() {
        let mut hop = Map::new();
        hop.insert("neg".into(), Value::from(-7_i64));
        hop.insert("zero".into(), Value::from(0_i64));
        hop.insert("big".into(), Value::from(u64::MAX));
        hop.insert("ratio".into(), Value::from(1.5_f64));

        for (src, want) in [
            ("hop.neg == -7", true),
            ("hop.zero == 0", true),
            ("hop.ratio == 1.5", true),
            ("hop.ratio > 1.0", true),
            // Above i64::MAX there is no int to bind to, so the value stays a
            // uint and only the uint spelling reaches it.
            ("hop.big == 18446744073709551615u", true),
            ("hop.big > 0", true),
        ] {
            let c = parse_condition(src).expect("parse");
            assert_eq!(
                evaluate_condition(&c, &Map::new(), &hop).expect("eval"),
                want,
                "GH #500: `{src}`"
            );
        }
    }

    /// GH #500: the nested shapes go through the same mapping. A header value
    /// is not always a scalar — a list of status codes and an object with a
    /// numeric field are both legitimate, and a number inside either must bind
    /// exactly as a top-level one does, or the fix is only half a fix.
    #[test]
    fn gh500_numbers_nested_in_lists_and_objects_bind_as_int_too() {
        let mut hop = Map::new();
        hop.insert(
            "codes".into(),
            Value::Array(vec![Value::from(200_i64), Value::from(404_i64)]),
        );
        let mut obj = Map::new();
        obj.insert("status".into(), Value::from(204_i64));
        obj.insert("label".into(), Value::String("no content".into()));
        hop.insert("last".into(), Value::Object(obj));

        for (src, want) in [
            ("200 in hop.codes", true),
            ("500 in hop.codes", false),
            ("hop.codes[0] == 200", true),
            ("hop.last.status == 204", true),
            ("hop.last.label == 'no content'", true),
        ] {
            let c = parse_condition(src).expect("parse");
            assert_eq!(
                evaluate_condition(&c, &Map::new(), &hop).expect("eval"),
                want,
                "GH #500: `{src}`"
            );
        }
    }

    /// GH #500: a modifier reads the same numbers a condition does. The two
    /// used to share a binding helper only in name — both went through serde,
    /// so both saw `uint`; now both go through `json_to_cel`, and the pin says
    /// so from the modifier side.
    #[test]
    fn gh500_a_modifier_sees_the_same_int_a_condition_sees() {
        let spec = ModifierSpec {
            set_hop: BTreeMap::from([
                ("ok".into(), "hop.http_status == 200".into()),
                ("next".into(), "context.iter + 1".into()),
            ]),
            ..ModifierSpec::default()
        };
        let m = parse_modifier(&spec).expect("parse");
        let mut h = Headers::new();
        h.hop.insert("http_status".into(), Value::from(200_i64));
        h.context.insert("iter".into(), Value::from(1_i64));
        let out = apply_modifier(&m, &h).expect("apply");
        assert_eq!(out.hop.get("ok"), Some(&Value::Bool(true)));
        assert_eq!(out.hop.get("next"), Some(&Value::from(2_i64)));
    }

    #[test]
    fn evaluate_condition_string_contains() {
        let c = parse_condition("hop.model.contains('gpt-4')").unwrap();
        let mut hop = Map::new();
        hop.insert("model".into(), Value::String("gpt-4o-mini".into()));
        assert!(evaluate_condition(&c, &Map::new(), &hop).unwrap());
    }

    #[test]
    fn condition_reads_both_namespaces() {
        let c = parse_condition("hop.route == 'fire' && context.iter > 0").unwrap();
        let mut h = Headers::new();
        h.hop.insert("route".into(), Value::String("fire".into()));
        h.context.insert("iter".into(), Value::from(2_i64));
        assert!(evaluate_condition(&c, &h.context, &h.hop).unwrap());
    }

    #[test]
    fn modifier_set_hop_inserts_new_key() {
        let mut spec = ModifierSpec::default();
        spec.set_hop.insert("tier".into(), "'gold'".into());
        let m = parse_modifier(&spec).unwrap();
        let out = apply_modifier(&m, &Headers::new()).expect("apply");
        assert_eq!(out.hop.get("tier"), Some(&Value::String("gold".into())));
    }

    #[test]
    fn modifier_set_hop_overrides_existing_key() {
        let mut spec = ModifierSpec::default();
        spec.set_hop.insert("priority".into(), "'high'".into());
        let m = parse_modifier(&spec).unwrap();
        let mut h = Headers::new();
        h.hop.insert("priority".into(), Value::String("low".into()));
        let out = apply_modifier(&m, &h).expect("apply");
        assert_eq!(out.hop.get("priority"), Some(&Value::String("high".into())));
    }

    #[test]
    fn modifier_set_context_promotes_hop_value() {
        let spec = ModifierSpec {
            set_context: BTreeMap::from([("turn_id".into(), "hop.turn_id".into())]),
            ..ModifierSpec::default()
        };
        let m = parse_modifier(&spec).unwrap();
        let mut h = Headers::new();
        h.hop.insert("turn_id".into(), Value::String("t1".into()));
        let out = apply_modifier(&m, &h).expect("apply");
        assert_eq!(
            out.context.get("turn_id"),
            Some(&Value::String("t1".into()))
        );
    }

    #[test]
    fn modifier_set_context_computes_loop_counter() {
        // cel-0.13 quirk: a positive JSON integer header value round-trips to
        // cel `UInt` (serde_json re-serialises positive ints via `serialize_u64`),
        // and `UInt + Int(literal)` is an unsupported mixed-type op. A loop
        // counter must therefore cast: `int(context.iter) + 1`. The behavioural
        // intent (1 → 2) is unchanged. See report concern for Slice-3 follow-up.
        let spec = ModifierSpec {
            set_context: BTreeMap::from([("iter".into(), "int(context.iter) + 1".into())]),
            ..ModifierSpec::default()
        };
        let m = parse_modifier(&spec).unwrap();
        let mut h = Headers::new();
        h.context.insert("iter".into(), Value::from(1_i64));
        let out = apply_modifier(&m, &h).expect("apply");
        assert_eq!(out.context.get("iter"), Some(&Value::from(2_i64)));
    }

    #[test]
    fn modifier_set_hop_and_deletes_per_fach() {
        let spec = ModifierSpec {
            set_hop: BTreeMap::from([("route".into(), "'fire'".into())]),
            delete_hop: vec!["operation".into()],
            delete_context: vec!["scratch".into()],
            ..ModifierSpec::default()
        };
        let m = parse_modifier(&spec).unwrap();
        let mut h = Headers::new();
        h.hop
            .insert("operation".into(), Value::String("select".into()));
        h.context.insert("scratch".into(), Value::Bool(true));
        let out = apply_modifier(&m, &h).expect("apply");
        assert_eq!(out.hop.get("route"), Some(&Value::String("fire".into())));
        assert!(out.hop.get("operation").is_none());
        assert!(out.context.get("scratch").is_none());
    }

    #[test]
    fn modifier_set_reads_from_input_headers() {
        let mut spec = ModifierSpec::default();
        spec.set_hop.insert(
            "tier".into(),
            "hop.priority == 'high' ? 'gold' : 'standard'".into(),
        );
        let m = parse_modifier(&spec).unwrap();
        let mut h = Headers::new();
        h.hop
            .insert("priority".into(), Value::String("high".into()));
        let out = apply_modifier(&m, &h).expect("apply");
        assert_eq!(out.hop.get("tier"), Some(&Value::String("gold".into())));
    }

    #[test]
    fn modifier_delete_hop_removes_existing_key() {
        let spec = ModifierSpec {
            delete_hop: vec!["debug".into()],
            ..ModifierSpec::default()
        };
        let m = parse_modifier(&spec).unwrap();
        let mut h = Headers::new();
        h.hop.insert("debug".into(), Value::String("on".into()));
        h.hop.insert("keep".into(), Value::String("yes".into()));
        let out = apply_modifier(&m, &h).expect("apply");
        assert!(out.hop.get("debug").is_none());
        assert!(out.hop.get("keep").is_some());
    }

    /// Phase 13.5-A1 T9 (F6 pin): the source string of `CompiledCondition` is
    /// preserved for `remove_edges`/`swap_nodes` match-pattern equality
    /// (string equality, not semantic).
    #[test]
    fn compiled_condition_preserves_source_for_match_pattern_f6() {
        let c = parse_condition("hop.x == 'y'").unwrap();
        assert_eq!(
            c.source, "hop.x == 'y'",
            "F6-PIN: source string preserved for remove_edges/swap_nodes match-pattern equality"
        );
    }

    /// Phase 13.5-A1 T9 (F6 pin): the source `ModifierSpec` of `CompiledModifier`
    /// is preserved for match-pattern equality (set-expr strings + delete keys).
    #[test]
    fn compiled_modifier_preserves_source_for_match_pattern_f6() {
        let mut spec = ModifierSpec::default();
        spec.set_hop.insert("tier".into(), "'gold'".into());
        spec.delete_hop = vec!["debug".into()];
        let m = parse_modifier(&spec).unwrap();
        assert_eq!(
            m.source.set_hop.get("tier").map(String::as_str),
            Some("'gold'")
        );
        assert_eq!(m.source.delete_hop, vec!["debug"]);
    }

    #[test]
    fn modifier_delete_non_existent_key_is_idempotent_noop() {
        let spec = ModifierSpec {
            delete_hop: vec!["never_was".into()],
            ..ModifierSpec::default()
        };
        let m = parse_modifier(&spec).unwrap();
        let mut h = Headers::new();
        h.hop.insert("keep".into(), Value::String("yes".into()));
        let out = apply_modifier(&m, &h).expect("apply");
        assert_eq!(out.hop.len(), 1);
        assert!(out.hop.get("keep").is_some());
    }
}
