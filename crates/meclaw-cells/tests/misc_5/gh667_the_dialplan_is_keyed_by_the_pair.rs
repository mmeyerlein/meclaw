//! GH #667 — the dialplan contract resolves a call by the PAIR (dialled number,
//! caller number), and the realm is what carries the first half.
//!
//! Up to `freeswitch@2.0.2` the switch's line table was keyed by the caller alone:
//! `db(select/meclaw_lines/${caller_id_number})`, with `destination_number`
//! checked only for being a number. One caller number therefore reached ONE
//! colony, whichever number they had dialled — a switch with two numbers in
//! front of two colonies could not tell them apart for a caller who was on both.
//!
//! The repair changes no cell. `mod_db` addresses a row by realm and key, so a
//! realm per dialled number is the second half of the pair: the dialplan reads
//! `db(select/meclaw_lines_${destination_number}/${caller_id_number})`, and a
//! colony behind a shared switch names the realm of the line it is reached on in
//! `params.db_realm` (`meclaw_lines_<dialled number>`), so that `add_number`,
//! `set_pin` and `disable_pin` — which all write through the one `db_url` that
//! reads that knob — land in the realm the dialplan reads for that number. The
//! caller-only form stays as the one-number case: one switch, one number, the
//! shipped default.
//!
//! A drift lock in the sense of the development rules § 2d, in both halves:
//!
//! - the SENTENCE: the `meclaw_line` block is cut out of the README and both
//!   `db(select/…)` lookups in it are asserted, as is the one-number form in the
//!   prose beside it and the `db_realm` row of the param table;
//! - the MECHANISM: the realm's name is not typed into this test. It is DERIVED
//!   from the shipped default of `db_realm` in `signal/config.json`, and the
//!   script is asserted to read that knob with the same fallback, once, and to
//!   route all three line tools through it. The number stands once (§ 2d).

use meclaw_core::serde_json::Value;

const SIGNAL: &str = "templates/freeswitch/signal/config.json";
const README: &str = "templates/freeswitch/README.md";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn signal() -> Value {
    meclaw_core::serde_json::from_str(&read(SIGNAL)).expect("the signal half parses")
}

/// The shipped realm, read from the ONE place it is declared.
fn shipped_realm(cfg: &Value) -> String {
    let default = cfg["contract"]["settings"]["db_realm"]["default"]
        .as_str()
        .expect("`db_realm` is a declared setting with a string default")
        .to_string();
    assert_eq!(
        cfg["params"]["db_realm"].as_str(),
        Some(default.as_str()),
        "the param value and the contract default are one realm"
    );
    default
}

/// The half-sentence that says whose form `<dialled number>` takes — the same
/// words in the README's prose, its param table and the config description.
const THE_SWITCHS_FORM: &str =
    "as it stands in `destination_number` on that switch, `+` included if the trunk delivers one";

