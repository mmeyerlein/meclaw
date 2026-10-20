//! GH #845 — `tool_scope`: the tool menu narrowed for ONE request.
//!
//! The menu lives durably in the brain (`system.tools.*` in `cell.db`). A
//! channel that may use only part of it sends the body slot
//! `tool_scope: {"allow": [..], "deny": [..], "hint": [..]}` with each call;
//! this module filters the menu for that request and never writes it back.
//! Pure, no I/O.
//!
//! **Two levels** (R-WM-1): an entry that stays travels whole (the default,
//! its ops and arguments), or — when `hint` names it — at the hint level: its
//! name, the first sentence of its description and the schema `{op: string}`.
//! The model fetches the rest of a hinted family through `help`; the hint
//! level keeps the prompt small for families the turn will rarely call.
//!
//! **`require`** (GH #1260, OR-WG-6): `true` asks the provider to answer with
//! a tool call at all (`tool_choice` required / `any`), for this request
//! only. A fast model given a turn that needs a lookup wrote "let me check"
//! and stopped, or made the answer up -- 7 of 10 calls with the tool, 10 of 10
//! with the call required (lab probe for GH #1260). A scope that leaves no
//! tool drops it (a provider refuses a required call without tools).
//!
//! **Order is kept, never re-sorted** (OR-SN-31): the filter walks the menu in
//! its own order and drops entries, so the same scope yields byte-identical
//! `tools` on every call — the precondition for a provider's prefix cache to
//! hold across the turns of one session (R-SN-1). The order of the names in
//! `allow` does not matter.

use meclaw_core::serde_json::{Value, json};

/// A parsed `tool_scope` slot. All three parts are optional.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ToolScope {
    /// When set, only these tools stay (intersection with the menu). An empty
    /// list leaves nothing.
    allow: Option<Vec<String>>,
    /// These tools leave; applied after `allow`.
    deny: Vec<String>,
    /// These tools stay at the hint level (name, first sentence, `{op}`).
    /// Never narrows and never widens: only an entry that `allow`/`deny` keep.
    hint: Vec<String>,
    /// GH #1260: the answer must be a tool call (absent = `false`).
    require: bool,
}

/// What the filter produced for one request.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Scoped {
    /// The provider-native tool objects that stay, in menu order.
    pub(crate) tools: Vec<Value>,
    /// Names the scope mentions that match no menu entry (ignored, reported).
    pub(crate) unknown: Vec<String>,
    /// GH #1260: the request asks for a tool call -- `require` was set and at
    /// least one tool stayed.
    pub(crate) require: bool,
    /// GH #1260: `require` was set but no tool stayed, so it was dropped
    /// (reported, never sent: a required call without tools is a 400).
    pub(crate) require_dropped: bool,
}

impl ToolScope {
    /// Parse the body slot. `None` input (slot absent) and `null` are
    /// `Ok(None)` — OR-SN-69: a writer that fills the slot from an optional
    /// value (the collector, T2) must not have to drop the key to mean "no
    /// scope" (rev-L1 M-3). Anything else that is not
    /// `{"allow"?: [string], "deny"?: [string], "hint"?: [string], "require"?: bool}` is an
    /// error: a scope that cannot be read must not silently widen the menu to
    /// all of it.
    pub(crate) fn parse(slot: Option<&Value>) -> Result<Option<Self>, String> {
        let Some(slot) = slot.filter(|v| !v.is_null()) else {
            return Ok(None);
        };
        let Some(obj) = slot.as_object() else {
            return Err("invalid tool_scope slot: not a JSON object".to_string());
        };
        if let Some(key) = obj
            .keys()
            .find(|k| !matches!(k.as_str(), "allow" | "deny" | "hint" | "require"))
        {
            return Err(format!(
                "invalid tool_scope slot: unknown key {key:?} (allowed: allow, deny, hint, require)"
            ));
        }
        let require = match obj.get("require") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => {
                return Err("invalid tool_scope slot: require is not a boolean".to_string());
            }
        };
        let allow = obj.get("allow").map(|v| names(v, "allow")).transpose()?;
        let half = |h: &str| -> Result<Vec<String>, String> {
            Ok(obj
                .get(h)
                .map(|v| names(v, h))
                .transpose()?
                .unwrap_or_default())
        };
        Ok(Some(Self {
            allow,
            deny: half("deny")?,
            hint: half("hint")?,
            require,
        }))
    }

    /// Filter `menu` — `(slot key, tool object)` pairs in menu order. A name
    /// matches an entry by its slot key under `system.tools` or by the tool's
    /// own function name (`function.name`, or a flat `name`). An entry that
    /// stays and that `hint` names is replaced by its hint form in place, so
    /// the order is the menu's either way.
    pub(crate) fn apply(&self, menu: Vec<(String, Value)>) -> Scoped {
        let mut unknown: Vec<String> = Vec::new();
        let named = self
            .allow
            .iter()
            .flatten()
            .chain(&self.deny)
            .chain(&self.hint);
        for name in named {
            let known = menu.iter().any(|(k, t)| matches_name(k, t, name));
            if !known && !unknown.contains(name) {
                unknown.push(name.clone());
            }
        }
        let tools = menu
            .into_iter()
            .filter(|(k, t)| {
                let allowed = self
                    .allow
                    .as_ref()
                    .is_none_or(|a| a.iter().any(|n| matches_name(k, t, n)));
                allowed && !self.deny.iter().any(|n| matches_name(k, t, n))
            })
            .map(|(k, t)| {
                if self.hint.iter().any(|n| matches_name(&k, &t, n)) {
                    hint_form(t)
                } else {
                    t
                }
            })
            .collect::<Vec<Value>>();
        let require = self.require && !tools.is_empty();
        Scoped {
            require,
            require_dropped: self.require && !require,
            tools,
            unknown,
        }
    }
}

