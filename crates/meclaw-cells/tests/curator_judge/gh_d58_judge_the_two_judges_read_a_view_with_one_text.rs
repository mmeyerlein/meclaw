//! D58 judge (GH #1480, contract points 1 and 7; close of wave d58-2, review
//! M5 of the judge strand) -- the route and the screen judge read a view with
//! ONE text.
//!
//! WHY this file exists: `talky/route` and `talky/screen_judge` both order a
//! view from the curator and read the answer with the same six helpers
//! (`view_target`, `leaf_text`, `leaf_data`, `frame_facts`, `kept_topics`,
//! `view_context`). Each cell is one script of its own, so the helpers stand
//! twice. The topics wire check reads only the route's copy; a change in the
//! screen judge's copy alone would let the two judges read one view two ways,
//! without a word. Each of the six stands byte for byte the same in both
//! scripts, as the curator's `# >>> parts` block does (`gh_d58_judge_parts_are_one_text`).

use meclaw_core::serde_json::Value;

const TWINS: [&str; 6] = [
    "view_target",
    "leaf_text",
    "leaf_data",
    "frame_facts",
    "kept_topics",
    "view_context",
];

/// The shipped `script_inline` of `talky/<cell>`.
fn talky_script(cell: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/talky/")
        .join(cell)
        .join("config.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let cfg: Value = meclaw_core::serde_json::from_str(&text).expect("config.json is JSON");
    cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

/// The whole text of the top-level function `name` of `script`: its `def`
/// line and every line up to the next top-level line, trailing blank lines
/// left out.
fn function(script: &str, cell: &str, name: &str) -> String {
    let head = format!("def {name}(");
    let mut lines = script.lines().skip_while(|l| !l.starts_with(&head));
    let first = lines
        .next()
        .unwrap_or_else(|| panic!("talky/{cell}: no function `{name}`"));
    let mut text = vec![first];
    for line in lines {
        if !line.is_empty() && !line.starts_with(' ') && !line.starts_with('\t') {
            break;
        }
        text.push(line);
    }
    while text.last().is_some_and(|l| l.trim().is_empty()) {
        text.pop();
    }
    text.join("\n")
}

#[test]
fn gh_d58_judge_the_two_judges_read_a_view_with_one_text() {
    let route = talky_script("route");
    let judge = talky_script("screen_judge");
    for name in TWINS {
        for (cell, script) in [("route", &route), ("screen_judge", &judge)] {
            assert_eq!(
                script.matches(&format!("\ndef {name}(")).count(),
                1,
                "talky/{cell}: `{name}` stands exactly once"
            );
        }
        assert_eq!(
            function(&judge, "screen_judge", name),
            function(&route, "route", name),
            "`{name}` in talky/screen_judge is not the text of talky/route"
        );
    }
}
