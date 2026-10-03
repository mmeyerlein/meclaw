//! GH #958 -- the catalogue is the one source of the screen's components.
//!
//! `templates/display/compose/catalog.json` names every component the display
//! defines: its role, its layer, its props with their types, which children it
//! takes, a sentence of what it shows and one valid instance. `compose.py`
//! carries it embedded and GENERATES `component.define` and the typed door out
//! of it. Three things are pinned here:
//!
//! (a) the thirty-six components that existed before the catalogue are defined
//!     byte for byte as before -- the hash of each `component.define` bag, taken
//!     from the code before the change, is checked in beside this file. The
//!     `web` cell and every browser lock read those bags; a drift here would be a
//!     silent change everywhere at once.
//! (b) every component of the catalogue says what it shows and carries an
//!     example that passes the door.
//! (c) `components()` and `VOCAB` follow the catalogue: same names, same order,
//!     the same types, and the fingerprint of exactly that list.
//!
//! Plus the door's own table: what a type takes and what it refuses.
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! library is skipped, and so is a host without `python3`.

use meclaw_core::serde_json::{self, Value};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const CATALOG: &str = "templates/display/compose/catalog.json";
const BEFORE: &str = "crates/meclaw-cells/tests/fixtures/gh958_component_define_before.json";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// One Python expression over the loaded `compose.py` module `m`, as JSON.
fn ask(expr: &str) -> Option<Value> {
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(format!(
            "import importlib.util, json, sys, hashlib\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             print(json.dumps({expr}))"
        ))
        .arg(repo(COMPOSE))
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(serde_json::from_slice(&out.stdout).expect("the answer is JSON"))
}

fn catalog() -> Value {
    serde_json::from_str(&std::fs::read_to_string(repo(CATALOG)).expect("catalog.json ships"))
        .expect("catalog.json parses")
}

fn entries(cat: &Value) -> &Vec<Value> {
    cat["components"].as_array().expect("components is a list")
}

/// (a) The thirty-six of before, byte for byte: the sha256 of each bag as
/// canonical JSON (`sort_keys`, the spelling `tool_call` puts on the wire).
///
/// The shell's template embeds the whole sheet (`KIT_CSS`), so every sheet
/// line would move its hash -- this strand's own card/steps rules did, and the
/// next strand's motion rules would again; a golden every CSS change has to
/// re-pull pins nothing. The sheet is replaced by a marker before hashing, in
/// the golden and here; its equality with `display-dna.css` is held by
/// `gh669_the_screen_ships_its_design_language_in_three_places`. Everything
/// else of the shell (`prop_schema`, `layer`, `editable`, the markup around
/// the sheet) stays in the hash.
#[test]
fn the_thirty_six_are_defined_as_before() {
    if !library_ships() {
        return;
    }
    let Some(now) = ask("dict((c['name'], hashlib.sha256(json.dumps(dict(c, \
         template=c['template'].replace(m.KIT_CSS, '<KIT_CSS>')), sort_keys=True)\
         .encode('utf-8')).hexdigest()) for c in m.components())")
    else {
        return;
    };
    let before: Value =
        serde_json::from_str(&std::fs::read_to_string(repo(BEFORE)).expect("the golden"))
            .expect("the golden parses");
    let before = before["components"].as_object().expect("a map");
    assert_eq!(before.len(), 36, "the golden holds the thirty-six");
    for (name, hash) in before {
        assert_eq!(
            &now[name], hash,
            "`{name}`: component.define is no longer what it was before the catalogue"
        );
    }
}

