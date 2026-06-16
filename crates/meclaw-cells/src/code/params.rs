//! Phase-9 code params: runner ("python3" — kanonischer Phase-9-Runner per
//! Marcus-Spec-Korrektur 2026-05-23) + EXACTLY ONE of
//! script_path/script_inline. Optional external_timeout_ms + max_concurrency.

use meclaw_core::serde_json::Value;

/// Script source for `CodeCell` — exactly one of file path or inline
/// code, validated by `CodeParams::parse`.
#[derive(Debug, Clone)]
pub enum Script {
    /// Filesystem path to a Python script (executed via `runner` binary).
    Path(String),
    /// Inline Python code (passed to `runner` as `-c <code>`).
    Inline(String),
}

/// Parsed `code` params.
///
/// Phase 9: `runner` must be exactly `"python3"` (Marcus-Spec-Korrektur
/// 2026-05-23 — kanonischer Phase-9-Runner ist das real existierende
/// Binary auf Ubuntu 24, cell-types.md Z.234 Doc-Update folgt im
/// Design-Repo). Other values (incl. `"python"`, `"ruby"`, ...) are
/// rejected with `'params.runner: only "python3" is supported in Phase 9'`.
#[derive(Debug, Clone)]
pub struct CodeParams {
    /// The runner binary — must be exactly `"python3"` in Phase 9.
    pub runner: String,
    /// Script source: exactly one of file path or inline code.
    pub script: Script,
    /// Optional per-execution timeout in milliseconds (A-Timeout).
    pub external_timeout_ms: Option<u64>,
    /// Optional maximum number of concurrent script executions.
    pub max_concurrency: Option<usize>,
}

impl CodeParams {
    /// Parse raw params. Returns `Err` with operator-readable message on
    /// missing/malformed fields. `runner` must be `"python3"`.
    pub fn parse(raw: &Value) -> Result<Self, String> {
        let obj = raw.as_object().ok_or("params must be JSON object")?;
        let runner = obj
            .get("runner")
            .and_then(|v| v.as_str())
            .ok_or("params.runner required")?;
        if runner != "python3" {
            return Err(format!(
                "params.runner: only \"python3\" is supported in Phase 9 (got {runner:?})"
            ));
        }
        let path = obj
            .get("script_path")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let inline = obj
            .get("script_inline")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let script = match (path, inline) {
            (Some(_), Some(_)) => {
                return Err("params: script_path AND script_inline both set".into());
            }
            (None, None) => return Err("params: one of script_path/script_inline required".into()),
            (Some(p), None) => Script::Path(p),
            (None, Some(s)) => Script::Inline(s),
        };
        let external_timeout_ms = match obj.get("external_timeout_ms") {
            None => None,
            Some(v) => Some(v.as_u64().ok_or("external_timeout_ms must be integer")?),
        };
        let max_concurrency = match obj.get("max_concurrency") {
            None => None,
            Some(v) => Some(v.as_u64().ok_or("max_concurrency must be integer")? as usize),
        };
        Ok(CodeParams {
            runner: runner.to_string(),
            script,
            external_timeout_ms,
            max_concurrency,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    #[test]
    fn parses_runner_python3_with_script_path() {
        let r = CodeParams::parse(&json!({"runner":"python3","script_path":"x.py"})).unwrap();
        assert_eq!(r.runner, "python3");
        match r.script {
            Script::Path(p) => assert_eq!(p, "x.py"),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_runner_python3_with_script_inline() {
        let r = CodeParams::parse(&json!({"runner":"python3","script_inline":"print(1)"})).unwrap();
        match r.script {
            Script::Inline(s) => assert_eq!(s, "print(1)"),
            _ => panic!(),
        }
    }

    #[test]
    fn rejects_both_path_and_inline() {
        let r = CodeParams::parse(&json!({
            "runner":"python3","script_path":"x.py","script_inline":"print(1)"
        }));
        assert!(r.is_err());
    }

    #[test]
    fn rejects_neither_path_nor_inline() {
        let r = CodeParams::parse(&json!({"runner":"python3"}));
        assert!(r.is_err());
    }

    #[test]
    fn rejects_non_python3_runner() {
        // "python" (without "3") rejected — Phase-9 kanonisch ist "python3"
        // (Marcus-Spec-Korrektur 2026-05-23, cell-types.md Z.234).
        let r1 = CodeParams::parse(&json!({"runner":"python","script_path":"x.py"}));
        assert!(
            r1.is_err(),
            "\"python\" must be rejected — Phase-9 runner is \"python3\""
        );
        // "ruby" rejected — only python3 in Phase 9.
        let r2 = CodeParams::parse(&json!({"runner":"ruby","script_path":"x.rb"}));
        assert!(r2.is_err());
    }

    #[test]
    fn parses_optional_timeout_and_concurrency() {
        let r = CodeParams::parse(&json!({
            "runner":"python3","script_path":"x.py",
            "external_timeout_ms":1234,"max_concurrency":2
        }))
        .unwrap();
        assert_eq!(r.external_timeout_ms, Some(1234));
        assert_eq!(r.max_concurrency, Some(2));
    }
}
