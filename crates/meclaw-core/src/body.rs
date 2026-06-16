//! Message body. Inline-Variante trägt JSON in 3a; Blob-Variante reserviert
//! für Phase 12 (Blob-Storage), in 3a nie konstruiert.

use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Inline JSON, < `blob_inline_max_bytes` (Default 64 KB ab Phase 4).
    Inline(Value),
    /// Blob-Referenz; aufgelöst ab Phase 12. In 3a unreachable.
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
