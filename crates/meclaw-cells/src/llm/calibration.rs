//! GH #1558: the calibration of a decider -- the curves that map the raw
//! probabilities of one model onto the one scale every application reads.
//!
//! A raw probability of a decisions model is the model's own number: two
//! models answering the same question with 0.7 do not mean the same thing,
//! and a threshold an application set for one of them is wrong for the other.
//! The catalogue row of each decider carries `decision_calibration`, a package
//! key the registry pushes with the model; the `decisions` lane maps every
//! answer through it, so the threshold of an application holds whichever
//! decider answers, and a decider changes without a rebirth.
//!
//! The form is fixed:
//!
//! ```text
//! {"yes_no": {"default": [[0, 0], ..., [1, 1]], "by_prefix": {"needs.": [...]}},
//!  "choice": {"default": [...], "by_prefix": {...}}}
//! ```
//!
//! A curve is a list of knots `[raw, p]`: raw rises strictly from exactly 0
//! to exactly 1, p never falls and stays inside [0, 1]; between two knots the
//! mapping is linear. A curve that never falls never reverses the order of two
//! answers, so the service's argmax stays an argmax; a flat stretch can make
//! two different raw values equal (a tie, never a reversal). `default` and
//! `by_prefix` are each optional, but a kind carries at least one curve; the
//! longest prefix of the question's cell key wins, else a DECLARED `default`,
//! else the question has no curve.
//!
//! GH #1561: calibration is judged per question. A question without a curve
//! is never sent and never answered -- not raw, not on another curve (the
//! live case: a decider whose `choice` curves named `path` answered `move`
//! silently on `choice.default`, a curve fitted to another question). Its
//! calibrated neighbours are asked and answered, and the answer names it in
//! `decision.uncalibrated`. Only a missing calibration, or a call in which no
//! question has a curve, is `decision_uncalibrated` before any request.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The param, as every refusal names it.
pub const PARAM: &str = "decision_calibration";

/// The kinds of question the form carries a curve for.
const KINDS: &[&str] = &["yes_no", "choice"];

/// One curve: knots `[raw, p]`, checked by [`checked`].
pub type Curve = Vec<[f64; 2]>;

/// The curves of one kind of question.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct KindCurves {
    /// The curve of every question no prefix names, when the kind declares
    /// one (GH #1561: optional; without it such a question is uncalibrated).
    /// Skipped when absent, so a form WITH a default serializes -- and stamps
    /// ([`DecisionCalibration::sha12`]) -- byte for byte as under 0.89.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Curve>,
    /// Curves by prefix of the question's cell key; the longest match wins.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub by_prefix: BTreeMap<String, Curve>,
}

/// GH #1558: the calibration of one decisions model (the package key
/// `decision_calibration`).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct DecisionCalibration {
    /// The curve(s) of the yes-probability of a `yes_no` question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yes_no: Option<KindCurves>,
    /// The curve(s) of each option probability of a `choice` question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<KindCurves>,
}

impl DecisionCalibration {
    /// The curve that maps the answers of question `key` of `kind`
    /// (`"yes_no"` or `"choice"`): the longest prefix of `key` under
    /// `by_prefix`, else the declared `default`. `None` when neither names
    /// the question -- never another question's curve (GH #1561).
    pub fn curve_for(&self, kind: &str, key: &str) -> Option<&Curve> {
        let curves = match kind {
            "yes_no" => self.yes_no.as_ref(),
            "choice" => self.choice.as_ref(),
            _ => None,
        }?;
        curves
            .by_prefix
            .iter()
            .filter(|(prefix, _)| key.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, curve)| curve)
            .or(curves.default.as_ref())
    }

    /// The first 12 hex digits of the sha256 of the calibration's canonical
    /// JSON (fixed field order, prefixes sorted): the stamp that names which
    /// mapping applied to an answer.
    pub fn sha12(&self) -> String {
        let canonical = serde_json::to_string(self).unwrap_or_default();
        let digest = Sha256::digest(canonical.as_bytes());
        digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
    }
}

/// GH #1558, split per question by GH #1561: the calibration of the lane and
/// the asked questions it has NO curve for, as `(cell key, kind)` in the
/// sorted order of the cell keys -- or the detail of a `decision_uncalibrated`
/// answer. Checked before any request: an answer the lane cannot map is not
/// worth a paid call. Without a calibration the detail names the model; when
/// no asked question has a curve it names the kind and the first question (a
/// `scale` question never has one -- the form carries no curve for it). Any
/// other split leaves the uncalibrated questions to the caller, which asks
/// only the rest and names these in `decision.uncalibrated`.
#[allow(clippy::type_complexity)]
pub(crate) fn covering<'a, 'b>(
    model: &str,
    calibration: Option<&'a DecisionCalibration>,
    asked: &'b crate::llm::translate_decisions::Asked,
) -> Result<(&'a DecisionCalibration, Vec<(&'b str, &'static str)>), String> {
    let Some(cal) = calibration else {
        return Err(format!(
            "model '{model}' has no {PARAM} in its package: its probabilities have no common scale"
        ));
    };
    let uncalibrated: Vec<(&str, &str)> = asked
        .kinds()
        .filter(|(key, kind)| cal.curve_for(kind, key).is_none())
        .collect();
    if uncalibrated.len() == asked.kinds().count() {
        let (key, kind) = uncalibrated[0];
        return Err(format!(
            "model '{model}' has no {PARAM} curve for '{kind}' (question '{key}') nor for any other question asked"
        ));
    }
    Ok((cal, uncalibrated))
}

