//! Message body. The inline variant carries JSON in 3a; the blob variant is
//! reserved for phase 12 (blob storage) and never constructed in 3a.

use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Inline JSON, < `blob_inline_max_bytes` (default 64 KB from phase 4 on).
    Inline(Value),
    /// Blob reference; resolved from phase 12 on. Unreachable in 3a.
    Blob(Uuid),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_variant_round_trips_value() {
        let b = Body::Inline(Value::String("hi".into()));
        match b {
            Body::Inline(v) => assert_eq!(v, Value::String("hi".into())),
            Body::Blob(_) => panic!("must be Inline"),
        }
    }

    #[test]
    fn blob_variant_carries_uuid() {
        let id = Uuid::now_v7();
        let b = Body::Blob(id);
        match b {
            Body::Blob(u) => assert_eq!(u, id),
            Body::Inline(_) => panic!("must be Blob"),
        }
    }
}
