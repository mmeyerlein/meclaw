//! Every shipped `llm` cell keeps its substrate backstop ABOVE the call it is
//! allowed to make.
//!
//! # The rule this file gates
//!
//! `docs/meclaw-overview.md` § "Timeouts: two concepts, cleanly separated",
//! rule of thumb:
//!
//! > **B generous, A precise.** Operation timeouts (A) are the actual
//! > protective layer for I/O, those are set tight in a cell-type-specific
//! > way. The message timeout (B) as a backstop lies **considerably above**,
//! > so that normally A always takes effect first and produces a clean error
//! > message. Only when the cell really hangs for an unknown reason does B
//! > step in.
//!
//! Repeated as a hard development rule: `cell.message_timeout` must be
//! considerably larger than `params.external_timeout_ms`, so that normally A
//! takes effect first and produces a clean error message.
//!
//! # Why a gate, and not a review note
//!
//! Because the inversion is invisible from the file that carries it. A cell
//! declares A and stays silent about B; B then falls to the colony default
//! (`message_timeout_default_ms`, 60 000 ms), and nothing in the template says
//! so. `templates/builder/compose/config.json` declared
//! `external_timeout_ms: 170000` and no backstop — so the watchdog killed the
//! cell at 60 s, the supervisor restarted it, and what the caller saw was
//! `message_timeout` instead of an answer. The measurement that found it
//! (`plans/welle-2026-08-27/receipts/builder-messreihe.md`) spent a whole run
//! series reading that as a statement about the MODEL: `effort: high` died 5
//! times out of 6, `low` never did, because reasoning depth lengthens exactly
//! the call the backstop cuts. The defect does not announce itself as a
//! defect — it announces itself as data.
//!
//! Nine more shipped `llm` cells carried the same inversion, all of them
//! silently: a cell that declares no `external_timeout_ms` at all still makes a
//! 110 s call (`LlmParams::external_timeout_ms` default), which is already
//! above the 60 s backstop. Declaring nothing is not neutral here.
//!
//! # What is judged, and how
//!
//! Every `config.json` under `templates/` whose `cell.type` is `"llm"`. Both
//! sides of the comparison are resolved the way the SUBSTRATE resolves them,
//! never re-derived:
//!
//! - A: `serde_json::from_value::<LlmParams>(params)` — the same struct the
//!   factory builds, so an undeclared `external_timeout_ms` yields the same
//!   110 s default the cell would really use.
//! - B: `resolve_message_timeout(cell.message_timeout, ColonyConfig::default()
//!   .message_timeout_default_ms)` — the same function `cell_task` is handed
//!   its `Option<Duration>` from. `None` back means "no backstop" (`0`/`-1`,
//!   documented in `docs/config.md`), which cannot pre-empt A and therefore
//!   passes.
//!
//! # The margin, and where its numbers come from
//!
//! "Considerably above" is not a number, so the gate takes the weakest reading
//! that still has teeth: the backstop must clear the operation timeout by at
//! least 10 s AND by at least 10 %. Both floors sit below every margin the
//! tree ships — the tightest is `cogny/brain_fast` at 90 000 over 60 000
//! (+50 %), then `builder/compose` at 240 000 over 170 000 (+41 %),
//! `cogny/brain` at 400 000 over 300 000 (+33 %). The gate judges the
//! ORDERING; the taste stays with the operator.
//!
//! # The test of the test
//!
//! The sweep is green on the tree as it stands, so on its own it would be
//! green whether the comparison works or not. `the_verdict_reads_both_sides`
//! and `an_inverted_pair_is_reported` put fabricated input through the SAME
//! function the sweep uses — no file is touched.

use meclaw_cells::LlmParams;
use meclaw_colony::{ColonyConfig, resolve_message_timeout};

/// Absolute floor on the gap between backstop and operation timeout.
const MARGIN_FLOOR_MS: u64 = 10_000;

/// Relative floor, as a divisor: the gap must be at least `external / 10`.
const MARGIN_DIVISOR: u64 = 10;

/// A sweep that finds nothing passes for free. The published subset ships
/// fewer templates than the development tree (30 of 36 at the time of writing)
/// and therefore fewer `llm` cells: 16 in the development tree, 10 in the
/// published one — counted, not estimated. The floor is that smaller count
/// exactly, so the gate cannot pass by finding nothing, and a shipped `llm`
/// cell that leaves the published subset has to be re-counted here rather than
/// silently shrinking the sweep.
const MIN_LLM_CELLS: usize = 10;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The two numbers a shipped `llm` cell really runs on, in milliseconds.
///
/// `backstop` is `None` when the cell declared `0`/`-1` — no backstop at all,
/// which is a documented choice and not an inversion.
#[derive(Debug, Clone, Copy)]
struct Deadlines {
    external_ms: u64,
    backstop_ms: Option<u64>,
}

