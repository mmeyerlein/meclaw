//! R-HK-15/16 (2026-10-06): the window of one request.
//!
//! Two rules, both applied to the built request body before the provider is
//! asked:
//!
//! - A prompt above `input_hard` is refused -- no model call, an
//!   `invalid_input` answer with `meta.error.kind` `input_over_hard`, and a
//!   line on [`WINDOW_TARGET`]. A prompt that alone fills `context_window` is
//!   refused the same way (`input_over_window`): the provider would refuse it
//!   anyway, after billing the trip.
//! - The output budget never asks past the window: `max_tokens` (chat) or
//!   `max_output_tokens` (responses) is clamped to `context_window` minus the
//!   prompt estimate. A local server refuses a larger budget with a 400 (KF1
//!   finding 4); a clamp is said on the same target.
//!
//! The estimate is deliberately high: a token is about four bytes of English,
//! and THREE counts JSON, German and code too, so the clamp never asks past
//! the window. An inline image (a `data:` URL) counts as [`IMAGE_TOKENS`], not
//! as its base64 bytes. Without `input_hard` and `context_window` (both 0, the
//! defaults) nothing here changes the request.

use crate::llm::params::LlmParams;
use meclaw_core::serde_json::{Map, Value};

/// The log target of the refusals and the clamp -- the journal line of
/// R-HK-15 (OR-HK.KF1.3: a log line, no hop key).
pub(crate) const WINDOW_TARGET: &str = "meclaw::llm::window";

/// Bytes counted as one token. High on purpose (see the module doc).
const BYTES_PER_TOKEN: u64 = 3;

/// Tokens counted for one inline image.
const IMAGE_TOKENS: u64 = 1024;

/// A refused request: what was estimated, against which bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    /// `meta.error.kind`: `input_over_hard` or `input_over_window`.
    pub(crate) kind: &'static str,
    /// The bound's param name: `input_hard` or `context_window`.
    pub(crate) bound_key: &'static str,
    pub(crate) bound: u64,
    pub(crate) estimate: u64,
}

impl Refusal {
    /// `meta.error` beside `source`/`detail`: the kind, the estimate and the
    /// bound by its param name.
    pub(crate) fn meta(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("kind".into(), Value::from(self.kind));
        m.insert("input_estimate".into(), Value::from(self.estimate));
        m.insert(self.bound_key.into(), Value::from(self.bound));
        m
    }

    pub(crate) fn detail(&self) -> String {
        format!(
            "window: prompt of about {} tokens over {}={}; no model call",
            self.estimate, self.bound_key, self.bound
        )
    }
}

/// The estimated prompt tokens of a request body.
pub(crate) fn estimate_prompt_tokens(body: &Value) -> u64 {
    let (mut bytes, mut images) = (0u64, 0u64);
    walk(body, &mut bytes, &mut images);
    bytes.div_ceil(BYTES_PER_TOKEN) + images * IMAGE_TOKENS
}

fn walk(v: &Value, bytes: &mut u64, images: &mut u64) {
    match v {
        Value::String(s) if s.starts_with("data:") => *images += 1,
        Value::String(s) => *bytes += s.len() as u64,
        Value::Array(a) => a.iter().for_each(|x| walk(x, bytes, images)),
        Value::Object(o) => {
            for (k, x) in o {
                *bytes += k.len() as u64;
                walk(x, bytes, images);
            }
        }
        Value::Number(_) | Value::Bool(_) => *bytes += 4,
        Value::Null => {}
    }
}

