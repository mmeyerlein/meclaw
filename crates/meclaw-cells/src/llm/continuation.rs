//! GH #1037: an answer cut on `length` is an event, never a silent cut.
//!
//! Measured before: the memory hive's dream step ran with `max_tokens: 4096`,
//! its close step with 8 192, and a long consolidation stopped on `length`.
//! The glue reads the answer as JSON, so a cut verdict was either refused as
//! "not JSON" or -- for prose -- kept as if whole. Nothing said the model had
//! more to give.
//!
//! The cell owns the fix because only the cell holds the request: a `code`
//! glue cell is stateless and would have to rebuild the prompt, park the
//! partial text in a store and correlate the second answer, once per glue
//! script (three call sites in two scripts). Here it is one loop on the one
//! wire every shipped consolidation cell speaks (chat completions): the cut
//! text goes back as the assistant's turn, a user turn asks to go on, and the
//! parts are joined into ONE answer with the usage of all calls summed -- the
//! glue downstream sees a whole verdict and books what it really cost.

use crate::llm::translate::TranslatedResponse;
use meclaw_core::serde_json::{Value, json};

/// The `tracing` target of the length lines -- one place to filter them.
pub(crate) const LENGTH_LOG_TARGET: &str = "meclaw::llm::length";

/// The user turn of a continuation call.
pub(crate) const CONTINUE_PROMPT: &str = "Your previous answer was cut off by the output \
limit. Continue exactly where it stopped: no repetition, no preamble, no summary of what \
came before.";

fn texts(t: &TranslatedResponse) -> String {
    t.assistant_turn
        .iter()
        .filter(|turn| turn.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|turn| turn.get("text").and_then(Value::as_str))
        .collect()
}

/// The text of a cut answer a continuation can extend: a `length` finish
/// with text and without a tool call. A cut tool call is the caller's
/// business (GH #842) and is never continued.
pub(crate) fn continuable_text(t: &TranslatedResponse) -> Option<String> {
    if t.finish_reason != "length"
        || t.assistant_turn
            .iter()
            .any(|turn| turn.get("type").and_then(Value::as_str) != Some("text"))
    {
        return None;
    }
    let text = texts(t);
    (!text.is_empty()).then_some(text)
}

/// The request of a continuation: the original request, the text so far as
/// the assistant's turn, and the user turn that asks to go on.
pub(crate) fn continuation_request(base: &Value, so_far: &str) -> Value {
    let mut req = base.clone();
    if let Some(msgs) = req.get_mut("messages").and_then(Value::as_array_mut) {
        msgs.push(json!({"role": "assistant", "content": so_far}));
        msgs.push(json!({"role": "user", "content": CONTINUE_PROMPT}));
    }
    req
}

fn sum(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, None) => None,
        _ => Some(a.unwrap_or(0) + b.unwrap_or(0)),
    }
}

/// One answer out of the text so far and its continuation: the text joined,
/// every usage figure summed (a figure neither call reported stays absent),
/// reason, model and response id of the last call.
pub(crate) fn joined(
    so_far: &str,
    prev: &TranslatedResponse,
    next: TranslatedResponse,
) -> TranslatedResponse {
    let mut turns: Vec<Value> = next
        .assistant_turn
        .iter()
        .filter(|turn| turn.get("type").and_then(Value::as_str) != Some("text"))
        .cloned()
        .collect();
    turns.push(json!({
        "origin": "assistant",
        "type": "text",
        "text": format!("{so_far}{}", texts(&next)),
    }));
    TranslatedResponse {
        assistant_turn: turns,
        finish_reason: next.finish_reason,
        tokens_prompt: sum(prev.tokens_prompt, next.tokens_prompt),
        tokens_completion: sum(prev.tokens_completion, next.tokens_completion),
        tokens_cached: sum(prev.tokens_cached, next.tokens_cached),
        tokens_cache_write: sum(prev.tokens_cache_write, next.tokens_cache_write),
        cost: match (prev.cost, next.cost) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
        },
        model: next.model,
        response_id: next.response_id,
    }
}

/// The line written before each continuation call.
pub(crate) fn continuing_line(
    path: &str,
    round: u32,
    max: u32,
    budget: u32,
    chars: usize,
) -> String {
    format!(
        "llm: length {path} answer cut at max_tokens={budget} after {chars} chars; \
         continuation {round}/{max}"
    )
}

/// The line of an answer that stays cut: no continuation allowed, the budget
/// spent, or a continuation call that failed. The partial text is emitted
/// with `finish_reason` `length`, so a reader can tell it was cut.
pub(crate) fn cut_line(path: &str, budget: u32, rounds: u32, why: &str) -> String {
    format!(
        "llm: length {path} answer stays cut at max_tokens={budget} after {rounds} \
         continuation(s): {why}"
    )
}

/// A continuation is not started with less than this left before the
/// backstop: a call that cannot answer only delays the cut answer.
pub(crate) const MIN_CONTINUATION_MS: u64 = 1_000;

