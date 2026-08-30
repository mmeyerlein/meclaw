//! Variable substitution for mutation diffs (phase 6).
//!
//! `${ENV_VAR}` from `.env`, `${ctx.<key>}` from the mutation ctx,
//! `${uuid7:label}` with a per-label cache. T7 covers only `${ENV_VAR}` — T8/T9
//! extend it.
//!
//! # Placeholder classes (GH #20)
//!
//! A placeholder belongs to exactly one of two classes, and the class decides
//! WHEN it resolves and what an instance carries on disk:
//!
//! | class | forms | owner | resolves at | on disk after instantiation |
//! |---|---|---|---|---|
//! | environment | `${VAR}`, `${VAR:-default}` | the environment (`.env`) | every read of `config.json` (boot AND instantiation), in memory | the token, LITERALLY |
//! | instance | `${ctx.<key>}`, `${uuid7:<label>}` | the instance | instantiation, once | the resolved value |
//!
//! The environment class never materializes: an API key referenced as `${VAR}`
//! in a template stays `${VAR}` in the instantiated `config.json` and is bound
//! late, from `.env`, at every read. The instance class is the opposite by
//! definition -- a `${ctx.*}`/`${uuid7:*}` value IS the instance's identity and
//! would be meaningless re-resolved later.
//!
//! [`substitute_instance_only`] is the disk-facing pass (instance class only),
//! [`substitute_env_only`] the late-binding pass applied in memory on top of it
//! -- at boot ([`crate::plan_bootstrap`]) and, with the same result, on the
//! freshly staged config so a just-instantiated cell sees exactly what it would
//! see after a reboot.

use super::MutationError;
use meclaw_core::JsonValue;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub fn substitute_env_only(
    diff: &JsonValue,
    env: &HashMap<String, String>,
) -> Result<JsonValue, MutationError> {
    fn walk(v: &JsonValue, env: &HashMap<String, String>) -> Result<JsonValue, MutationError> {
        match v {
            JsonValue::String(s) => Ok(JsonValue::String(replace_env(s, env)?)),
            JsonValue::Array(a) => {
                let mut out = Vec::with_capacity(a.len());
                for item in a {
                    out.push(walk(item, env)?);
                }
                Ok(JsonValue::Array(out))
            }
            JsonValue::Object(m) => {
                let mut out = meclaw_core::serde_json::Map::new();
                for (k, val) in m {
                    out.insert(k.clone(), walk(val, env)?);
                }
                Ok(JsonValue::Object(out))
            }
            _ => Ok(v.clone()),
        }
    }
    walk(diff, env)
}

/// Parsed shape of an env substitution token's `inner` (the text between `${`
/// and `}`). ONLY for env tokens — callers must route `ctx.`/`uuid7:` tokens
/// elsewhere before calling this.
#[derive(Debug, PartialEq)]
enum EnvToken {
    /// Plain `${VAR}` — strict lookup, missing var is an error.
    Plain(String),
    /// POSIX `${VAR:-fallback}` — use `fallback` when the var is unset OR empty.
    DefaultIfUnsetOrEmpty { name: String, fallback: String },
    /// Any other operator form (`${VAR:=x}`, `${VAR-x}`, `${VAR:+x}`,
    /// `${VAR:?m}`, …) — unsupported, raised as a loud error. Carries `inner`.
    Unsupported(String),
}

/// Classify an env-token `inner` into [`EnvToken`].
///
/// Rules: no operator char → `Plain`. Exactly `name:-fallback` →
/// `DefaultIfUnsetOrEmpty`. Anything else carrying an operator (`:` followed by
/// something other than `-`, or a bare `-`/`+`/`?`/`=` right after the name) →
/// `Unsupported`. Spec § Variable substitution → `${ENV_VAR}` from `.env`.
fn parse_env_token(inner: &str) -> EnvToken {
    // POSIX default: name + ":-" + fallback. Split on the FIRST ":-".
    if let Some(op) = inner.find(':') {
        let name = &inner[..op];
        let after_colon = &inner[op + 1..];
        if let Some(fallback) = after_colon.strip_prefix('-') {
            return EnvToken::DefaultIfUnsetOrEmpty {
                name: name.to_owned(),
                fallback: fallback.to_owned(),
            };
        }
        // `:` followed by anything other than `-` (`:=`, `:+`, `:?`, …).
        return EnvToken::Unsupported(inner.to_owned());
    }
    // No `:` — a bare operator right in the token (`-`, `+`, `?`, `=`) is
    // unsupported. `${VAR}` itself contains none of these → Plain.
    if inner.contains(['-', '+', '?', '=']) {
        return EnvToken::Unsupported(inner.to_owned());
    }
    EnvToken::Plain(inner.to_owned())
}

