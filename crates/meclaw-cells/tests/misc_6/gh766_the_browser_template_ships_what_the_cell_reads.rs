//! GH #766 (wave G, T11) — the template ships exactly what the cell reads.
//!
//! Four arms, and each of them is a thing a shipped template has got wrong
//! before:
//!
//! (a) `contract.settings` and the params surface are the SAME surface, key for
//!     key — a settings block that drifts is documentation of a cell that does
//!     not exist;
//! (b) the shipped params parse, with the one exception the export forces: an
//!     empty `chromium_path` is refused, with a sentence that says why, and the
//!     same document with a path parses into the cap profile;
//! (c) every route the cell can send is declared in `contract.emits` of THIS
//!     cell — an undeclared route does not lose a message, it loses the whole
//!     send (v2v#13) — and every one of the nine error codes is in the README;
//! (d) nothing in the three files is a path, an infrastructure address or a
//!     person's name (export R4/R5).

use meclaw_cells::browser::{BrowserParams, ERROR_CODES};
use meclaw_cells::sandbox::SandboxProfile;
use meclaw_core::serde_json::Value;
use std::collections::BTreeSet;

/// The template this file is about. It travels with the export, so a public
/// clone finds it (`PUBLIC_TEMPLATES`).
const TEMPLATE: &str = "../../templates/browser";

/// The template directory, from this test binary's own crate.
fn template(file: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(TEMPLATE)
        .join(file)
}

fn read(file: &str) -> String {
    std::fs::read_to_string(template(file))
        .unwrap_or_else(|e| panic!("{}: {e}", template(file).display()))
}

fn json(file: &str) -> Value {
    meclaw_core::serde_json::from_str(&read(file)).expect("the file is JSON")
}

#[test]
fn the_settings_are_the_params_surface_key_for_key() {
    let config = json("config.json");
    let params: BTreeSet<String> = config["params"]
        .as_object()
        .expect("a params object")
        .keys()
        .cloned()
        .collect();
    let settings: BTreeSet<String> = config["contract"]["settings"]
        .as_object()
        .expect("a settings object")
        .keys()
        .cloned()
        .collect();
    assert_eq!(params, settings, "(a) one surface, described once");
    for (key, described) in config["contract"]["settings"]
        .as_object()
        .expect("an object")
    {
        for field in ["type", "secret", "default", "description"] {
            assert!(
                described.get(field).is_some(),
                "settings.{key} says nothing about {field}"
            );
        }
        assert_eq!(
            described["default"], config["params"][key],
            "settings.{key}.default and the shipped value are two statements \
             about one thing, and they have to agree"
        );
        assert_eq!(
            described["secret"],
            Value::Bool(false),
            "nothing this cell takes is a secret"
        );
    }
}

#[test]
fn the_shipped_params_parse_once_a_browser_is_named() {
    let config = json("config.json");
    let shipped = config["params"].clone();
    // The one exception the export forces: the template may not carry a path,
    // so it ships an empty one — and the refusal explains it rather than
    // reading as a bug.
    let e = BrowserParams::parse(&shipped).expect_err("(b) no binary, no browser");
    assert!(e.contains("params.chromium_path"), "{e}");
    assert!(
        e.contains("package") && e.contains("does not search"),
        "the refusal says where a browser comes from and that the cell will \
         not look for one: {e}"
    );

    let mut with_path = shipped;
    with_path["chromium_path"] = Value::String("/usr/bin/chromium".to_string());
    let p = BrowserParams::parse(&with_path).expect("and with a path it parses");
    assert_eq!(p.mount, "browser");
    assert_eq!(p.max_pages, 8);
    assert_eq!(p.suspend_after_ms, 300_000);
    assert_eq!(p.screencast.max_fps, 20);
    assert!(
        p.user_data_dir.is_none(),
        "the profile directory is the cell's to name, so the template names none"
    );
    match p.sandbox.expect("the cap reaches the cell") {
        SandboxProfile::Restricted {
            filesystem,
            limits,
            syscalls,
            ..
        } => {
            assert!(
                filesystem.is_none() && syscalls.is_none(),
                "the third shape"
            );
            assert_eq!(limits.expect("a cap").memory_max_bytes, Some(2_000_000_000));
        }
        other => panic!("expected a restricted profile, got {other:?}"),
    }
    assert_eq!(json("config.json")["cell"]["timeout"], -1, "long-running");
    assert!(
        json("config.json")["cell"].get("message_timeout").is_none(),
        "and no backstop behind the A-timeouts"
    );
}

#[test]
fn every_route_is_declared_and_every_code_is_written_down() {
    let config = json("config.json");
    let routes: Vec<&str> = config["contract"]["emits"]["hop"]["route"]["values"]
        .as_array()
        .expect("declared route values")
        .iter()
        .map(|v| v.as_str().expect("a string"))
        .collect();
    assert_eq!(
        routes,
        vec!["page", "error", "receipt"],
        "(c) the three lanes this cell sends, and an undeclared one would cost \
         not a message but the whole send"
    );
    let readme = read("README.md");
    for code in ERROR_CODES {
        assert!(
            readme.contains(code),
            "the README does not name {code}, and an operator reading one has \
             nowhere to look it up"
        );
    }
    let capabilities: Vec<&str> = config["contract"]["capabilities"]
        .as_array()
        .expect("capabilities")
        .iter()
        .map(|v| v.as_str().expect("a string"))
        .collect();
    assert_eq!(capabilities, vec!["shell:exec", "network:http", "db:own"]);
}

#[test]
fn nothing_in_the_three_files_is_a_path_or_an_address_or_a_name() {
    for file in ["config.json", "template.json", "README.md"] {
        let text = read(file);
        for pattern in [
            "/home/",
            "~/.cache",
            "/tmp/meclaw-browser-",
            "/snap/",
            "ms-playwright",
        ] {
            assert!(
                !text.contains(pattern),
                "(d) {file} carries {pattern:?}, which is a machine somebody owns"
            );
        }
        // A handle is the one way a person gets named in a template, and
        // `template.json`'s `author` is the only place that may carry one.
        // Checked by SHAPE rather than by a name, so this assertion does not
        // itself put a person's name into an exported file (export R5).
        let author = json("template.json")["author"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        for line in text.lines() {
            let handles = line
                .match_indices('@')
                .filter(|(i, _)| line[i + 1..].starts_with(char::is_alphabetic))
                .count();
            if handles == 0 {
                continue;
            }
            assert!(
                file == "template.json" && line.contains(&author),
                "{file} names somebody outside the author field: {line}"
            );
        }
        // Addresses: only the example ones.
        for suspicious in text.split_whitespace().filter(|w| w.contains("://")) {
            let host = suspicious
                .trim_start_matches(|c: char| !c.is_alphanumeric())
                .split("://")
                .nth(1)
                .unwrap_or_default();
            assert!(
                host.starts_with("example.com")
                    || host.starts_with("127.0.0.1")
                    || host.starts_with("host/"),
                "{file}: {suspicious} is neither an example nor a loopback"
            );
        }
    }
    // And the two values that would otherwise be a path are empty on purpose.
    let config = json("config.json");
    assert_eq!(config["params"]["chromium_path"], "");
    assert_eq!(config["params"]["user_data_dir"], "");
    assert_eq!(
        config["params"]["mount"], "browser",
        "the mount is the type's own name, never one instance's"
    );
}
