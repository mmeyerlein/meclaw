//! GH #1329 — a channel template brings no access of its own; the cells that
//! spend a key declare the credential lane at the template's rim.
//!
//! A template ships no access of its own; its cells obtain their credentials
//! by grant from an access of the colony (the builder recipe seeds them in the
//! member's access). A channel
//! that shipped its own `./access` grew a second broker inside the member,
//! with its own vault and its own seed, which the colony's operator had to
//! find, fill and audit apart from the one they already keep. `freeswitch` did
//! exactly that up to 2.4.8 (three grants for its media half). Since 3.0.0 the
//! hive names the lane instead -- `credential_request` out of each asking cell
//! and `in_sealed` back into it, both at the cell's connect point (`at`) -- and
//! the consumer draws the two v-lanes to an access of the colony and seeds the
//! grants there, the form `simple-agent@2` and the figure use.
//!
//! | lock | claim |
//! |---|---|
//! | `gh1329_no_channel_template_ships_an_access` | no template tagged `channel` carries an `access/` (no broker, no seed) and no edge to `./access` |
//! | `gh1329_every_asking_cell_of_a_channel_is_declared_at_its_rim` | every cell under a channel hive whose contract emits `credential_request` is a connect point of `credential_request` (emits) and of `in_sealed` (accepts, required) at the hive's rim |

use meclaw_core::serde_json::Value;
use std::path::{Path, PathBuf};

fn templates() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn read(p: &Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The shipped templates tagged `channel`, sorted, as `(name, dir)`.
fn channel_templates() -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(templates()) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("template.json").is_file())
        .filter(|p| {
            read(&p.join("template.json"))["tags"]
                .as_array()
                .is_some_and(|ts| ts.iter().any(|t| t == "channel"))
        })
        .map(|p| {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            (name, p)
        })
        .collect();
    out.sort();
    out
}

fn emits_credential_request(cfg: &Value) -> bool {
    cfg.pointer("/contract/emits/hop/route/values")
        .and_then(Value::as_array)
        .is_some_and(|vs| vs.iter().any(|v| v == "credential_request"))
}

/// Every cell below the template root at `dir` whose own contract emits
/// `credential_request`, refs followed by name, as paths relative to the root.
fn askers(dir: &Path, rel: &str, out: &mut Vec<String>) {
    let cfg_path = dir.join("config.json");
    if !cfg_path.is_file() {
        return;
    }
    let cfg = read(&cfg_path);
    if cfg["cell"]["type"] == "ref" {
        let name = cfg["cell"]["template"]
            .as_str()
            .expect("a ref names its template")
            .split('@')
            .next()
            .unwrap_or_default()
            .to_string();
        askers(&templates().join(name), rel, out);
        return;
    }
    if !rel.is_empty() && emits_credential_request(&cfg) {
        out.push(rel.to_string());
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut kids: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    kids.sort();
    for k in kids {
        let name = k
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if name == "seed" || name == "src" {
            continue;
        }
        let child = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        askers(&k, &child, out);
    }
}

/// The rim entry of `list` (`accepts` or `emits`) for `route`, if any.
fn rim_lane<'a>(root: &'a Value, list: &str, route: &str) -> Option<&'a Value> {
    root["params"]["contract"][list]
        .as_array()?
        .iter()
        .find(|l| l["route"] == route)
}

fn at_names(lane: &Value, point: &str) -> bool {
    lane["at"]
        .as_array()
        .is_some_and(|a| a.iter().any(|p| p == point))
}

#[test]
fn gh1329_no_channel_template_ships_an_access() {
    let channels = channel_templates();
    if !channels.iter().any(|(n, _)| n == "freeswitch") {
        return; // GH #49: a tree without the library is skipped, never judged
    }
    for (name, dir) in &channels {
        assert!(
            !dir.join("access").exists(),
            "templates/{name} ships an access of its own -- a channel brings none, the \
             cells take their grants from an access of the colony (#1329)"
        );
        let edges = read(&dir.join("config.json"))["params"]["graph"]["edges"].clone();
        for e in edges.as_array().into_iter().flatten() {
            assert!(
                e["from"] != "./access" && e["to"] != "./access",
                "templates/{name} still draws an edge to or from ./access: {e}"
            );
        }
    }
}

#[test]
fn gh1329_every_asking_cell_of_a_channel_is_declared_at_its_rim() {
    let channels = channel_templates();
    if !channels.iter().any(|(n, _)| n == "freeswitch") {
        return; // GH #49
    }
    let mut checked = 0;
    for (name, dir) in &channels {
        let root = read(&dir.join("config.json"));
        if root["cell"]["type"] != "hive" {
            continue; // a single-cell channel IS the asker; its consumer draws to it
        }
        let mut found = Vec::new();
        askers(dir, "", &mut found);
        for path in &found {
            let point = format!("./{path}");
            let ask = rim_lane(&root, "emits", "credential_request").unwrap_or_else(|| {
                panic!(
                    "templates/{name}: {point} asks for a key, but the rim emits no \
                     `credential_request` -- the consumer cannot draw the ask to its access"
                )
            });
            assert!(
                at_names(ask, &point),
                "templates/{name}: `credential_request` is not declared at {point}: {ask}"
            );
            assert!(
                ask["because"].as_str().is_some_and(|b| !b.is_empty()),
                "templates/{name}: the ask half says no `because`"
            );
            let back = rim_lane(&root, "accepts", "in_sealed").unwrap_or_else(|| {
                panic!(
                    "templates/{name}: {point} asks for a key, but the rim accepts no \
                     `in_sealed` -- the sealed box has no way back in"
                )
            });
            assert!(
                at_names(back, &point),
                "templates/{name}: `in_sealed` is not declared at {point}: {back}"
            );
            assert_eq!(
                back["required"], true,
                "templates/{name}: `in_sealed` is required -- a line born without it \
                 refuses every session after its wait"
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 1,
        "no asking cell under a channel hive found -- freeswitch/voice is one"
    );
}
