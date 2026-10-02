//! GH #951 (with GH #948): one spelling, one key. An object alias and a fact
//! subject only meet in memory when both sides write the same normal form, so
//! the objects hive carries the memory hive writer's `normalize_text` word for
//! word, fenced in `templates/objects/gate` and `templates/objects/tools`
//! between `# --8<-- normalize_text (copy of memory-hive writer, ...)` and
//! `# --8<-- end normalize_text`; `norm_key` calls it.
//!
//! 1. **The copy is the function**: the source text of `def normalize_text` in
//!    both objects cells equals the one in `templates/memory-hive/writer`.
//!    Red until the memory hive's writer carries the function (strand M) and
//!    the fenced block is swapped for its copy.
//! 2. **One spelling, one key**: case, a combining accent after its base
//!    letter and every run of whitespace do not make a second key.

#[path = "support/objects_hive.rs"]
mod objects_hive;

use meclaw_core::serde_json::{Value, json};
use objects_hive::*;

const WRITER: &str = "templates/memory-hive/writer/config.json";
const OPEN: &str = "# --8<-- normalize_text (copy of memory-hive writer, GH #948/#951)\n";
const CLOSE: &str = "# --8<-- end normalize_text\n";

/// The source text of the top-level `def <name>(...)` in `src`: its line and
/// every following line up to the next top-level one, without trailing blank
/// lines. `None` when there is no such function.
fn def_of(src: &str, name: &str) -> Option<String> {
    let head = format!("def {name}(");
    let mut lines = src.lines().skip_while(|l| !l.starts_with(&head));
    let first = lines.next()?;
    let mut out = vec![first];
    for l in lines {
        if !l.is_empty() && !l.starts_with([' ', '\t']) {
            break;
        }
        out.push(l);
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    Some(out.join("\n"))
}

#[test]
fn the_objects_copy_is_the_memory_hive_function() {
    let writer = repo(WRITER);
    if !shipped() || !writer.is_file() {
        return;
    }
    let script = read_json(&writer)["params"]["script_inline"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let theirs = def_of(&script, "normalize_text").unwrap_or_else(|| {
        panic!(
            "{WRITER} has no `def normalize_text`: red until strand M (GH #948) is merged \
             and the fenced block `normalize_text` in templates/objects/gate and \
             templates/objects/tools is swapped for its copy"
        )
    });
    for cell in ["gate", "tools"] {
        let src = script_of(cell);
        let a = src
            .find(OPEN)
            .unwrap_or_else(|| panic!("objects/{cell}: no fenced `normalize_text` block"));
        let b = src[a..]
            .find(CLOSE)
            .unwrap_or_else(|| panic!("objects/{cell}: the `normalize_text` block is not closed"));
        let ours = def_of(&src[a..a + b], "normalize_text")
            .unwrap_or_else(|| panic!("objects/{cell}: the block holds no `def normalize_text`"));
        assert_eq!(
            ours, theirs,
            "objects/{cell}: the fenced `normalize_text` is not the memory hive writer's -- \
             swap the whole block for a copy of {WRITER} (GH #948/#951)"
        );
    }
}

#[test]
fn one_spelling_one_key() {
    if !shipped() {
        return;
    }
    let table: [(&str, &str); 9] = [
        ("Blue Bike", "blue bike"),
        ("BLUE bike", "blue bike"),
        ("  blue \t bike\n", "blue bike"),
        ("blue\u{a0}bike", "blue bike"),
        ("Cafe\u{301}", "caf\u{e9}"),
        ("CAFE\u{301}", "caf\u{e9}"),
        ("caf\u{e9}", "caf\u{e9}"),
        ("Stra\u{df}e", "stra\u{df}e"),
        ("", ""),
    ];
    let inputs: Vec<&str> = table.iter().map(|(i, _)| *i).collect();
    let want: Vec<Value> = table.iter().map(|(_, o)| json!([o, o])).collect();
    for cell in ["gate", "tools"] {
        assert_eq!(
            pure(
                cell,
                "[[normalize_text(x), norm_key(x)] for x in ARGS]",
                json!(inputs)
            ),
            json!(want),
            "objects/{cell}: `normalize_text` and `norm_key` give one key per spelling"
        );
    }
}
