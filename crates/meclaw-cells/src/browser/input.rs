//! What a finger on a screen becomes inside a page.
//!
//! Six shapes, all of them text on the link the picture travels back on, and
//! none of them a message (OR-G25). A keystroke per message would put a
//! person's typing through the router, the log and the edge table; the link is
//! already open and already has the page's identity in its topic.
//!
//! Three of the mappings are measured rather than guessed (befund 02 § D):
//! touch needs **no** `Input.setTouchEmulationEnabled` to be delivered, a run of
//! text goes through `Input.insertText` in one call while Enter, Backspace and
//! the arrows stay single keys, and one click is **three** calls — the pointer
//! has to be somewhere before it can be pressed.

use crate::browser::pages::Viewport;
use meclaw_core::serde_json::{Value, json};

/// One thing a viewer did.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// The pointer moved, went down or came up.
    Pointer {
        /// `move`, `down` or `up`.
        kind: String,
        /// CSS pixels from the left of the page's viewport.
        x: f64,
        /// CSS pixels from the top of the page's viewport.
        y: f64,
        /// `left`, `middle` or `right`.
        button: String,
        /// How many clicks this one is part of.
        clicks: u32,
    },
    /// A touch began, moved or ended.
    Touch {
        /// `start`, `move` or `end`.
        kind: String,
        /// Where the fingers are, in CSS pixels.
        points: Vec<(f64, f64)>,
    },
    /// The page was scrolled.
    Wheel {
        /// Where the pointer was.
        x: f64,
        /// Where the pointer was.
        y: f64,
        /// How far sideways.
        dx: f64,
        /// How far down.
        dy: f64,
    },
    /// A key went down or came up.
    Key {
        /// `down` or `up`.
        kind: String,
        /// The key's value, as a browser reports it.
        key: String,
        /// The physical key's code.
        code: String,
        /// What the key produces, when it produces text.
        text: String,
        /// The modifier bitfield CDP takes (alt 1, ctrl 2, meta 4, shift 8).
        mods: u32,
    },
    /// A run of text, typed or pasted.
    Text {
        /// What was typed.
        text: String,
    },
    /// A move through the page's own history.
    ///
    /// Only the three history words. A free address is the app's business
    /// (`in_navigate`), because a viewer who can send a browser anywhere is a
    /// viewer who can send it somewhere the member never asked for.
    Navigate {
        /// `about:back`, `about:forward` or `about:reload`.
        url: String,
    },
}

impl Input {
    /// Read one frame from the link, or say what was wrong with it.
    ///
    /// Every failure is text, and it goes back on the link (`invalid_input`):
    /// a frame this cell cannot read is never a panic and never a message.
    pub fn parse(text: &str) -> Result<Self, String> {
        let v: Value =
            meclaw_core::serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
        let kind_of = |field: &str, allowed: &[&str]| -> Result<String, String> {
            let k = v
                .get(field)
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{field} is required"))?;
            if allowed.contains(&k) {
                Ok(k.to_string())
            } else {
                Err(format!(
                    "{field} {k:?} is not one of {}",
                    allowed.join(", ")
                ))
            }
        };
        let num = |field: &str| -> f64 { v.get(field).and_then(Value::as_f64).unwrap_or(0.0) };
        match v.get("type").and_then(Value::as_str) {
            Some("pointer") => Ok(Self::Pointer {
                kind: kind_of("kind", &["move", "down", "up"])?,
                x: num("x"),
                y: num("y"),
                button: v
                    .get("button")
                    .and_then(Value::as_str)
                    .unwrap_or("left")
                    .to_string(),
                clicks: v
                    .get("clicks")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(1),
            }),
            Some("touch") => {
                let kind = kind_of("kind", &["start", "move", "end"])?;
                let points = v
                    .get("points")
                    .and_then(Value::as_array)
                    .ok_or("points is required")?
                    .iter()
                    .map(|p| {
                        (
                            p.get("x").and_then(Value::as_f64).unwrap_or(0.0),
                            p.get("y").and_then(Value::as_f64).unwrap_or(0.0),
                        )
                    })
                    .collect();
                Ok(Self::Touch { kind, points })
            }
            Some("wheel") => Ok(Self::Wheel {
                x: num("x"),
                y: num("y"),
                dx: num("dx"),
                dy: num("dy"),
            }),
            Some("key") => Ok(Self::Key {
                kind: kind_of("kind", &["down", "up"])?,
                key: v
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or("key is required")?
                    .to_string(),
                code: v
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                text: v
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                mods: v
                    .get("mods")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(0),
            }),
            Some("text") => Ok(Self::Text {
                text: v
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or("text is required")?
                    .to_string(),
            }),
            Some("navigate") => {
                let url = v
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or("url is required")?;
                if !["about:back", "about:forward", "about:reload"].contains(&url) {
                    return Err(format!(
                        "navigate {url:?} is not one of about:back, about:forward, about:reload \
                         — a free address travels as in_navigate, from the app"
                    ));
                }
                Ok(Self::Navigate {
                    url: url.to_string(),
                })
            }
            Some(other) => Err(format!("{other:?} is not an input this cell knows")),
            None => Err("type is required".to_string()),
        }
    }
}