/// (b) Every entry describes itself and carries an example the door takes.
#[test]
fn every_entry_describes_itself_and_its_example_passes_the_door() {
    if !library_ships() {
        return;
    }
    let cat = catalog();
    for e in entries(&cat) {
        let name = e["name"].as_str().expect("a name");
        let describe = e["describe"].as_str().unwrap_or("");
        assert!(
            describe.len() > 10,
            "`{name}` says nothing about what it shows"
        );
        assert_eq!(
            e["example"]["component"], e["name"],
            "`{name}`: the example is another component"
        );
        for (prop, spec) in e["props"].as_object().expect("props") {
            assert!(
                ["text", "int", "number", "boolean", "html"]
                    .contains(&spec["type"].as_str().unwrap_or("")),
                "`{name}.{prop}` has no type the catalogue knows"
            );
            assert!(
                spec["describe"].as_str().is_some_and(|d| !d.is_empty()),
                "`{name}.{prop}` is not described"
            );
            assert!(
                spec["required"].is_boolean(),
                "`{name}.{prop}` says nothing of required"
            );
        }
        let slots = &e["slots"];
        assert!(
            slots == "any" || slots == "none" || slots.is_array(),
            "`{name}`: slots is any, none or a list"
        );
        assert!(e["block"].is_boolean(), "`{name}` says nothing of block");
        assert!(
            ["own", "window", "content"].contains(&e["role"].as_str().unwrap_or("")),
            "`{name}` has no role"
        );
    }
    let Some(refusals) =
        ask("dict((c['name'], m.typed_refusal(c['example'])) for c in m.CATALOG['components'])")
    else {
        return;
    };
    for (name, why) in refusals.as_object().expect("a map") {
        assert!(
            why.is_null(),
            "the door refuses the example of `{name}`: {why}"
        );
    }
}

/// (c) `components()` is the catalogue, in its order, with its types, and
/// `VOCAB` is the fingerprint of exactly that list.
#[test]
fn components_and_the_vocabulary_follow_the_catalogue() {
    if !library_ships() {
        return;
    }
    let Some(got) = ask(
        "{'components': m.components(), 'vocab': m.VOCAB, 'want': hashlib.sha256(\
         json.dumps(m.components(), sort_keys=True).encode('utf-8')).hexdigest()[:12]}",
    ) else {
        return;
    };
    let cat = catalog();
    let defined = got["components"].as_array().expect("a list");
    let names: Vec<&str> = defined
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    let listed: Vec<&str> = entries(&cat)
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names, listed,
        "components() defines the catalogue, in its order"
    );
    let curated = cat["curated"].as_object().expect("the curated group");
    for (c, e) in defined.iter().zip(entries(&cat)) {
        let name = e["name"].as_str().unwrap();
        assert_eq!(c["layer"], e["layer"], "`{name}`: layer");
        let schema = c["prop_schema"].as_object().expect("a schema");
        let mut want: usize = e["props"].as_object().unwrap().len();
        for (prop, spec) in e["props"].as_object().unwrap() {
            assert_eq!(schema[prop], spec["type"], "`{name}.{prop}`: type");
        }
        if e["curated"] == true {
            want += curated.len();
            for (prop, spec) in curated {
                assert_eq!(schema[prop], spec["type"], "`{name}.{prop}`: curated type");
            }
        }
        assert_eq!(
            schema.len(),
            want,
            "`{name}` declares exactly the catalogue's props"
        );
    }
    assert_eq!(
        got["vocab"], got["want"],
        "VOCAB is the fingerprint of components()"
    );
    // The three new blocks stand in the catalogue with the props named in the plan.
    for (name, props) in [
        (
            "display-card",
            vec!["kicker", "title", "body", "value", "unit"],
        ),
        ("display-steps", vec!["title"]),
        ("display-step", vec!["label", "state", "detail", "at"]),
    ] {
        let e = entries(&cat)
            .iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("`{name}` is not in the catalogue"));
        assert_eq!(e["block"], true, "`{name}` is a block");
        for p in props {
            assert!(e["props"][p].is_object(), "`{name}` declares `{p}`");
        }
    }
    let steps = entries(&cat)
        .iter()
        .find(|e| e["name"] == "display-steps")
        .unwrap();
    assert_eq!(steps["slots"], serde_json::json!(["display-step"]));
}

