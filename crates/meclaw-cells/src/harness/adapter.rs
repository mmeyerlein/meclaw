//! Which vendor dialect the child process speaks.
//!
//! An enum with one variant, not a trait. The cell type is generic — child
//! process, line-JSON, task lifecycle — but the dialect layer is deliberately
//! thin, and a trait for a single implementation would be decoration, not
//! abstraction. The seam that matters is the module boundary: everything
//! Claude-Code-specific lives in `harness::claude_code` and is reached through
//! the `match` on this enum. Adapter two turns that match into a second arm,
//! and only if the arms start to disagree structurally does a trait earn its
//! place.

/// The agent harness dialects this cell type can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessAdapter {
    /// Claude Code in print mode, speaking `stream-json` both ways.
    ClaudeCode,
}

impl HarnessAdapter {
    /// Parse the `params.adapter` value. Unknown values are a loud reject:
    /// falling back to a default would start the wrong vendor binary.
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "claude-code" => Ok(Self::ClaudeCode),
            other => Err(format!(
                "adapter: unknown value {other:?} (known: \"claude-code\")"
            )),
        }
    }

    /// The binary to run when the config does not name one.
    pub fn default_command(&self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_known_adapter_parses_and_carries_its_command() {
        let a = HarnessAdapter::parse("claude-code").expect("known adapter");
        assert_eq!(a, HarnessAdapter::ClaudeCode);
        assert_eq!(a.default_command(), "claude");
    }

    #[test]
    fn an_unknown_adapter_names_what_it_knows() {
        let err = HarnessAdapter::parse("codex").expect_err("must reject");
        assert!(err.contains("claude-code"), "got: {err}");
    }
}