/// The timeout of the next continuation call, or why there is none.
///
/// A continuation runs inside the same message as the first call, so the
/// cell's `message_timeout` backstop covers all of them together. Each call
/// with a fresh `external_timeout_ms` (memory hive: 110 s x 3 against a
/// 180 s backstop) let the backstop fire mid-answer and drop the whole
/// answer -- worse than a cut one. The deadline keeps the margin the
/// backstop rule reserves above `external_timeout_ms` ([`backstop_shortfall`]):
/// `backstop - max(10 s, external / 10)` after the message started. A call
/// gets what is left of that, at most `external`; under
/// [`MIN_CONTINUATION_MS`] left, the answer stays cut. No backstop, no
/// deadline.
///
/// [`backstop_shortfall`]: crate::llm::params::backstop_shortfall
pub(crate) fn continuation_timeout(
    external: std::time::Duration,
    backstop_ms: Option<u64>,
    elapsed_ms: u64,
) -> Result<std::time::Duration, String> {
    use crate::llm::params::{BACKSTOP_MARGIN_DIVISOR, BACKSTOP_MARGIN_FLOOR_MS};
    let Some(backstop) = backstop_ms else {
        return Ok(external);
    };
    let ext_ms = external.as_millis() as u64;
    let margin = BACKSTOP_MARGIN_FLOOR_MS.max(ext_ms / BACKSTOP_MARGIN_DIVISOR);
    let left = backstop.saturating_sub(margin).saturating_sub(elapsed_ms);
    if left < MIN_CONTINUATION_MS {
        return Err(format!(
            "backstop: {left} ms left of message_timeout {backstop} ms after {elapsed_ms} ms"
        ));
    }
    Ok(external.min(std::time::Duration::from_millis(left)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_continuation_gets_only_the_time_left_before_the_backstop() {
        let ext = std::time::Duration::from_millis(110_000);
        assert_eq!(
            continuation_timeout(ext, None, 500_000),
            Ok(ext),
            "no backstop"
        );
        // 180 s backstop, 11 s margin: 169 s deadline.
        assert_eq!(continuation_timeout(ext, Some(180_000), 30_000), Ok(ext));
        assert_eq!(
            continuation_timeout(ext, Some(180_000), 100_000),
            Ok(std::time::Duration::from_millis(69_000))
        );
        let cut = continuation_timeout(ext, Some(180_000), 168_500).unwrap_err();
        assert!(cut.starts_with("backstop: 500 ms left"), "{cut}");
        assert!(
            continuation_timeout(ext, Some(5_000), 0).is_err(),
            "below the margin"
        );
    }

    fn t(turns: Value, finish: &str, prompt: u64, completion: u64) -> TranslatedResponse {
        TranslatedResponse {
            assistant_turn: turns.as_array().cloned().unwrap_or_default(),
            finish_reason: finish.to_string(),
            tokens_prompt: Some(prompt),
            tokens_completion: Some(completion),
            tokens_cached: None,
            tokens_cache_write: None,
            cost: Some(0.5),
            model: "m".into(),
            response_id: format!("r{completion}"),
        }
    }

    #[test]
    fn only_a_text_answer_cut_on_length_is_continued() {
        let text = json!([{"origin": "assistant", "type": "text", "text": "a"}]);
        assert_eq!(
            continuable_text(&t(text.clone(), "length", 1, 1)).as_deref(),
            Some("a")
        );
        assert_eq!(continuable_text(&t(text, "stop", 1, 1)), None);
        let call = json!([{"origin": "assistant", "type": "tool_call", "id": "c", "text": "{}"}]);
        assert_eq!(continuable_text(&t(call, "length", 1, 1)), None);
    }

    #[test]
    fn the_continuation_carries_the_text_so_far_and_the_request_to_go_on() {
        let base = json!({"model": "m", "messages": [{"role": "user", "content": "q"}]});
        let req = continuation_request(&base, "part one");
        let msgs = req["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1], json!({"role": "assistant", "content": "part one"}));
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"], CONTINUE_PROMPT);
    }

    #[test]
    fn the_parts_join_into_one_answer_with_the_usage_summed() {
        let a = t(
            json!([{"origin": "assistant", "type": "text", "text": "one "}]),
            "length",
            10,
            5,
        );
        let b = t(
            json!([{"origin": "assistant", "type": "text", "text": "two"}]),
            "stop",
            15,
            3,
        );
        let j = joined("one ", &a, b);
        assert_eq!(
            j.assistant_turn,
            vec![json!({"origin": "assistant", "type": "text", "text": "one two"})]
        );
        assert_eq!(j.finish_reason, "stop");
        assert_eq!(j.tokens_prompt, Some(25));
        assert_eq!(j.tokens_completion, Some(8));
        assert_eq!(j.tokens_cached, None, "never a measured zero");
        assert_eq!(j.cost, Some(1.0));
        assert_eq!(j.response_id, "r3");
    }
}