/// The door's type table, asked of the script: what each type takes, what it
/// refuses, and the form of the reason.
#[test]
fn the_door_types_props_as_the_catalogue_spells_them() {
    if !library_ships() {
        return;
    }
    let Some(fits) = ask("[m.type_fits(k, v) for k, v in [\
         ('int', 3), ('int', '42'), ('int', '-7'), ('int', 'abc'), ('int', True), ('int', 2.5),\
         ('number', 2.5), ('number', '2.5'), ('number', 'x'),\
         ('boolean', True), ('boolean', 'false'), ('boolean', '1'),\
         ('text', 'a'), ('text', 3), ('text', ['a']), ('html', '<b>x</b>'), ('html', 3),\
         ('int', None)]]")
    else {
        return;
    };
    let want = [
        true, true, true, false, false, false, true, true, false, true, true, false, true, true,
        false, true, false, true,
    ];
    let got: Vec<bool> = fits
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_bool().unwrap())
        .collect();
    assert_eq!(got, want);
    let Some(why) = ask(
        "[m.typed_refusal({'component': 'display-progress', 'props': {'value': 'abc'}}),\
          m.typed_refusal({'component': 'display-step', 'props': {'state': 'done'}}),\
          m.typed_refusal({'component': 'display-steps', 'children': [\
            {'component': 'display-text', 'props': {'body': 'x'}}]}),\
          m.typed_refusal({'component': 'display-pane', 'children': [\
            {'component': 'display-progress', 'props': {'value': 40}}]}),\
          m.typed_refusal({'component': 'weather-own', 'props': {'anything': [1]}}),\
          m.typed_refusal({'component': 'display-pane', 'props': {'context': ['a']}}),\
          m.typed_refusal({'component': 'display-pane', 'children': [\
            {'component': 'display-steps', 'children': [\
              {'component': 'display-step', 'props': {'label': 'x', 'state': ['done']}}]}]})]",
    ) else {
        return;
    };
    assert_eq!(
        why,
        serde_json::json!([
            "display-progress.value: int",
            "display-step.label: required text",
            "display-steps.children: display-step",
            null,
            null,
            // a hint on the window is the one door's (`view_refused`, § 3.3), not this one's
            null,
            // `state` is a hint word only on the window; on a step it is typed
            "display-step.state: text"
        ])
    );
}

/// The raw props of #868 and the catalogue's `html` type are one set: every
/// prop the catalogue types `html` is a `RAW_PROPS` key and every `RAW_PROPS`
/// key is typed `html` in the catalogue. The door's `html` type only asks "is
/// it a string"; the sanitiser and `check_components` hang on `RAW_PROPS`, so
/// an `html` prop added to the catalogue without its `RAW_PROPS` line would
/// reach the screen unsanitised.
#[test]
fn every_html_prop_is_a_raw_prop_and_back() {
    if !library_ships() {
        return;
    }
    let Some(raw) = ask("sorted('%s.%s' % k for k in m.RAW_PROPS)") else {
        return;
    };
    let cat = catalog();
    let mut html: Vec<String> = Vec::new();
    for e in entries(&cat) {
        let name = e["name"].as_str().expect("a name");
        for (prop, spec) in e["props"].as_object().expect("props") {
            if spec["type"] == "html" {
                html.push(format!("{name}.{prop}"));
            }
        }
    }
    html.sort();
    let raw: Vec<String> = raw
        .as_array()
        .expect("a list")
        .iter()
        .map(|v| v.as_str().expect("a string").to_string())
        .collect();
    assert_eq!(
        html, raw,
        "the catalogue's html props and RAW_PROPS are no longer the same set"
    );
}

/// A new block draws no empty element for a prop nobody said: every optional
/// prop that is the content of an element (`>{{prop}}<`) in the templates of
/// card, steps and step stands under its own `{{#if prop}}`. An unguarded
/// `display-card` title drew an empty heading that took a line (review K, M4).
#[test]
fn the_new_blocks_draw_no_empty_element_for_an_unsaid_prop() {
    if !library_ships() {
        return;
    }
    let Some(bare) = ask(
        "[n + '.' + p for n in ('display-card', 'display-steps', 'display-step') \
         for p, s in m.CATALOG_BY_NAME[n]['props'].items() \
         if not s['required'] and ('>{{' + p + '}}<') in m.define_of(m.CATALOG_BY_NAME[n])['template'] \
         and ('{{#if ' + p + '}}') not in m.define_of(m.CATALOG_BY_NAME[n])['template']]",
    ) else {
        return;
    };
    assert_eq!(
        bare,
        Value::Array(vec![]),
        "an optional prop draws an empty element when it is not said"
    );
}