/// What this input is, as CDP calls, in the order they go.
///
/// Scaling is the caller's: the coordinates that arrive are already CSS pixels
/// of the page's viewport, because the client that sent them is the one that
/// knows how big its canvas is drawn.
pub fn calls(input: &Input) -> Vec<(&'static str, Value)> {
    match input {
        Input::Pointer {
            kind,
            x,
            y,
            button,
            clicks,
        } => match kind.as_str() {
            // A press needs a pointer that is already there. Chromium routes a
            // press by the coordinates of the last move, so a `mousePressed`
            // without a `mouseMoved` lands wherever the pointer was left.
            "down" => vec![
                (
                    "Input.dispatchMouseEvent",
                    json!({"type": "mouseMoved", "x": x, "y": y, "button": "none", "buttons": 0}),
                ),
                (
                    "Input.dispatchMouseEvent",
                    json!({"type": "mousePressed", "x": x, "y": y, "button": button,
                           "buttons": 1, "clickCount": clicks}),
                ),
            ],
            "up" => vec![(
                "Input.dispatchMouseEvent",
                json!({"type": "mouseReleased", "x": x, "y": y, "button": button,
                       "buttons": 0, "clickCount": clicks}),
            )],
            _ => vec![(
                "Input.dispatchMouseEvent",
                json!({"type": "mouseMoved", "x": x, "y": y, "button": "none", "buttons": 0}),
            )],
        },
        Input::Touch { kind, points } => {
            let touch_points: Vec<Value> = points
                .iter()
                .map(|(x, y)| json!({"x": x, "y": y}))
                .collect();
            let cdp = match kind.as_str() {
                "start" => "touchStart",
                "end" => "touchEnd",
                _ => "touchMove",
            };
            // No `Input.setTouchEmulationEnabled` anywhere: measured on
            // 2026-09-18, a dispatched touch is delivered without it, and
            // turning emulation on would change what the PAGE thinks it is
            // running on rather than what happened to it.
            vec![(
                "Input.dispatchTouchEvent",
                json!({"type": cdp, "touchPoints": touch_points}),
            )]
        }
        Input::Wheel { x, y, dx, dy } => vec![(
            "Input.dispatchMouseEvent",
            json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy,
                   "button": "none", "buttons": 0}),
        )],
        Input::Key {
            kind,
            key,
            code,
            text,
            mods,
        } => {
            let cdp = if kind == "up" { "keyUp" } else { "keyDown" };
            let mut params = json!({"type": cdp, "key": key, "code": code, "modifiers": mods});
            if !text.is_empty() {
                params["text"] = Value::String(text.clone());
            }
            vec![("Input.dispatchKeyEvent", params)]
        }
        // One call for a whole run, and that is the point: a paste of two
        // hundred characters is one round trip, not two hundred.
        Input::Text { text } => vec![("Input.insertText", json!({"text": text}))],
        Input::Navigate { url } => match url.as_str() {
            "about:reload" => vec![("Page.reload", json!({}))],
            // History, not an address. `Page.navigate` with `about:back` would
            // send the page to a URL of that name.
            "about:forward" => vec![(
                "Runtime.evaluate",
                json!({"expression": "history.forward()", "returnByValue": true}),
            )],
            _ => vec![(
                "Runtime.evaluate",
                json!({"expression": "history.back()", "returnByValue": true}),
            )],
        },
    }
}

