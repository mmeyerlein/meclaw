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
//!
//! # The third destination: a template's own files (GH #611)
//!
//! `add_templates[].files` is neither class. A registration files a CLASS in
//! the library, and a class has no instance to bind to yet: its file bodies are
//! carried through **verbatim**, byte for byte, and every `${…}` in them binds
//! where it always binds -- the instance class at instantiation, the
//! environment class at read time (`docs/config.md` § Access). Substituting
//! them at registration was measured to write the colony's API key in clear
//! text into `{templates_root}/local/<name>/`, and to refuse a README that only
//! MENTIONS `${VAR}` in prose with `env_var_missing`.

use super::MutationError;
use meclaw_core::JsonValue;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The late-binding pass: environment tokens only, over a whole cell config
/// (boot, `bootstrap.rs`) or a bare `params` object.
///
/// GH #949 (review I-3): the CEL slots of `params.graph.edges` -- the edges a
/// hive's config draws, filled by a template, an `add_nodes` override or a
/// `swap_nodes`/`replace_nodes` lift -- are judged before anything binds,
/// exactly as the mutation door judges `add_edges` (`walk_edges`): a value a
/// CEL string cannot hold, or a bare token's value that is not a plain number
/// ([`env_fits`]), is refused as `env_value_unsafe`, and the boot fails like
/// `env_var_missing`.
/// Those edges reach this pass with their tokens intact (`params` are the
/// disk-facing slot, [`walk_instance_only`]), so this is the one place their
/// values meet their slots at read time.
pub fn substitute_env_only(
    diff: &JsonValue,
    env: &HashMap<String, String>,
) -> Result<JsonValue, MutationError> {
    if let Some(params) = diff.get("params") {
        check_graph_edges(params, env, "params")?;
    }
    check_graph_edges(diff, env, "")?;
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
    replace_bound(s, env, ctx, cache, None)
}

/// [`replace_full`], and with `cel_slot` set the string is CEL source: an
/// environment value that a CEL string literal cannot hold
/// ([`cel_string_safe`]) is refused as [`MutationError::EnvValueUnsafe`]
/// instead of bound (GH #949). `cel_slot` names the slot for the refusal.
fn replace_bound(
    s: &str,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    cache: &mut HashMap<String, String>,
    cel_slot: Option<&str>,
) -> Result<String, MutationError> {
    // GH #949 (review I-3): where each token stands -- inside a string literal
    // or not -- decides what its value may be (`env_fits`).
    // The table is read off the CEL SOURCE `s`, never off the slot name: built
    // from `cel_slot` it had no token at all, every lookup was `None`, and
    // `hop.n == ${N}` bound `N` = `s3cr3t || true` (measured red in the strand's
    // single-test run, `an_unquoted_env_token_binds_a_number_and_nothing_else`).
    let quoting = if cel_slot.is_some() {
        token_quoting(s)
    } else {
        Vec::new()
    };
    let nth = std::cell::Cell::new(0usize);
    // `expand` takes `impl Fn`; the uuid7 cache needs interior mutability so the
    // closure can mint-and-store a UUID per label. RefCell is single-threaded
    // (one cell task), no lock contention — CONTRIBUTING.md concurrency model holds.
    let cache = std::cell::RefCell::new(cache);
    expand(s, |inner| {
        let k = nth.get();
        nth.set(k + 1);
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
            let value = resolve_env_token(inner, env)?;
            match cel_slot {
                Some(slot) => {
                    env_fits(inner, &value, quoting.get(k).copied(), slot).map(|()| value)
                }
                None => Ok(value),
            }
        }
    })
}

/// GH #949 — whether `value` can stand inside a CEL string literal without
/// ending it: no single or double quote, no backslash (it would escape the closing
/// quote or start an escape of its own), no control character (Unicode `Cc`:
/// C0 U+0000–U+001F, DEL U+007F, C1 U+0080–U+009F — a raw line break ends a
/// quoted CEL string) and neither U+2028 nor U+2029. The same set the builder
/// recipe refuses in a literal binding (`_binding` / `_control`), so a value
/// is judged alike whether the wish spells it or the `.env` holds it.
///
/// Both quotes, although the recipe quotes a binding as `'<value>'` (and the
/// shipped `audience_set` modifier carries `"` inside `'…'` as plain text): the
/// door serves every author, does not parse which literal surrounds a token,
/// and CEL spells a string with either quote. No id a connector stamps carries
/// a `"`, so refusing it costs nothing that binds today.
fn cel_string_safe(value: &str) -> bool {
    !value
        .chars()
        .any(|c| c.is_control() || matches!(c, '\'' | '"' | '\\' | '\u{2028}' | '\u{2029}'))
}

/// The refusal of an environment value in a CEL slot: the variable and the
/// slot, NEVER the value. A value bound from `.env` may be a secret, and the
/// refusal travels into the mutation log, the EDA reply and the builder's
/// receipt (`details`).
fn env_value_unsafe(inner: &str, slot: &str) -> MutationError {
    let name = env_name(inner);
    MutationError::EnvValueUnsafe(format!(
        "the value bound for ${{{name}}} in {slot} carries a quote, a backslash or a control \
         character, which a CEL string cannot hold: it would break the expression or extend it \
         past the one value it names (the value is not shown)"
    ))
}

/// The variable an env token names -- its name, never its value.
fn env_name(inner: &str) -> String {
    match parse_env_token(inner) {
        EnvToken::Plain(name) | EnvToken::DefaultIfUnsetOrEmpty { name, .. } => name,
        // `resolve_env_token` refuses this form before a value exists. The
        // token's own text is a name either way, never a value.
        EnvToken::Unsupported(form) => form,
    }
}

/// Where an env token stands in CEL source ([`token_quoting`]).
#[derive(Clone, Copy, Debug)]
enum Quoting {
    /// Inside a plain `'…'` or `"…"` string literal.
    Inside,
    /// Outside every literal: the value is CEL source.
    Outside,
    /// After a raw (`r'…'`) or triple-quoted (`'''…'''`) literal, which this
    /// scanner does not lex (GH #949, review I-R2): refused whatever it holds.
    Unlexed,
}

