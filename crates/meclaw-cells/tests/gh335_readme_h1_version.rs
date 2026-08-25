//! GH #335 — a template README's H1 version is gated against `template.json`.
//!
//! `templates/README.md` § Versioning makes the H1 of a template's README a
//! statement about that template: `` # `talky@3.0.8` `` says which version the
//! page below it describes. Nothing checked it. The version therefore lived in
//! two places that could disagree silently, and twice on 2026-08-20 they did —
//! a bump moved `template.json` and left the H1 quoting the version before it,
//! so the page told a reader one number while the substrate instantiated
//! another. Both instances are repaired; this file is why a third one cannot
//! sit in the tree unnoticed.
//!
//! # What is judged, and what is exempt
//!
//! Every direct child of `templates/` that carries BOTH a `template.json` and a
//! `README.md` — a sub-unit inside a composite carries no `template.json` and
//! is skipped by construction, and so is `_cell-types`, which is a README with
//! no template behind it.
//!
//! A README whose H1 names no version is not a defect: seven templates title
//! their page with the bare name (`# egon`) or with a sentence (`# canvy — a
//! canvas the colony serves itself`). Those are exempt — but the exemption is
//! a closed list, not a silence: `the_versionless_set_stays_inside_its_list`
//! requires every version-less README to be one of the seven, so a template
//! that DROPS its version to escape the gate fails here instead of joining
//! them.
//!
//! # Why a floor, and why 15
//!
//! A sweep that finds nothing passes for free. The floor is set for the
//! SMALLER of the two trees this file has to be green in: the published subset
//! ships 19 of these templates, 18 of which are judged, while the full tree
//! ships 31 and judges 24. A subset is not a defect, an empty sweep is — 15
//! sits below the smaller count and far above zero.
//!
//! # The test of the test
//!
//! The sweep is green on the tree as it stands, so on its own it would be green
//! whether the comparison works or not. `h1_version_reads_the_slot_and_nothing_else`
//! and `a_fabricated_mismatch_is_reported` put fabricated input through the
//! SAME two functions the sweep uses — no file is touched — so a comparison
//! that stopped comparing is red here.

use meclaw_core::serde_json::Value;

/// Templates whose README H1 names no version. Exempt from the comparison, but
/// asserted as an upper bound: see `the_versionless_set_stays_inside_its_list`.
const VERSIONLESS: &[&str] = &[
    "bot-basic",
    // `canvy` left this set with 2.0.0 (W8, GH #383): its README H1 names the
    // version now, so the sweep judges it like any other shipped template.
    "coder-pipeline",
    "daily-digest",
    "egon",
    "research-assistant",
    "slack-agent",
];

/// Fewest templates the sweep must actually judge. Chosen for the published
/// subset (18 there, 24 in the full tree) — see the header.
const MIN_JUDGED: usize = 15;

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// ───────────────────────────────────────────────────────── the comparison

/// What one README H1 says about one `template.json` version.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// The H1 names a version and it is the one the template declares.
    Agrees,
    /// The H1 names no version at all — exempt, but counted.
    Silent,
    /// The H1 names a version and it is a different one. Carries the message.
    Disagrees(String),
}

/// True for the characters a version may use after the leading digit.
fn is_version_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+')
}

/// The version a README H1 names, if it names one.
///
/// The slot is `@` followed by a digit, then the run of characters a version
/// may use — equivalently `@([0-9][0-9A-Za-z.\-+]*)`. Everything around it
/// (the `#`, the backticks, a trailing sentence) ends the run, so an H1 that
/// merely contains an `@` in prose yields nothing.
fn h1_version(h1: &str) -> Option<&str> {
    let bytes = h1.as_bytes();
    for (at, b) in bytes.iter().enumerate() {
        if *b != b'@' {
            continue;
        }
        let start = at + 1;
        if !bytes.get(start).is_some_and(u8::is_ascii_digit) {
            continue;
        }
        let mut end = start;
        while end < bytes.len() && is_version_char(bytes[end]) {
            end += 1;
        }
        return Some(&h1[start..end]);
    }
    None
}

/// Judges one README H1 against one declared version.
///
/// `readme` is the path as it should appear in a failure message. This is the
/// single comparison — the sweep and the fabricated cases below all go through
/// it, so there is no second implementation to drift.
fn judge(readme: &str, h1: &str, version: &str) -> Verdict {
    match h1_version(h1) {
        None => Verdict::Silent,
        Some(named) if named == version => Verdict::Agrees,
        Some(named) => Verdict::Disagrees(format!(
            "{readme}: the H1 names @{named} but template.json declares version {version}\n  \
             H1:            {h1}\n  \
             template.json: \"version\": \"{version}\"\n  \
             A version bump moves three places in one commit: template.json, the README H1 \
             and the row in templates/README.md where the template has one."
        )),
    }
}

