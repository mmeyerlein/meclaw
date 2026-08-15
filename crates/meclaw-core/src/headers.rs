//! Two-compartment routing headers per spec § Headers vs. Body.
//!
//! `context` is persistent (only an edge writes/deletes it); `hop` is the
//! isolated output of the immediately-preceding cell, refined by the traversed
//! edge, and is dropped on the next cell emission (structural freshness).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Routing headers split into the persistent `context` and the single-hop `hop`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Headers {
    /// Persistent across the whole message lifecycle. Only edges write/delete.
    #[serde(default)]
    pub context: Map<String, Value>,
    /// Exactly one hop: the preceding cell's isolated contract output, refined
    /// by the traversed edge. Dropped on the next cell emission.
    #[serde(default)]
    pub hop: Map<String, Value>,
}

impl Headers {
    /// Empty both compartments.
    pub fn new() -> Self {
        Self::default()
    }
    /// Construct from explicit compartments.
    pub fn from_parts(context: Map<String, Value>, hop: Map<String, Value>) -> Self {
        Self { context, hop }
    }
    /// Carry `context` forward unchanged and replace `hop` with `new_hop`
    /// (the structural verfall used by the outputs-arm).
    pub fn carry_context_with_hop(&self, new_hop: Map<String, Value>) -> Self {
        Self {
            context: self.context.clone(),
            hop: new_hop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn carry_context_drops_old_hop_and_replaces() {
        let mut ctx = Map::new();
        ctx.insert("turn_id".into(), json!("t1"));
        let mut old_hop = Map::new();
        old_hop.insert("operation".into(), json!("select"));
        let h = Headers::from_parts(ctx.clone(), old_hop);

        let mut new_hop = Map::new();
        new_hop.insert("finish_reason".into(), json!("tool_calls"));
        let out = h.carry_context_with_hop(new_hop);

        assert_eq!(
            out.context.get("turn_id"),
            Some(&json!("t1")),
            "context survives"
        );
        assert!(
            out.hop.get("operation").is_none(),
            "old hop dropped (verfall)"
        );
        assert_eq!(
            out.hop.get("finish_reason"),
            Some(&json!("tool_calls")),
            "new hop present"
        );
    }

    #[test]
    fn roundtrips_through_serde() {
        let mut ctx = Map::new();
        ctx.insert("session_id".into(), json!("s1"));
        let h = Headers::from_parts(ctx, Map::new());
        let s = serde_json::to_string(&h).unwrap();
        let back: Headers = serde_json::from_str(&s).unwrap();
        assert_eq!(h, back);
    }
}
