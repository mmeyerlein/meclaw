//! Reading one of the four verbs out of an inbound message.
//!
//! The form is the one `harness`, `store` and `mcp` already read
//! (`harness::parse::parse_tool_call`): the tail `tool_call` turn's `text` is a
//! JSON string with `name` and `arguments`. It is IN the text rather than on
//! the turn because the UBF turn schema is closed — an app cannot invent two
//! turn properties, so it writes a document into the one property that takes
//! prose (OR-G33).

use crate::browser::pages::Viewport;
use meclaw_core::Message;
use meclaw_core::serde_json::Value;

/// One of the four things an app asks a browser to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Verb {
    /// Open a page, or make sure it is open at this address.
    Open {
        /// The page id.
        page: String,
        /// Where it should be.
        url: String,
        /// The identity it belongs to.
        context: String,
        /// The shape it should be rendered at, when the app said one.
        viewport: Option<Viewport>,
    },
    /// Send an open page somewhere else.
    Navigate {
        /// The page id.
        page: String,
        /// Where it should go.
        url: String,
    },
    /// Close a page. The context stays.
    Close {
        /// The page id.
        page: String,
    },
    /// Close a context. That is what logging out means here.
    ContextClose {
        /// The context name.
        context: String,
    },
}

impl Verb {
    /// The page this verb is about, for the receipt. Empty for a context verb,
    /// because every key of an emission is always set (`compose.py refuse`).
    /// The verb's own name, as the app wrote it in `tool_call.name`.
    ///
    /// It is what a refusal about the verb names — a caller with three windows
    /// in flight has to learn WHICH request ran out of time.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Open { .. } => "in_open",
            Self::Navigate { .. } => "in_navigate",
            Self::Close { .. } => "in_close",
            Self::ContextClose { .. } => "in_context_close",
        }
    }

    pub fn page(&self) -> &str {
        match self {
            Self::Open { page, .. } | Self::Navigate { page, .. } | Self::Close { page } => page,
            Self::ContextClose { .. } => "",
        }
    }
}

/// What a message turned out to be.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedVerb {
    /// The verb.
    pub verb: Verb,
    /// The call id, echoed into the receipt.
    pub call_id: String,
}

/// Read one verb, or say what was wrong with the message.
///
/// Every failure names what was missing: these sentences reach an operator
/// through a `receipt` with `error_code: "invalid_input"`, not a debugger.
pub fn parse_verb(msg: &Message) -> Result<ParsedVerb, String> {
    let call = crate::harness::parse::parse_tool_call(msg)?;
    let args = &call.arguments;
    let verb = match call.name.as_str() {
        "in_open" => Verb::Open {
            page: required_str(args, "page")?,
            url: required_str(args, "url")?,
            context: args
                .get("context")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or("default")
                .to_string(),
            viewport: viewport_of(args.get("viewport"))?,
        },
        "in_navigate" => Verb::Navigate {
            page: required_str(args, "page")?,
            url: required_str(args, "url")?,
        },
        "in_close" => Verb::Close {
            page: required_str(args, "page")?,
        },
        "in_context_close" => Verb::ContextClose {
            context: required_str(args, "context")?,
        },
        // There is no `in_input`, and that is a ruling rather than an omission
        // (OR-G25): an input reaches a page over the link it is being watched
        // through, never as a message. A message per keystroke would put a
        // person's typing through the router.
        other => {
            return Err(format!(
                "{other:?} is not one of this cell's verbs \
                 (in_open, in_navigate, in_close, in_context_close)"
            ));
        }
    };
    Ok(ParsedVerb {
        verb,
        call_id: call.call_id,
    })
}

/// One argument that has to be a non-empty string.
fn required_str(args: &Value, field: &str) -> Result<String, String> {
    match args.get(field).and_then(Value::as_str) {
        Some(s) if !s.is_empty() => Ok(s.to_string()),
        Some(_) => Err(format!("arguments.{field} must not be empty")),
        None => Err(format!("arguments.{field} is required")),
    }
}

/// An optional viewport. Absent is absent; present and broken is a refusal.
fn viewport_of(raw: Option<&Value>) -> Result<Option<Viewport>, String> {
    let Some(v) = raw else { return Ok(None) };
    if v.is_null() {
        return Ok(None);
    }
    let obj = v
        .as_object()
        .ok_or("arguments.viewport must be a JSON object")?;
    let dim = |key: &str| -> Result<u32, String> {
        obj.get(key)
            .and_then(Value::as_u64)
            .filter(|n| *n > 0)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| format!("arguments.viewport.{key} must be a positive integer"))
    };
    Ok(Some(Viewport {
        width: dim("width")?,
        height: dim("height")?,
        dpr: obj
            .get("dpr")
            .and_then(Value::as_f64)
            .filter(|d| *d > 0.0)
            .unwrap_or(1.0),
        mobile: obj.get("mobile").and_then(Value::as_bool).unwrap_or(false),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;
    use meclaw_core::{Body, MessageBuilder, Path};

    fn call(name: &str, arguments: Value) -> Message {
        let text = json!({"name": name, "arguments": arguments}).to_string();
        MessageBuilder::new(Path::new("/browser"))
            .body(Body::Inline(json!({
                "messages": [{
                    "origin": "tool", "type": "tool_call", "id": "call-1", "text": text
                }]
            })))
            .build()
    }

    #[test]
    fn the_four_verbs_read_as_themselves() {
        let opened = parse_verb(&call(
            "in_open",
            json!({"page": "card-1", "url": "https://example.com/#f",
                   "viewport": {"width": 960, "height": 600, "dpr": 2.0, "mobile": true}}),
        ))
        .expect("an open");
        assert_eq!(opened.call_id, "call-1");
        match opened.verb {
            Verb::Open {
                page,
                url,
                context,
                viewport,
            } => {
                assert_eq!(page, "card-1");
                assert_eq!(url, "https://example.com/#f", "the fragment travels");
                assert_eq!(context, "default", "an unnamed identity is the default one");
                let v = viewport.expect("the app named a shape");
                assert_eq!((v.width, v.height, v.mobile), (960, 600, true));
            }
            other => panic!("expected an open, got {other:?}"),
        }
        assert!(matches!(
            parse_verb(&call(
                "in_navigate",
                json!({"page": "card-1", "url": "https://example.com/b"})
            ))
            .expect("a navigate")
            .verb,
            Verb::Navigate { .. }
        ));
        assert!(matches!(
            parse_verb(&call("in_close", json!({"page": "card-1"})))
                .expect("a close")
                .verb,
            Verb::Close { .. }
        ));
        assert!(matches!(
            parse_verb(&call("in_context_close", json!({"context": "alex"})))
                .expect("a context close")
                .verb,
            Verb::ContextClose { .. }
        ));
    }

    #[test]
    fn there_is_no_verb_for_an_input() {
        let e = parse_verb(&call("in_input", json!({"page": "card-1"}))).expect_err("no such verb");
        assert!(
            e.contains("in_open"),
            "the refusal lists what there is: {e}"
        );
    }

    #[test]
    fn a_missing_argument_names_itself() {
        let e = parse_verb(&call("in_open", json!({"page": "card-1"}))).expect_err("no url");
        assert!(e.contains("arguments.url"), "{e}");
        let e = parse_verb(&call("in_open", json!({"page": "", "url": "u"}))).expect_err("empty");
        assert!(e.contains("arguments.page"), "{e}");
    }
}