/// Wrapped prose as one line, so a phrase can be asserted across a line break.
fn unwrapped(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The sentence of `text` (already unwrapped) that contains `needle`: from the
/// previous full stop to the next one.
fn sentence_around(text: &str, needle: &str) -> Option<String> {
    let at = text.find(needle)?;
    let start = text[..at].rfind(". ").map(|i| i + 2).unwrap_or(0);
    let end = text[at..]
        .find(". ")
        .map(|i| at + i + 1)
        .unwrap_or(text.len());
    Some(text[start..end].to_string())
}

/// The section of the README that owes the switch its dialplan.
fn dialplan_section(readme: &str) -> String {
    let start = readme
        .find("## What the dialplan owes")
        .expect("the README has § What the dialplan owes");
    let rest = &readme[start..];
    let end = rest[3..].find("\n## ").map(|i| i + 3).unwrap_or(rest.len());
    rest[..end].to_string()
}

/// The `meclaw_line` extension, cut out of the XML the README ships.
fn meclaw_line_block(section: &str) -> String {
    let open = "<extension name=\"meclaw_line\">";
    let start = section
        .find(open)
        .expect("the block declares the meclaw_line extension");
    let rest = &section[start..];
    let end = rest.find("</extension>").expect("and closes it");
    rest[..end].to_string()
}

#[test]
fn the_dialplan_block_is_keyed_by_the_pair_and_the_tools_write_that_realm() {
    let cfg = signal();
    let realm = shipped_realm(&cfg);
    let readme = read(README);
    let section = dialplan_section(&readme);
    let block = meclaw_line_block(&section);

    // THE SENTENCE, first half: the row is looked up by the pair. The realm is
    // derived, not typed — `<shipped default>_${destination_number}` — so the
    // README cannot spell a prefix the config does not ship.
    let pair_lookup = format!("db(select/{realm}_${{destination_number}}/${{caller_id_number}})");
    assert!(
        block.contains(&pair_lookup),
        "the meclaw_line block reads the line row by (dialled number, caller): \
         expected {pair_lookup:?} in\n{block}"
    );
    // The PIN row is read from the SAME realm: `set_pin` writes it there.
    let pin_lookup = format!("db(select/{realm}_${{destination_number}}/pin.${{meclaw_user}})");
    assert!(
        block.contains(&pin_lookup),
        "the PIN row lives in the realm of the dialled number too, because \
         set_pin writes through the same db_realm: expected {pin_lookup:?}"
    );
    // And the caller-only form is gone from the block. It is not a second way
    // of reading the same row; it is the one-number case, and it is explained
    // as that in the prose, not shipped as the default XML.
    let caller_only = format!("db(select/{realm}/${{caller_id_number}})");
    assert!(
        !block.contains(&caller_only),
        "the shipped block resolves the pair; the caller-only lookup is the \
         one-number case and lives in the prose"
    );
    // The prose is wrapped; read it as one line before looking for a phrase.
    let prose = unwrapped(&section.replace(&block, ""));
    let one_number_sentence = sentence_around(&prose, &caller_only).unwrap_or_else(|| {
        panic!("the prose beside the block explains {caller_only:?} as the one-number case")
    });
    assert!(
        one_number_sentence.contains("one number"),
        "the caller-only form is named as the one-number case where it appears: \
         {one_number_sentence:?}"
    );
    // And the one-number case says what to CHANGE. The block as shipped reads
    // `<realm>_${destination_number}`; a switch that keeps the default realm and
    // copies the block as it stands selects an empty row on every call and
    // rejects it with nothing in the log to say why. So the sentence is
    // imperative, and names both ways out — in the derived shape.
    let replace = format!("replace `{realm}_${{destination_number}}` by `{realm}` in both lookups");
    let or_set = format!("set `db_realm` to `{realm}_<dialled number>` and keep the block");
    assert!(
        prose.contains(&replace) && prose.contains(&or_set),
        "the one-number case tells the operator what to change: expected \
         {replace:?} and {or_set:?}"
    );
    // The realm's form is the switch's: `<dialled number>` is the string the
    // switch presents in `destination_number` after its trunk and inbound
    // rewrites, `+` included when the trunk delivers one — not a canonical form
    // this colony chooses. The first condition of the block admits both.
    assert!(
        block.contains("expression=\"^\\+?\\d+$\""),
        "the block admits a dialled number with or without `+`, which is why the \
         realm's form has to be said"
    );
    assert!(
        prose.contains(THE_SWITCHS_FORM),
        "the prose says whose form the dialled number takes: expected {THE_SWITCHS_FORM:?}"
    );
    // The two properties that fall out of the pair are both said out loud.
    assert!(
        prose.contains("different colonies on different numbers")
            && prose.contains("many callers on many colonies"),
        "the section names both directions of the pair: one caller reaching \
         different colonies on different numbers, one number serving many \
         callers on many colonies"
    );

    // THE SENTENCE, second half: the `db_realm` row of the param table says which
    // realm a colony behind a shared switch names — the realm of the line it is
    // reached on, in the same derived shape.
    let shared_realm = format!("{realm}_<dialled number>");
    let param_row = readme
        .lines()
        .find(|l| l.starts_with("| `db_realm` |"))
        .expect("the param table has a db_realm row");
    assert!(
        param_row.contains(&shared_realm),
        "the db_realm row names the realm of the line a colony is reached on: \
         expected {shared_realm:?} in {param_row:?}"
    );
    assert!(
        param_row.contains(THE_SWITCHS_FORM),
        "and whose form that number takes: expected {THE_SWITCHS_FORM:?} in {param_row:?}"
    );
    let description = cfg["contract"]["settings"]["db_realm"]["description"]
        .as_str()
        .expect("db_realm carries a description");
    assert!(
        description.contains(&shared_realm) && description.contains(THE_SWITCHS_FORM),
        "the config description says the same, in the same shape: expected \
         {shared_realm:?} and {THE_SWITCHS_FORM:?} in {description:?}"
    );

    // THE MECHANISM: every line tool writes the realm the knob names, and the
    // knob's fallback is the shipped default — the realm stands once in the
    // script, in that knob, and nowhere as a literal path.
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let knob = format!("realm = str(knob(\"db_realm\", \"{realm}\"))");
    assert!(
        script.contains(&knob),
        "the signal half reads the realm from `db_realm`, falling back to the \
         shipped default: expected {knob:?}"
    );
    assert_eq!(
        script.matches(&realm).count(),
        1,
        "the realm's name appears exactly once in the script — as that fallback. \
         A second spelling would be a path the param cannot move"
    );
    assert!(
        script.contains("quote(\"insert/%s/%s/%s\" % (realm, key, value))")
            && script.contains("quote(\"delete/%s/%s\" % (realm, key))"),
        "both `db` commands are addressed by that realm"
    );
    for (tool, call) in [
        ("add_number", "db_url(\"insert\", number, "),
        ("set_pin", "db_url(\"insert\", \"pin.%s\" % user, "),
        ("disable_pin", "db_url(\"delete\", \"pin.%s\" % user"),
    ] {
        assert!(
            script.contains(call),
            "`{tool}` writes through db_url, so it lands in the realm the \
             dialplan reads for that number: expected {call:?}"
        );
    }
    let line_ops = script
        .lines()
        .find(|l| l.starts_with("LINE_OPS = "))
        .expect("the script declares LINE_OPS");
    assert_eq!(
        line_ops, "LINE_OPS = (\"add_number\", \"set_pin\", \"disable_pin\")",
        "three line tools, and the three asserted above are all of them"
    );
}

/// A realm outlives the switch, and the README says so with its reason: `mod_db`
/// keeps its rows in the `db_data` table of the switch's own database and, on
/// load, creates that table only where it is missing — it purges `limit_data`
/// for its host and touches `db_data` not at all — and an `insert` is a delete
/// of the same key followed by the insert, under a unique index on
/// `(data_key, realm)`. The mechanism is FreeSWITCH's (`mod_db.c`, `do_config`
/// and `db_api_function`), which is why this half pins the README's citation of
/// it rather than a line of this tree — the same form the four-token claim of
/// § *What leaves for the switch* already takes.
#[test]
fn the_readme_says_a_realm_is_persistent_and_cites_the_table() {
    let readme = read(README);
    let start = readme
        .find("## What leaves for the switch")
        .expect("§ What leaves for the switch");
    let section = &readme[start..];
    let end = section[3..]
        .find("\n## ")
        .map(|i| i + 3)
        .unwrap_or(section.len());
    let section = unwrapped(&section[..end]);
    let bullet = section
        .find("* **A realm is persistent")
        .map(|i| &section[i..])
        .expect("the note says a realm is persistent, as the bullet's own claim");
    let bullet = &bullet[..bullet[2..]
        .find("* **")
        .map(|i| i + 2)
        .unwrap_or(bullet.len())];
    assert!(
        bullet.contains("`db_data`") && bullet.contains("outlive a restart"),
        "and names the table the rows are in and what that means for a row: \
         {bullet:?}"
    );
    assert!(
        bullet.contains("`do_config`") && bullet.contains("`limit_data`"),
        "the reason is cited against mod_db.c: the load path recreates `db_data` \
         only where it is missing and purges `limit_data` alone"
    );
    assert!(
        bullet.contains("through `mod_db`")
            && bullet.contains("`db delete`")
            && bullet.contains("overwrites"),
        "and the two ways a row ends THROUGH THE MODULE are named: a delete, or an \
         insert on the same key, which overwrites — what the host does to the \
         file or the backend is outside that count"
    );
}
