//! Response sanitation at the translate boundary: provider-internal annotation
//! markers never reach UBF (GH #569).
//!
//! Some providers decorate answer text with inline citation markers built from
//! Private-Use-Area codepoints. The shape observed on a live turn (hexdump of
//! the reported text): `U+E200` `cite` `U+E202` `turn0search0` `U+E201`. The PUA
//! codepoints render as invisible glyphs or boxes, the enclosed `cite` /
//! `turnNsearchM` tokens render as literal junk, and the whole span refers to
//! the provider's own tool-round numbering — it means nothing outside the
//! provider. No provider-internal token may reach a person, so the response
//! translation drops these spans before the text enters an assistant turn.

use std::ops::RangeInclusive;

/// The Unicode Basic Multilingual Plane's Private Use Area.
const PRIVATE_USE_AREA: RangeInclusive<char> = '\u{E000}'..='\u{F8FF}';

/// Opens an annotation span.
const MARKER_START: char = '\u{E200}';

/// Closes an annotation span.
const MARKER_END: char = '\u{E201}';

/// True for a codepoint in the Private Use Area.
fn is_private_use(c: char) -> bool {
    PRIVATE_USE_AREA.contains(&c)
}

/// Strip provider-internal annotation markers from response text (GH #569).
///
/// Rules, in the order the scan applies them:
///
/// * A span from [`MARKER_START`] up to and including the next [`MARKER_END`]
///   falls **with its content** — the enclosed `cite` / `turnNsearchM` tokens
///   and any separator codepoint (`U+E202`) inside it.
/// * **Truncation:** a [`MARKER_START`] whose closing codepoint never arrives
///   (a cut-off response) takes **the rest of the text** with it. Everything
///   after an opened marker is marker content by construction — there is no
///   answer text behind it to save, and keeping it would leak exactly the
///   literal `citeturn0search0` junk this function exists to remove.
/// * Any other Private-Use-Area codepoint is a stray and falls **on its own**;
///   the text around it is untouched.
/// * Everything else is returned byte-identical: no trimming, no whitespace
///   normalisation. A space in front of a dropped marker stays — that is honest
///   and visibly harmless.
///
/// **Limit.** No Private-Use-Area codepoint of any provider survives this, but
/// only the observed `U+E200`/`U+E201` pair carries its enclosed text away with
/// it. A different provider's marker protocol would leave its ASCII payload
/// behind until its opener pair is measured and added — a heuristic that eats
/// everything between any two PUA codepoints would swallow real answer text.
pub(crate) fn strip_provider_annotations(text: &str) -> String {
    if !text.contains(is_private_use) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == MARKER_START {
            // Consume the span, content included, up to and including its
            // closing codepoint. If none arrives, the iterator runs dry and the
            // rest of the text falls with the marker.
            for inner in chars.by_ref() {
                if inner == MARKER_END {
                    break;
                }
            }
        } else if !is_private_use(c) {
            out.push(c);
        }
        // A stray Private-Use-Area codepoint outside a span falls on its own.
    }
    out
}

// ---------------------------------------------------------------------------
// GH #1260 (OR-WG-7): a tool call leaked as raw harmony tokens in `content`.
// ---------------------------------------------------------------------------

use crate::llm::translate::{TranslateError, TranslatedResponse};
use meclaw_core::serde_json::{Value, json};

/// Harmony special tokens (the gpt-oss chat format). Only the ones that
/// frame a message; any other `<|...|>` text is left alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HarmonyToken {
    Start,
    Channel,
    Message,
    Constrain,
    End,
    Call,
    Return,
}

const HARMONY_TOKENS: &[(&str, HarmonyToken)] = &[
    ("<|start|>", HarmonyToken::Start),
    ("<|channel|>", HarmonyToken::Channel),
    ("<|message|>", HarmonyToken::Message),
    ("<|constrain|>", HarmonyToken::Constrain),
    ("<|end|>", HarmonyToken::End),
    ("<|call|>", HarmonyToken::Call),
    ("<|return|>", HarmonyToken::Return),
];

/// True when `text` carries a harmony message frame. A `<|channel|>` or an
/// opened assistant message is the pattern; a lone `<|` or an unrelated
/// `<|endoftext|>` in prose is not, and such text stays byte-identical.
fn is_harmony(text: &str) -> bool {
    text.contains("<|channel|>") || text.contains("<|start|>assistant")
}

enum Segment<'a> {
    Token(HarmonyToken),
    Text(&'a str),
}

fn tokenize(text: &str) -> Vec<Segment<'_>> {
    tokenize_counted(text, &mut 0)
}