// ───────────────────────────────────────────────────────────── the sweep

/// One shipped template that carries both files.
struct Template {
    /// Directory basename under `templates/`.
    name: String,
    /// The path as it appears in a failure message.
    readme: String,
    /// First line of `README.md`, trailing whitespace removed.
    h1: String,
    /// `template.json`'s `version` field.
    version: String,
}

/// Every direct child of `templates/` with both a `template.json` carrying a
/// `version` and a `README.md`.
fn shipped() -> Vec<Template> {
    let templates = core_root().join("templates");
    let entries = std::fs::read_dir(&templates).expect("templates/ is readable");
    let mut dirs: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    let mut out = Vec::new();
    for dir in dirs {
        let Ok(raw) = std::fs::read_to_string(dir.join("template.json")) else {
            continue;
        };
        let Ok(readme_raw) = std::fs::read_to_string(dir.join("README.md")) else {
            continue;
        };
        let name = dir
            .file_name()
            .expect("a directory has a name")
            .to_string_lossy()
            .into_owned();
        let manifest: Value =
            meclaw_core::serde_json::from_str(&raw).expect("template.json parses as JSON");
        let version = manifest
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("templates/{name}/template.json has a string `version`"))
            .to_string();
        out.push(Template {
            readme: format!("templates/{name}/README.md"),
            name,
            h1: readme_raw
                .lines()
                .next()
                .unwrap_or("")
                .trim_end()
                .to_string(),
            version,
        });
    }
    out
}

#[test]
fn every_readme_h1_version_agrees_with_its_template_json() {
    let mut findings = Vec::new();
    let mut judged = 0usize;
    for t in shipped() {
        match judge(&t.readme, &t.h1, &t.version) {
            Verdict::Agrees => judged += 1,
            Verdict::Disagrees(msg) => {
                judged += 1;
                findings.push(msg);
            }
            Verdict::Silent => {}
        }
    }
    assert!(
        findings.is_empty(),
        "{} template README(s) quote a version their template.json does not declare:\n\n{}",
        findings.len(),
        findings.join("\n\n")
    );
    assert!(
        judged >= MIN_JUDGED,
        "the sweep judged only {judged} template README(s) — below the floor of {MIN_JUDGED}. \
         Either templates/ moved or the H1 slot stopped being readable; a sweep that finds \
         nothing must not pass for free."
    );
}

#[test]
fn the_versionless_set_stays_inside_its_list() {
    let silent: Vec<String> = shipped()
        .into_iter()
        .filter(|t| judge(&t.readme, &t.h1, &t.version) == Verdict::Silent)
        .map(|t| t.name)
        .collect();
    let strayed: Vec<&String> = silent
        .iter()
        .filter(|n| !VERSIONLESS.contains(&n.as_str()))
        .collect();
    assert!(
        strayed.is_empty(),
        "template README H1(s) name no version and are not on the exempt list: {strayed:?}\n\
         Exempt: {VERSIONLESS:?}\n\
         Give the H1 its `@<version>`, or extend the list on purpose — dropping the version \
         is not a way out of this gate."
    );
}

// ───────────────────────────────────────────────── the test of the test

#[test]
fn h1_version_reads_the_slot_and_nothing_else() {
    assert_eq!(h1_version("# `talky@3.0.8`"), Some("3.0.8"));
    assert_eq!(
        h1_version("# canvy — a canvas the colony serves itself"),
        None
    );
    assert_eq!(h1_version("# `x@1.0.0` and more"), Some("1.0.0"));
    // An `@` that introduces no digit is not the slot.
    assert_eq!(h1_version("# ask me @ the door"), None);
    assert_eq!(
        h1_version("# `pre@1.0.0-rc.1+build`"),
        Some("1.0.0-rc.1+build")
    );
}

#[test]
fn a_fabricated_mismatch_is_reported() {
    // Two lines that no file carries: an H1 quoting one version, a manifest
    // declaring another. The sweep's own comparison must call it out.
    let verdict = judge(
        "templates/fabricated/README.md",
        "# `fabricated@9.9.9`",
        "1.0.0",
    );
    let Verdict::Disagrees(msg) = verdict else {
        panic!("a mismatched pair was not reported: {verdict:?}");
    };
    assert!(msg.contains("templates/fabricated/README.md"), "{msg}");
    assert!(msg.contains("9.9.9"), "{msg}");
    assert!(msg.contains("1.0.0"), "{msg}");

    // And the same pair agrees once the manifest catches up.
    assert_eq!(
        judge(
            "templates/fabricated/README.md",
            "# `fabricated@9.9.9`",
            "9.9.9"
        ),
        Verdict::Agrees
    );
}