/// Shared `$`-driven scanner for all substitute functions.
///
/// Walks `s` from one `$` to the next. At a `$`:
/// - `$${` (escape, checked FIRST) → consume one `$`, emit `${inner}` literally
///   (no resolve), continue after the closing `}`. With `keep_escapes` the whole
///   `$${inner}` is emitted unchanged instead, so a LATER pass still sees the
///   escape (GH #20: the disk pass must not eat an escape the boot pass owns).
/// - `${` (regular token) → find the closing `}`, hand `inner` to `resolve`.
///   No closing `}` → `MutationError::Schema`.
/// - lone `$` → literal `$`.
///
/// `resolve` is the per-function token resolver. Byte-correct: scanning works on
/// byte offsets, but all slice boundaries land on `$`/`{`/`}` (ASCII) or on a
/// `find('$')` hit, so a multi-byte char is never split.
fn expand_with(
    s: &str,
    keep_escapes: bool,
    resolve: impl Fn(&str) -> Result<String, MutationError>,
) -> Result<String, MutationError> {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            // Push the run up to the next `$` in one go (byte-safe: `$` is ASCII,
            // so the slice boundary never lands inside a multi-byte char).
            let next = s[i..].find('$').map(|d| i + d).unwrap_or(bytes.len());
            out.push_str(&s[i..next]);
            i = next;
            continue;
        }
        // bytes[i] == '$'. Check escape `$${` BEFORE regular `${`.
        if bytes.get(i + 1) == Some(&b'$') && bytes.get(i + 2) == Some(&b'{') {
            // Escape: consume the leading `$`, emit `${inner}` literally.
            let after = &s[i + 3..];
            let end = after
                .find('}')
                .ok_or_else(|| MutationError::Schema(format!("unterminated ${{...}} in: {s}")))?;
            let inner = &after[..end];
            if keep_escapes {
                out.push('$');
            }
            out.push_str("${");
            out.push_str(inner);
            out.push('}');
            i = i + 3 + end + 1;
        } else if bytes.get(i + 1) == Some(&b'{') {
            // Regular token: ${inner}.
            let after = &s[i + 2..];
            let end = after
                .find('}')
                .ok_or_else(|| MutationError::Schema(format!("unterminated ${{...}} in: {s}")))?;
            let inner = &after[..end];
            out.push_str(&resolve(inner)?);
            i = i + 2 + end + 1;
        } else {
            // Lone `$`.
            out.push('$');
            i += 1;
        }
    }
    Ok(out)
}

/// [`expand_with`] with the historic escape behavior (the escape is consumed).
fn expand(
    s: &str,
    resolve: impl Fn(&str) -> Result<String, MutationError>,
) -> Result<String, MutationError> {
    expand_with(s, false, resolve)
}

/// Resolve a plain env-token `inner` against `env` (shared by all three fns).
fn resolve_env_token(inner: &str, env: &HashMap<String, String>) -> Result<String, MutationError> {
    match parse_env_token(inner) {
        EnvToken::Plain(name) => env
            .get(&name)
            .cloned()
            .ok_or(MutationError::EnvVarMissing(name)),
        EnvToken::DefaultIfUnsetOrEmpty { name, fallback } => Ok(env
            .get(&name)
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or(fallback)),
        EnvToken::Unsupported(form) => Err(MutationError::UnsupportedSubstitution(form)),
    }
}

fn replace_env(s: &str, env: &HashMap<String, String>) -> Result<String, MutationError> {
    expand(s, |inner| resolve_env_token(inner, env))
}

/// Every `${ctx.<key>}` key that occurs in a STRING VALUE of `value` (GH #292).
///
/// This is the read half of the `requires.ctx` declaration: what a template
/// ASKS for is derived from what it USES, so the two cannot drift apart. It is
/// asked of the substrate's own scanner ([`expand_with`], the one
/// [`substitute_instance_only`] resolves with) rather than of a pattern of its
/// own, and three properties follow from that rather than from a rule written
/// down here:
///
/// - an escaped `$${ctx.x}` is a literal, so it is not a requirement — the
///   scanner never hands an escape to the resolver;
/// - a malformed token (`${ctx.x` with no closing brace) is the SAME error it
///   already is on the resolving path, instead of a key silently dropped;
/// - object KEYS are not scanned. A key literally named `ctx.model` is a name,
///   not a placeholder, and only values are ever substituted.
///
/// The result is ordered, so a message listing missing or superfluous keys is
/// stable.
pub fn collect_ctx_keys(value: &JsonValue) -> Result<BTreeSet<String>, MutationError> {
    // `expand_with` takes `impl Fn`; the accumulator therefore needs interior
    // mutability, the same single-threaded RefCell as `replace_full`'s uuid7
    // cache (one cell task, no contention — AGENTS.md concurrency model).
    let found = std::cell::RefCell::new(BTreeSet::new());
    walk_ctx_keys(value, &found)?;
    Ok(found.into_inner())
}

