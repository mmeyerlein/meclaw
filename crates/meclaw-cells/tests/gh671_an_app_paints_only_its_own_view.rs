//! GH #671 -- an application paints its own view, not the page.
//!
//! A view's `client_css` goes into the page raw, so a rule on `html`, `body` or
//! `:root` written by one application paints over every other view standing
//! beside it on the same screen, and over the screen's own ground. Measured on
//! 0.35.0: `colony-view`'s sheet wrote `html,body{…background…}` and two
//! document-level colour-scheme blocks on `body`, and the hand-run bridge had
//! attributed those to the host page.
//!
//! The rule is NOT enforced by the `web` cell -- a CSS parser in the substrate
//! would be a second language in it -- so what holds it is this file: the
//! shipped app sheet is read and every selector whose subject is the document
//! is refused (ADR 0036). The three copies of the sheet are kept in step by
//! `gh455_the_two_templates_ship::the_browser_half_is_the_same_bytes_in_three_places`;
//! this test reads the file a person edits AND the copy the runtime is handed.

use meclaw_core::serde_json::Value;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read(p: &std::path::Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// A view's own stylesheet describes the view. `html`, `body`, `:root` and a
/// document-level colour-scheme query belong to the screen: an application that
/// writes one paints over every other view standing beside it.
///
/// The subject of a selector is its last compound -- the element the rule
/// lands on. `:root[data-theme=dark] .colony-view` is a rule on the view and
/// stays; `:root[data-theme=dark] body` is a rule on the document and goes.
///
/// It catches the BARE document subject (`html`, `body`, `:root`); `body:not(.x)`,
/// `body[data-x]` and `body::before` pass -- a lock against oversight, not
/// against evasion.
fn subject_is_the_document(selector: &str) -> bool {
    selector.split(',').any(|part| {
        let subject = part
            .split(|c: char| c.is_whitespace() || matches!(c, '>' | '+' | '~'))
            .rfind(|s| !s.is_empty())
            .unwrap_or("");
        matches!(subject, "html" | "body" | ":root")
    })
}

/// Every selector in a sheet with the line it starts on. Comments are blanked
/// (their newlines kept, so the line numbers stay the file's), at-rule
/// preludes are skipped and their blocks descended into. Deliberately dumb: a
/// sheet that needed more than this to be read would be the wrong sheet.
fn selectors(sheet: &str) -> Vec<(usize, String)> {
    let mut plain = String::with_capacity(sheet.len());
    let mut rest = sheet;
    while let Some(open) = rest.find("/*") {
        plain.push_str(&rest[..open]);
        let close = rest[open..].find("*/").map_or(rest.len(), |c| open + c + 2);
        plain.extend(rest[open..close].chars().filter(|c| *c == '\n'));
        rest = &rest[close..];
    }
    plain.push_str(rest);

    let mut out = Vec::new();
    let mut buf = String::new();
    let mut line = 1;
    let mut buf_line = 1;
    for c in plain.chars() {
        match c {
            '{' => {
                let sel = buf.trim();
                if !sel.is_empty() && !sel.starts_with('@') {
                    out.push((buf_line, sel.to_string()));
                }
                buf.clear();
                buf_line = line;
            }
            '}' | ';' => {
                buf.clear();
                buf_line = line;
            }
            _ => {
                if buf.trim().is_empty() {
                    buf_line = line;
                }
                buf.push(c);
            }
        }
        if c == '\n' {
            line += 1;
        }
    }
    out
}

/// The sheet as the runtime gets it: `CLIENT_CSS` cut out of the `layout.py`
/// that `config.json` carries in `script_inline`. Same dumb extraction as the
/// three-copies lock.
fn shipped_client_css(config: &std::path::Path) -> String {
    let cfg: Value = meclaw_core::serde_json::from_str(&read(config))
        .unwrap_or_else(|e| panic!("{}: {e}", config.display()));
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("layout/config.json carries script_inline");
    let open = "CLIENT_CSS = r\"\"\"";
    let start = script.find(open).expect("script_inline carries CLIENT_CSS") + open.len();
    let end = script[start..].find("\"\"\"").expect("CLIENT_CSS closes") + start;
    script[start..end].to_string()
}

#[test]
fn an_app_paints_only_its_own_view() {
    let dir = repo("templates/colony-view/layout");
    let sheets = [
        ("colony-view.css", read(&dir.join("colony-view.css"))),
        (
            "config.json:script_inline",
            shipped_client_css(&dir.join("config.json")),
        ),
    ];
    for (name, sheet) in &sheets {
        let hits: Vec<String> = selectors(sheet)
            .into_iter()
            .filter(|(_, sel)| subject_is_the_document(sel))
            .map(|(line, sel)| format!("{name}:{line}: {sel}"))
            .collect();
        assert!(
            hits.is_empty(),
            "the app's sheet writes a rule on the document; the page's ground belongs \
             to the screen (ADR 0036):\n{}",
            hits.join("\n")
        );
    }

    // The sheet still describes the view, and the view still has a size of its
    // own inside a view slot -- the height rule replaced the document rules and
    // is not simply gone with them.
    let sheet = &sheets[0].1;
    assert!(
        sheet.contains(".colony-view{ position:relative; height:62vh; min-height:22rem;"),
        "the view carries its own in-view height"
    );
    assert!(
        !sheet.contains("inset:0"),
        "a view does not fill the page it is placed on"
    );

    // The since-sentence on the template surface names this repair, and the
    // two halves above are its mechanism (development rules § 2d).
    let tpl: Value =
        meclaw_core::serde_json::from_str(&read(&repo("templates/colony-view/template.json")))
            .expect("template.json parses");
    assert!(
        tpl["description"]["purpose"]
            .as_str()
            .unwrap_or("")
            .contains("Since 1.1.1 the view's stylesheet describes the view and nothing else"),
        "the template's purpose names the repair it ships"
    );
}

#[test]
fn the_document_subject_is_read_off_the_last_compound() {
    for refused in [
        "html,body",
        "body",
        ":root",
        ":root:not([data-theme=light]) body",
        ":root[data-theme=dark] body",
        ".a, html",
        "div > body",
        ".a,\nbody",
        ":root[data-theme=dark]\nbody",
        ":root\tbody",
    ] {
        assert!(
            subject_is_the_document(refused),
            "{refused:?} is a document rule"
        );
    }
    for allowed in [
        ".colony-view",
        ":root[data-theme=dark] .colony-view",
        ":root:not([data-theme=light]) .colony-view",
        "[data-phx-main].phx-loading .colony-view",
        "body.dark",
        "html .colony-view",
        ".colony-view .node text.nm",
    ] {
        assert!(
            !subject_is_the_document(allowed),
            "{allowed:?} is a rule on the view"
        );
    }
}

#[test]
fn the_selector_walk_reads_nested_blocks_and_skips_comments() {
    let sheet = "/* html,body{ in a comment } */\n\
                 .a{ color:red; }\n\
                 @media (x){ :root b{ c:d } .e{ f:g } }\n\
                 .f{ content:\"x\"; }\n";
    let got = selectors(sheet);
    assert_eq!(
        got,
        vec![
            (2, ".a".to_string()),
            (3, ":root b".to_string()),
            (3, ".e".to_string()),
            (4, ".f".to_string()),
        ]
    );
    // The two halves compose: a document rule inside an at-rule block is found.
    assert!(
        selectors("@media (x){ body{} }")
            .iter()
            .any(|(_, s)| subject_is_the_document(s))
    );
}

/// The rule is written down where a template author reads, and it is written
/// down as one the cell does NOT enforce. Both halves are asserted: the
/// sentence, and that no ops path in the `web` cell has grown a selector check
/// behind it -- a CSS parser in the substrate would be a second language, and
/// the README already says for the neighbouring `backdrop-filter` case why such
/// a thing is named rather than pretended.
#[test]
fn the_readme_names_the_rule_the_cell_does_not_enforce() {
    // ── The sentence. ───────────────────────────────────────────────────────
    let readme = read(&repo("templates/web/README.md"));
    let rules = readme
        .find("The rules are about the class vocabulary, not about CSS in general")
        .expect("the README keeps the heading sentence the rule is filed under");
    let after = &readme[rules..];
    let section = &after[..after.find("\n## ").unwrap_or(after.len())];
    for needed in [
        "`html`, `body`, `:root`",
        "`@media (prefers-color-scheme)`",
        "belong to the screen",
        "does not check",
    ] {
        assert!(
            section.contains(needed),
            "the README names the rule under the class-vocabulary sentence: missing {needed:?}"
        );
    }

    // ── The mechanism: there is none, on purpose. ───────────────────────────
    // The Rust word `body` is an ordinary identifier in `ops.rs` (a slice of a
    // class attribute), so the assertion is on CSS-selector LITERALS: a quoted
    // `html`/`body` followed by what a selector is followed by, a quoted
    // `:root`, the document-level media query, or any mention of the prop the
    // sheet travels in.
    let ops = read(&repo("crates/meclaw-cells/src/web/ops.rs"));
    for literal in [
        "\"html{",
        "\"html,",
        "\"html ",
        "\"body{",
        "\"body,",
        "\"body ",
        "\":root",
        "prefers-color-scheme",
        "client_css",
    ] {
        assert!(
            !ops.contains(literal),
            "ops.rs checks a document selector ({literal:?}); the README says the cell does \
             not, and the sheet is not a language the substrate reads"
        );
    }
}
