//! GH #957: the `decisions` provider -- typed questions about one state in,
//! typed answers out, one round trip, no generated text.
//!
//! The cell's own vocabulary is vendor-neutral (`kind`: `choice` | `yes_no` |
//! `scale`, with `options` / `levels`); it is translated HERE and nowhere else
//! into the wire of the hosted decisions API: `POST <base_url>/alpha/decisions`
//! with `{model, state, questions: {<key>: {type, instructions, criteria}}}`,
//! where `choice` stays `choice`, `yes_no` becomes `noul` and `scale` becomes
//! `score`. The path hangs off `base_url` WITHOUT the chat wire's `/v1`
//! (example `base_url`: `https://openrouter.ai/api`) -- that prefix does not
//! exist on this wire.
//!
//! Both directions are pure functions. The answer is normalized per asked key
//! into `{choice, p}`, `{yes}` or `{value, p}`. A question the service left
//! unanswered is named in `missing` and the other answers stand (GH #977: one
//! omitted answer of a display's 1 + 2·N questions had tipped every topic's
//! verdict); a call with no answer at all, or a malformed answer -- a choice
//! that was not asked, a missing probability -- is `decision_incomplete`.

use meclaw_core::serde_json::{Map, Value, json};

/// The path of the decisions call, appended to `base_url`.
pub(crate) const DECISIONS_PATH: &str = "/alpha/decisions";

/// At most this many questions per call. A display asks 1 + 2·N questions
/// per turn over N topics; the fan-out is measured up to 32 questions at
/// +10 % latency against one, and 64 leaves room for that shape without
/// letting a caller build an unbounded request.
pub(crate) const MAX_QUESTIONS: usize = 64;
/// A choice names 2..=255 options (the service's own bound).
pub(crate) const MIN_OPTIONS: usize = 2;
pub(crate) const MAX_OPTIONS: usize = 255;
/// A scale names 2..=10 ordered levels (the service's own bound).
pub(crate) const MIN_LEVELS: usize = 2;
pub(crate) const MAX_LEVELS: usize = 10;
/// A question key: `[a-z0-9_.-]`, 1..=64 characters.
pub(crate) const KEY_MAX_CHARS: usize = 64;

/// What one question asks, as the cell needs it to read the answer back.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kind {
    /// One of the named options.
    Choice(Vec<String>),
    /// A probability that the answer is yes.
    YesNo,
    /// A position on the ordered levels.
    Scale(Vec<String>),
}

/// A validated `decide` slot: the state, and each question with its kind.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Asked {
    state: Value,
    /// The wire form of each question, keyed as asked.
    wire: Map<String, Value>,
    /// The kind of each question, keyed as asked.
    kinds: Vec<(String, Kind)>,
}

/// One normalized answer of the service, plus what the hop header carries.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Decided {
    /// `{<key>: {choice, p} | {yes} | {value, p}}`, one entry per answered key.
    pub(crate) answers: Map<String, Value>,
    /// The asked keys the service left unanswered, sorted (GH #977).
    pub(crate) missing: Vec<String>,
    /// The model the service says it ran, when it says so.
    pub(crate) model: Option<String>,
    /// The service's id of the call, when it gives one.
    pub(crate) response_id: Option<String>,
    pub(crate) tokens_prompt: Option<u64>,
    pub(crate) tokens_completion: Option<u64>,
    pub(crate) cost: Option<f64>,
}

fn key_ok(k: &str) -> bool {
    !k.is_empty()
        && k.chars().count() <= KEY_MAX_CHARS
        && k.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-'))
}

fn text_of<'a>(q: &'a Map<String, Value>, field: &str, key: &str) -> Result<&'a str, String> {
    match q.get(field).and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => Ok(s),
        _ => Err(format!(
            "question '{key}': '{field}' must be a non-empty string"
        )),
    }
}