/// The hint level of one tool object (R-WM-1): the name stays, the
/// description shrinks to its first sentence, the schema becomes `{op:
/// string}`. Chat-completions nests the fields under `function`, the flat
/// form carries them at the top; both keep their shape.
fn hint_form(mut tool: Value) -> Value {
    let nested = tool.get("function").is_some_and(Value::is_object);
    let inner = if nested {
        &mut tool["function"]
    } else {
        &mut tool
    };
    if let Some(obj) = inner.as_object_mut() {
        let first = obj
            .get("description")
            .and_then(Value::as_str)
            .map(first_sentence);
        if let Some(first) = first {
            obj.insert("description".into(), Value::String(first));
        }
        obj.insert(
            "parameters".into(),
            json!({"type": "object", "properties": {"op": {"type": "string"}}, "required": ["op"]}),
        );
    }
    tool
}

/// The first sentence of a description: up to and including the first `.`
/// that ends the text or is followed by whitespace, never past the first line.
fn first_sentence(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    let end = line
        .char_indices()
        .find(|&(i, c)| c == '.' && line[i + 1..].chars().next().is_none_or(char::is_whitespace))
        .map_or(line.len(), |(i, _)| i + 1);
    line[..end].to_string()
}

fn names(v: &Value, half: &str) -> Result<Vec<String>, String> {
    let Some(arr) = v.as_array() else {
        return Err(format!(
            "invalid tool_scope slot: {half} is not an array of tool names"
        ));
    };
    arr.iter()
        .map(|n| {
            n.as_str().map(str::to_string).ok_or_else(|| {
                format!("invalid tool_scope slot: {half} holds a value that is not a string")
            })
        })
        .collect()
}

/// The name a provider-native tool object carries: chat-completions nests it
/// under `function`, the flat form has it at the top.
fn tool_name(tool: &Value) -> Option<&str> {
    tool.get("function")
        .and_then(|f| f.get("name"))
        .or_else(|| tool.get("name"))
        .and_then(Value::as_str)
}