/// The shape a page is rendered at, for a viewer that brought one.
///
/// The viewport of a page is the profile of the output that last sent an input
/// (R-G5): two screens of different sizes watching one page have to agree on
/// one, and the one somebody is touching is the one that is right.
pub fn viewport_for(current: Viewport, viewer: Option<Viewport>) -> Viewport {
    viewer.unwrap_or(current)
}

/// Whether the page may be reshaped on the back of this input.
///
/// Only an input whose meaning does not depend on the layout, or the end of a
/// gesture. Everything positional says no, because the coordinates a viewer
/// sends were measured against the ONE picture it holds, and a reshape between
/// that picture and the next input makes them point at something else (wave G,
/// g9, finding B-G16: the first click of an output whose profile differs from
/// the page's opening size missed a field at `left: 50%` by 320 px, measured in
/// a colony on 2026-09-20). A release is safe: it is the last frame of its
/// gesture, and it has already been dispatched by the time this is asked.
pub fn reshapes_the_page(input: &Input) -> bool {
    match input {
        Input::Pointer { kind, .. } => kind == "up",
        Input::Touch { kind, .. } => kind == "end",
        Input::Wheel { .. } => false,
        Input::Key { .. } | Input::Text { .. } | Input::Navigate { .. } => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn methods(input: &Input) -> Vec<&'static str> {
        calls(input).into_iter().map(|(m, _)| m).collect()
    }

    #[test]
    fn a_click_is_three_calls_and_a_move_is_one() {
        let down = Input::parse(r#"{"type":"pointer","kind":"down","x":10,"y":20}"#).expect("down");
        let up = Input::parse(r#"{"type":"pointer","kind":"up","x":10,"y":20}"#).expect("up");
        assert_eq!(
            methods(&down),
            vec!["Input.dispatchMouseEvent", "Input.dispatchMouseEvent"]
        );
        assert_eq!(methods(&up), vec!["Input.dispatchMouseEvent"]);
        let all: Vec<Value> = calls(&down)
            .into_iter()
            .chain(calls(&up))
            .map(|(_, p)| p)
            .collect();
        assert_eq!(all.len(), 3, "moved, pressed, released");
        assert_eq!(all[0]["type"], "mouseMoved");
        assert_eq!(all[1]["type"], "mousePressed");
        assert_eq!(all[2]["type"], "mouseReleased");
    }

    #[test]
    fn a_run_of_text_is_one_call_and_a_special_key_is_its_own() {
        let text = Input::parse(r#"{"type":"text","text":"Gruesse"}"#).expect("text");
        assert_eq!(calls(&text)[0].0, "Input.insertText");
        assert_eq!(calls(&text)[0].1["text"], "Gruesse");
        let enter = Input::parse(r#"{"type":"key","kind":"down","key":"Enter","code":"Enter"}"#)
            .expect("key");
        assert_eq!(calls(&enter)[0].0, "Input.dispatchKeyEvent");
        assert_eq!(calls(&enter)[0].1["key"], "Enter");
    }

    #[test]
    fn touch_needs_no_emulation_switch() {
        let touch = Input::parse(r#"{"type":"touch","kind":"start","points":[{"x":1,"y":2}]}"#)
            .expect("touch");
        assert_eq!(methods(&touch), vec!["Input.dispatchTouchEvent"]);
        assert_eq!(calls(&touch)[0].1["touchPoints"][0]["x"], 1.0);
    }

    #[test]
    fn a_navigate_is_history_and_only_the_three_words() {
        assert_eq!(
            methods(&Input::parse(r#"{"type":"navigate","url":"about:back"}"#).expect("back")),
            vec!["Runtime.evaluate"]
        );
        assert_eq!(
            methods(&Input::parse(r#"{"type":"navigate","url":"about:reload"}"#).expect("reload")),
            vec!["Page.reload"]
        );
        let e = Input::parse(r#"{"type":"navigate","url":"https://elsewhere.example/"}"#)
            .expect_err("a viewer does not choose the address");
        assert!(
            e.contains("in_navigate"),
            "and the refusal says who does: {e}"
        );
    }

    #[test]
    fn a_frame_this_cell_cannot_read_is_a_sentence_and_not_a_panic() {
        assert!(Input::parse("not json").is_err());
        assert!(Input::parse(r#"{"type":"nudge"}"#).is_err());
        assert!(Input::parse(r#"{"type":"pointer","kind":"wiggle"}"#).is_err());
        assert!(Input::parse(r#"{}"#).is_err());
    }
}