/// Read the `decide` slot of a body. `Err` is the detail of a
/// `decide_invalid` refusal; nothing is sent for it.
pub(crate) fn parse_decide(decide: Option<&Value>) -> Result<Asked, String> {
    let d = decide
        .and_then(Value::as_object)
        .ok_or("the body carries no 'decide' object")?;
    for k in d.keys() {
        if k != "state" && k != "questions" {
            return Err(format!("'decide' takes 'state' and 'questions', not '{k}'"));
        }
    }
    let state = match d.get("state") {
        Some(Value::String(s)) if !s.trim().is_empty() => Value::String(s.clone()),
        Some(Value::Object(o)) if !o.is_empty() => Value::Object(o.clone()),
        _ => return Err("'decide.state' must be a non-empty string or object".into()),
    };
    let qs = d
        .get("questions")
        .and_then(Value::as_object)
        .ok_or("'decide.questions' must be an object")?;
    if qs.is_empty() || qs.len() > MAX_QUESTIONS {
        return Err(format!(
            "'decide.questions' holds {} questions; 1..={MAX_QUESTIONS} are allowed",
            qs.len()
        ));
    }
    let mut wire = Map::new();
    let mut kinds = Vec::with_capacity(qs.len());
    for (key, q) in qs {
        if !key_ok(key) {
            return Err(format!(
                "question key '{key}' must be 1..={KEY_MAX_CHARS} characters of [a-z0-9_.-]"
            ));
        }
        let q = q
            .as_object()
            .ok_or_else(|| format!("question '{key}' must be an object"))?;
        let kind = q.get("kind").and_then(Value::as_str).unwrap_or_default();
        let allowed: &[&str] = match kind {
            "choice" => &["kind", "instructions", "options"],
            "yes_no" => &["kind", "instructions"],
            "scale" => &["kind", "instructions", "levels"],
            other => {
                return Err(format!(
                    "question '{key}': kind '{other}' is none of 'choice', 'yes_no', 'scale'"
                ));
            }
        };
        if let Some(extra) = q.keys().find(|f| !allowed.contains(&f.as_str())) {
            return Err(format!(
                "question '{key}' of kind '{kind}' takes no '{extra}'"
            ));
        }
        let instructions = text_of(q, "instructions", key)?;
        match kind {
            "choice" => {
                let opts = q
                    .get("options")
                    .and_then(Value::as_object)
                    .ok_or_else(|| format!("question '{key}': 'options' must be an object"))?;
                if opts.len() < MIN_OPTIONS || opts.len() > MAX_OPTIONS {
                    return Err(format!(
                        "question '{key}' names {} options; {MIN_OPTIONS}..={MAX_OPTIONS} are allowed",
                        opts.len()
                    ));
                }
                for (name, describe) in opts {
                    if name.trim().is_empty() || !describe.is_string() {
                        return Err(format!(
                            "question '{key}': every option is a name with a description"
                        ));
                    }
                }
                wire.insert(
                    key.clone(),
                    json!({"type": "choice", "instructions": instructions,
                           "criteria": Value::Object(opts.clone())}),
                );
                kinds.push((key.clone(), Kind::Choice(opts.keys().cloned().collect())));
            }
            "yes_no" => {
                wire.insert(
                    key.clone(),
                    json!({"type": "noul", "instructions": instructions}),
                );
                kinds.push((key.clone(), Kind::YesNo));
            }
            _ => {
                let levels = q
                    .get("levels")
                    .and_then(Value::as_array)
                    .ok_or_else(|| format!("question '{key}': 'levels' must be a list"))?;
                if levels.len() < MIN_LEVELS || levels.len() > MAX_LEVELS {
                    return Err(format!(
                        "question '{key}' names {} levels; {MIN_LEVELS}..={MAX_LEVELS} are allowed",
                        levels.len()
                    ));
                }
                let mut names: Vec<String> = Vec::with_capacity(levels.len());
                for l in levels {
                    match l.as_str() {
                        Some(s) if !s.trim().is_empty() && !names.iter().any(|n| n == s) => {
                            names.push(s.to_string());
                        }
                        _ => {
                            return Err(format!(
                                "question '{key}': levels are distinct non-empty strings"
                            ));
                        }
                    }
                }
                wire.insert(
                    key.clone(),
                    json!({"type": "score", "instructions": instructions,
                           "criteria": levels.clone()}),
                );
                kinds.push((key.clone(), Kind::Scale(names)));
            }
        }
    }
    Ok(Asked { state, wire, kinds })
}