/// GH #949 (review I-3) — for every regular `${…}` token of `s`, in the order
/// [`expand`] hands them to its resolver, whether it stands INSIDE a CEL string
/// literal (`'…'` or `"…"`, backslash escapes honoured). Same token grammar as
/// [`expand`]: an escaped `$${…}` is no token, and a `${` the scanner cannot
/// close ends the scan (`expand` names it).
///
/// GH #967.2 — outside a literal, `//` opens a CEL comment that runs to the
/// end of the line; a quote in it is text, not the start of a literal. Read
/// as code, `1 // it's` opened a literal at the apostrophe, the next line's
/// `${N}` counted as quoted, only [`cel_string_safe`] judged it, and
/// `1 || true` bound into an always-true condition. A token INSIDE a comment
/// is judged as quoted: a value without a control character (no newline)
/// cannot end the comment, so it stays text.
fn token_quoting(s: &str) -> Vec<Quoting> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut quote: Option<u8> = None;
    let mut unlexed = false;
    let mut comment = false;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'$' && b.get(i + 1) == Some(&b'$') && b.get(i + 2) == Some(&b'{') {
            let Some(end) = s[i + 3..].find('}') else {
                break;
            };
            i += 3 + end + 1;
            continue;
        }
        if c == b'$' && b.get(i + 1) == Some(&b'{') {
            let Some(end) = s[i + 2..].find('}') else {
                break;
            };
            out.push(match (unlexed, quote) {
                (true, _) => Quoting::Unlexed,
                (false, Some(_)) => Quoting::Inside,
                (false, None) if comment => Quoting::Inside,
                (false, None) => Quoting::Outside,
            });
            i += 2 + end + 1;
            continue;
        }
        if comment {
            comment = c != b'\n';
            i += 1;
            continue;
        }
        if quote.is_none() && c == b'/' && b.get(i + 1) == Some(&b'/') {
            comment = true;
            i += 2;
            continue;
        }
        match quote {
            Some(_) if c == b'\\' => i += 1,
            Some(q) if c == q => quote = None,
            None if c == b'\'' || c == b'"' => {
                // GH #949 (review I-R2): in `r'\'` the backslash is text, in
                // `'''it's'''` the inner quote is; read as plain literals both
                // put a later `${N}` "inside" and `N` = `1 || true` bound.
                let triple = b.get(i + 1) == Some(&c) && b.get(i + 2) == Some(&c);
                unlexed |= triple || (i > 0 && matches!(b[i - 1], b'r' | b'R'));
                quote = Some(c);
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// GH #949 — may the environment value of `inner` stand at its place in the
/// CEL slot `slot`? Inside a string literal it must not end the literal
/// ([`cel_string_safe`]). OUTSIDE one (review I-3) it is CEL source, not a
/// value: `hop.n == ${N}` with `N` = `1 || true` parses and is always true,
/// and carries nothing `cel_string_safe` would catch -- so there it must be a
/// plain number ([`cel_number_safe`]), the one shape a bare token is written
/// for (`templates/retry`: `int(context.attempt) < ${RETRY_MAX:-3}`).
/// `quoted` is `None` only for a token past an unclosed `${`, which `expand`
/// refuses anyway.
fn env_fits(
    inner: &str,
    value: &str,
    quoted: Option<Quoting>,
    slot: &str,
) -> Result<(), MutationError> {
    use Quoting::{Outside, Unlexed};
    match quoted {
        Some(Unlexed) => Err(MutationError::EnvValueUnsafe(format!(
            "${{{}}} in {slot} stands after a raw (r'...') or triple-quoted ('''...''') CEL \
             string, whose quoting this door does not read, so it cannot tell a value from CEL \
             source there -- put the token before that literal or write the literal plainly \
             (the value is not shown)",
            env_name(inner)
        ))),
        Some(Outside) if !cel_number_safe(value) => Err(MutationError::EnvValueUnsafe(format!(
            "${{{}}} in {slot} stands outside a CEL string literal, so the value bound there is \
             CEL source, and it is not a plain number: it would extend the expression past the \
             one value it names -- quote the token, or bind a number (the value is not shown)",
            env_name(inner)
        ))),
        Some(Outside) => Ok(()),
        _ if !cel_string_safe(value) => Err(env_value_unsafe(inner, slot)),
        _ => Ok(()),
    }
}

/// A CEL number literal and nothing else: an optional `-`, ASCII digits, and
/// at most one `.` between digits.
fn cel_number_safe(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    let mut parts = digits.split('.');
    let whole = parts.next().unwrap_or("");
    let fraction = parts.next();
    parts.next().is_none()
        && !whole.is_empty()
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
}

/// The CEL slots of one edge spec (`config::EdgeSpec`): `condition` and every
/// string value of `modifier.set_context` / `modifier.set_hop`, each with the
/// name a refusal gives it.
fn edge_cel_slots(edge: &JsonValue) -> Vec<(String, &str)> {
    let mut out = Vec::new();
    if let Some(c) = edge.get("condition").and_then(JsonValue::as_str) {
        out.push(("condition".to_string(), c));
    }
    if let Some(m) = edge.get("modifier").and_then(JsonValue::as_object) {
        for mk in ["set_context", "set_hop"] {
            if let Some(exprs) = m.get(mk).and_then(JsonValue::as_object) {
                for (key, expr) in exprs {
                    if let Some(src) = expr.as_str() {
                        out.push((format!("modifier.{mk}.{key}"), src));
                    }
                }
            }
        }
    }
    out
}

/// GH #949 (review I-3) — judge the CEL slots of `params.graph.edges` against
/// `env` without binding anything: a value that does not fit where its token
/// stands ([`env_fits`]) is `env_value_unsafe` (slot `<at>.graph.edges[i].
/// <slot>`). A variable that is missing or a form that is unsupported is NOT
/// judged here -- the binding pass names it (`env_var_missing`,
/// `unsupported_substitution`), at the door or at boot, as before. Used by both
/// paths: the mutation door over a node's `override_params` / `with.params`
/// (whose tokens stay tokens on disk) and the boot pass over a whole config.
fn check_graph_edges(
    params: &JsonValue,
    env: &HashMap<String, String>,
    at: &str,
) -> Result<(), MutationError> {
    let Some(edges) = params
        .get("graph")
        .and_then(|g| g.get("edges"))
        .and_then(JsonValue::as_array)
    else {
        return Ok(());
    };
    let at = if at.is_empty() {
        String::new()
    } else {
        format!("{at}.")
    };
    for (i, edge) in edges.iter().enumerate() {
        // GH #972 (R-NL-4) -- a config's own edge never sets a stamped key
        // (`STAMPED_CONTEXT_KEYS`): only the wiring may, so an app cannot
        // claim the round of a turn it was not raised in.
        if let Some(set) = edge
            .get("modifier")
            .and_then(|m| m.get("set_context"))
            .and_then(JsonValue::as_object)
        {
            for key in crate::cel_eval::STAMPED_CONTEXT_KEYS {
                // GH #979 (OR-NL-179): the literal restore of the same key
                // from a parked hop hands back what the turn arrived with
                // and claims nothing (`cel_eval::is_stamped_restore`).
                let Some(value) = set.get(key) else {
                    continue;
                };
                if value
                    .as_str()
                    .is_some_and(|expr| crate::cel_eval::is_stamped_restore(key, expr))
                {
                    continue;
                }
                return Err(MutationError::EdgeSchema(format!(
                    "{at}graph.edges[{i}].modifier.set_context.{key}: `{key}` is stamped by \
                     the wiring (an `add_edges` entry) and never by an edge a config draws \
                     for itself -- an app could otherwise claim a turn it was not raised in; \
                     the one form a config edge may write is the restore \
                     `has(hop.ctx_{key}) ? hop.ctx_{key} : ''`"
                )));
            }
        }
        for (slot, src) in edge_cel_slots(edge) {
            let slot = format!("{at}graph.edges[{i}].{slot}");
            let quoting = token_quoting(src);
            let nth = std::cell::Cell::new(0usize);
            expand(src, |inner| {
                let k = nth.get();
                nth.set(k + 1);
                if inner.starts_with("ctx.") || inner.starts_with("uuid7:") {
                    return Ok(String::new());
                }
                match resolve_env_token(inner, env) {
                    Ok(value) => {
                        env_fits(inner, &value, quoting.get(k).copied(), &slot)?;
                        Ok(String::new())
                    }
                    Err(_) => Ok(String::new()),
                }
            })?;
        }
    }
    Ok(())
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
/// three slots whose values are written verbatim into an instance's
/// `config.json` -- `add_nodes[].override_params`, `swap_nodes[].with.params`
/// and `replace_nodes[].with.params` (GH #796; the three ops that instantiate,
/// and the three that reach `patch_and_substitute_config` with an override
/// set). Those get the disk-facing pass ([`substitute_instance_only`]), so an
/// environment placeholder handed in by a mutation ends up on disk as a token
/// and not as a materialized secret (GH #20). The staging step re-applies the
/// env pass in memory, so the spawned cell still sees the resolved value.
///
/// `add_templates[].files` is exempt from BOTH passes (GH #611): the bodies of
/// a registered template travel untouched, because a class is not an instance
/// and nothing of the mutation that happened to deliver it belongs in it. Its
/// `${…}` bind where they always bind -- the instance class at instantiation,
/// the environment class at read time -- exactly as in a shipped template.
///
/// The `${uuid7:<label>}` cache is shared across BOTH passes: one label yields
/// one UUID per mutation, wherever in the diff it appears.
///
/// `add_edges[]` takes the full pass, and in its CEL slots (`condition`, every
/// value of `modifier.set_context` and `modifier.set_hop`) an environment value
/// that a CEL string cannot hold is refused with `env_value_unsafe` and the
/// whole mutation with it (GH #949, see `walk_edges`).
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
                // GH #949 (review I-3): the edges a node's params draw are
                // judged like `add_edges`, though their tokens stay on disk.
                for (i, entry) in entries.iter().enumerate() {
                    if let Some(params) = entry.get("override_params") {
                        let at = format!("add_nodes[{i}].override_params");
                        check_graph_edges(params, env, &at)?;
                    }
                }
                walk_entries(entries, env, ctx, &mut cache, &["override_params"], &[])?
            }
            // GH #611: `files` carries the template's own bytes, not a value of
            // this mutation. It is the one slot that is copied and never read.
            ("add_templates", Some(entries)) => {
                walk_entries(entries, env, ctx, &mut cache, &[], &["files"])?
            }
            ("swap_nodes", Some(entries)) => {
                walk_with_params("swap_nodes", entries, env, ctx, &mut cache)?
            }
            // GH #796: a lift's `with.params` reach `config.json` through the
            // very same `override_params` contract (`stage_replace.rs` ->
            // `override_entry`), so the slot is the swap's slot and the arm is
            // the swap's arm. Without it the key fell through `_ => walk_full`.
            ("replace_nodes", Some(entries)) => {
                walk_with_params("replace_nodes", entries, env, ctx, &mut cache)?
            }
            // GH #949: an edge's CEL slots take the full pass WITH the guard
            // -- an environment value must not close a string literal there.
            ("add_edges", Some(entries)) => walk_edges(entries, env, ctx, &mut cache)?,
            _ => walk_full(val, env, ctx, &mut cache)?,
        };
        out.insert(key.clone(), substituted);
    }
    Ok(JsonValue::Object(out))
}