fn walk_ctx_keys(
    v: &JsonValue,
    found: &std::cell::RefCell<BTreeSet<String>>,
) -> Result<(), MutationError> {
    match v {
        JsonValue::String(s) => {
            // The expansion itself is discarded: every token resolves to the
            // empty string, because the question is WHICH keys occur and no
            // value is available (or needed) to answer it.
            expand_with(s, true, |inner| {
                if let Some(key) = inner.strip_prefix("ctx.") {
                    found.borrow_mut().insert(key.to_owned());
                }
                Ok(String::new())
            })?;
            Ok(())
        }
        JsonValue::Array(a) => {
            for item in a {
                walk_ctx_keys(item, found)?;
            }
            Ok(())
        }
        JsonValue::Object(m) => {
            for val in m.values() {
                walk_ctx_keys(val, found)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Every `${ENV_VAR}` name that occurs in a STRING VALUE of `value`, and
/// whether that occurrence carried a default (GH #465).
///
/// The env twin of [`collect_ctx_keys`], and it exists for the same reason: a
/// `requires.env` declaration is the read half of what a template USES, so the
/// two have to be derivable from one another or they drift. It is asked of the
/// substrate's own scanner ([`expand_with`]) and its own token grammar
/// ([`parse_env_token`]) rather than of a pattern of its own — a test with a
/// regex would be free to disagree with both.
///
/// The boolean is `true` when the token spelled a POSIX default
/// (`${VAR:-fallback}`), `false` for the plain `${VAR}` that is a hard
/// requirement at substitution time. A name that appears BOTH ways collapses to
/// `false`: one plain occurrence is enough to make the key unavoidable.
///
/// `${ctx.<key>}` tokens are skipped (they are [`collect_ctx_keys`]'s), and so
/// are the `${uuid7:*}` labels and every unsupported operator form — those are
/// not environment at all, and mis-reporting one as a variable would put a
/// name into a declaration that nothing can ever satisfy.
pub fn collect_env_keys(value: &JsonValue) -> Result<BTreeMap<String, bool>, MutationError> {
    let found = std::cell::RefCell::new(BTreeMap::new());
    walk_env_keys(value, &found)?;
    Ok(found.into_inner())
}

fn walk_env_keys(
    v: &JsonValue,
    found: &std::cell::RefCell<BTreeMap<String, bool>>,
) -> Result<(), MutationError> {
    match v {
        JsonValue::String(s) => {
            expand_with(s, true, |inner| {
                if !inner.starts_with("ctx.") && !inner.starts_with("uuid7:") {
                    match parse_env_token(inner) {
                        EnvToken::Plain(name) => {
                            found.borrow_mut().insert(name, false);
                        }
                        EnvToken::DefaultIfUnsetOrEmpty { name, .. } => {
                            // A plain occurrence elsewhere wins: `or` never
                            // turns a hard requirement into a soft one.
                            found.borrow_mut().entry(name).or_insert(true);
                        }
                        EnvToken::Unsupported(_) => {}
                    }
                }
                Ok(String::new())
            })?;
            Ok(())
        }
        JsonValue::Array(a) => {
            for item in a {
                walk_env_keys(item, found)?;
            }
            Ok(())
        }
        JsonValue::Object(m) => {
            for val in m.values() {
                walk_env_keys(val, found)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Phase-6 T8: substitute both `${ENV_VAR}` and `${ctx.<key>}`. `${uuid7:*}` still
/// passes through unchanged (T9 caches it once per label).
pub fn substitute_env_and_ctx(
    diff: &JsonValue,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
) -> Result<JsonValue, MutationError> {
    fn walk(
        v: &JsonValue,
        env: &HashMap<String, String>,
        ctx: &HashMap<String, String>,
    ) -> Result<JsonValue, MutationError> {
        match v {
            JsonValue::String(s) => Ok(JsonValue::String(replace_env_ctx(s, env, ctx)?)),
            JsonValue::Array(a) => {
                let mut out = Vec::with_capacity(a.len());
                for item in a {
                    out.push(walk(item, env, ctx)?);
                }
                Ok(JsonValue::Array(out))
            }
            JsonValue::Object(m) => {
                let mut out = meclaw_core::serde_json::Map::new();
                for (k, val) in m {
                    out.insert(k.clone(), walk(val, env, ctx)?);
                }
                Ok(JsonValue::Object(out))
            }
            _ => Ok(v.clone()),
        }
    }
    walk(diff, env, ctx)
}

/// Resolve a `${ctx.<key>}` token against `ctx` (shared by T8/T9).
fn resolve_ctx_token(key: &str, ctx: &HashMap<String, String>) -> Result<String, MutationError> {
    ctx.get(key)
        .cloned()
        .ok_or_else(|| MutationError::CtxKeyMissing(key.to_owned()))
}

fn replace_env_ctx(
    s: &str,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
) -> Result<String, MutationError> {
    expand(s, |inner| {
        if let Some(key) = inner.strip_prefix("ctx.") {
            resolve_ctx_token(key, ctx)
        } else if inner.starts_with("uuid7:") {
            // T9 handles uuid7; pass-through preserves the placeholder.
            Ok(format!("${{{inner}}}"))
        } else {
            resolve_env_token(inner, env)
        }
    })
}

/// Phase-6 T9: full substitute — `${ENV_VAR}`, `${ctx.<key>}`, AND `${uuid7:label}`.
///
/// `${uuid7:label}` is cached per label: repeated `${uuid7:foo}` resolves to the
/// same UUID within one diff (spec § Variable substitution).
pub fn substitute_full(
    diff: &JsonValue,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
) -> Result<JsonValue, MutationError> {
    let mut uuid_cache: HashMap<String, String> = HashMap::new();
    walk_full(diff, env, ctx, &mut uuid_cache)
}

/// Full-substitution walk over one value with an EXTERNAL `${uuid7:*}` cache, so
/// several walks over disjoint parts of the same diff still share one label map.
fn walk_full(
    v: &JsonValue,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    cache: &mut HashMap<String, String>,
) -> Result<JsonValue, MutationError> {
    match v {
        JsonValue::String(s) => Ok(JsonValue::String(replace_full(s, env, ctx, cache)?)),
        JsonValue::Array(a) => {
            let mut out = Vec::with_capacity(a.len());
            for item in a {
                out.push(walk_full(item, env, ctx, cache)?);
            }
            Ok(JsonValue::Array(out))
        }
        JsonValue::Object(m) => {
            let mut out = meclaw_core::serde_json::Map::new();
            for (k, val) in m {
                out.insert(k.clone(), walk_full(val, env, ctx, cache)?);
            }
            Ok(JsonValue::Object(out))
        }
        _ => Ok(v.clone()),
    }
}

fn replace_full(
    s: &str,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    cache: &mut HashMap<String, String>,
) -> Result<String, MutationError> {
    // `expand` takes `impl Fn`; the uuid7 cache needs interior mutability so the
    // closure can mint-and-store a UUID per label. RefCell is single-threaded
    // (one cell task), no lock contention — CONTRIBUTING.md concurrency model holds.
    let cache = std::cell::RefCell::new(cache);
    expand(s, |inner| {
        if let Some(key) = inner.strip_prefix("ctx.") {
            resolve_ctx_token(key, ctx)
        } else if let Some(label) = inner.strip_prefix("uuid7:") {
            let val = cache
                .borrow_mut()
                .entry(label.to_owned())
                .or_insert_with(|| meclaw_core::Uuid::now_v7().to_string())
                .clone();
            Ok(val)
        } else {
            resolve_env_token(inner, env)
        }
    })
}

/// Resolve the INSTANCE class only -- `${ctx.<key>}` and `${uuid7:<label>}`.
///
/// Environment-class tokens (`${VAR}`, `${VAR:-default}`) survive LITERALLY, and
/// so does the `$${...}` escape (a later env pass owns it). This is the pass
/// whose result is written to an instance's `config.json`: whatever the
/// environment owns stays a token on disk, so a secret referenced by a template
/// is never materialized (GH #20).
///
/// Unsupported operator forms (`${VAR:=x}`, `${VAR:?m}`, …) are still rejected
/// here -- loudly and pre-destructively, exactly as on the full path -- because a
/// form that can never resolve is an authoring error, not a late binding.
///
/// `${uuid7:<label>}` is cached per label across the whole value, like
/// [`substitute_full`].
pub fn substitute_instance_only(
    value: &JsonValue,
    ctx: &HashMap<String, String>,
) -> Result<JsonValue, MutationError> {
    let mut uuid_cache: HashMap<String, String> = HashMap::new();
    walk_instance_only(value, ctx, &mut uuid_cache)
}

fn walk_instance_only(
    v: &JsonValue,
    ctx: &HashMap<String, String>,
    cache: &mut HashMap<String, String>,
) -> Result<JsonValue, MutationError> {
    match v {
        JsonValue::String(s) => Ok(JsonValue::String(replace_instance_only(s, ctx, cache)?)),
        JsonValue::Array(a) => {
            let mut out = Vec::with_capacity(a.len());
            for item in a {
                out.push(walk_instance_only(item, ctx, cache)?);
            }
            Ok(JsonValue::Array(out))
        }
        JsonValue::Object(m) => {
            let mut out = meclaw_core::serde_json::Map::new();
            for (k, val) in m {
                out.insert(k.clone(), walk_instance_only(val, ctx, cache)?);
            }
            Ok(JsonValue::Object(out))
        }
        _ => Ok(v.clone()),
    }
}

fn replace_instance_only(
    s: &str,
    ctx: &HashMap<String, String>,
    cache: &mut HashMap<String, String>,
) -> Result<String, MutationError> {
    // Same RefCell reason as `replace_full`: `expand_with` takes `impl Fn`, the
    // uuid7 label cache needs to mint-and-store. Single-threaded (one cell task).
    let cache = std::cell::RefCell::new(cache);
    expand_with(s, true, |inner| {
        if let Some(key) = inner.strip_prefix("ctx.") {
            resolve_ctx_token(key, ctx)
        } else if let Some(label) = inner.strip_prefix("uuid7:") {
            let val = cache
                .borrow_mut()
                .entry(label.to_owned())
                .or_insert_with(|| meclaw_core::Uuid::now_v7().to_string())
                .clone();
            Ok(val)
        } else {
            // Environment class: keep the token, reject what can never resolve.
            match parse_env_token(inner) {
                EnvToken::Unsupported(form) => Err(MutationError::UnsupportedSubstitution(form)),
                EnvToken::Plain(_) | EnvToken::DefaultIfUnsetOrEmpty { .. } => {
                    Ok(format!("${{{inner}}}"))
                }
            }
        }
    })
}

/// Substitute a mutation diff, splitting the placeholder classes by DESTINATION.
///
/// Every part of the diff is fully substituted ([`substitute_full`]) except the
/// two slots whose values are written verbatim into an instance's
/// `config.json` -- `add_nodes[].override_params` and `swap_nodes[].with.params`.
/// Those get the disk-facing pass ([`substitute_instance_only`]), so an
/// environment placeholder handed in by a mutation ends up on disk as a token
/// and not as a materialized secret (GH #20). The staging step re-applies the
/// env pass in memory, so the spawned cell still sees the resolved value.
///
/// The `${uuid7:<label>}` cache is shared across BOTH passes: one label yields
/// one UUID per mutation, wherever in the diff it appears.
pub fn substitute_mutation_diff(
    diff: &JsonValue,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
) -> Result<JsonValue, MutationError> {
    let mut cache: HashMap<String, String> = HashMap::new();
    let Some(top) = diff.as_object() else {
        // Not an object -- no node slots to protect; behave exactly as before.
        return walk_full(diff, env, ctx, &mut cache);
    };
    let mut out = meclaw_core::serde_json::Map::new();
    for (key, val) in top {
        let substituted = match (key.as_str(), val.as_array()) {
            ("add_nodes", Some(entries)) => {
                walk_entries(entries, env, ctx, &mut cache, &["override_params"])?
            }
            ("swap_nodes", Some(entries)) => walk_swaps(entries, env, ctx, &mut cache)?,
            _ => walk_full(val, env, ctx, &mut cache)?,
        };
        out.insert(key.clone(), substituted);
    }
    Ok(JsonValue::Object(out))
}

/// Substitute each entry of a node-list, routing the named keys through the
/// disk-facing pass and everything else through the full pass.
fn walk_entries(
    entries: &[JsonValue],
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    cache: &mut HashMap<String, String>,
    disk_keys: &[&str],
) -> Result<JsonValue, MutationError> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(obj) = entry.as_object() else {
            out.push(walk_full(entry, env, ctx, cache)?);
            continue;
        };
        let mut new_obj = meclaw_core::serde_json::Map::new();
        for (k, v) in obj {
            let sub = if disk_keys.contains(&k.as_str()) {
                walk_instance_only(v, ctx, cache)?
            } else {
                walk_full(v, env, ctx, cache)?
            };
            new_obj.insert(k.clone(), sub);
        }
        out.push(JsonValue::Object(new_obj));
    }
    Ok(JsonValue::Array(out))
}

/// `swap_nodes[]`: the config-destined slot sits one level deeper, in
/// `with.params` (the with-side's `override_params` equivalent, see
/// `mutation/stage.rs`).
fn walk_swaps(
    entries: &[JsonValue],
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    cache: &mut HashMap<String, String>,
) -> Result<JsonValue, MutationError> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(obj) = entry.as_object() else {
            out.push(walk_full(entry, env, ctx, cache)?);
            continue;
        };
        let mut new_obj = meclaw_core::serde_json::Map::new();
        for (k, v) in obj {
            let sub = match (k.as_str(), v.as_object()) {
                ("with", Some(with_obj)) => {
                    let mut new_with = meclaw_core::serde_json::Map::new();
                    for (wk, wv) in with_obj {
                        let ws = if wk == "params" {
                            walk_instance_only(wv, ctx, cache)?
                        } else {
                            walk_full(wv, env, ctx, cache)?
                        };
                        new_with.insert(wk.clone(), ws);
                    }
                    JsonValue::Object(new_with)
                }
                _ => walk_full(v, env, ctx, cache)?,
            };
            new_obj.insert(k.clone(), sub);
        }
        out.push(JsonValue::Object(new_obj));
    }
    Ok(JsonValue::Array(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn env_var_replaced_in_string_value() {
        let mut env = HashMap::new();
        env.insert("API_KEY".into(), "sk-xyz".into());
        let diff = json!({"add_nodes": [{"override_params": {"key": "${API_KEY}"}}]});
        let out = substitute_env_only(&diff, &env).unwrap();
        assert_eq!(out["add_nodes"][0]["override_params"]["key"], "sk-xyz");
    }

    #[test]
    fn missing_env_var_returns_error() {
        let env = HashMap::new();
        let diff = json!({"override_params": {"key": "${MISSING}"}});
        let err = substitute_env_only(&diff, &env).unwrap_err();
        assert_eq!(err.error_code(), "env_var_missing");
    }

    #[test]
    fn ctx_key_replaced_in_string_value() {
        let env = HashMap::new();
        let mut ctx = HashMap::new();
        ctx.insert("user_id".into(), "u-7".into());
        let diff = json!({"params": {"target": "${ctx.user_id}"}});
        let out = substitute_env_and_ctx(&diff, &env, &ctx).unwrap();
        assert_eq!(out["params"]["target"], "u-7");
    }

    #[test]
    fn missing_ctx_key_returns_error() {
        let env = HashMap::new();
        let ctx = HashMap::new();
        let diff = json!({"x": "${ctx.absent}"});
        let err = substitute_env_and_ctx(&diff, &env, &ctx).unwrap_err();
        assert_eq!(err.error_code(), "ctx_key_missing");
    }

    #[test]
    fn uuid7_label_same_label_same_uuid() {
        let env = HashMap::new();
        let ctx = HashMap::new();
        let diff = json!({"a": "${uuid7:foo}", "b": "${uuid7:foo}", "c": "${uuid7:bar}"});
        let out = substitute_full(&diff, &env, &ctx).unwrap();
        let a = out["a"].as_str().unwrap();
        let b = out["b"].as_str().unwrap();
        let c = out["c"].as_str().unwrap();
        assert_eq!(a, b, "same label resolves to same UUID");
        assert_ne!(a, c, "different labels differ");
        assert_eq!(a.len(), 36, "UUID string-form");
    }

    // --- T22: parse_env_token classification ---

    #[test]
    fn parse_env_token_plain_has_no_operator() {
        assert_eq!(parse_env_token("VAR"), EnvToken::Plain("VAR".into()));
    }

    #[test]
    fn parse_env_token_posix_default() {
        assert_eq!(
            parse_env_token("VAR:-fb"),
            EnvToken::DefaultIfUnsetOrEmpty {
                name: "VAR".into(),
                fallback: "fb".into()
            }
        );
    }

    #[test]
    fn parse_env_token_empty_fallback_is_default() {
        assert_eq!(
            parse_env_token("VAR:-"),
            EnvToken::DefaultIfUnsetOrEmpty {
                name: "VAR".into(),
                fallback: "".into()
            }
        );
    }

    #[test]
    fn parse_env_token_other_operators_unsupported() {
        for form in [
            "VAR:=x", "VAR-x", "VAR:+x", "VAR:?m", "VAR=x", "VAR+x", "VAR?x",
        ] {
            assert_eq!(
                parse_env_token(form),
                EnvToken::Unsupported(form.into()),
                "{form} must be Unsupported"
            );
        }
    }

    // --- T22: ${VAR:-fallback} POSIX semantics via replace_env ---

    #[test]
    fn default_fallback_when_unset() {
        let env = HashMap::new();
        assert_eq!(replace_env("${VAR:-fb}", &env).unwrap(), "fb");
    }

    #[test]
    fn default_fallback_when_empty() {
        let mut env = HashMap::new();
        env.insert("VAR".into(), "".into());
        assert_eq!(replace_env("${VAR:-fb}", &env).unwrap(), "fb");
    }

    #[test]
    fn default_uses_value_when_set_nonempty() {
        let mut env = HashMap::new();
        env.insert("VAR".into(), "real".into());
        assert_eq!(replace_env("${VAR:-fb}", &env).unwrap(), "real");
    }

    #[test]
    fn plain_set_returns_value() {
        let mut env = HashMap::new();
        env.insert("VAR".into(), "v".into());
        assert_eq!(replace_env("${VAR}", &env).unwrap(), "v");
    }

    #[test]
    fn plain_missing_is_env_var_missing() {
        let env = HashMap::new();
        let err = replace_env("${VAR}", &env).unwrap_err();
        assert_eq!(err.error_code(), "env_var_missing");
    }

    // --- T22: unsupported operator forms raise loud error ---

    #[test]
    fn unsupported_forms_raise_error() {
        let env = HashMap::new();
        for form in ["${VAR:=x}", "${VAR-x}", "${VAR:+x}", "${VAR:?m}"] {
            let err = replace_env(form, &env).unwrap_err();
            assert_eq!(
                err.error_code(),
                "unsupported_substitution",
                "{form} must be unsupported"
            );
        }
    }

    // --- T22: $${...} escape (generic over all token kinds) ---

    #[test]
    fn escape_env_token_is_literal() {
        let env = HashMap::new();
        assert_eq!(replace_env("$${VAR}", &env).unwrap(), "${VAR}");
    }

    #[test]
    fn escape_default_token_is_literal() {
        let env = HashMap::new();
        assert_eq!(replace_env("$${VAR:-x}", &env).unwrap(), "${VAR:-x}");
    }

    #[test]
    fn escape_mixed_with_real_substitution() {
        let mut env = HashMap::new();
        env.insert("REAL".into(), "value".into());
        assert_eq!(
            replace_env("$${KEEP}-${REAL}", &env).unwrap(),
            "${KEEP}-value"
        );
    }

    #[test]
    fn lone_dollar_is_literal() {
        let env = HashMap::new();
        assert_eq!(replace_env("$x", &env).unwrap(), "$x");
        assert_eq!(replace_env("a$", &env).unwrap(), "a$");
    }

    #[test]
    fn escape_ctx_token_is_literal_no_lookup() {
        let env = HashMap::new();
        let ctx = HashMap::new();
        // No CtxKeyMissing despite empty ctx — escape skips resolve entirely.
        assert_eq!(
            replace_env_ctx("$${ctx.user_id}", &env, &ctx).unwrap(),
            "${ctx.user_id}"
        );
    }

    #[test]
    fn escape_uuid7_token_is_literal() {
        let env = HashMap::new();
        let ctx = HashMap::new();
        assert_eq!(
            replace_env_ctx("$${uuid7:s}", &env, &ctx).unwrap(),
            "${uuid7:s}"
        );
    }

    #[test]
    fn unterminated_token_is_schema_error() {
        let env = HashMap::new();
        let err = replace_env("${VAR", &env).unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    // --- T23/T24: ctx + default coexist on the ctx/full paths ---

    #[test]
    fn env_ctx_path_supports_default_fallback() {
        let env = HashMap::new();
        let ctx = HashMap::new();
        assert_eq!(replace_env_ctx("${VAR:-fb}", &env, &ctx).unwrap(), "fb");
    }

    #[test]
    fn env_ctx_path_rejects_unsupported() {
        let env = HashMap::new();
        let ctx = HashMap::new();
        let err = replace_env_ctx("${VAR:=x}", &env, &ctx).unwrap_err();
        assert_eq!(err.error_code(), "unsupported_substitution");
    }

    #[test]
    fn uuid7_passthrough_unchanged_on_env_ctx_path() {
        let env = HashMap::new();
        let ctx = HashMap::new();
        assert_eq!(
            replace_env_ctx("${uuid7:s}", &env, &ctx).unwrap(),
            "${uuid7:s}"
        );
    }

    // --- GH #20: placeholder classes on the disk-facing pass ---

    #[test]
    fn instance_only_keeps_env_tokens_literal() {
        let ctx = HashMap::new();
        let v = json!({"params": {"key": "${API_KEY}", "url": "${BASE:-http://x}"}});
        let out = substitute_instance_only(&v, &ctx).unwrap();
        assert_eq!(out["params"]["key"], "${API_KEY}");
        assert_eq!(out["params"]["url"], "${BASE:-http://x}");
    }

    #[test]
    fn instance_only_never_touches_env_even_when_set() {
        // The env map is not even a parameter here -- the class decides, not the
        // presence of a value. Pinned via the boot pass on the SAME output.
        let ctx = HashMap::new();
        let mut env = HashMap::new();
        env.insert("API_KEY".into(), "sk-sentinel".into());
        let disk = substitute_instance_only(&json!({"k": "${API_KEY}"}), &ctx).unwrap();
        assert_eq!(disk["k"], "${API_KEY}", "disk view carries the token");
        let runtime = substitute_env_only(&disk, &env).unwrap();
        assert_eq!(runtime["k"], "sk-sentinel", "the late pass resolves it");
    }

    #[test]
    fn instance_only_resolves_ctx_and_uuid7() {
        let mut ctx = HashMap::new();
        ctx.insert("user_id".into(), "u-7".into());
        let v = json!({"a": "${ctx.user_id}", "b": "${uuid7:s}", "c": "${uuid7:s}"});
        let out = substitute_instance_only(&v, &ctx).unwrap();
        assert_eq!(out["a"], "u-7");
        let b = out["b"].as_str().unwrap();
        assert_eq!(b.len(), 36, "uuid7 resolves at instantiation");
        assert_eq!(b, out["c"].as_str().unwrap(), "same label, same UUID");
    }

    #[test]
    fn instance_only_missing_ctx_key_still_errors() {
        let err =
            substitute_instance_only(&json!({"x": "${ctx.absent}"}), &HashMap::new()).unwrap_err();
        assert_eq!(err.error_code(), "ctx_key_missing");
    }

    #[test]
    fn instance_only_rejects_unsupported_operator_forms() {
        for form in ["${VAR:=x}", "${VAR-x}", "${VAR:+x}", "${VAR:?m}"] {
            let err = substitute_instance_only(&json!({"x": form}), &HashMap::new()).unwrap_err();
            assert_eq!(
                err.error_code(),
                "unsupported_substitution",
                "{form} must be rejected pre-destructively"
            );
        }
    }

    #[test]
    fn instance_only_keeps_the_escape_for_the_late_pass() {
        // The disk pass must NOT eat the escape: the env pass owns it, and only
        // then does `$${V}` become the literal `${V}` the author asked for.
        let disk = substitute_instance_only(&json!({"x": "$${V}"}), &HashMap::new()).unwrap();
        assert_eq!(disk["x"], "$${V}", "escape survives the disk pass");
        let mut env = HashMap::new();
        env.insert("V".into(), "should-not-appear".into());
        let runtime = substitute_env_only(&disk, &env).unwrap();
        assert_eq!(runtime["x"], "${V}", "the late pass yields the literal");
    }

    #[test]
    fn instance_only_mixed_token_string() {
        let mut ctx = HashMap::new();
        ctx.insert("u".into(), "u-7".into());
        let out = substitute_instance_only(&json!({"x": "${ctx.u}/${TOKEN}"}), &ctx).unwrap();
        assert_eq!(out["x"], "u-7/${TOKEN}");
    }

    // --- GH #20: diff-level class split ---

    #[test]
    fn diff_keeps_override_params_env_literal_but_resolves_the_rest() {
        let mut env = HashMap::new();
        env.insert("NAME".into(), "worker".into());
        env.insert("API_KEY".into(), "sk-sentinel".into());
        let diff = json!({
            "add_nodes": [{
                "name": "n-${NAME}",
                "template": "llm@1.0.0",
                "override_params": {"api_key": "${API_KEY}"}
            }]
        });
        let out = substitute_mutation_diff(&diff, &env, &HashMap::new()).unwrap();
        assert_eq!(out["add_nodes"][0]["name"], "n-worker", "name resolves");
        assert_eq!(
            out["add_nodes"][0]["override_params"]["api_key"], "${API_KEY}",
            "a config-destined value keeps its token"
        );
    }

    #[test]
    fn diff_keeps_swap_with_params_env_literal() {
        let mut env = HashMap::new();
        env.insert("API_KEY".into(), "sk-sentinel".into());
        let diff = json!({
            "swap_nodes": [{
                "replace": "old",
                "with": {"name": "new", "template": "t", "params": {"api_key": "${API_KEY}"}}
            }]
        });
        let out = substitute_mutation_diff(&diff, &env, &HashMap::new()).unwrap();
        assert_eq!(
            out["swap_nodes"][0]["with"]["params"]["api_key"], "${API_KEY}",
            "the with-side params are config-destined too"
        );
        assert_eq!(out["swap_nodes"][0]["with"]["name"], "new");
    }

    #[test]
    fn diff_shares_one_uuid7_cache_across_both_passes() {
        let diff = json!({
            "add_nodes": [{
                "name": "n-${uuid7:s}",
                "override_params": {"session": "${uuid7:s}"}
            }]
        });
        let out = substitute_mutation_diff(&diff, &HashMap::new(), &HashMap::new()).unwrap();
        let name = out["add_nodes"][0]["name"].as_str().unwrap();
        let session = out["add_nodes"][0]["override_params"]["session"]
            .as_str()
            .unwrap();
        assert_eq!(
            name.trim_start_matches("n-"),
            session,
            "one label is one UUID across the class boundary"
        );
    }

    #[test]
    fn diff_resolves_ctx_in_override_params() {
        let mut ctx = HashMap::new();
        ctx.insert("user_id".into(), "u-7".into());
        let diff = json!({"add_nodes": [{"override_params": {"owner": "${ctx.user_id}"}}]});
        let out = substitute_mutation_diff(&diff, &HashMap::new(), &ctx).unwrap();
        assert_eq!(out["add_nodes"][0]["override_params"]["owner"], "u-7");
    }

    #[test]
    fn diff_non_object_falls_back_to_full_substitution() {
        let mut env = HashMap::new();
        env.insert("V".into(), "v".into());
        let out = substitute_mutation_diff(&json!(["${V}"]), &env, &HashMap::new()).unwrap();
        assert_eq!(out[0], "v");
    }

    // --- GH #292: which ctx keys a JSON value USES ---

    /// The exact shape of the brief: two keys in string values (one nested, one
    /// mixed with an env token), a KEY that merely looks like a token, and an
    /// escaped token that is a literal and therefore not a requirement.
    #[test]
    fn collect_ctx_keys_reads_string_values_only() {
        let v = json!({
            "a": "${ctx.model}",
            "b": {"c": "${ctx.model_fast}/${ENV}"},
            "ctx.not_a_token": 1,
            "d": "$${ctx.escaped}"
        });
        let keys = collect_ctx_keys(&v).unwrap();
        assert_eq!(
            keys,
            ["model", "model_fast"]
                .into_iter()
                .map(str::to_owned)
                .collect::<std::collections::BTreeSet<String>>()
        );
    }

    /// Arrays are walked, and one key used twice is one requirement.
    #[test]
    fn collect_ctx_keys_walks_arrays_and_deduplicates() {
        let v = json!({"xs": ["${ctx.model}", {"y": "${ctx.model}"}, "${uuid7:s}", 7]});
        let keys = collect_ctx_keys(&v).unwrap();
        assert_eq!(keys.len(), 1, "one key, however often it occurs: {keys:?}");
        assert!(keys.contains("model"));
    }

    /// The scanner is the substrate's own, so a token nothing can resolve is the
    /// same error here as on the resolving path — not a silently dropped key.
    #[test]
    fn collect_ctx_keys_reports_a_malformed_token_as_the_substrate_does() {
        let err = collect_ctx_keys(&json!({"x": "${ctx.model"})).unwrap_err();
        assert_eq!(err.error_code(), "schema");
    }

    #[test]
    fn multibyte_input_no_panic() {
        let mut env = HashMap::new();
        env.insert("V".into(), "wert".into());
        // Umlauts and emoji around the token exercise byte-correct slicing.
        assert_eq!(
            replace_env("über-${V}-😀-$${X}", &env).unwrap(),
            "über-wert-😀-${X}"
        );
    }
}

#[cfg(test)]
mod collect_env_keys_tests {
    use super::collect_env_keys;
    use meclaw_core::serde_json::json;

    /// GH #465 — the two token shapes, told apart by the boolean.
    #[test]
    fn a_plain_token_is_required_and_a_defaulted_one_is_not() {
        let v = json!({"a": "${HARD}", "b": "${SOFT:-x}"});
        let found = collect_env_keys(&v).unwrap();
        assert_eq!(found.get("HARD"), Some(&false));
        assert_eq!(found.get("SOFT"), Some(&true));
        assert_eq!(found.len(), 2);
    }

    /// One plain occurrence makes the key unavoidable, wherever the soft one
    /// stands — otherwise the walk order would decide a contract.
    #[test]
    fn a_key_written_both_ways_is_required() {
        for v in [json!(["${K:-d}", "${K}"]), json!(["${K}", "${K:-d}"])] {
            assert_eq!(collect_env_keys(&v).unwrap().get("K"), Some(&false));
        }
    }

    /// The instance class is not environment, and neither is an escape or an
    /// unsupported operator form: naming one in a `requires.env` block would ask
    /// an operator for a value nothing can ever bind.
    #[test]
    fn ctx_uuid_escapes_and_unsupported_forms_are_not_environment() {
        let v = json!({
            "a": "${ctx.model}",
            "b": "${uuid7:cell}",
            "c": "$${LITERAL}",
            "d": "${WEIRD:=x}",
            "e": "plain text, no dollar",
        });
        assert!(collect_env_keys(&v).unwrap().is_empty());
    }

    /// Object KEYS are names, never placeholders — only values substitute.
    #[test]
    fn object_keys_are_not_scanned() {
        let v = json!({"${NOT_A_TOKEN}": "literal"});
        assert!(collect_env_keys(&v).unwrap().is_empty());
    }
}