/// The request body for `model`, whole. Its size is no business of the
/// translation: whether it fits is the model's window, read off the catalog
/// row (`input_hard` / `context_window`) by `llm::window` before the call --
/// GH #1085 removed the fixed 128 KiB bound that stood here.
pub(crate) fn build_request(model: &str, asked: &Asked) -> Value {
    json!({
        "model": model,
        "state": asked.state.clone(),
        "questions": Value::Object(asked.wire.clone()),
    })
}

fn prob(v: &Value) -> Option<f64> {
    v.as_f64()
        .filter(|f| f.is_finite() && (0.0..=1.0).contains(f))
}

/// The probabilities of a choice, keyed by the asked names. A name the
/// service did not list gets no key; a name nobody asked for is dropped.
fn choice_p(a: &Map<String, Value>, names: &[String], key: &str) -> Result<Value, String> {
    let probs = a
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("answer '{key}' carries no probabilities"))?;
    let mut p = Map::new();
    for n in names {
        if let Some(v) = probs.get(n) {
            let f = prob(v).ok_or_else(|| format!("answer '{key}': p('{n}') is no probability"))?;
            p.insert(n.clone(), Value::from(f));
        }
    }
    Ok(Value::Object(p))
}

/// The probabilities of a scale, keyed by level NAME. The service keys them
/// by level index (`"0"`, `"1"`, …, with a `legend` beside them); a key that
/// already is a level name is taken as it stands.
fn scale_p(a: &Map<String, Value>, levels: &[String], key: &str) -> Result<Value, String> {
    let probs = a
        .get("probabilities")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("answer '{key}' carries no probabilities"))?;
    let mut p = Map::new();
    for (k, v) in probs {
        let name = match k.parse::<usize>() {
            Ok(i) => levels.get(i).cloned(),
            Err(_) => levels.iter().find(|l| *l == k).cloned(),
        }
        .ok_or_else(|| format!("answer '{key}': level '{k}' was not asked"))?;
        let f = prob(v).ok_or_else(|| format!("answer '{key}': p('{k}') is no probability"))?;
        p.insert(name, Value::from(f));
    }
    Ok(Value::Object(p))
}

