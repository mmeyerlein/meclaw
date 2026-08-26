//! Phase-9 code params: runner ("python3" — kanonischer Phase-9-Runner per
//! spec correction 2026-05-23) + EXACTLY ONE of
//! script_path/script_inline. Optional external_timeout_ms + max_concurrency.

use meclaw_core::serde_json::Value;

/// Script source for `CodeCell` — exactly one of file path or inline
/// code, validated by `CodeParams::parse`.
#[derive(Debug, Clone)]
pub enum Script {
    /// Filesystem path to a Python script (executed via `runner` binary).
    Path(String),
    /// Inline Python code. Handed to `runner` as `-c <code>`, except above the
    /// platform's per-argv-string cap, where it is materialised into a
    /// per-spawn temp file instead (GH #349, [`crate::code::script_file`]).
    Inline(String),
}

/// Which runner a `code` cell uses. `cold` is the default and the pre-lane
/// behaviour; the other two keep a Python child alive between messages.
///
/// The words are about the RUNNER, not about the Hot/Cold-Cell model in
/// `docs/meclaw-overview.md` (which means awake/asleep and applies to stateful
/// cells only). A `code` cell has no wake state at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunnerMode {
    /// One fresh process per message. Default.
    #[default]
    Cold,
    /// A pool of `max_concurrency` resident children. The script is compiled
    /// once per child; every message runs the body in a FRESH globals dict, so
    /// nothing can accumulate — warm is cold with the interpreter start removed.
    Warm,
    /// Exactly one child, strictly serial. The globals dict PERSISTS between
    /// messages; RAM is a cache of the cell's durable store, never its truth.
    Resident,
}

impl RunnerMode {
    /// The wire spelling, for diagnostics and for `docs/cell-types.md`.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            RunnerMode::Cold => "cold",
            RunnerMode::Warm => "warm",
            RunnerMode::Resident => "resident",
        }
    }
}

