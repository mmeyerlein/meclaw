//! GH #663 — the counting road leaves the builder.
//!
//! `templates/builder/tally` counted the members an organisation already
//! carried so that `recipes` could pick a free port for the next screen. Since
//! `display@2.0.0` / `builder@1.8.0` a screen has a MOUNT and no number: the
//! count was still taken, still paid for with a `/colony` round trip and two
//! store operations per member wish, and spent on nothing.
//!
//! Two claims, and the first one is a drift lock in the sense of the
//! development rules § 2d — it greps the SENTENCE out of the shipped template
//! and asserts the MECHANISM behind it, so a removal that left the prose
//! standing (or prose that outlived the removal) is red either way:
//!
//! 1. No path in the builder names the counting road any more — not a cell,
//!    not an edge, not a context key, not a sentence. The one exception is
//!    `template.json`'s `description.purpose`, which is the template's own
//!    HISTORY and says what USED to be true (the issue: *"`template.json`
//!    history stays as history"*).
//! 2. Every wish goes straight to the renderer: `classify` routes a member
//!    wish on `recipe` like every other level, and there is no `count` route
//!    left to take.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const CLASSIFY: &str = "templates/builder/classify/config.json";

/// The organisation the builder's own examples are written for.
const ORG: &str = "/os/orgs/acme";

/// Every word the counting road was made of. A key, a code, a cell name — each
/// one of them is a path into the mechanism, and a template that still spells
/// one of them still carries part of the road.
const GONE: &[&str] = &["tally", "member_index", "count_unavailable"];

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    repo(CLASSIFY).is_file()
}

/// Every file of the builder template, except the one that carries its history.
fn builder_files() -> Vec<std::path::PathBuf> {
    let root = repo("templates/builder");
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("the builder template") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path != root.join("template.json") {
                out.push(path);
            }
        }
    }
    out
}

/// The one claim the removal makes, greped out of the shipped tree.
#[test]
fn nothing_in_the_builder_counts_members_any_more() {
    if !shipped() {
        return;
    }
    assert!(
        !repo("templates/builder/tally").exists(),
        "the counting cell is a directory, and a directory nobody routes to is \
         still a cell the registry grows"
    );
    for file in builder_files() {
        let raw = std::fs::read_to_string(&file).unwrap_or_default();
        for word in GONE {
            assert!(
                !raw.contains(word),
                "{} still spells {word:?}: the road is gone, so the prose and \
                 the keys that described it are gone with it",
                file.display()
            );
        }
    }

    // The mechanism behind the sentence: the hive routes no `count` lane and
    // draws no edge onto the cell that served it.
    let hive: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(repo("templates/builder/config.json")).expect("the hive"),
    )
    .expect("parses");
    for edge in hive["params"]["graph"]["edges"].as_array().expect("edges") {
        let cond = edge["condition"].as_str().unwrap_or("");
        assert!(
            !cond.contains("'count'"),
            "an edge still conditions on the counting route: {edge:?}"
        );
    }

    // And the history stays history: the purpose block is allowed to say what
    // used to be true, and it must still say something.
    let template: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(repo("templates/builder/template.json")).expect("template.json"),
    )
    .expect("parses");
    assert!(
        template["description"]["purpose"]
            .as_str()
            .expect("a purpose")
            .len()
            > 500,
        "the history is not what was removed"
    );
}

/// The switch sends every wish to the renderer, the member wish included.
#[test]
fn every_wish_goes_straight_to_the_renderer() {
    if !shipped() {
        return;
    }
    let route_of = |level: &str, template: &str, name: &str, scope: &str| -> Value {
        let out = emit_all(
            &shipped_script(repo(CLASSIFY).to_str().expect("utf-8 path")),
            &json!({
                "target": "/os/builder/classify",
                "header": {"hop": {"route": "in_build"}, "context": {}},
                "ttl": 64,
                "messages": [{"origin": "tool", "type": "tool_call", "id": "c1",
                    "text": json!({"request": "…", "recipe": "grow_level",
                        "params": {"scope": scope, "level": level, "name": name,
                                   "template": template}}).to_string()}],
            }),
        );
        out[0]["header"]["route"].clone()
    };
    assert_eq!(
        route_of("member", "member@1.7.0", "alex", ORG),
        json!("recipe"),
        "a member wish is a wish like any other now: nothing about it has to be \
         read off the tree first"
    );
    assert_eq!(
        route_of("org", "org@1.4.1", "acme", "/os"),
        json!("recipe"),
        "and the level that never took the detour is unchanged"
    );
}

/// The corpus is retrieved in CHUNKS, and a chunk is what a lane gets to read.
/// The template's history therefore has to survive being cut: every chunk that
/// still spells a word of the counting road has to carry the version that
/// ended it, or that chunk on its own describes a cell the colony no longer
/// has. Development rules § 2d, applied to the seed rather than to the file.
#[test]
fn no_corpus_chunk_describes_the_counting_road_as_present() {
    let seed = repo("templates/builder-librarian/store/seed/docs.jsonl");
    if !seed.is_file() {
        return;
    }
    /// `contains` on a substring answers yes inside `incrementally`; the road
    /// is spelled as whole words, so the check reads whole words.
    fn names(haystack: &str, word: &str) -> bool {
        let edge = |c: Option<char>| match c {
            None => true,
            Some(c) => !(c.is_alphanumeric() || c == '_' || c == '-'),
        };
        haystack.match_indices(word).any(|(at, _)| {
            edge(haystack[..at].chars().next_back())
                && edge(haystack[at + word.len()..].chars().next())
        })
    }
    let raw = std::fs::read_to_string(&seed).expect("the seed corpus");
    let mut carried = 0usize;
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let doc: Value = meclaw_core::serde_json::from_str(line).expect("a seed row parses");
        let text = doc["text"].as_str().unwrap_or_default();
        let id = doc["id"].as_str().unwrap_or("<unnamed>");
        for word in GONE {
            if names(text, word) {
                carried += 1;
                assert!(
                    text.contains("1.9.0"),
                    "seed chunk {id} spells {word:?} without naming the version \
                     that removed it: retrieved alone, it describes a cell this \
                     template no longer carries"
                );
            }
        }
    }
    assert!(
        carried > 0,
        "the history is supposed to stay in the corpus — if no chunk names the \
         road at all, this lock stopped guarding anything"
    );
}