/// Read the service's answer against what was asked. `Err` is the detail of
/// a `decision_incomplete` failure: no answer at all, or a malformed one. An
/// asked key without an answer lands in `missing` (GH #977).
pub(crate) fn parse_response(resp: &Value, asked: &Asked) -> Result<Decided, String> {
    let answers = resp
        .get("answers")
        .and_then(Value::as_object)
        .ok_or("the answer carries no 'answers' object")?;
    let mut out = Map::new();
    let mut missing = Vec::new();
    for (key, kind) in &asked.kinds {
        let Some(a) = answers.get(key).filter(|v| !v.is_null()) else {
            missing.push(key.clone());
            continue;
        };
        let a = a
            .as_object()
            .ok_or_else(|| format!("answer '{key}' is no object"))?;
        let normalized = match kind {
            Kind::Choice(names) => {
                let choice = a
                    .get("choice")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("answer '{key}' names no choice"))?;
                if !names.iter().any(|n| n == choice) {
                    return Err(format!(
                        "answer '{key}' chose '{choice}', which was not asked"
                    ));
                }
                json!({"choice": choice, "p": choice_p(a, names, key)?})
            }
            Kind::YesNo => {
                let yes = a
                    .get("noul")
                    .and_then(prob)
                    .ok_or_else(|| format!("answer '{key}' carries no probability"))?;
                json!({"yes": yes})
            }
            Kind::Scale(levels) => {
                let value = a
                    .get("score")
                    .and_then(Value::as_f64)
                    .filter(|f| f.is_finite())
                    .ok_or_else(|| format!("answer '{key}' carries no score"))?;
                json!({"value": value, "p": scale_p(a, levels, key)?})
            }
        };
        out.insert(key.clone(), normalized);
    }
    if out.is_empty() {
        return Err("no question was answered".into());
    }
    missing.sort();
    let usage = resp.get("usage");
    let count = |k: &str| usage.and_then(|u| u.get(k)).and_then(Value::as_u64);
    Ok(Decided {
        answers: out,
        missing,
        model: resp
            .get("model")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        response_id: resp
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        tokens_prompt: count("input_tokens"),
        tokens_completion: count("output_tokens"),
        cost: usage
            .and_then(|u| u.get("cost"))
            .and_then(Value::as_f64)
            .filter(|c| c.is_finite()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three() -> Value {
        json!({
            "state": "The user said: Will it rain tomorrow?",
            "questions": {
                "topic": {"kind": "choice", "instructions": "Which topic?",
                          "options": {"weather": "Weather", "none": "Nothing"}},
                "wants": {"kind": "yes_no", "instructions": "Show it?"},
                "urgency": {"kind": "scale", "instructions": "How urgent?",
                            "levels": ["low", "medium", "high"]}
            }
        })
    }

    #[test]
    fn the_three_kinds_map_onto_the_wire() {
        let asked = parse_decide(Some(&three())).unwrap();
        let body = build_request("vendor/decider", &asked);
        assert_eq!(body["model"], "vendor/decider");
        assert_eq!(body["questions"]["topic"]["type"], "choice");
        assert_eq!(body["questions"]["topic"]["criteria"]["weather"], "Weather");
        assert_eq!(
            body["questions"]["wants"],
            json!({"type": "noul", "instructions": "Show it?"})
        );
        assert_eq!(body["questions"]["urgency"]["type"], "score");
        assert_eq!(
            body["questions"]["urgency"]["criteria"],
            json!(["low", "medium", "high"])
        );
    }

    #[test]
    fn the_bounds_are_refused() {
        let mut too_many = three();
        let mut qs = Map::new();
        for i in 0..=MAX_QUESTIONS {
            qs.insert(
                format!("q{i}"),
                json!({"kind": "yes_no", "instructions": "x"}),
            );
        }
        too_many["questions"] = Value::Object(qs);
        assert!(
            parse_decide(Some(&too_many))
                .unwrap_err()
                .contains("questions")
        );

        let mut opts = Map::new();
        for i in 0..=MAX_OPTIONS {
            opts.insert(format!("o{i}"), json!("x"));
        }
        let mut wide = three();
        wide["questions"]["topic"]["options"] = Value::Object(opts);
        assert!(parse_decide(Some(&wide)).unwrap_err().contains("options"));

        let mut one = three();
        one["questions"]["urgency"]["levels"] = json!(["only"]);
        assert!(parse_decide(Some(&one)).unwrap_err().contains("levels"));

        let mut bad_key = three();
        bad_key["questions"] = json!({"Topic!": {"kind": "yes_no", "instructions": "x"}});
        assert!(parse_decide(Some(&bad_key)).unwrap_err().contains("key"));

        let mut bad_kind = three();
        bad_kind["questions"]["wants"]["kind"] = json!("noul");
        assert!(parse_decide(Some(&bad_kind)).unwrap_err().contains("kind"));

        assert!(parse_decide(None).is_err());
        assert!(parse_decide(Some(&json!({"questions": {}}))).is_err());
    }

    /// GH #1085 lock: a state over the old fixed 128 KiB is translated whole;
    /// the window, not the translation, decides whether it may be sent.
    #[test]
    fn a_state_over_the_old_fixed_bound_is_built_whole() {
        let mut big = three();
        let state = "x".repeat(200 * 1024);
        big["state"] = json!(state);
        let asked = parse_decide(Some(&big)).unwrap();
        let body = build_request("m", &asked);
        assert_eq!(body["state"], json!(state));
    }

    #[test]
    fn the_answer_is_normalized_per_kind() {
        let asked = parse_decide(Some(&three())).unwrap();
        let resp = json!({
            "model": "vendor/decider-1",
            "id": "gen-1",
            "answers": {
                "topic": {"type": "choice", "choice": "weather",
                          "probabilities": {"weather": 0.9, "none": 0.1}, "confidence": 0.8},
                "wants": {"type": "noul", "noul": 0.75},
                "urgency": {"type": "score", "score": 0.2,
                            "legend": {"0": "low", "1": "medium", "2": "high"},
                            "probabilities": {"0": 0.8, "1": 0.2, "2": 0}}
            },
            "usage": {"input_tokens": 300, "output_tokens": 20, "cost": 0.00001}
        });
        let d = parse_response(&resp, &asked).unwrap();
        assert_eq!(
            d.answers["topic"],
            json!({"choice": "weather", "p": {"weather": 0.9, "none": 0.1}})
        );
        assert_eq!(d.answers["wants"], json!({"yes": 0.75}));
        assert_eq!(
            d.answers["urgency"],
            json!({"value": 0.2, "p": {"low": 0.8, "medium": 0.2, "high": 0.0}})
        );
        assert_eq!(d.model.as_deref(), Some("vendor/decider-1"));
        assert_eq!(d.response_id.as_deref(), Some("gen-1"));
        assert_eq!(d.tokens_prompt, Some(300));
        assert_eq!(d.tokens_completion, Some(20));
        assert_eq!(d.cost, Some(0.00001));
    }

    /// GH #977: an omitted answer is named in `missing`, the rest stands.
    #[test]
    fn a_missing_answer_is_named_and_the_rest_stands() {
        let asked = parse_decide(Some(&three())).unwrap();
        let part = json!({"answers": {"wants": {"noul": 0.5}}});
        let d = parse_response(&part, &asked).unwrap();
        assert_eq!(Value::Object(d.answers), json!({"wants": {"yes": 0.5}}));
        assert_eq!(d.missing, vec!["topic".to_string(), "urgency".to_string()]);
        let whole = json!({"answers": {
            "topic": {"choice": "none", "probabilities": {"none": 1}},
            "wants": {"noul": 0.5},
            "urgency": {"score": 1, "probabilities": {"1": 1}}}});
        assert!(parse_response(&whole, &asked).unwrap().missing.is_empty());
    }

    /// GH #977: without a single answer there is no decision.
    #[test]
    fn no_answer_at_all_is_no_decision() {
        let asked = parse_decide(Some(&three())).unwrap();
        assert!(
            parse_response(&json!({"answers": {}}), &asked)
                .unwrap_err()
                .contains("no question was answered")
        );
        // An answer under a key nobody asked is no answer to anything asked.
        let stray = json!({"answers": {"other": {"noul": 0.5}}});
        assert!(parse_response(&stray, &asked).is_err());
    }

    #[test]
    fn an_unasked_choice_is_no_decision() {
        let asked = parse_decide(Some(&three())).unwrap();
        let unasked = json!({"answers": {
            "topic": {"choice": "sport", "probabilities": {"sport": 1}},
            "wants": {"noul": 0.5},
            "urgency": {"score": 1, "probabilities": {"1": 1}}}});
        assert!(
            parse_response(&unasked, &asked)
                .unwrap_err()
                .contains("not asked")
        );
        assert!(parse_response(&json!({"choices": []}), &asked).is_err());
    }
}