/// Substitute each entry of a list-shaped operation, routing three key sets
/// apart: `disk_keys` through the disk-facing pass ([`walk_instance_only`]),
/// `verbatim_keys` through no pass at all, everything else through the full one.
///
/// The verbatim set exists for `add_templates[].files` (GH #611) and is a
/// stronger statement than the disk-facing pass: not "resolve one class and
/// keep the other" but "these are somebody else's bytes". A body that is not
/// even parsed cannot leak a value into a file and cannot be refused for a
/// placeholder it only mentions.
fn walk_entries(
    entries: &[JsonValue],
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    cache: &mut HashMap<String, String>,
    disk_keys: &[&str],
    verbatim_keys: &[&str],
) -> Result<JsonValue, MutationError> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(obj) = entry.as_object() else {
            out.push(walk_full(entry, env, ctx, cache)?);
            continue;
        };
        let mut new_obj = meclaw_core::serde_json::Map::new();
        for (k, v) in obj {
            let sub = if verbatim_keys.contains(&k.as_str()) {
                v.clone()
            } else if disk_keys.contains(&k.as_str()) {
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

/// The two operations whose config-destined slot sits one level deeper, in
/// `with.params`: `swap_nodes[]` (the with-side's `override_params`
/// equivalent, see `mutation/stage.rs`) and `replace_nodes[]` (a lift's
/// params, mapped onto the same contract in `mutation/stage_replace.rs`).
///
/// One walk for both because it is one slot: both ops hand `with.params`
/// verbatim to `patch_and_substitute_config`, which merges them into the
/// instance's `config.json`. GH #796 measured what a second, missing arm
/// costs -- a lift resolved the token and wrote the value out.
///
/// GH #949 (review I-3): the edges `with.params` draw (`graph.edges`) are judged
/// like `add_edges` ([`check_graph_edges`]) before anything is staged; their
/// tokens stay tokens on disk and bind at read time, where the boot pass judges
/// them again.
fn walk_with_params(
    op: &str,
    entries: &[JsonValue],
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    cache: &mut HashMap<String, String>,
) -> Result<JsonValue, MutationError> {
    let mut out = Vec::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
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
                            check_graph_edges(wv, env, &format!("{op}[{i}].with.params"))?;
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

/// GH #949 — `add_edges[]`: the full pass, and in the three CEL slots of an
/// entry (`condition`, every value of `modifier.set_context` and of
/// `modifier.set_hop`) an environment value that a CEL string cannot hold is
/// refused ([`MutationError::EnvValueUnsafe`]) instead of bound.
///
/// Why here: the builder recipe renders a channel binding as
/// `string(hop.chat_id) == '${NAME}'` and only ever sees the token; the colony
/// reads its `.env` into a map and binds the value in this pass. A value like
/// `1' || true || '1` closes the string and PARSES, so the CEL check at
/// validation sees a well-formed condition that takes every chat (and, in the
/// speaker modifier, names every sender the member). This is the one place the
/// value and the slot meet.
///
/// Everything else in an entry (`from`, `to`, `lane`, `delete_context`,
/// `delete_hop`) is a name and not CEL, and takes the plain full pass. So does
/// all of `remove_edges[]`: its `match` names a standing edge by identity and
/// draws nothing, so an edge bound before this guard stays removable.
fn walk_edges(
    entries: &[JsonValue],
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    cache: &mut HashMap<String, String>,
) -> Result<JsonValue, MutationError> {
    let mut out = Vec::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        let Some(obj) = entry.as_object() else {
            out.push(walk_full(entry, env, ctx, cache)?);
            continue;
        };
        let mut new_obj = meclaw_core::serde_json::Map::new();
        for (k, v) in obj {
            let sub = match (k.as_str(), v) {
                ("condition", JsonValue::String(s)) => {
                    let slot = format!("add_edges[{i}].condition");
                    JsonValue::String(replace_bound(s, env, ctx, cache, Some(slot.as_str()))?)
                }
                ("modifier", JsonValue::Object(m)) => walk_modifier(m, i, env, ctx, cache)?,
                _ => walk_full(v, env, ctx, cache)?,
            };
            new_obj.insert(k.clone(), sub);
        }
        out.push(JsonValue::Object(new_obj));
    }
    Ok(JsonValue::Array(out))
}

/// The modifier of `add_edges[i]`: `set_context` and `set_hop` map a key to a
/// CEL expression (`config::ModifierSpec`), so each string value is a CEL slot;
/// the rest of the modifier (`delete_*` lists of key names, `restore_ttl`)
/// takes the plain full pass.
fn walk_modifier(
    modifier: &meclaw_core::serde_json::Map<String, JsonValue>,
    i: usize,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    cache: &mut HashMap<String, String>,
) -> Result<JsonValue, MutationError> {
    let mut out = meclaw_core::serde_json::Map::new();
    for (mk, mv) in modifier {
        let sub = match (mk.as_str(), mv) {
            ("set_context" | "set_hop", JsonValue::Object(exprs)) => {
                let mut bound = meclaw_core::serde_json::Map::new();
                for (key, expr) in exprs {
                    let value = match expr {
                        JsonValue::String(s) => {
                            let slot = format!("add_edges[{i}].modifier.{mk}.{key}");
                            JsonValue::String(replace_bound(
                                s,
                                env,
                                ctx,
                                cache,
                                Some(slot.as_str()),
                            )?)
                        }
                        _ => walk_full(expr, env, ctx, cache)?,
                    };
                    bound.insert(key.clone(), value);
                }
                JsonValue::Object(bound)
            }
            _ => walk_full(mv, env, ctx, cache)?,
        };
        out.insert(mk.clone(), sub);
    }
    Ok(JsonValue::Object(out))
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

    /// GH #796 (R-L6): `replace_nodes[].with.params` is a config-destined slot
    /// like the other two. A lift hands them to `patch_and_substitute_config`
    /// through the very same `override_params` contract
    /// (`mutation/stage_replace.rs` → `override_entry`), so they are merged
    /// into the instance's `config.json` on disk — and before this arm existed
    /// they fell through `_ => walk_full` and were merged RESOLVED. Measured on
    /// a throwaway stage on 2026-09-21: a manifest carrying the placeholder
    /// twice left a 164-character literal in `params.duplex.api_key` of two
    /// cells, while the cells of a colony instantiated from the template kept
    /// the 19-character token.
    #[test]
    fn diff_keeps_replace_with_params_env_literal() {
        let mut env = HashMap::new();
        env.insert("SOME_KEY".into(), "sk-sentinel".into());
        let diff = json!({
            "replace_nodes": [{
                "match": {"name": "voice"},
                "with": {
                    "template": "voice@2.1.0",
                    "params": {"duplex": {"api_key": "${SOME_KEY}"}}
                }
            }]
        });
        let out = substitute_mutation_diff(&diff, &env, &HashMap::new()).unwrap();
        assert_eq!(
            out["replace_nodes"][0]["with"]["params"]["duplex"]["api_key"], "${SOME_KEY}",
            "a lift's params are config-destined -- the token stays a token"
        );
        assert_eq!(
            out["replace_nodes"][0]["with"]["template"], "voice@2.1.0",
            "everything outside the config slot still resolves"
        );
        // The counter-proof: the runtime view binds what the disk view withheld
        // -- the same late binding `stage.rs` applies over the staged config.
        let runtime = substitute_env_only(&out, &env).unwrap();
        assert_eq!(
            runtime["replace_nodes"][0]["with"]["params"]["duplex"]["api_key"], "sk-sentinel",
            "the lifted cell still spawns with the resolved value"
        );
    }

    /// The instance class behaves in a lift's params as it does in the other
    /// two slots: `${ctx.*}` IS the instance and resolves onto the disk view.
    #[test]
    fn diff_resolves_ctx_in_replace_with_params() {
        let mut ctx = HashMap::new();
        ctx.insert("user_id".into(), "u-7".into());
        let diff = json!({
            "replace_nodes": [{
                "match": {"name": "voice"},
                "with": {"template": "voice@2.1.0", "params": {"owner": "${ctx.user_id}"}}
            }]
        });
        let out = substitute_mutation_diff(&diff, &HashMap::new(), &ctx).unwrap();
        assert_eq!(out["replace_nodes"][0]["with"]["params"]["owner"], "u-7");
    }

    /// GH #796 (review, 2026-09-21): the diff vocabulary and the arms of
    /// [`substitute_mutation_diff`] are walked TOGETHER or they drift apart.
    ///
    /// `validate::DIFF_OPERATIONS` says "adding an operation means adding its
    /// key here" and nothing said what the door's substitution pass then owes
    /// that key. #796 is what the silence costs: `replace_nodes` was added as
    /// the ninth operation, `substitute_mutation_diff` had no arm for it, and
    /// its params fell through `_ => walk_full` — resolved onto disk. A tenth
    /// operation with a config-destined slot would fall exactly the same way,
    /// in silence.
    ///
    /// So every operation states its side here: either it carries a slot whose
    /// value is written verbatim into an instance's `config.json` — then the
    /// environment token survives the pass — or it carries none, and then a
    /// token in it resolves like everything else. A new key in
    /// `DIFF_OPERATIONS` without a line here fails on the set comparison
    /// BEFORE anybody has to notice the difference in production.
    #[test]
    fn every_diff_operation_states_its_config_destined_slot() {
        use crate::mutation::validate::DIFF_OPERATIONS;
        use std::collections::BTreeSet;

        const TOKEN: &str = "${LATE_BOUND}";
        const VALUE: &str = "the-environment-owns-this";
        let mut env = HashMap::new();
        env.insert("LATE_BOUND".to_string(), VALUE.to_string());

        // (operation, a diff carrying the token where that operation's
        // config-destined slot is — or anywhere at all, for one that has none,
        // JSON pointer to it, what the pass must leave there).
        let cases: Vec<(&str, JsonValue, &str, &str)> = vec![
            // Config-destined: the token stays a token (GH #20, GH #796).
            (
                "add_nodes",
                json!({"add_nodes": [{"name": "a", "template": "t@1.0.0",
                                      "override_params": {"k": TOKEN}}]}),
                "/add_nodes/0/override_params/k",
                TOKEN,
            ),
            (
                "swap_nodes",
                json!({"swap_nodes": [{"match": {"name": "a"},
                                       "with": {"template": "t@1.0.0", "params": {"k": TOKEN}}}]}),
                "/swap_nodes/0/with/params/k",
                TOKEN,
            ),
            (
                "replace_nodes",
                json!({"replace_nodes": [{"match": {"name": "a"},
                                          "with": {"template": "t@1.0.0", "params": {"k": TOKEN}}}]}),
                "/replace_nodes/0/with/params/k",
                TOKEN,
            ),
            // Somebody else's bytes: no pass at all (GH #611), so the token
            // stands here too — for the other reason.
            (
                "add_templates",
                json!({"add_templates": [{"name": "t", "files": {"config.json": TOKEN}}]}),
                "/add_templates/0/files/config.json",
                TOKEN,
            ),
            // No config-destined slot: fully substituted, as every part of a
            // diff was before the classes were split.
            (
                "remove_nodes",
                json!({"remove_nodes": [{"name": TOKEN}]}),
                "/remove_nodes/0/name",
                VALUE,
            ),
            (
                "move_nodes",
                json!({"move_nodes": [{"name": TOKEN, "to": "/b"}]}),
                "/move_nodes/0/name",
                VALUE,
            ),
            (
                "add_edges",
                json!({"add_edges": [{"from": TOKEN, "to": "/b"}]}),
                "/add_edges/0/from",
                VALUE,
            ),
            (
                "remove_edges",
                json!({"remove_edges": [{"from": TOKEN, "to": "/b"}]}),
                "/remove_edges/0/from",
                VALUE,
            ),
            (
                "seed_rows",
                json!({"seed_rows": [{"path": "/a", "rows": [{"v": TOKEN}]}]}),
                "/seed_rows/0/rows/0/v",
                VALUE,
            ),
        ];

        let stated: BTreeSet<&str> = cases.iter().map(|(op, ..)| *op).collect();
        let declared: BTreeSet<&str> = DIFF_OPERATIONS.iter().copied().collect();
        assert_eq!(
            stated, declared,
            "every operation of the diff vocabulary states here whether its values \
             reach a config.json -- a new key needs its line in this table"
        );

        for (op, diff, pointer, expected) in &cases {
            let out = substitute_mutation_diff(diff, &env, &HashMap::new())
                .unwrap_or_else(|e| panic!("{op}: the door's pass refused the diff: {e:?}"));
            assert_eq!(
                out.pointer(pointer).and_then(|v| v.as_str()),
                Some(*expected),
                "{op}{pointer}: the substitution pass wrote the wrong class out"
            );
        }
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

    // --- GH #611: a registered template's files are nobody's to substitute ---

    #[test]
    fn diff_keeps_template_file_bodies_verbatim() {
        let mut env = HashMap::new();
        env.insert("API_KEY".into(), "sk-sentinel".into());
        let mut ctx = HashMap::new();
        ctx.insert("user_id".into(), "u-7".into());
        let diff = json!({
            "add_templates": [{
                "name": "note-${ctx.user_id}",
                "files": {
                    "template.json": r#"{"name":"note","version":"1.0.0"}"#,
                    "config.json": r#"{"params":{"api_key":"${API_KEY}","owner":"${ctx.user_id}","s":"${uuid7:x}"}}"#,
                }
            }]
        });
        let out = substitute_mutation_diff(&diff, &env, &ctx).unwrap();
        let files = &out["add_templates"][0]["files"];
        assert_eq!(
            files["config.json"],
            r#"{"params":{"api_key":"${API_KEY}","owner":"${ctx.user_id}","s":"${uuid7:x}"}}"#,
            "a template's bytes reach the library exactly as they were declared"
        );
        assert_eq!(
            out["add_templates"][0]["name"], "note-u-7",
            "the entry's own fields still resolve -- only `files` is exempt"
        );
    }

    #[test]
    fn a_template_file_that_only_mentions_a_placeholder_is_not_an_error() {
        // The README case: `${OPENAI_API_KEY}` stands in PROSE, and no colony
        // that registers the class has to own the variable to do so.
        let diff = json!({
            "add_templates": [{
                "name": "note",
                "files": {
                    "template.json": r#"{"name":"note"}"#,
                    "README.md": "Set `${OPENAI_API_KEY}` in your `.env` before instantiating.",
                }
            }]
        });
        let out = substitute_mutation_diff(&diff, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(
            out["add_templates"][0]["files"]["README.md"],
            "Set `${OPENAI_API_KEY}` in your `.env` before instantiating.",
        );
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

    // --- GH #949: an environment value never reopens a CEL string ---

    /// One value per class a CEL string literal cannot hold. Each carries the
    /// marker `s3cr3t`, so a test can tell that the value never reaches the
    /// refusal, and the class sits in the interior: the door does not trim.
    const UNSAFE: [(&str, &str); 16] = [
        ("the reopened string", "s3cr3t' || true || '1"),
        ("a single quote", "s3cr3t'1"),
        ("a double quote", "s3cr3t\"1"),
        ("a backslash", "s3cr3t\\1"),
        ("a line feed", "s3cr3t\n1"),
        ("a carriage return", "s3cr3t\r1"),
        ("a tab", "s3cr3t\t1"),
        ("NUL", "s3cr3t\u{0}1"),
        ("ESC", "s3cr3t\u{1b}1"),
        ("the last C0 control", "s3cr3t\u{1f}1"),
        ("DEL", "s3cr3t\u{7f}1"),
        ("the first C1 control", "s3cr3t\u{80}1"),
        ("NEL", "s3cr3t\u{85}1"),
        ("the last C1 control", "s3cr3t\u{9f}1"),
        ("the line separator", "s3cr3t\u{2028}1"),
        ("the paragraph separator", "s3cr3t\u{2029}1"),
    ];

    /// The three CEL slots of an `add_edges` entry, as the refusal names them
    /// after `add_edges[<i>].`.
    const CEL_SLOTS: [&str; 3] = [
        "condition",
        "modifier.set_context.speaker",
        "modifier.set_hop.route",
    ];

    fn env_of(name: &str, value: &str) -> HashMap<String, String> {
        HashMap::from([(name.to_string(), value.to_string())])
    }

    /// One channel ingress edge with `${CHAT}` quoted the way the builder
    /// recipe quotes a binding, in exactly ONE of its three CEL slots, so a
    /// refusal can only have come from that slot.
    fn edge_with_token_in(slot: &str) -> JsonValue {
        let quoted = "string(hop.chat_id) == '${CHAT}'";
        let mut edge = json!({
            "from": "./telegram", "to": ".",
            "condition": "!has(hop.error_code)",
            "modifier": {"set_context": {"speaker": "''"}, "set_hop": {"route": "'turn'"}}
        });
        match slot {
            "condition" => edge["condition"] = json!(quoted),
            "modifier.set_context.speaker" => {
                edge["modifier"]["set_context"]["speaker"] =
                    json!(format!("{quoted} ? 'member:alex' : ''"));
            }
            "modifier.set_hop.route" => {
                edge["modifier"]["set_hop"]["route"] =
                    json!(format!("{quoted} ? 'turn' : 'stray'"));
            }
            other => panic!("no such slot: {other}"),
        }
        json!({"add_edges": [edge]})
    }

    /// Red before GH #949: the pass bound every one of these values and
    /// answered `Ok` — `1' || true || '1` came out as a condition that parses
    /// and takes every chat. Now each is refused under its own code, naming
    /// the variable and the slot and never the value.
    #[test]
    fn an_env_value_a_cel_string_cannot_hold_is_refused_in_every_cel_slot() {
        for slot in CEL_SLOTS {
            for (what, value) in UNSAFE {
                let err = match substitute_mutation_diff(
                    &edge_with_token_in(slot),
                    &env_of("CHAT", value),
                    &HashMap::new(),
                ) {
                    Ok(out) => panic!("{slot}: {what} was bound into CEL: {out}"),
                    Err(e) => e,
                };
                assert_eq!(err.error_code(), "env_value_unsafe", "{slot}: {what}");
                let said = err.message();
                assert!(
                    said.contains("${CHAT}") && said.contains(&format!("add_edges[0].{slot}")),
                    "{slot}: {what}: the refusal names the variable and the slot: {said}"
                );
                assert!(
                    !said.contains("s3cr3t") && !format!("{err:?}").contains("s3cr3t"),
                    "{slot}: {what}: the refusal carries the value"
                );
            }
        }
    }

    /// The ids the connectors stamp bind exactly as before, in every slot: a
    /// Telegram chat (negative for a group), a Slack channel and a thread of it,
    /// a non-ASCII letter, a space.
    #[test]
    fn a_plain_id_from_the_environment_binds_in_every_cel_slot() {
        for slot in CEL_SLOTS {
            for value in ["111", "-1001234", "C1", "C1:1700.1", "chat-ä", "a b"] {
                let out = substitute_mutation_diff(
                    &edge_with_token_in(slot),
                    &env_of("CHAT", value),
                    &HashMap::new(),
                )
                .unwrap_or_else(|e| panic!("{slot}: {value:?} was refused: {e:?}"));
                let pointer = format!("/add_edges/0/{}", slot.replace('.', "/"));
                let bound = out
                    .pointer(&pointer)
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default();
                assert!(
                    bound.contains(&format!("string(hop.chat_id) == '{value}'")),
                    "{slot}: {bound}"
                );
            }
        }
    }

    /// The guard is the CEL slots' and nobody else's. The same value is still
    /// substituted, or kept a token, everywhere else: a secret in a node's
    /// params may carry any character (it stays `${CHAT}` on disk and binds at
    /// read time), an endpoint or a `delete_*` list is a name, a seed row is
    /// data, and a `remove_edges` pattern names a standing edge by identity —
    /// an edge bound before GH #949 stays removable.
    #[test]
    fn the_same_value_outside_a_cel_slot_is_untouched() {
        let value = "s3cr3t' || true || '1\\\"";
        let diff = json!({
            "add_nodes": [{"name": "n", "template": "t@1.0.0",
                           "override_params": {"api_key": "${CHAT}"}}],
            "swap_nodes": [{"match": {"name": "a"},
                            "with": {"template": "t@1.0.0", "params": {"api_key": "${CHAT}"}}}],
            "replace_nodes": [{"match": {"name": "b"},
                               "with": {"template": "t@1.0.0", "params": {"api_key": "${CHAT}"}}}],
            "add_edges": [{"from": "${CHAT}", "to": "./b",
                           "modifier": {"delete_context": ["${CHAT}"], "delete_hop": ["${CHAT}"]}}],
            "remove_edges": [{"match": {"from": "./a", "to": "./b",
                                        "condition": "hop.chat_id == '${CHAT}'"}}],
            "seed_rows": [{"path": "/s", "rows": [{"v": "${CHAT}"}]}]
        });
        let out = substitute_mutation_diff(&diff, &env_of("CHAT", value), &HashMap::new())
            .unwrap_or_else(|e| panic!("a value outside a CEL slot was refused: {e:?}"));
        assert_eq!(out["add_nodes"][0]["override_params"]["api_key"], "${CHAT}");
        assert_eq!(out["swap_nodes"][0]["with"]["params"]["api_key"], "${CHAT}");
        assert_eq!(
            out["replace_nodes"][0]["with"]["params"]["api_key"],
            "${CHAT}"
        );
        assert_eq!(out["add_edges"][0]["from"], value);
        assert_eq!(out["add_edges"][0]["modifier"]["delete_context"][0], value);
        assert_eq!(out["add_edges"][0]["modifier"]["delete_hop"][0], value);
        assert_eq!(
            out["remove_edges"][0]["match"]["condition"],
            format!("hop.chat_id == '{value}'")
        );
        assert_eq!(out["seed_rows"][0]["rows"][0]["v"], value);
    }

    /// A POSIX default is the same binding: the default that stands in for an
    /// unset variable binds like a value, and a set value is judged although a
    /// harmless default stands beside it — under the variable's name.
    #[test]
    fn a_defaulted_token_is_judged_by_what_it_binds() {
        let diff = json!({"add_edges": [{"from": "a", "to": "b",
                                         "condition": "hop.chat_id == '${CHAT:-111}'"}]});
        let out = substitute_mutation_diff(&diff, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(out["add_edges"][0]["condition"], "hop.chat_id == '111'");
        let err = substitute_mutation_diff(
            &diff,
            &env_of("CHAT", "s3cr3t' || true || '1"),
            &HashMap::new(),
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "env_value_unsafe");
        assert!(err.message().contains("${CHAT}"), "{err:?}");
    }

    /// An escaped token binds nothing, so there is nothing to judge: `$${CHAT}`
    /// stays the literal text `${CHAT}` in a condition, whatever the variable
    /// holds.
    #[test]
    fn an_escaped_token_in_a_condition_binds_nothing() {
        let diff = json!({"add_edges": [{"from": "a", "to": "b",
                                         "condition": "hop.x == '$${CHAT}'"}]});
        let out =
            substitute_mutation_diff(&diff, &env_of("CHAT", "s3cr3t'"), &HashMap::new()).unwrap();
        assert_eq!(out["add_edges"][0]["condition"], "hop.x == '${CHAT}'");
    }

    // --- GH #949, review I-3: unquoted tokens, and the edges params draw ---

    /// Red before the fix: `1 || true` carries no quote, no backslash and no
    /// control character, so `cel_string_safe` passed it, and `hop.n == ${N}`
    /// bound to a condition that is always true. A token outside a string
    /// literal is CEL source: in every CEL slot it binds a plain number (the
    /// shape `templates/retry` writes, `< ${RETRY_MAX:-3}`) and nothing else;
    /// inside `'…'` or `"…"` it binds what a string can hold; an escaped token
    /// and the instance class are not judged.
    #[test]
    fn an_unquoted_env_token_binds_a_number_and_nothing_else() {
        for value in ["7", "-3", "2.5", "0"] {
            let out = substitute_mutation_diff(
                &json!({"add_edges": [{"from": "a", "to": "b", "condition": "hop.n < ${N}"}]}),
                &env_of("N", value),
                &HashMap::new(),
            )
            .unwrap_or_else(|e| panic!("{value}: {e:?}"));
            assert_eq!(out["add_edges"][0]["condition"], format!("hop.n < {value}"));
        }
        for (slot, edge) in [
            (
                "condition",
                json!({"from": "a", "to": "b", "condition": "hop.n == ${N}"}),
            ),
            (
                "modifier.set_context.k",
                json!({"from": "a", "to": "b", "modifier": {"set_context": {"k": "${N}"}}}),
            ),
            (
                "modifier.set_hop.route",
                json!({"from": "a", "to": "b",
                       "modifier": {"set_hop": {"route": "hop.x == 'q' ? ${N} : 'r'"}}}),
            ),
        ] {
            for value in ["s3cr3t || true", "s3cr3t", "1 || true", "1.", "-", ""] {
                let err = substitute_mutation_diff(
                    &json!({"add_edges": [edge.clone()]}),
                    &env_of("N", value),
                    &HashMap::new(),
                )
                .expect_err(slot);
                assert_eq!(err.error_code(), "env_value_unsafe", "{slot}");
                let said = err.message();
                assert!(
                    said.contains("${N}") && said.contains(&format!("add_edges[0].{slot}")),
                    "{slot}: {said}"
                );
                assert!(!said.contains("s3cr3t"), "{slot}: the value leaked");
            }
        }
        for (condition, bound) in [
            ("hop.n == '${N}'", "hop.n == '7'"),
            ("hop.n == \"${N}\"", "hop.n == \"7\""),
            ("'[\"member:${N}\"]'", "'[\"member:7\"]'"),
            (
                "hop.s == 'it\\'s' && hop.n == '${N}'",
                "hop.s == 'it\\'s' && hop.n == '7'",
            ),
            ("hop.n == $${N}", "hop.n == ${N}"),
            ("hop.n == ${ctx.n}", "hop.n == 7"),
            ("int(context.attempt) < ${N:-3}", "int(context.attempt) < 7"),
            ("hop.n == ${N}", "hop.n == 7"),
            (
                "hop.n == -${N} || hop.n == ${N}.0",
                "hop.n == -7 || hop.n == 7.0",
            ),
        ] {
            let out = substitute_mutation_diff(
                &json!({"add_edges": [{"from": "a", "to": "b", "condition": condition}]}),
                &env_of("N", "7"),
                &env_of("n", "7"),
            )
            .unwrap_or_else(|e| panic!("{condition}: {e:?}"));
            assert_eq!(out["add_edges"][0]["condition"], bound, "{condition}");
        }
        // A quote that closes before the token leaves it outside again.
        let err = substitute_mutation_diff(
            &json!({"add_edges": [{"from": "a", "to": "b",
                                   "condition": "hop.s == 'x' && hop.n == ${N}"}]}),
            &env_of("N", "1 || true"),
            &HashMap::new(),
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "env_value_unsafe");
    }

    /// GH #949 (review I-R2) -- CEL also spells a raw string (`r'…'`, where a
    /// backslash is text) and a triple-quoted one (`'''…'''`, where a single
    /// quote is text), and `token_quoting` lexes neither. Red before the fix:
    /// after `r'\'` or `'''it's'''` the scanner held `${N}` for quoted, only
    /// `cel_string_safe` judged it, and `1 || true` bound into an always-true
    /// condition. A token after such a literal is refused whatever it holds; a
    /// token before one is judged as before.
    #[test]
    fn a_token_after_a_raw_or_triple_quoted_literal_is_refused() {
        for condition in [
            "hop.p == r'\\' && hop.n == ${N}",
            "hop.p == R\"\\\" && hop.n == ${N}",
            "hop.s == '''it's''' && hop.n == ${N}",
            "hop.s == \"\"\"a\"b\"\"\" && hop.n == ${N}",
        ] {
            for value in ["1 || true", "s3cr3t", "7"] {
                let err = substitute_mutation_diff(
                    &json!({"add_edges": [{"from": "a", "to": "b", "condition": condition}]}),
                    &env_of("N", value),
                    &HashMap::new(),
                )
                .expect_err(condition);
                assert_eq!(err.error_code(), "env_value_unsafe", "{condition}");
                assert!(!err.message().contains("s3cr3t"), "the value leaked");
            }
        }
        let out = substitute_mutation_diff(
            &json!({"add_edges": [{"from": "a", "to": "b",
                                   "condition": "hop.n == ${N} && hop.p == r'\\'"}]}),
            &env_of("N", "7"),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            out["add_edges"][0]["condition"],
            "hop.n == 7 && hop.p == r'\\'"
        );
    }

    /// GH #967.2 -- a quote inside a `//` comment is text. Red before the fix:
    /// the apostrophe of `it's` opened a literal, the next line's `${N}` was
    /// held for quoted, and `1 || true` bound into an always-true condition --
    /// at the door and in the boot pass alike. A token inside the comment is
    /// judged as quoted (a value without a newline cannot leave the comment).
    #[test]
    fn a_comment_does_not_open_a_literal() {
        let condition = "hop.m == 1 // it's\n && hop.n == ${N}";
        let err = substitute_mutation_diff(
            &json!({"add_edges": [{"from": "a", "to": "b", "condition": condition}]}),
            &env_of("N", "1 || true"),
            &HashMap::new(),
        )
        .expect_err("a quote in a comment opened a literal");
        assert_eq!(err.error_code(), "env_value_unsafe");
        let out = substitute_mutation_diff(
            &json!({"add_edges": [{"from": "a", "to": "b", "condition": condition}]}),
            &env_of("N", "7"),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            out["add_edges"][0]["condition"],
            "hop.m == 1 // it's\n && hop.n == 7"
        );
        // A token inside the comment is text: a plain word binds, a newline does not.
        let in_comment = "hop.m == 1 // note ${N}\n && true";
        substitute_mutation_diff(
            &json!({"add_edges": [{"from": "a", "to": "b", "condition": in_comment}]}),
            &env_of("N", "some word"),
            &HashMap::new(),
        )
        .unwrap();
        let err = substitute_mutation_diff(
            &json!({"add_edges": [{"from": "a", "to": "b", "condition": in_comment}]}),
            &env_of("N", "x\n || true"),
            &HashMap::new(),
        )
        .unwrap_err();
        assert_eq!(err.error_code(), "env_value_unsafe");
        // `//` inside a literal is no comment: the quote after it still closes it.
        let out = substitute_mutation_diff(
            &json!({"add_edges": [{"from": "a", "to": "b",
                                   "condition": "hop.u == 'http://x' && hop.n == ${N}"}]}),
            &env_of("N", "7"),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(
            out["add_edges"][0]["condition"],
            "hop.u == 'http://x' && hop.n == 7"
        );
        // The boot pass reads the same table.
        let boot = json!({"params": graph_params(condition)});
        let boot_env = |n: &str| {
            let mut e = env_of("N", n);
            e.insert("CHAT".into(), "111".into());
            e
        };
        let err = substitute_env_only(&boot, &boot_env("1 || true")).unwrap_err();
        assert_eq!(err.error_code(), "env_value_unsafe");
        let out = substitute_env_only(&boot, &boot_env("7")).unwrap();
        assert_eq!(
            out["params"]["graph"]["edges"][1]["condition"],
            "hop.m == 1 // it's\n && hop.n == 7"
        );
    }

    /// GH #972 (R-NL-4) -- `an_app_cannot_set_the_turn_round`: a config's own
    /// edge that sets a stamped key is refused at the door (every lift of a
    /// node's params) and in the boot pass, whatever the environment; the same
    /// key on an `add_edges` entry -- the wiring -- passes, and so does a
    /// config edge that deletes it (fail-closed for every reader).
    #[test]
    fn an_app_cannot_set_the_turn_round() {
        for key in crate::cel_eval::STAMPED_CONTEXT_KEYS {
            let params = json!({"graph": {"edges": [
                {"from": "./pin", "to": ".", "modifier": {"set_context": {(key): "'[\"*\"]'"}}}
            ]}});
            for diff in [
                json!({"add_nodes": [{"name": "apps/x", "template": "x@1.0.0",
                                      "override_params": params}]}),
                json!({"swap_nodes": [{"match": {"name": "a"},
                                       "with": {"template": "x@1.0.0", "params": params}}]}),
                json!({"replace_nodes": [{"match": {"name": "a"},
                                          "with": {"template": "x@1.0.0", "params": params}}]}),
            ] {
                let err = substitute_mutation_diff(&diff, &HashMap::new(), &HashMap::new())
                    .expect_err("a config set a stamped key");
                assert_eq!(err.error_code(), "edge_schema", "{diff}");
                assert!(err.message().contains(key), "{err:?}");
            }
            for cfg in [json!({"params": params.clone()}), params.clone()] {
                let err = substitute_env_only(&cfg, &HashMap::new()).unwrap_err();
                assert_eq!(err.error_code(), "edge_schema", "boot: {cfg}");
            }
            substitute_mutation_diff(
                &json!({"add_edges": [{"from": "./ch", "to": ".",
                                       "modifier": {"set_context": {(key): "'[\"m\"]'"}}}]}),
                &HashMap::new(),
                &HashMap::new(),
            )
            .expect("the wiring stamps it");
            substitute_env_only(
                &json!({"params": {"graph": {"edges": [
                    {"from": "./a", "to": ".", "modifier": {"delete_context": [key]}}
                ]}}}),
                &HashMap::new(),
            )
            .expect("deleting proves nothing and is not refused");
        }
    }

    /// GH #979 (OR-NL-179) -- `a_config_edge_restores_a_stamped_key_and_claims_none`:
    /// `speaker` is stamped like `turn_round`, so an app's own edge that writes
    /// it is refused at the door and at boot; the ONE form a config edge may
    /// write either key with is the literal restore of that same key from a
    /// parked hop (the firewall's release edge). A fallback literal, a second
    /// key or an added term is a claim again.
    #[test]
    fn a_config_edge_restores_a_stamped_key_and_claims_none() {
        assert!(crate::cel_eval::STAMPED_CONTEXT_KEYS.contains(&"speaker"));
        let edge_setting = |key: &str, expr: &str| {
            json!({"graph": {"edges": [
                {"from": "./warden", "to": ".", "condition": "has(hop.route) && hop.route == 'pass'",
                 "modifier": {"set_context": {(key): expr}}}
            ]}})
        };
        for key in crate::cel_eval::STAMPED_CONTEXT_KEYS {
            let restore = format!("has(hop.ctx_{key}) ? hop.ctx_{key} : ''");
            let params = edge_setting(key, &restore);
            substitute_env_only(&json!({"params": params.clone()}), &HashMap::new())
                .expect("the boot pass takes the restore");
            substitute_mutation_diff(
                &json!({"add_nodes": [{"name": "firewall", "template": "firewall@1.0.0",
                                       "override_params": params}]}),
                &HashMap::new(),
                &HashMap::new(),
            )
            .expect("the door takes the restore");
            let other = if key == "speaker" {
                "turn_round"
            } else {
                "speaker"
            };
            for claim in [
                "'member:alex'".to_string(),
                format!("has(hop.ctx_{key}) ? hop.ctx_{key} : 'member:alex'"),
                format!("has(hop.ctx_{other}) ? hop.ctx_{other} : ''"),
                format!("has(hop.ctx_{key}) ? hop.ctx_{key} : '' + 'x'"),
                format!("hop.ctx_{key}"),
                format!("has(hop.{key}) ? hop.{key} : ''"),
            ] {
                let params = edge_setting(key, &claim);
                let err = substitute_env_only(&json!({"params": params.clone()}), &HashMap::new())
                    .expect_err("a config edge claimed a stamped key at boot");
                assert_eq!(err.error_code(), "edge_schema", "{key} = {claim}");
                let err = substitute_mutation_diff(
                    &json!({"add_nodes": [{"name": "apps/x", "template": "x@1.0.0",
                                           "override_params": params}]}),
                    &HashMap::new(),
                    &HashMap::new(),
                )
                .expect_err("a config edge claimed a stamped key at the door");
                assert_eq!(err.error_code(), "edge_schema", "{key} = {claim}");
            }
        }
    }

    /// GH #979 (OR-NL-179) -- every shipped template boots past the stamped-key
    /// check: the firewall's release edge restores `speaker` and `turn_round`
    /// in the one allowed form, and no other template's own edge writes them.
    /// (A variable a template leaves to the environment is not this check's.)
    #[test]
    fn no_shipped_template_claims_a_stamped_key() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).expect("readable").flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.file_name().is_some_and(|n| n == "config.json") {
                    out.push(p);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates");
        if !root.is_dir() {
            return;
        }
        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(files.len() > 50, "the template library: {}", files.len());
        let firewall = root.join("firewall/config.json");
        assert!(files.contains(&firewall));
        let raw = std::fs::read_to_string(&firewall).expect("the firewall");
        for key in crate::cel_eval::STAMPED_CONTEXT_KEYS {
            assert!(
                raw.contains(&format!(
                    "\"{key}\": \"has(hop.ctx_{key}) ? hop.ctx_{key} : ''\""
                )),
                "the firewall's release edge restores `{key}`"
            );
        }
        for f in files {
            let Ok(cfg) = meclaw_core::serde_json::from_str::<JsonValue>(
                &std::fs::read_to_string(&f).expect("readable"),
            ) else {
                continue;
            };
            if let Err(e) = substitute_env_only(&cfg, &HashMap::new()) {
                assert_ne!(e.error_code(), "edge_schema", "{}: {e:?}", f.display());
            }
        }
    }

    /// One hive config whose own edge carries `cond` as its condition, under
    /// `params.graph.edges[1]` (the first edge is harmless).
    fn graph_params(cond: &str) -> JsonValue {
        json!({"graph": {"edges": [
            {"from": ".", "to": "./a", "condition": "has(hop.route)"},
            {"from": ".", "to": "./b", "condition": cond,
             "modifier": {"set_context": {"seen": "'yes'"}}}
        ]}, "api_key": "${CHAT}"})
    }

    /// Red before the fix: a node's params bound `${CHAT}` into the edges they
    /// draw unjudged -- `add_nodes` / `swap_nodes` / `replace_nodes` kept the
    /// token for disk and the boot pass bound it. Now the door judges those CEL
    /// slots against the environment before anything is staged, names the
    /// operation, the entry and the slot, and still writes the token (never
    /// the value) to disk; a variable that is not set is the binding pass's to
    /// name, not this check's.
    #[test]
    fn the_edges_a_nodes_params_draw_are_judged_at_the_door() {
        let quoted = graph_params("string(hop.chat_id) == '${CHAT}'");
        let unquoted = graph_params("hop.chat_id == ${CHAT}");
        let diff_of = |params: &JsonValue, op: &str| match op {
            "add_nodes" => json!({"add_nodes": [{"name": "n", "template": "t@1.0.0",
                                                  "override_params": params}]}),
            _ => json!({op: [{"match": {"name": "a"},
                               "with": {"template": "t@1.0.0", "params": params}}]}),
        };
        for (op, at) in [
            ("add_nodes", "add_nodes[0].override_params"),
            ("swap_nodes", "swap_nodes[0].with.params"),
            ("replace_nodes", "replace_nodes[0].with.params"),
        ] {
            let slot = format!("{at}.graph.edges[1].condition");
            for (what, params, value) in [
                ("a reopened string", &quoted, "s3cr3t' || true || '1"),
                ("an unquoted token", &unquoted, "111 || true"),
            ] {
                let err = substitute_mutation_diff(
                    &diff_of(params, op),
                    &env_of("CHAT", value),
                    &HashMap::new(),
                )
                .expect_err(what);
                assert_eq!(err.error_code(), "env_value_unsafe", "{op}: {what}");
                assert!(
                    err.message().contains("${CHAT}") && err.message().contains(&slot),
                    "{op}: {what}: {}",
                    err.message()
                );
                assert!(!err.message().contains("s3cr3t"), "{op}: the value leaked");
            }
            let out = substitute_mutation_diff(
                &diff_of(&quoted, op),
                &env_of("CHAT", "111"),
                &HashMap::new(),
            )
            .unwrap_or_else(|e| panic!("{op}: a plain id was refused: {e:?}"));
            assert!(
                out.to_string().contains("'${CHAT}'") && !out.to_string().contains("'111'"),
                "{op}: the token stays a token on disk: {out}"
            );
            substitute_mutation_diff(&diff_of(&quoted, op), &HashMap::new(), &HashMap::new())
                .unwrap_or_else(|e| panic!("{op}: an unset variable is not this check's: {e:?}"));
        }
    }

    /// Red before the fix: the boot pass bound `${CHAT}` into a config's own
    /// edges unjudged. It judges them now as the door does -- over a whole
    /// config (`params.graph.edges`) and over a bare `params` object
    /// (`graph.edges`) -- and binds a plain id exactly as before; the same
    /// value outside the edges (a key) binds whatever it holds, and a missing
    /// variable is still `env_var_missing`.
    #[test]
    fn the_boot_pass_judges_the_edges_a_config_draws() {
        let quoted = graph_params("string(hop.chat_id) == '${CHAT}'");
        for (input, slot) in [
            (
                json!({"name": "n", "params": quoted.clone()}),
                "params.graph.edges[1].condition",
            ),
            (quoted.clone(), "graph.edges[1].condition"),
        ] {
            let err =
                substitute_env_only(&input, &env_of("CHAT", "s3cr3t' || true || '1")).unwrap_err();
            assert_eq!(err.error_code(), "env_value_unsafe", "{slot}");
            assert!(
                err.message().contains("${CHAT}") && err.message().contains(slot),
                "{err:?}"
            );
            assert!(!err.message().contains("s3cr3t"), "the value leaked");

            let out = substitute_env_only(&input, &env_of("CHAT", "111")).unwrap();
            assert!(
                out.to_string().contains("string(hop.chat_id) == '111'"),
                "{out}"
            );

            let err = substitute_env_only(&input, &HashMap::new()).unwrap_err();
            assert_eq!(err.error_code(), "env_var_missing", "{slot}");
        }
        let unquoted = json!({"params": graph_params("int(hop.n) < ${CHAT}")});
        let err = substitute_env_only(&unquoted, &env_of("CHAT", "3 || true")).unwrap_err();
        assert_eq!(err.error_code(), "env_value_unsafe");
        assert!(
            err.message().contains("params.graph.edges[1].condition"),
            "{err:?}"
        );
        let out = substitute_env_only(&unquoted, &env_of("CHAT", "3")).unwrap();
        assert_eq!(
            out["params"]["graph"]["edges"][1]["condition"],
            "int(hop.n) < 3"
        );
        // Outside the edges a value may hold anything: a secret is not CEL.
        let out = substitute_env_only(
            &json!({"params": {"api_key": "${CHAT}", "graph": {"edges": []}}}),
            &env_of("CHAT", "s3cr3t' \\ \""),
        )
        .unwrap();
        assert_eq!(out["params"]["api_key"], "s3cr3t' \\ \"");
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