/// GH #1558: `answers` mapped through `calibration` -- the yes-probability of
/// a `yes_no` answer and every option probability of a `choice` answer; the
/// service's `choice` stays (the curves never fall, so its argmax holds).
/// Call only with the calibration [`covering`] returned; an answer whose kind
/// has no curve is passed on unchanged, which `covering` rules out.
pub(crate) fn calibrated(
    calibration: &DecisionCalibration,
    asked: &crate::llm::translate_decisions::Asked,
    answers: &serde_json::Map<String, Value>,
) -> serde_json::Map<String, Value> {
    let mut out = answers.clone();
    for (key, kind) in asked.kinds() {
        let (Some(curve), Some(answer)) = (calibration.curve_for(kind, key), out.get_mut(key))
        else {
            continue;
        };
        match kind {
            "yes_no" => {
                if let Some(yes) = answer.get("yes").and_then(Value::as_f64) {
                    answer["yes"] = Value::from(map(curve, yes));
                }
            }
            "choice" => {
                if let Some(p) = answer.get_mut("p").and_then(Value::as_object_mut) {
                    for v in p.values_mut() {
                        if let Some(raw) = v.as_f64() {
                            *v = Value::from(map(curve, raw));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The value of `raw` on `curve`: exact at a knot, linear between two. A raw
/// outside [0, 1] (the lane reads only probabilities, so never) is clamped.
/// Private (review jevr_o1-r2, Minor 3): its fallback for an empty curve passes
/// `raw` on unchanged, which only a curve [`checked`] refused could reach.
fn map(curve: &Curve, raw: f64) -> f64 {
    let r = raw.clamp(0.0, 1.0);
    for pair in curve.windows(2) {
        let ([x0, y0], [x1, y1]) = (pair[0], pair[1]);
        if r <= x1 {
            if r <= x0 {
                return y0;
            }
            return y0 + (r - x0) / (x1 - x0) * (y1 - y0);
        }
    }
    curve.last().map(|k| k[1]).unwrap_or(r)
}

/// GH #1558: `decision_calibration` as the cell takes it, checked by name
/// before serde sees it: an object naming at least one of the kinds, each
/// kind an object with an optional `default` curve and an optional
/// `by_prefix` object of non-empty prefixes -- at least one curve between
/// them (GH #1561) --, every curve in form. `null` is no calibration and
/// stays `null`. Every refusal starts with the param's name.
pub fn checked(value: &Value) -> Result<Value, String> {
    let refuse = |why: String| format!("{PARAM} {why}");
    if value.is_null() {
        return Ok(Value::Null);
    }
    let obj = value
        .as_object()
        .ok_or_else(|| refuse("must be an object of curves by kind".into()))?;
    if obj.is_empty() {
        return Err(refuse(format!(
            "must name at least one of {}",
            KINDS.join(", ")
        )));
    }
    for (kind, curves) in obj {
        if !KINDS.contains(&kind.as_str()) {
            return Err(refuse(format!(
                "names the kind '{kind}', which has no curve (only {})",
                KINDS.join(", ")
            )));
        }
        let c = curves
            .as_object()
            .ok_or_else(|| refuse(format!("{kind} must be an object")))?;
        if let Some(k) = c.keys().find(|k| *k != "default" && *k != "by_prefix") {
            return Err(refuse(format!("{kind} names an unknown key '{k}'")));
        }
        let by_prefix_empty = c
            .get("by_prefix")
            .and_then(Value::as_object)
            .is_none_or(|by| by.is_empty());
        match c.get("default") {
            Some(default) => {
                curve_ok(default).map_err(|why| refuse(format!("{kind}.default {why}")))?
            }
            None if by_prefix_empty => {
                return Err(refuse(format!(
                    "{kind} needs at least one curve (a default or a by_prefix curve)"
                )));
            }
            None => {}
        }
        if let Some(by) = c.get("by_prefix") {
            let by = by
                .as_object()
                .ok_or_else(|| refuse(format!("{kind}.by_prefix must be an object")))?;
            for (prefix, curve) in by {
                if prefix.is_empty() {
                    return Err(refuse(format!(
                        "{kind}.by_prefix names an empty prefix (that is the default)"
                    )));
                }
                curve_ok(curve)
                    .map_err(|why| refuse(format!("{kind}.by_prefix '{prefix}' {why}")))?;
            }
        }
    }
    Ok(value.clone())
}

/// One curve in form, or why not.
fn curve_ok(value: &Value) -> Result<(), String> {
    let knots = value.as_array().ok_or("must be a list of knots [raw, p]")?;
    if knots.len() < 2 {
        return Err("needs at least two knots, raw 0 and raw 1".into());
    }
    let mut last: Option<(f64, f64)> = None;
    for knot in knots {
        let pair = knot
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or("must be a list of knots [raw, p]")?;
        let num = |v: &Value| v.as_f64().filter(|f| f.is_finite());
        let (Some(raw), Some(p)) = (num(&pair[0]), num(&pair[1])) else {
            return Err("must be a list of knots of two numbers [raw, p]".into());
        };
        if !(0.0..=1.0).contains(&p) {
            return Err(format!("has p {p} outside [0, 1]"));
        }
        match last {
            None if raw != 0.0 => return Err(format!("must start at raw 0, not {raw}")),
            Some((r0, _)) if raw <= r0 => {
                return Err(format!("must rise strictly in raw ({r0} then {raw})"));
            }
            Some((_, p0)) if p < p0 => {
                return Err(format!("must not fall in p ({p0} then {p})"));
            }
            _ => {}
        }
        last = Some((raw, p));
    }
    match last {
        Some((1.0, _)) => Ok(()),
        Some((raw, _)) => Err(format!("must end at raw 1, not {raw}")),
        None => Err("needs at least two knots, raw 0 and raw 1".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calib(v: Value) -> DecisionCalibration {
        serde_json::from_value(checked(&v).expect("in form")).expect("parses")
    }

    #[test]
    fn the_mapping_is_exact_at_a_knot_and_linear_between() {
        let c: Curve = vec![[0.0, 0.1], [0.5, 0.5], [1.0, 0.9]];
        assert_eq!(map(&c, 0.0), 0.1);
        assert_eq!(map(&c, 0.5), 0.5);
        assert_eq!(map(&c, 1.0), 0.9);
        assert!((map(&c, 0.25) - 0.3).abs() < 1e-12);
        assert!((map(&c, 0.75) - 0.7).abs() < 1e-12);
    }

    #[test]
    fn the_longest_prefix_wins_over_the_default() {
        let c = calib(json!({"yes_no": {"default": [[0, 0], [1, 1]],
            "by_prefix": {"needs.": [[0, 0], [1, 0.5]], "needs.memory": [[0, 0], [1, 0.2]]}}}));
        let at = |key: &str| map(c.curve_for("yes_no", key).unwrap(), 1.0);
        assert_eq!(at("needs.memory"), 0.2);
        assert_eq!(at("needs.tool"), 0.5);
        assert_eq!(at("wants"), 1.0);
        assert!(
            c.curve_for("choice", "path").is_none(),
            "no curve, no default"
        );
        assert!(c.curve_for("scale", "urgency").is_none());
    }

    #[test]
    fn the_stamp_follows_the_curves_not_the_key_order() {
        let a = calib(json!({"choice": {"default": [[0, 0], [1, 1]]},
                             "yes_no": {"default": [[0, 0], [1, 1]]}}));
        let b = calib(json!({"yes_no": {"default": [[0, 0], [1, 1]]},
                             "choice": {"default": [[0, 0], [1, 1]]}}));
        let c = calib(json!({"yes_no": {"default": [[0, 0], [1, 0.9]]},
                             "choice": {"default": [[0, 0], [1, 1]]}}));
        assert_eq!(a.sha12(), b.sha12());
        assert_ne!(a.sha12(), c.sha12());
        assert_eq!(a.sha12().len(), 12);
    }

    /// GH #1561: a kind may carry its curves by prefix alone; it must carry
    /// at least one curve, else the kind names nothing to map with.
    #[test]
    fn a_kind_without_a_default_is_in_form_but_not_an_empty_one() {
        let c = calib(json!({"choice": {"by_prefix": {"path": [[0, 0], [1, 1]]}}}));
        assert!(c.curve_for("choice", "path").is_some());
        assert!(
            c.curve_for("choice", "move").is_none(),
            "no own curve and no declared default: no curve at all"
        );
        for bad in [json!({"choice": {}}), json!({"choice": {"by_prefix": {}}})] {
            let err = checked(&bad).expect_err("a kind without any curve");
            assert!(err.starts_with(PARAM), "{err}");
        }
    }

    /// GH #1561: `default` became optional, but a form that declares one keeps
    /// the stamp it had under 0.89.0 byte for byte (the literal is the sha12 of
    /// this form as 0.89.0 serialized it) -- every journal line before the fix
    /// still names the same calibration.
    #[test]
    fn a_form_with_a_default_keeps_its_stamp_of_0_89_0() {
        let c = calib(json!({
            "yes_no": {"default": [[0, 0], [0.5, 0.2], [1, 1]],
                       "by_prefix": {"needs.": [[0, 0], [1, 0.5]]}},
            "choice": {"default": [[0, 0.1], [1, 0.9]]}}));
        assert_eq!(c.sha12(), "05f980f2019b");
    }
}