fn matches_name(key: &str, tool: &Value, name: &str) -> bool {
    key == name || tool_name(tool) == Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu(entries: &[(&str, &str)]) -> Vec<(String, Value)> {
        entries
            .iter()
            .map(|(k, n)| ((*k).to_string(), json!({"function": {"name": n}})))
            .collect()
    }

    fn names_of(s: &Scoped) -> Vec<&str> {
        s.tools.iter().map(|t| tool_name(t).unwrap()).collect()
    }

    fn scope(v: Value) -> ToolScope {
        ToolScope::parse(Some(&v)).unwrap().unwrap()
    }

    #[test]
    fn an_absent_slot_is_no_scope() {
        assert_eq!(ToolScope::parse(None), Ok(None));
    }

    /// OR-SN-69 (rev-L1 M-3): `tool_scope: null` is read as absent, so a
    /// writer that fills the slot from an optional value needs no special case.
    #[test]
    fn a_null_slot_is_no_scope() {
        assert_eq!(ToolScope::parse(Some(&Value::Null)), Ok(None));
    }

    #[test]
    fn a_malformed_slot_is_refused() {
        for bad in [
            json!("x"),
            json!({"allow": "a"}),
            json!({"deny": [1]}),
            json!({"only": ["a"]}),
        ] {
            assert!(ToolScope::parse(Some(&bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn deny_and_allow_keep_the_menu_order() {
        let m = menu(&[("t1", "c"), ("t2", "a"), ("t3", "x"), ("t4", "b")]);
        let s = scope(json!({"deny": ["x"]})).apply(m.clone());
        assert_eq!(names_of(&s), vec!["c", "a", "b"]);
        let s = scope(json!({"allow": ["b", "c"]})).apply(m.clone());
        assert_eq!(names_of(&s), vec!["c", "b"], "menu order, not allow order");
        let s = scope(json!({"allow": ["b", "c"], "deny": ["c"]})).apply(m);
        assert_eq!(names_of(&s), vec!["b"]);
    }

    #[test]
    fn a_name_matches_by_slot_key_or_function_name_and_unknown_names_are_reported() {
        let m = menu(&[("calculator", "calc"), ("web", "search")]);
        let s = scope(json!({"deny": ["calculator", "nobody"]})).apply(m.clone());
        assert_eq!(names_of(&s), vec!["search"]);
        assert_eq!(s.unknown, vec!["nobody".to_string()]);
        let s = scope(json!({"allow": ["calc"]})).apply(m);
        assert_eq!(names_of(&s), vec!["calc"]);
        assert!(s.unknown.is_empty());
    }

    /// GH #1260: the quick way's class tools come as `{allow: [web], hint:
    /// []}` -- the folded `web` keeps its whole schema with `args`, so the
    /// fast model can call it; only a hinted family shrinks to `{op}`.
    #[test]
    fn an_allowed_tool_outside_the_hint_keeps_its_whole_schema() {
        let params = json!({"type": "object", "properties": {
            "op": {"type": "string", "enum": ["search", "fetch", "help"]},
            "args": {"type": "object"}}, "required": ["op", "args"]});
        let m = vec![
            (
                "web".to_string(),
                json!({"function": {"name": "web", "description": "Search. Fetch.",
                                    "parameters": params.clone()}}),
            ),
            (
                "history".to_string(),
                json!({"function": {"name": "history", "description": "Recall. More.",
                                    "parameters": params.clone()}}),
            ),
        ];
        let s = scope(json!({"allow": ["web", "history"], "hint": ["history"]})).apply(m);
        assert_eq!(s.tools[0]["function"]["parameters"], params, "ops stage");
        assert_eq!(s.tools[0]["function"]["description"], "Search. Fetch.");
        assert_eq!(
            s.tools[1]["function"]["parameters"]["properties"]
                .as_object()
                .map(|p| p.keys().cloned().collect::<Vec<_>>()),
            Some(vec!["op".to_string()]),
            "hint stage"
        );
        let s = scope(json!({"allow": ["web"], "hint": []})).apply(vec![(
            "web".to_string(),
            json!({"function": {"name": "web", "parameters": params.clone()}}),
        )]);
        assert_eq!(s.tools[0]["function"]["parameters"], params);
    }

    /// GH #1260 (OR-WG-6): `require` reaches the request only while a tool
    /// stays; with none left it is dropped and said, never sent (a provider
    /// refuses a required call without tools).
    #[test]
    fn require_holds_only_while_a_tool_stays() {
        let m = menu(&[("web", "web"), ("weather", "weather")]);
        let s = scope(json!({"allow": ["web"], "require": true})).apply(m.clone());
        assert!(s.require && !s.require_dropped, "{s:?}");
        let s = scope(json!({"allow": [], "require": true})).apply(m.clone());
        assert!(s.tools.is_empty());
        assert!(
            !s.require && s.require_dropped,
            "no tool, no require: {s:?}"
        );
        let s = scope(json!({"allow": ["web"], "require": false})).apply(m.clone());
        assert!(!s.require && !s.require_dropped);
        let s = scope(json!({"allow": ["web"]})).apply(m);
        assert!(!s.require && !s.require_dropped, "absent = false");
        assert!(ToolScope::parse(Some(&json!({"require": "yes"}))).is_err());
    }

    #[test]
    fn an_empty_allow_leaves_nothing() {
        let s = scope(json!({"allow": []})).apply(menu(&[("a", "a")]));
        assert!(s.tools.is_empty());
    }
}