/// Parsed `code` params.
///
/// Phase 9: `runner` must be exactly `"python3"` (spec correction
/// 2026-05-23 — the canonical phase-9 runner is the actually existing
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
    /// Optional process sandbox for the script (S4, GH #35). `None` means the
    /// legacy unsandboxed behaviour: the script keeps the daemon's rights.
    pub sandbox: Option<crate::sandbox::SandboxProfile>,
    /// How long a runner process lives (R2). `cold` is the default and the
    /// behaviour every `code` cell had before the modes existed.
    pub runner_mode: RunnerMode,
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
        // GH #334: `0` survives `as_u64()` and reaches `CodeParams` — an
        // A-timeout of zero milliseconds elapses before the script can run, so
        // every execution fails on the deadline. Refuse it here, in the shape of
        // the GH #322 `max_concurrency` guard below and with the literal bash,
        // web_search and web_fetch already return for the same input.
        if external_timeout_ms == Some(0) {
            return Err("params.external_timeout_ms must be >= 1".into());
        }
        let max_concurrency = match obj.get("max_concurrency") {
            None => None,
            Some(v) => Some(v.as_u64().ok_or("max_concurrency must be integer")? as usize),
        };
        // GH #322: `0` survives the integer check and the `unwrap_or(4)` default,
        // and `Semaphore::new(0)` hands out no permits — the dispatcher task waits
        // forever and never drains its mailbox. Refuse it here, with the wording
        // the five sibling cell types already use.
        if max_concurrency == Some(0) {
            return Err("params.max_concurrency must be >= 1".into());
        }
        let runner_mode = match obj.get("runner_mode").map(|v| v.as_str()) {
            None => RunnerMode::Cold,
            Some(Some("cold")) => RunnerMode::Cold,
            Some(Some("warm")) => RunnerMode::Warm,
            Some(Some("resident")) => RunnerMode::Resident,
            Some(other) => {
                let got =
                    other.map_or_else(|| obj["runner_mode"].to_string(), |s| format!("{s:?}"));
                return Err(format!(
                    "params.runner_mode: one of \"cold\", \"warm\", \"resident\" (got {got})"
                ));
            }
        };
        // R2: resident is serial BY CONSTRUCTION, so a declaration that says
        // otherwise is a contradiction, not a preference. Refused here, in the
        // same place and the same shape as the `max_concurrency: 0` guard above.
        if runner_mode == RunnerMode::Resident && matches!(max_concurrency, Some(n) if n != 1) {
            return Err("params.max_concurrency must be 1 when runner_mode is \"resident\"".into());
        }
        let sandbox = crate::sandbox::SandboxProfile::parse(raw)?;
        Ok(CodeParams {
            runner: runner.to_string(),
            script,
            external_timeout_ms,
            max_concurrency,
            sandbox,
            runner_mode,
        })
    }

    /// How many script executions may run at once — the value the factory hands
    /// to the dispatcher's semaphore AND the pool's size.
    ///
    /// `resident` forces 1 even when nothing was declared: the mode's promise is
    /// a single serial child, and a default of 4 would quietly break it.
    #[must_use]
    pub fn effective_max_concurrency(&self) -> usize {
        match self.runner_mode {
            RunnerMode::Resident => 1,
            _ => self.max_concurrency.unwrap_or(4),
        }
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
        // "python" (without "3") rejected — the phase-9 canonical name is "python3"
        // (spec correction 2026-05-23, cell-types.md Z.234).
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

    /// GH #322: `max_concurrency: 0` used to parse. `factory.rs` then built a
    /// `Semaphore::new(0)` — the cell task never acquires a permit and never
    /// drains its mailbox: registered, active, permanently silent. The refusal
    /// message is the one the five sibling cell types use verbatim (bash, file,
    /// edit, web_search, web_fetch), so operators read one sentence, not six.
    #[test]
    fn rejects_max_concurrency_zero() {
        let r = CodeParams::parse(&json!({
            "runner":"python3","script_path":"x.py","max_concurrency":0
        }));
        assert_eq!(
            r.unwrap_err(),
            "params.max_concurrency must be >= 1",
            "code must refuse 0 with the sibling wording"
        );
    }

    /// GH #334: `external_timeout_ms: 0` used to parse — `as_u64()` accepts `0`
    /// and it reached `CodeParams` untouched. The siblings (bash, web_search,
    /// web_fetch) refuse it; `code` must use the same literal.
    #[test]
    fn rejects_external_timeout_ms_zero() {
        let r = CodeParams::parse(&json!({
            "runner":"python3","script_path":"x.py","external_timeout_ms":0
        }));
        assert_eq!(
            r.unwrap_err(),
            "params.external_timeout_ms must be >= 1",
            "code must refuse 0 with the sibling wording"
        );
    }

    /// Sibling parity: the wordings above are not local inventions — they are
    /// the literals the other cell types return for the same input.
    #[test]
    fn zero_valued_knobs_refuse_with_the_sibling_wording() {
        use meclaw_colony::CellFactory;
        let code = CodeParams::parse(&json!({
            "runner":"python3","script_inline":"print(1)","max_concurrency":0
        }))
        .unwrap_err();
        let bash = crate::bash::BashCellFactory
            .validate_params(&json!({"max_concurrency":0}))
            .unwrap_err();
        let file = crate::file::FileCellFactory
            .validate_params(&json!({"base_path":"/tmp","max_concurrency":0}))
            .unwrap_err();
        assert_eq!(code, bash, "refusal wording drifted from bash");
        assert_eq!(code, file, "refusal wording drifted from file");

        let code_ms = CodeParams::parse(&json!({
            "runner":"python3","script_inline":"print(1)","external_timeout_ms":0
        }))
        .unwrap_err();
        let bash_ms = crate::bash::BashCellFactory
            .validate_params(&json!({"external_timeout_ms":0}))
            .unwrap_err();
        let web_fetch_ms = crate::web_fetch::WebFetchCellFactory
            .validate_params(&json!({"external_timeout_ms":0}))
            .unwrap_err();
        assert_eq!(code_ms, bash_ms, "refusal wording drifted from bash");
        assert_eq!(
            code_ms, web_fetch_ms,
            "refusal wording drifted from web_fetch"
        );
    }

    #[test]
    fn runner_mode_defaults_to_cold() {
        let r = CodeParams::parse(&json!({"runner":"python3","script_inline":"pass"})).unwrap();
        assert_eq!(
            r.runner_mode,
            RunnerMode::Cold,
            "no declaration means the old path"
        );
        assert_eq!(
            r.effective_max_concurrency(),
            4,
            "the pre-lane default stands"
        );
    }

    #[test]
    fn runner_mode_parses_warm_and_resident() {
        let w = CodeParams::parse(
            &json!({"runner":"python3","script_inline":"pass","runner_mode":"warm"}),
        )
        .unwrap();
        assert_eq!(w.runner_mode, RunnerMode::Warm);
        let r = CodeParams::parse(
            &json!({"runner":"python3","script_inline":"pass","runner_mode":"resident"}),
        )
        .unwrap();
        assert_eq!(r.runner_mode, RunnerMode::Resident);
        assert_eq!(
            r.effective_max_concurrency(),
            1,
            "resident is serial by construction"
        );
    }

    #[test]
    fn an_unknown_runner_mode_is_refused_loudly() {
        let e = CodeParams::parse(
            &json!({"runner":"python3","script_inline":"pass","runner_mode":"hot"}),
        )
        .unwrap_err();
        assert_eq!(
            e,
            "params.runner_mode: one of \"cold\", \"warm\", \"resident\" (got \"hot\")"
        );
    }

    #[test]
    fn resident_refuses_a_max_concurrency_other_than_one() {
        let e = CodeParams::parse(&json!({
            "runner":"python3","script_inline":"pass","runner_mode":"resident","max_concurrency":2
        }))
        .unwrap_err();
        assert_eq!(
            e, "params.max_concurrency must be 1 when runner_mode is \"resident\"",
            "a differing value is a spawn-time reject, like max_concurrency=0"
        );
        // The value that agrees with the mode is accepted, not merely tolerated.
        assert!(
            CodeParams::parse(&json!({
                "runner":"python3","script_inline":"pass","runner_mode":"resident","max_concurrency":1
            }))
            .is_ok()
        );
    }
}