/// [`tokenize`], adding to `steps` the bytes its scan touches (the
/// complexity probe of the tests).
///
/// One pass: find the next `<|`, compare the seven literals at that spot,
/// move on. Review I-1 (GH #1260): the first form searched every literal
/// over the whole rest per token, quadratic -- 40 000 tokens of model output
/// held a worker 10-13 s inside `handle()`; this scan touches each byte a
/// bounded number of times.
fn tokenize_counted<'a>(text: &'a str, steps: &mut usize) -> Vec<Segment<'a>> {
    let mut out = Vec::new();
    let mut plain_from = 0;
    let mut pos = 0;
    while let Some(off) = text[pos..].find("<|") {
        let at = pos + off;
        *steps += off + 2;
        let rest = &text[at..];
        let hit = HARMONY_TOKENS.iter().find(|(lit, _)| {
            *steps += 1;
            rest.starts_with(lit)
        });
        match hit {
            Some((lit, tok)) => {
                if at > plain_from {
                    out.push(Segment::Text(&text[plain_from..at]));
                }
                out.push(Segment::Token(*tok));
                pos = at + lit.len();
                plain_from = pos;
            }
            // `<|` is ASCII, so `at + 2` is a char boundary.
            None => pos = at + 2,
        }
    }
    *steps += text.len() - pos;
    if plain_from < text.len() {
        out.push(Segment::Text(&text[plain_from..]));
    }
    out
}

/// One harmony message: `<|start|>{role}<|channel|>{channel}[ to={recipient}]
/// [<|constrain|>{format}]<|message|>{body}<|end|>|<|call|>|<|return|>`.
/// The measured upstream left out `<|message|>` behind the constraint
/// (`<|constrain|>json{...}<|call|>`), so the format word may run straight
/// into the body.
#[derive(Default)]
struct HarmonyMessage {
    role: String,
    channel: String,
    constrain: String,
    body: String,
    /// A `<|channel|>` opened the channel slot.
    saw_channel: bool,
    /// The message reached its body or a closing token.
    framed: bool,
    /// A closing token (`<|end|>`, `<|call|>`, `<|return|>`) ended it.
    closed: bool,
}

impl HarmonyMessage {
    fn recipient(&self) -> Option<&str> {
        self.role
            .split_whitespace()
            .chain(self.channel.split_whitespace())
            .find_map(|w| w.strip_prefix("to="))
    }
    fn channel_name(&self) -> &str {
        self.channel.split_whitespace().next().unwrap_or("")
    }
    /// A harmony message for real: a known channel (or none at all) and a
    /// body or a closing token. Prose that only names a token is not
    /// (review M-4): the whole text then stays as it came.
    fn is_complete(&self) -> bool {
        let channel_ok = if self.saw_channel {
            HARMONY_CHANNELS.contains(&self.channel_name())
        } else {
            true
        };
        channel_ok && self.framed
    }
    /// The first word of the role: `assistant`, `user`, `functions.<name>` ...
    fn author(&self) -> &str {
        self.role.split_whitespace().next().unwrap_or("")
    }
    /// The body never reaches a person: the `analysis` trace, or a message
    /// another author wrote (the rules of [`read_harmony`]).
    fn body_falls(&self) -> bool {
        !self.author().starts_with("assistant")
            || (self.recipient().is_none() && self.channel_name() == "analysis")
    }
}

/// The channels of the harmony format.
const HARMONY_CHANNELS: &[&str] = &["analysis", "commentary", "final"];

/// Where the next text segment of the header goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Outside,
    Role,
    Channel,
    Constrain,
    Body,
}

enum Part {
    Plain(String),
    Message(HarmonyMessage),
}

fn split_messages(text: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut cur: Option<HarmonyMessage> = None;
    let mut slot = Slot::Outside;
    let close = |cur: &mut Option<HarmonyMessage>, parts: &mut Vec<Part>| {
        if let Some(m) = cur.take() {
            parts.push(Part::Message(m));
        }
    };
    for seg in tokenize(text) {
        match seg {
            Segment::Text(t) => match (slot, cur.as_mut()) {
                (Slot::Role, Some(m)) => m.role.push_str(t),
                (Slot::Channel, Some(m)) => m.channel.push_str(t),
                (Slot::Constrain, Some(m)) => {
                    // The format word, and when `<|message|>` is missing, the
                    // body right behind it.
                    let word_end = t
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
                        .unwrap_or(t.len());
                    m.constrain.push_str(&t[..word_end]);
                    if word_end < t.len() {
                        m.body.push_str(&t[word_end..]);
                        m.framed = true;
                        slot = Slot::Body;
                    }
                }
                (Slot::Body, Some(m)) => m.body.push_str(t),
                _ => parts.push(Part::Plain(t.to_string())),
            },
            Segment::Token(HarmonyToken::Start) => {
                close(&mut cur, &mut parts);
                cur = Some(HarmonyMessage::default());
                slot = Slot::Role;
            }
            Segment::Token(HarmonyToken::Channel) => {
                // A channel inside a body starts the next message; a channel
                // with no message open is one whose `<|start|>assistant` the
                // upstream already consumed.
                if cur.is_none() || slot == Slot::Body {
                    close(&mut cur, &mut parts);
                    cur = Some(HarmonyMessage {
                        role: "assistant".into(),
                        ..HarmonyMessage::default()
                    });
                }
                if let Some(m) = cur.as_mut() {
                    m.saw_channel = true;
                }
                slot = Slot::Channel;
            }
            Segment::Token(HarmonyToken::Constrain) => {
                if cur.is_some() && slot != Slot::Body {
                    slot = Slot::Constrain;
                }
            }
            Segment::Token(HarmonyToken::Message) => {
                if let Some(m) = cur.as_mut() {
                    m.framed = true;
                    slot = Slot::Body;
                }
            }
            Segment::Token(HarmonyToken::End | HarmonyToken::Call | HarmonyToken::Return) => {
                if let Some(m) = cur.as_mut() {
                    m.framed = true;
                    m.closed = true;
                }
                close(&mut cur, &mut parts);
                slot = Slot::Outside;
            }
        }
    }
    // A cut-off answer: the open message ends with the text.
    close(&mut cur, &mut parts);
    parts
}

