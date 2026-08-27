//! GH #423 — `--apply <file|->`: hand one mutation manifest to the door and
//! print the receipt.
//!
//! Two jobs, both small on purpose: read the manifest a `--apply` names, and
//! render the verdict for a human. Everything between them is the ordinary
//! mutation door — `--apply` is a pipe, not a second mutation engine.
//!
//! **No second validation model.** This module checks exactly one thing about
//! the body: that it carries a top-level `manifest` key. Whether the array is
//! well formed, whether the entries resolve, whether the diffs are legal — all
//! of that is the colony's answer, and it is the SAME answer whether the body
//! arrived by `--apply` or by `curl`. Anything checked here would be a rule
//! written twice, and two copies of a rule drift.

use meclaw_colony::ManifestOutcome;

/// Read the manifest a `--apply` names.
///
/// `-` reads `reader` to the end (production passes `std::io::stdin()`);
/// anything else is a path. `reader` is a parameter rather than a direct
/// `stdin()` call so the stdin case is testable without touching the process.
///
/// Three named refusals, each carrying the path the operator wrote:
///
/// * the file is not there;
/// * the content is not JSON (with serde's line and column);
/// * the content is JSON but not a manifest — the most common mistake, so the
///   message shows the wrapper instead of merely naming the omission.
pub fn read_manifest_source(
    source: &std::path::Path,
    reader: &mut dyn std::io::Read,
) -> anyhow::Result<meclaw_core::JsonValue> {
    let shown = source.display();
    let raw = if source.as_os_str() == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(reader, &mut buf)
            .map_err(|e| anyhow::anyhow!("--apply -: reading stdin failed: {e}"))?;
        buf
    } else {
        match std::fs::read_to_string(source) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(anyhow::anyhow!("--apply {shown}: no such file"));
            }
            Err(e) => return Err(anyhow::anyhow!("--apply {shown}: {e}")),
        }
    };
    let value: meclaw_core::JsonValue = meclaw_core::serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("--apply {shown}: not valid JSON: {e}"))?;
    if value.get("manifest").is_none() {
        return Err(anyhow::anyhow!(
            "--apply {shown}: this is a single mutation body, not a manifest; \
             wrap it: {{\"manifest\": [ … ]}}"
        ));
    }
    Ok(value)
}

/// Render one manifest receipt for a terminal.
///
/// Free text, not a parseable format: `docs/meclaw-overview.md` § CLI says the
/// output is prose and the EXIT CODE is the contract. What the prose owes a
/// reader of a refusal is three things — where it stopped, why, and how to pick
/// it up again — and the third is the one a receipt without rollback must say
/// out loud.
pub fn render_receipt(outcome: &ManifestOutcome) -> String {
    match outcome {
        ManifestOutcome::Committed { ids } => {
            let n = ids.len();
            format!("applied {n} of {n} mutations.\n")
        }
        ManifestOutcome::Rejected {
            ids,
            failed_at,
            error_code,
            details,
            remaining,
            ..
        } => {
            let applied = ids.len();
            let total = applied + 1 + remaining;
            format!(
                "applied {applied} of {total} mutations; entry {failed_at} was refused.\n  \
                 error_code: {error_code}\n  details:    {details}\n  \
                 the first {applied} entries are committed and stay committed (no rollback).\n  \
                 to resume: drop the first {applied} entries from the manifest and apply it again.\n"
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    fn empty() -> std::io::Empty {
        std::io::empty()
    }

    #[test]
    fn a_file_is_read_and_parsed() {
        let td = tempfile::TempDir::new().unwrap();
        let p = td.path().join("m.json");
        std::fs::write(&p, r#"{"manifest":[{"scope":"/"}]}"#).unwrap();
        let v = read_manifest_source(&p, &mut empty()).expect("read");
        assert_eq!(v["manifest"][0]["scope"], "/");
    }

    #[test]
    fn a_dash_reads_the_reader() {
        let mut src = std::io::Cursor::new(br#"{"manifest":[{"scope":"/x"}]}"#.to_vec());
        let v = read_manifest_source(std::path::Path::new("-"), &mut src).expect("read");
        assert_eq!(v["manifest"][0]["scope"], "/x");
    }

    #[test]
    fn a_missing_file_is_named() {
        let e = read_manifest_source(std::path::Path::new("/nope/nope.json"), &mut empty())
            .unwrap_err()
            .to_string();
        assert!(e.contains("/nope/nope.json"), "{e}");
        assert!(e.contains("no such file"), "{e}");
    }

    #[test]
    fn a_file_that_is_not_json_is_named_with_the_position() {
        let td = tempfile::TempDir::new().unwrap();
        let p = td.path().join("m.json");
        std::fs::write(&p, "{ oops").unwrap();
        let e = read_manifest_source(&p, &mut empty())
            .unwrap_err()
            .to_string();
        assert!(e.contains("m.json"), "{e}");
        assert!(e.contains("not valid JSON"), "{e}");
        assert!(e.contains("line 1"), "serde names the position: {e}");
    }

    #[test]
    fn a_single_mutation_body_is_told_how_to_become_a_manifest() {
        let td = tempfile::TempDir::new().unwrap();
        let p = td.path().join("grow.json");
        std::fs::write(&p, r#"{"scope":"/","diff":{}}"#).unwrap();
        let e = read_manifest_source(&p, &mut empty())
            .unwrap_err()
            .to_string();
        assert!(e.contains("grow.json"), "{e}");
        assert!(e.contains("not a manifest"), "{e}");
        assert!(
            e.contains(r#"{"manifest": [ … ]}"#),
            "it shows the wrapper: {e}"
        );
    }

    #[test]
    fn a_broken_manifest_array_is_not_judged_here() {
        // The colony judges the form; this reader only asks whether the key is
        // there. One rule, one place.
        let td = tempfile::TempDir::new().unwrap();
        let p = td.path().join("m.json");
        std::fs::write(&p, r#"{"manifest": "not an array"}"#).unwrap();
        let v = read_manifest_source(&p, &mut empty()).expect("read, not judged");
        assert_eq!(v["manifest"], json!("not an array"));
    }

    #[test]
    fn render_receipt_counts_a_full_run() {
        let out = render_receipt(&ManifestOutcome::Committed {
            ids: vec!["a".into(), "b".into(), "c".into()],
        });
        assert_eq!(out, "applied 3 of 3 mutations.\n");
    }

    #[test]
    fn render_receipt_names_the_position_and_the_resume() {
        let out = render_receipt(&ManifestOutcome::Rejected {
            ids: vec!["a".into(), "b".into(), "c".into()],
            failed_at: 4,
            id: None,
            error_code: "edge_schema".into(),
            details: "from='./orgs' unknown in scope /os".into(),
            remaining: 1,
        });
        assert!(
            out.starts_with("applied 3 of 5 mutations; entry 4 was refused."),
            "{out}"
        );
        assert!(out.contains("error_code: edge_schema"), "{out}");
        assert!(out.contains("from='./orgs' unknown in scope /os"), "{out}");
        assert!(out.contains("no rollback"), "{out}");
        assert!(
            out.contains("to resume: drop the first 3 entries"),
            "the receipt says how to pick it up again: {out}"
        );
    }
}
