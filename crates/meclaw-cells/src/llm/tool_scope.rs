//! GH #845 — `tool_scope`: the tool menu narrowed for ONE request.
//!
//! The menu lives durably in the brain (`system.tools.*` in `cell.db`). A
//! channel that may use only part of it sends the body slot
//! `tool_scope: {"allow": [..], "deny": [..]}` with each call; this module
//! filters the menu for that request and never writes it back. Pure, no I/O.
//!
//! **Order is kept, never re-sorted** (OR-SN-31): the filter walks the menu in
//! its own order and drops entries, so the same scope yields byte-identical
//! `tools` on every call — the precondition for a provider's prefix cache to
//! hold across the turns of one session (R-SN-1). The order of the names in
//! `allow` does not matter.

use meclaw_core::serde_json::Value;

/// A parsed `tool_scope` slot. Both halves are optional.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ToolScope {
    /// When set, only these tools stay (intersection with the menu). An empty
    /// list leaves nothing.
    allow: Option<Vec<String>>,
    /// These tools leave; applied after `allow`.
    deny: Vec<String>,
}

/// What the filter produced for one request.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Scoped {
    /// The provider-native tool objects that stay, in menu order.
    pub(crate) tools: Vec<Value>,
    /// Names the scope mentions that match no menu entry (ignored, reported).
    pub(crate) unknown: Vec<String>,
}

impl ToolScope {
    /// Parse the body slot. `None` input (slot absent) and `null` are
    /// `Ok(None)` — OR-SN-69: a writer that fills the slot from an optional
    /// value (the collector, T2) must not have to drop the key to mean "no
    /// scope" (rev-L1 M-3). Anything else that is not
    /// `{"allow"?: [string], "deny"?: [string]}` is an error: a scope that
    /// cannot be read must not silently widen the menu to all of it.
    pub(crate) fn parse(slot: Option<&Value>) -> Result<Option<Self>, String> {
        let Some(slot) = slot.filter(|v| !v.is_null()) else {
            return Ok(None);
        };
        let Some(obj) = slot.as_object() else {
            return Err("invalid tool_scope slot: not a JSON object".to_string());
        };
        if let Some(key) = obj.keys().find(|k| !matches!(k.as_str(), "allow" | "deny")) {
            return Err(format!(
                "invalid tool_scope slot: unknown key {key:?} (allowed: allow, deny)"
            ));
        }
        let allow = obj.get("allow").map(|v| names(v, "allow")).transpose()?;
        let deny = obj
            .get("deny")
            .map(|v| names(v, "deny"))
            .transpose()?
            .unwrap_or_default();
        Ok(Some(Self { allow, deny }))
    }

    /// Filter `menu` — `(slot key, tool object)` pairs in menu order. A name
    /// matches an entry by its slot key under `system.tools` or by the tool's
    /// own function name (`function.name`, or a flat `name`).
    pub(crate) fn apply(&self, menu: Vec<(String, Value)>) -> Scoped {
        let mut unknown: Vec<String> = Vec::new();
        for name in self.allow.iter().flatten().chain(self.deny.iter()) {
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
            .map(|(_, t)| t)
            .collect();
        Scoped { tools, unknown }
    }
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
    use meclaw_core::serde_json::json;

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

    #[test]
    fn an_empty_allow_leaves_nothing() {
        let s = scope(json!({"allow": []})).apply(menu(&[("a", "a")]));
        assert!(s.tools.is_empty());
    }
}
