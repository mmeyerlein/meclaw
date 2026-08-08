//! Variable substitution for mutation diffs (phase 6).
//!
//! `${ENV_VAR}` from `.env`, `${ctx.<key>}` from the mutation ctx,
//! `${uuid7:label}` with a per-label cache. T7 covers only `${ENV_VAR}` — T8/T9
//! extend it.

use super::MutationError;
use meclaw_core::JsonValue;
use std::collections::HashMap;

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

/// Shared `$`-driven scanner for all three substitute functions.
///
/// Walks `s` from one `$` to the next. At a `$`:
/// - `$${` (escape, checked FIRST) → consume one `$`, emit `${inner}` literally
///   (no resolve), continue after the closing `}`.
/// - `${` (regular token) → find the closing `}`, hand `inner` to `resolve`.
///   No closing `}` → `MutationError::Schema`.
/// - lone `$` → literal `$`.
///
/// `resolve` is the per-function token resolver. Byte-correct: scanning works on
/// byte offsets, but all slice boundaries land on `$`/`{`/`}` (ASCII) or on a
/// `find('$')` hit, so a multi-byte char is never split.
fn expand(
    s: &str,
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
    fn walk(
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
                    out.push(walk(item, env, ctx, cache)?);
                }
                Ok(JsonValue::Array(out))
            }
            JsonValue::Object(m) => {
                let mut out = meclaw_core::serde_json::Map::new();
                for (k, val) in m {
                    out.insert(k.clone(), walk(val, env, ctx, cache)?);
                }
                Ok(JsonValue::Object(out))
            }
            _ => Ok(v.clone()),
        }
    }
    walk(diff, env, ctx, &mut uuid_cache)
}

fn replace_full(
    s: &str,
    env: &HashMap<String, String>,
    ctx: &HashMap<String, String>,
    cache: &mut HashMap<String, String>,
) -> Result<String, MutationError> {
    // `expand` takes `impl Fn`; the uuid7 cache needs interior mutability so the
    // closure can mint-and-store a UUID per label. RefCell is single-threaded
    // (one cell task), no lock contention — CLAUDE.md concurrency model holds.
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