/// Apply both rules to `body`. `Err` = refuse, the body is untouched and no
/// provider may be called. `Ok(Some((asked, now)))` = the budget under
/// `budget_key` was clamped. Every outcome but a plain pass writes its line on
/// [`WINDOW_TARGET`], naming `path` first.
pub(crate) fn apply(
    params: &LlmParams,
    body: &mut Value,
    budget_key: &str,
    path: &str,
) -> Result<Option<(u64, u64)>, Refusal> {
    if params.input_hard == 0 && params.context_window == 0 {
        return Ok(None);
    }
    let estimate = estimate_prompt_tokens(body);
    let refusal = if params.input_hard > 0 && estimate > params.input_hard {
        Some(Refusal {
            kind: "input_over_hard",
            bound_key: "input_hard",
            bound: params.input_hard,
            estimate,
        })
    } else if params.context_window > 0 && estimate >= params.context_window {
        Some(Refusal {
            kind: "input_over_window",
            bound_key: "context_window",
            bound: params.context_window,
            estimate,
        })
    } else {
        None
    };
    if let Some(r) = refusal {
        tracing::warn!(
            target: WINDOW_TARGET,
            "{path} refused: prompt of about {} tokens over {}={} -- no model call",
            r.estimate,
            r.bound_key,
            r.bound
        );
        return Err(r);
    }
    if params.context_window == 0 {
        return Ok(None);
    }
    let left = params.context_window - estimate;
    let Some(asked) = body.get(budget_key).and_then(Value::as_u64) else {
        return Ok(None);
    };
    if asked <= left {
        return Ok(None);
    }
    body[budget_key] = Value::from(left);
    tracing::info!(
        target: WINDOW_TARGET,
        "{path} clamped {budget_key} {asked} -> {left}: context_window={} minus a prompt of about {estimate} tokens",
        params.context_window
    );
    Ok(Some((asked, left)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    fn params(extra: Value) -> LlmParams {
        let mut raw = json!({"provider": "openai", "model": "m", "api_key": "k"});
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            raw[k] = v;
        }
        LlmParams::parse(&raw).unwrap()
    }

    fn body(text: &str) -> Value {
        json!({"model": "m", "messages": [{"role": "user", "content": text}], "max_tokens": 32768})
    }

    #[test]
    fn without_bounds_nothing_changes() {
        let mut b = body(&"x".repeat(300_000));
        let before = b.clone();
        assert_eq!(
            apply(&params(json!({})), &mut b, "max_tokens", "/p"),
            Ok(None)
        );
        assert_eq!(b, before);
    }

    #[test]
    fn the_estimate_is_high_and_images_count_flat() {
        let plain = estimate_prompt_tokens(&json!({"t": "x".repeat(3000)}));
        assert!(plain >= 1000, "{plain}");
        let image = estimate_prompt_tokens(
            &json!({"u": format!("data:image/png;base64,{}", "A".repeat(900_000))}),
        );
        assert!(image < 2 * IMAGE_TOKENS, "{image}");
    }

    #[test]
    fn over_hard_is_refused_and_the_body_kept() {
        let mut b = body(&"x".repeat(3000));
        let before = b.clone();
        let r = apply(
            &params(json!({"input_hard": 500})),
            &mut b,
            "max_tokens",
            "/p",
        )
        .unwrap_err();
        assert_eq!(r.kind, "input_over_hard");
        assert_eq!(r.meta()["input_hard"], 500);
        assert_eq!(b, before);
    }

    #[test]
    fn a_prompt_that_fills_the_window_is_refused() {
        let mut b = body(&"x".repeat(3000));
        let r = apply(
            &params(json!({"context_window": 900})),
            &mut b,
            "max_tokens",
            "/p",
        )
        .unwrap_err();
        assert_eq!(r.kind, "input_over_window");
    }

    #[test]
    fn the_budget_is_clamped_to_what_the_window_leaves() {
        let mut b = body("hello");
        let est = estimate_prompt_tokens(&b);
        let got = apply(
            &params(json!({"context_window": 8000})),
            &mut b,
            "max_tokens",
            "/p",
        )
        .unwrap();
        assert_eq!(got, Some((32768, 8000 - est)));
        assert_eq!(b["max_tokens"], 8000 - est);
        // A budget that fits stays.
        let mut b = body("hello");
        assert_eq!(
            apply(
                &params(json!({"context_window": 1_000_000})),
                &mut b,
                "max_tokens",
                "/p"
            ),
            Ok(None)
        );
        assert_eq!(b["max_tokens"], 32768);
    }
}