/// Resolve both deadlines out of one parsed `config.json`, through the
/// substrate's own readers.
fn deadlines_of(config: &serde_json::Value) -> Result<Deadlines, String> {
    let params = config
        .get("params")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let parsed: LlmParams = serde_json::from_value(params)
        .map_err(|e| format!("params do not deserialize as LlmParams: {e}"))?;

    let declared = config
        .get("cell")
        .and_then(|c| c.get("message_timeout"))
        .map(|v| {
            v.as_i64()
                .ok_or_else(|| format!("cell.message_timeout is not an integer: {v}"))
        })
        .transpose()?;

    let default_ms = ColonyConfig::default().message_timeout_default_ms;
    Ok(Deadlines {
        external_ms: parsed.external_timeout_ms,
        backstop_ms: resolve_message_timeout(declared, default_ms).map(|d| d.as_millis() as u64),
    })
}

/// The whole verdict: `Some(complaint)` iff B would pre-empt A, or sit so
/// close above it that A has no room to fire first.
fn inversion(d: Deadlines) -> Option<String> {
    let Some(backstop) = d.backstop_ms else {
        // No backstop at all — B can never cut A short. Documented in
        // `docs/config.md` (`0`/`-1`) and normal for long-running handlers.
        return None;
    };
    let required = d.external_ms + MARGIN_FLOOR_MS.max(d.external_ms / MARGIN_DIVISOR);
    (backstop < required).then(|| {
        format!(
            "backstop {backstop} ms does not clear the {} ms call it wraps \
             (needs at least {required} ms: +10 s and +10 %)",
            d.external_ms
        )
    })
}

#[test]
fn every_shipped_llm_cell_keeps_its_backstop_above_its_call() {
    let mut judged = 0usize;
    let mut findings: Vec<String> = Vec::new();

    let mut stack = vec![repo("templates")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.file_name().and_then(|n| n.to_str()) != Some("config.json") {
                continue;
            }
            let raw = std::fs::read_to_string(&path).expect("a config.json is readable");
            let config: serde_json::Value = match serde_json::from_str(&raw) {
                Ok(v) => v,
                // Not this gate's question — `gh221_shipped_template_versions`
                // and the boot itself judge parseability.
                Err(_) => continue,
            };
            if config
                .get("cell")
                .and_then(|c| c.get("type"))
                .and_then(|t| t.as_str())
                != Some("llm")
            {
                continue;
            }
            judged += 1;
            let rel = path.strip_prefix(repo("")).unwrap_or(&path).to_path_buf();
            match deadlines_of(&config) {
                Err(e) => findings.push(format!("{}: {e}", rel.display())),
                Ok(d) => {
                    if let Some(complaint) = inversion(d) {
                        findings.push(format!("{}: {complaint}", rel.display()));
                    }
                }
            }
        }
    }

    assert!(
        judged >= MIN_LLM_CELLS,
        "the sweep judged only {judged} llm cells — it is not finding the tree"
    );
    assert!(
        findings.is_empty(),
        "these shipped llm cells would be cut short by their own substrate \
         backstop before their declared call can time out cleanly \
         (docs/meclaw-overview.md § Timeouts, \"B generous, A precise\"):\n  {}",
        findings.join("\n  ")
    );
}

#[test]
fn the_verdict_reads_both_sides() {
    // Comfortably above: the shape memory-hive ships.
    assert!(
        inversion(Deadlines {
            external_ms: 110_000,
            backstop_ms: Some(180_000)
        })
        .is_none()
    );
    // No backstop is not an inversion.
    assert!(
        inversion(Deadlines {
            external_ms: 900_000,
            backstop_ms: None
        })
        .is_none()
    );
    // Exactly on the floor passes; one millisecond under does not.
    assert!(
        inversion(Deadlines {
            external_ms: 100_000,
            backstop_ms: Some(110_000)
        })
        .is_none()
    );
    assert!(
        inversion(Deadlines {
            external_ms: 100_000,
            backstop_ms: Some(109_999)
        })
        .is_some()
    );
    // Below 100 s the absolute floor is the binding one.
    assert!(
        inversion(Deadlines {
            external_ms: 20_000,
            backstop_ms: Some(29_000)
        })
        .is_some()
    );
}

#[test]
fn an_inverted_pair_is_reported() {
    // The shape `templates/builder/compose/config.json` shipped: a declared
    // 170 s call under the 60 s colony default.
    let fabricated = serde_json::json!({
        "cell": { "type": "llm" },
        "params": { "provider": "openai", "model": "x", "external_timeout_ms": 170_000u64 }
    });
    let d = deadlines_of(&fabricated).expect("the fabricated params parse");
    assert_eq!(d.external_ms, 170_000);
    assert_eq!(d.backstop_ms, Some(60_000));
    let complaint = inversion(d).expect("the inversion is reported");
    assert!(complaint.contains("60000"), "{complaint}");
}

#[test]
fn an_undeclared_call_is_not_a_neutral_one() {
    // A cell that declares no `external_timeout_ms` still makes a 110 s call,
    // which is already above the 60 s colony default. This is the half of the
    // audit that no reading of the FILES alone would have found.
    let fabricated = serde_json::json!({
        "cell": { "type": "llm" },
        "params": { "provider": "openai", "model": "x" }
    });
    let d = deadlines_of(&fabricated).expect("the fabricated params parse");
    assert_eq!(d.external_ms, 110_000);
    assert!(inversion(d).is_some());
}