/// The tool names of the menu the request sent, in both wire shapes:
/// Chat `tools[i].function.name`, Responses `tools[i].name`.
fn menu_names(request: &Value) -> Vec<&str> {
    request
        .get("tools")
        .and_then(|v| v.as_array())
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| {
                    t.get("function")
                        .and_then(|f| f.get("name"))
                        .or_else(|| t.get("name"))
                        .and_then(|n| n.as_str())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The most characters of a model-written name an error detail carries: a
/// safety ceiling at the carrier (R-IG-1), so one oversized word of model
/// output cannot fill the detail (review M-6).
const NAME_SHOWN_MAX: usize = 64;

/// `name` for an error detail, cut at [`NAME_SHOWN_MAX`] characters with the
/// numbers said.
fn shown(name: &str) -> String {
    let total = name.chars().count();
    if total <= NAME_SHOWN_MAX {
        return name.to_string();
    }
    let head: String = name.chars().take(NAME_SHOWN_MAX).collect();
    format!("{head}... ({NAME_SHOWN_MAX} of {total} characters)")
}

fn malformed(why: String) -> TranslateError {
    TranslateError::ToolCallShape(format!("harmony tokens in content: {why}"))
}

/// What one harmony-framed text turns into: recovered calls `(name,
/// arguments-as-JSON-object-string)` and the text a person may read.
fn read_harmony(
    text: &str,
    menu: &[&str],
) -> Result<(Vec<(String, String)>, String), TranslateError> {
    let mut calls = Vec::new();
    let mut said = String::new();
    let parts = split_messages(text);
    let complete = parts.iter().all(|p| match p {
        Part::Plain(_) => true,
        Part::Message(m) => m.is_complete(),
    });
    if !complete {
        // Not harmony, prose naming a token: nothing of it may fall.
        return Ok((calls, text.to_string()));
    }
    for part in parts {
        match part {
            Part::Plain(t) => said.push_str(&t),
            Part::Message(m) => {
                if !m.author().starts_with("assistant") {
                    // An echoed tool result (`functions.<name> to=assistant`)
                    // or a system/user message: never a call, never the
                    // answer (review M-5).
                } else if let Some(to) = m.recipient() {
                    let Some(name) = to.strip_prefix("functions.") else {
                        return Err(malformed(format!(
                            "call to `{}`, which is no function of the menu",
                            shown(to)
                        )));
                    };
                    if !menu.contains(&name) {
                        return Err(malformed(format!(
                            "call to `{}`, which the menu did not carry",
                            shown(name)
                        )));
                    }
                    let body = m.body.trim();
                    let args = if body.is_empty() {
                        Value::Object(Default::default())
                    } else {
                        meclaw_core::serde_json::from_str::<Value>(body)
                            .ok()
                            .filter(|v| v.is_object())
                            .ok_or_else(|| {
                                malformed(format!(
                                    "arguments of `{}` are not a JSON object",
                                    shown(name)
                                ))
                            })?
                    };
                    calls.push((name.to_string(), args.to_string()));
                } else if m.channel_name() == "analysis" {
                    // The thinking trace never reaches a person (GH #999 F3).
                } else {
                    // `final`, a recipient-less `commentary` preamble, or a
                    // message without a channel: the answer text.
                    said.push_str(&m.body);
                }
            }
        }
    }
    Ok((calls, said.trim().to_string()))
}

/// GH #1260 (OR-WG-7): a tool call an upstream leaks as raw harmony tokens in
/// `content` becomes the tool call it is, never spoken text.
///
/// Measured (a routing probe, gpt-oss-120b on one hosted upstream, 2 of 200
/// one-call turns): the model called `handoff{path: complex}`, the upstream
/// delivered `<|start|>assistant<|channel|>commentary to=functions.handoff
/// <|constrain|>json{"path": "complex"}<|call|>` as `content` with no
/// `tool_calls` — without this the turn reached the person as a "short
/// answer" of token junk.
///
/// Rules, for every text turn that carries a harmony frame ([`is_harmony`]):
///
/// * A message addressed `to=functions.<name>` becomes a `tool_call` turn
///   (same text shape as a wire tool call, id `harmony-<response_id>-<n>`),
///   but only for a `<name>` the request's menu carried. Another recipient, a
///   name outside the menu, or arguments that are no JSON object are a
///   `ToolCallShape` error (`malformed_tool_call`) — never text to a person.
/// * The `analysis` channel and non-assistant messages fall with their body;
///   `final`, a recipient-less `commentary` and channel-less messages keep
///   their body as the answer text. The control tokens themselves fall.
/// * A recovered call sets `finish_reason` to `tool_calls`. Nothing left — no
///   call, no text — is `EmptyAnswer`, as for any other empty answer.
///
/// Text without the frame passes byte-identical, and so does text whose
/// frame is no complete harmony pattern (prose naming a token).
pub(crate) fn recover_harmony_tool_calls(
    mut t: TranslatedResponse,
    request: &Value,
) -> Result<TranslatedResponse, TranslateError> {
    let leaked = |turn: &Value| {
        turn.get("type").and_then(|v| v.as_str()) == Some("text")
            && turn
                .get("text")
                .and_then(|v| v.as_str())
                .is_some_and(is_harmony)
    };
    if !t.assistant_turn.iter().any(leaked) {
        return Ok(t);
    }
    let menu = menu_names(request);
    let mut calls: Vec<Value> = Vec::new();
    let mut texts: Vec<Value> = Vec::new();
    let mut recovered = 0usize;
    for turn in std::mem::take(&mut t.assistant_turn) {
        if !leaked(&turn) {
            if turn.get("type").and_then(|v| v.as_str()) == Some("tool_call") {
                calls.push(turn);
            } else {
                texts.push(turn);
            }
            continue;
        }
        let text = turn.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let (found, said) = read_harmony(text, &menu)?;
        for (name, arguments) in found {
            calls.push(json!({
                "origin": "assistant",
                "type": "tool_call",
                "id": format!("harmony-{}-{recovered}", t.response_id),
                "text": json!({"name": name, "arguments": arguments}).to_string(),
            }));
            recovered += 1;
        }
        if !said.is_empty() {
            texts.push(json!({"origin": "assistant", "type": "text", "text": said}));
        }
    }
    let has_text = texts.iter().any(|turn| {
        turn.get("text")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty())
    });
    if calls.is_empty() && !has_text {
        return Err(TranslateError::EmptyAnswer);
    }
    if recovered > 0 {
        t.finish_reason = "tool_calls".to_string();
    }
    // Tool calls first, then text — the order of the wire translations.
    calls.extend(texts);
    t.assistant_turn = calls;
    Ok(t)
}

/// The header that reopens the harmony message a `length` cut left open in
/// its body, when that body falls ([`HarmonyMessage::body_falls`]); `None`
/// for any other answer.
///
/// GH #1260 (delta review H): each part of a cut answer is
/// read on its own. Cut inside an `analysis` body, the first part drops the
/// trace, and its rest arrives at the head of the continuation without a
/// frame -- read as text, it reached the person. Reopening the frame on the
/// continuation ([`reopen`]) makes the rest the body it is. Only falling
/// bodies are reopened: the rest of a cut `final` answer is text already, and
/// left alone the continuation keeps its bytes (and the join its spaces). A
/// frame cut in its header leaves the whole text as it came (`is_complete`), so
/// there is nothing to reopen.
pub(crate) fn cut_frame(t: &TranslatedResponse) -> Option<String> {
    if t.finish_reason != "length" {
        return None;
    }
    let text = t
        .assistant_turn
        .iter()
        .rev()
        .find(|turn| turn.get("type").and_then(|v| v.as_str()) == Some("text"))
        .and_then(|turn| turn.get("text"))
        .and_then(|v| v.as_str())?;
    if !is_harmony(text) {
        return None;
    }
    let parts = split_messages(text);
    let complete = parts.iter().all(|p| match p {
        Part::Plain(_) => true,
        Part::Message(m) => m.is_complete(),
    });
    match parts.last() {
        Some(Part::Message(m)) if complete && !m.closed && m.body_falls() => {
            let channel = if m.saw_channel {
                format!("<|channel|>{}", m.channel)
            } else {
                String::new()
            };
            Some(format!("<|start|>{}{channel}<|message|>", m.role))
        }
        _ => None,
    }
}

/// Puts `head` (from [`cut_frame`]) in front of the first text turn of a
/// continuation, so its leading rest is read inside the reopened frame --
/// only when that text is such a rest ([`is_frameless_rest`]).
///
/// GH #1260 (review I-1): reopening every continuation swallowed a clean
/// one whole -- `part two` became the body of the reopened `analysis`, fell,
/// and the person got `part one` alone. The continuation request carries only
/// the cleaned text (`continuation_request`), so the model mostly starts a
/// new turn; a clean continuation stays text, byte-identical.
pub(crate) fn reopen(t: &mut TranslatedResponse, head: &str) {
    if let Some(text) = t
        .assistant_turn
        .iter_mut()
        .find(|turn| turn.get("type").and_then(|v| v.as_str()) == Some("text"))
        .and_then(|turn| turn.get_mut("text"))
        && let Some(s) = text.as_str()
        && is_frameless_rest(s)
    {
        *text = Value::String(format!("{head}{s}"));
    }
}

/// True when `text` opens with the rest of a message body and no frame of its
/// own: its first harmony token closes a message (`<|end|>`, `<|call|>`,
/// `<|return|>`), or is a `<|start|>` or `<|channel|>` behind leading text. A
/// text without harmony tokens, or one whose first token opens a frame
/// (`<|channel|>`, `<|start|>` at its head), is a clean continuation.
fn is_frameless_rest(text: &str) -> bool {
    let mut lead = false;
    for seg in tokenize(text) {
        match seg {
            Segment::Text(s) => lead |= !s.trim().is_empty(),
            Segment::Token(HarmonyToken::End | HarmonyToken::Call | HarmonyToken::Return) => {
                return true;
            }
            // Review Z M-1: `split_messages` reads a channel inside a body as the
            // next message just as it reads a start, so text before either is a rest.
            Segment::Token(HarmonyToken::Start | HarmonyToken::Channel) => return lead,
            Segment::Token(_) => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::strip_provider_annotations;

    #[test]
    fn a_pua_wrapped_citation_marker_is_dropped_whole() {
        let raw = "It may rain this evening. \u{E200}cite\u{E202}turn0search0\u{E201}";
        assert_eq!(
            strip_provider_annotations(raw),
            "It may rain this evening. "
        );
    }

    #[test]
    fn text_without_markers_is_returned_byte_identical() {
        let raw = "plain text, naïve café, 日本語, and a lone \\ backslash";
        assert_eq!(strip_provider_annotations(raw), raw);
    }

    #[test]
    fn two_markers_and_a_stray_codepoint_all_fall() {
        let raw = "a\u{E200}cite\u{E202}turn0search0\u{E201}b\u{E200}cite\u{E202}turn1search3\u{E201}c\u{E203}";
        assert_eq!(strip_provider_annotations(raw), "abc");
    }

    #[test]
    fn an_unclosed_marker_takes_the_rest_of_the_text() {
        // Truncated response: the closing codepoint never arrives. Everything
        // behind the opener is marker content, so it falls with it.
        let raw = "It rains. \u{E200}cite\u{E202}turn0sea";
        assert_eq!(strip_provider_annotations(raw), "It rains. ");
    }

    #[test]
    fn a_marker_in_the_middle_leaves_both_sides_untouched() {
        let raw = "before \u{E200}cite\u{E202}turn0search0\u{E201} after";
        assert_eq!(strip_provider_annotations(raw), "before  after");
    }

    // -- GH #1260: harmony leak ------------------------------------------------

    use super::recover_harmony_tool_calls;
    use crate::llm::translate::{TranslateError, parse_openai_response};
    use meclaw_core::serde_json::{Value, json};

    /// The raw `content` of a measured chat-completions answer (a routing
    /// probe, one-call variant, 2 of 200 calls): the model called `handoff`, the upstream
    /// shipped the call as harmony tokens in `content` and no `tool_calls`.
    const LEAKED_HANDOFF: &str = "<|start|>assistant<|channel|>commentary to=functions.handoff <|constrain|>json{\n  \"path\": \"complex\"\n}<|call|>";

    fn chat_request() -> Value {
        json!({
            "model": "m",
            "messages": [],
            "tools": [
                {"type": "function", "function": {"name": "web_search", "parameters": {}}},
                {"type": "function", "function": {"name": "get_weather", "parameters": {}}},
                {"type": "function", "function": {"name": "handoff", "parameters": {}}},
            ],
        })
    }

    fn chat_answer(content: &str) -> Value {
        json!({
            "id": "gen-1",
            "model": "m",
            "choices": [{
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": content},
            }],
        })
    }

    fn recovered(
        content: &str,
        request: &Value,
    ) -> Result<crate::llm::translate::TranslatedResponse, TranslateError> {
        let t = parse_openai_response(&chat_answer(content)).unwrap();
        recover_harmony_tool_calls(t, request)
    }

    fn tool_call_function(turn: &Value) -> Value {
        assert_eq!(turn["type"], "tool_call", "turn: {turn}");
        meclaw_core::serde_json::from_str(turn["text"].as_str().unwrap()).unwrap()
    }

    #[test]
    fn a_leaked_harmony_call_becomes_a_tool_call_and_no_text() {
        let t = recovered(LEAKED_HANDOFF, &chat_request()).unwrap();
        assert_eq!(t.finish_reason, "tool_calls");
        assert_eq!(t.assistant_turn.len(), 1, "turns: {:?}", t.assistant_turn);
        let f = tool_call_function(&t.assistant_turn[0]);
        assert_eq!(f["name"], "handoff");
        let args: Value =
            meclaw_core::serde_json::from_str(f["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(args, json!({"path": "complex"}));
        assert!(!t.assistant_turn[0]["id"].as_str().unwrap().is_empty());
    }

    #[test]
    fn the_responses_menu_shape_is_read_too() {
        let request = json!({"tools": [{"type": "function", "name": "handoff", "parameters": {}}]});
        let t = recovered(LEAKED_HANDOFF, &request).unwrap();
        assert_eq!(tool_call_function(&t.assistant_turn[0])["name"], "handoff");
    }

    #[test]
    fn a_harmony_call_with_message_token_and_recipient_in_the_role_is_read() {
        let raw = "<|start|>assistant to=functions.get_weather<|channel|>commentary json<|message|>{\"location\": \"Potsdam\"}<|call|>";
        let t = recovered(raw, &chat_request()).unwrap();
        let f = tool_call_function(&t.assistant_turn[0]);
        assert_eq!(f["name"], "get_weather");
    }

    #[test]
    fn a_harmony_call_to_a_name_outside_the_menu_is_an_error_never_text() {
        let raw = LEAKED_HANDOFF.replace("functions.handoff", "functions.delete_everything");
        let err = recovered(&raw, &chat_request()).unwrap_err();
        assert!(
            matches!(err, TranslateError::ToolCallShape(_)),
            "err: {err:?}"
        );
    }

    #[test]
    fn a_harmony_call_without_any_menu_is_an_error() {
        let err = recovered(LEAKED_HANDOFF, &json!({"model": "m"})).unwrap_err();
        assert!(
            matches!(err, TranslateError::ToolCallShape(_)),
            "err: {err:?}"
        );
    }

    #[test]
    fn a_harmony_call_with_unparsable_arguments_is_an_error() {
        let raw = "<|start|>assistant<|channel|>commentary to=functions.handoff <|constrain|>json{\"path\": <|call|>";
        let err = recovered(raw, &chat_request()).unwrap_err();
        assert!(
            matches!(err, TranslateError::ToolCallShape(_)),
            "err: {err:?}"
        );
    }

    #[test]
    fn a_harmony_call_to_a_builtin_recipient_is_an_error() {
        let raw = "<|start|>assistant<|channel|>analysis to=browser.search<|message|>{\"query\": \"x\"}<|call|>";
        let err = recovered(raw, &chat_request()).unwrap_err();
        assert!(
            matches!(err, TranslateError::ToolCallShape(_)),
            "err: {err:?}"
        );
    }

    #[test]
    fn the_analysis_channel_falls_and_the_final_answer_stays() {
        let raw = "<|channel|>analysis<|message|>User greets. Reply briefly.<|end|><|start|>assistant<|channel|>final<|message|>Hello! Nice to have you here.";
        let t = recovered(raw, &chat_request()).unwrap();
        assert_eq!(t.finish_reason, "stop");
        assert_eq!(t.assistant_turn.len(), 1);
        assert_eq!(t.assistant_turn[0]["type"], "text");
        assert_eq!(t.assistant_turn[0]["text"], "Hello! Nice to have you here.");
    }

    #[test]
    fn an_answer_of_nothing_but_analysis_is_an_empty_answer() {
        let raw = "<|channel|>analysis<|message|>thinking only<|end|>";
        let err = recovered(raw, &chat_request()).unwrap_err();
        assert_eq!(err, TranslateError::EmptyAnswer);
    }

    #[test]
    fn the_tokenizer_scans_a_long_degenerate_text_once() {
        // Review I-1: one `find` per literal over the whole rest made the
        // scan quadratic (40 000 tokens: 10-13 s, synchronous in handle()).
        // The probe counts the bytes the scan touches; a linear scan stays
        // within a small multiple of the text length.
        let n = 40_000;
        for text in [
            format!("{}<|channel|>final<|message|>x", "<|end|>".repeat(n)),
            "<|start|>".repeat(n),
            "<|nope|>".repeat(n),
        ] {
            let mut steps = 0usize;
            let segments = super::tokenize_counted(&text, &mut steps);
            assert!(!segments.is_empty());
            assert!(
                steps <= 8 * text.len(),
                "{steps} scan steps for {} bytes",
                text.len()
            );
        }
    }

    #[test]
    fn the_tokenizer_splits_known_tokens_and_keeps_other_angle_pipes_as_text() {
        let segs = super::tokenize("a<|x|>b<|end|><|<|call|>c<|");
        let shown: Vec<String> = segs
            .iter()
            .map(|s| match s {
                super::Segment::Token(t) => format!("{t:?}"),
                super::Segment::Text(t) => format!("'{t}'"),
            })
            .collect();
        assert_eq!(shown, ["'a<|x|>b'", "End", "'<|'", "Call", "'c<|'"]);
    }

    #[test]
    fn two_leaked_calls_in_one_text_become_two_tool_calls() {
        let raw = format!(
            "{LEAKED_HANDOFF}<|start|>assistant<|channel|>commentary to=functions.get_weather <|constrain|>json<|message|>{{\"location\": \"Potsdam\"}}<|call|>"
        );
        let t = recovered(&raw, &chat_request()).unwrap();
        assert_eq!(t.finish_reason, "tool_calls");
        assert_eq!(t.assistant_turn.len(), 2, "turns: {:?}", t.assistant_turn);
        assert_eq!(tool_call_function(&t.assistant_turn[0])["name"], "handoff");
        assert_eq!(
            tool_call_function(&t.assistant_turn[1])["name"],
            "get_weather"
        );
        assert_ne!(t.assistant_turn[0]["id"], t.assistant_turn[1]["id"]);
    }

    #[test]
    fn a_leaked_call_with_text_before_and_after_keeps_the_text() {
        let raw = format!("Let me check. {LEAKED_HANDOFF} One moment.");
        let t = recovered(&raw, &chat_request()).unwrap();
        assert_eq!(t.finish_reason, "tool_calls");
        assert_eq!(t.assistant_turn.len(), 2, "turns: {:?}", t.assistant_turn);
        assert_eq!(tool_call_function(&t.assistant_turn[0])["name"], "handoff");
        assert_eq!(t.assistant_turn[1]["type"], "text");
        assert_eq!(t.assistant_turn[1]["text"], "Let me check.  One moment.");
    }

    #[test]
    fn prose_naming_the_channel_token_is_untouched() {
        // Review M-4: prose about harmony (any model) is no harmony message;
        // nothing of it may fall silently.
        for raw in [
            "Use the token <|channel|> to pick a channel; then the rest of my long answer.",
            "Write <|channel|> to pick a channel, then <|message|> for the body.",
            "<|start|>assistant is how a turn opens, nothing more.",
        ] {
            let t = recovered(raw, &chat_request()).unwrap();
            assert_eq!(t.finish_reason, "stop", "{raw}");
            assert_eq!(
                t.assistant_turn,
                vec![json!({"origin": "assistant", "type": "text", "text": raw})],
                "{raw}"
            );
        }
    }

    #[test]
    fn an_echoed_tool_result_falls_and_is_no_error() {
        // Review M-5: the result block of a tool is addressed to the
        // assistant; it is no call and never the answer.
        let raw = "<|start|>functions.get_weather to=assistant<|channel|>commentary<|message|>{\"temp\": 12}<|end|><|start|>assistant<|channel|>final<|message|>It is 12 degrees.";
        let t = recovered(raw, &chat_request()).unwrap();
        assert_eq!(t.finish_reason, "stop");
        assert_eq!(
            t.assistant_turn,
            vec![json!({"origin": "assistant", "type": "text", "text": "It is 12 degrees."})]
        );
    }

    #[test]
    fn an_oversized_name_is_cut_in_the_error_text() {
        // Review M-6: the name comes from model output; the detail carries
        // at most NAME_SHOWN_MAX characters of it and says so with numbers.
        let long = "x".repeat(10_000);
        let raw = LEAKED_HANDOFF.replace("handoff", &long);
        let err = recovered(&raw, &chat_request()).unwrap_err();
        let TranslateError::ToolCallShape(detail) = err else {
            panic!("a ToolCallShape error, got {err:?}");
        };
        assert!(detail.len() < 300, "detail: {} bytes", detail.len());
        assert!(
            detail.contains(&format!("{} of 10000 characters", super::NAME_SHOWN_MAX)),
            "{detail}"
        );
    }

    #[test]
    fn plain_text_with_angle_pipes_but_no_harmony_pattern_is_untouched() {
        let raw = "The signs <| and |> and <|endoftext|> are just text.";
        let t = recovered(raw, &chat_request()).unwrap();
        assert_eq!(t.finish_reason, "stop");
        assert_eq!(
            t.assistant_turn,
            vec![json!({"origin": "assistant", "type": "text", "text": raw})]
        );
    }

    // -- GH #1260 delta review H: a harmony frame split by the length cut ------

    fn cut_answer(content: &str) -> crate::llm::translate::TranslatedResponse {
        let mut raw = chat_answer(content);
        raw["choices"][0]["finish_reason"] = json!("length");
        parse_openai_response(&raw).unwrap()
    }

    /// The text a continuation turns into once the cut frame of `first` is
    /// reopened on it.
    fn continued(
        first: &str,
        next: &str,
    ) -> Result<crate::llm::translate::TranslatedResponse, TranslateError> {
        let head = super::cut_frame(&cut_answer(first));
        let mut t = parse_openai_response(&chat_answer(next)).unwrap();
        if let Some(head) = head.as_deref() {
            super::reopen(&mut t, head);
        }
        recover_harmony_tool_calls(t, &chat_request())
    }

    #[test]
    fn the_rest_of_an_analysis_cut_on_length_falls_in_the_continuation() {
        let first = "<|channel|>final<|message|>part one<|end|><|start|>assistant<|channel|>analysis<|message|>secret-";
        let next = "trace goes on<|end|><|start|>assistant<|channel|>final<|message|>part two";
        let t = continued(first, next).unwrap();
        assert_eq!(
            t.assistant_turn,
            vec![json!({"origin": "assistant", "type": "text", "text": "part two"})]
        );
    }

    #[test]
    fn a_clean_continuation_after_a_cut_analysis_stays_text() {
        // The continuation request carries only the cleaned text, so the
        // model mostly opens a new turn: text without harmony, or a message
        // with its own frame, is the answer and never falls.
        let first = "<|channel|>final<|message|>part one<|end|><|start|>assistant<|channel|>analysis<|message|>secret-";
        for (next, said) in [
            ("part two without an end", "part two without an end"),
            (
                "<|start|>assistant<|channel|>final<|message|>part two",
                "part two",
            ),
            ("<|channel|>final<|message|>part two", "part two"),
        ] {
            let t = continued(first, next).unwrap();
            assert_eq!(
                t.assistant_turn,
                vec![json!({"origin": "assistant", "type": "text", "text": said})],
                "{next}"
            );
        }
    }

    #[test]
    fn a_frameless_analysis_rest_falls_whatever_closes_it() {
        let first = "<|channel|>final<|message|>part one<|end|><|start|>assistant<|channel|>analysis<|message|>secret-";
        for next in [
            "trace<|end|>",
            "trace<|return|>",
            "trace<|call|>",
            "<|end|>",
        ] {
            assert_eq!(
                continued(first, next).unwrap_err(),
                TranslateError::EmptyAnswer,
                "{next}"
            );
        }
        for next in [
            "trace<|start|>assistant<|channel|>final<|message|>part two",
            "trace<|channel|>final<|message|>part two",
        ] {
            let t = continued(first, next).unwrap();
            assert_eq!(
                t.assistant_turn,
                vec![json!({"origin": "assistant", "type": "text", "text": "part two"})],
                "{next}"
            );
        }
    }

    #[test]
    fn a_cut_tool_result_falls_in_the_continuation_too() {
        let first = "<|channel|>final<|message|>one<|end|><|start|>functions.get_weather to=assistant<|channel|>commentary<|message|>{\"te";
        let t = continued(
            first,
            "mp\": 12}<|end|><|start|>assistant<|channel|>final<|message|>two",
        )
        .unwrap();
        assert_eq!(
            t.assistant_turn,
            vec![json!({"origin": "assistant", "type": "text", "text": "two"})]
        );
    }

    #[test]
    fn only_a_falling_frame_open_at_a_length_cut_is_reopened() {
        // A cut final answer keeps its continuation byte-identical (the join
        // keeps its spaces); a closed frame, an answer that is no length cut,
        // a frame cut in its header (`is_complete`) and plain text reopen nothing.
        let analysis =
            "<|channel|>final<|message|>a<|end|><|start|>assistant<|channel|>analysis<|message|>b";
        assert!(super::cut_frame(&cut_answer(analysis)).is_some());
        for first in [
            "<|channel|>final<|message|>part one ",
            "<|channel|>analysis<|message|>x<|end|><|start|>assistant<|channel|>final<|message|>y",
            "<|channel|>final<|message|>a<|end|><|start|>assistant<|channel|>analysis<|message|>b<|end|>",
            "<|channel|>final<|message|>a<|end|><|start|>assistant<|channel|>anal",
            "plain text cut in the middle",
        ] {
            assert_eq!(super::cut_frame(&cut_answer(first)), None, "{first}");
        }
        let stopped = parse_openai_response(&chat_answer(analysis)).unwrap();
        assert_eq!(super::cut_frame(&stopped), None);
    }
}
