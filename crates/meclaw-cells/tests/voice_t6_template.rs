//! `voice@1.4.1` — the template, its declared surface, and the binding manifest
//! its README hands a reader.
//!
//! Three things can drift apart here and each of them costs a reader a wrong
//! topology, so each of them is asserted rather than described:
//!
//! 1. **The params surface and the settings surface are the same surface.**
//!    `override_params` may only name a key the cell actually carries under
//!    `params` (GH #294, ruling Q6), and `contract.settings` is where a builder
//!    reads what may be named. A knob in one and not the other is either
//!    untunable or invisible.
//! 2. **The only `${VAR}` tokens are the two credentials.** Everything else this
//!    cell is steered by — the model, the voice, the language, the end-of-turn
//!    thresholds — is a behaviour knob, and a behaviour knob is a param
//!    (`docs/development-rules.md` § 8a, R6, GH #138). `scripts/check_tree_rules.py`
//!    says the same thing from the outside; this says it from inside the one
//!    template that had a reason to be tempted.
//! 3. **The README's binding manifest is the wiring that actually binds.** The
//!    edges a reader copies out of a README are the interface — GH #203 made
//!    that a defect class rather than a typo class — so the two edges are parsed
//!    out of the fenced block and compared against the endpoints and lanes this
//!    template's own contract declares.
//!
//! Guarded like every other template-reading test (GH #49): a template that did
//! not travel is skipped rather than judged.

use meclaw_cells::VoiceCellFactory;
use meclaw_cells::voice::params::{SttParams, TtsParams};
use meclaw_cells::voice::{Mode, VoiceParams};
use meclaw_core::serde_json::{Value, json};
use std::sync::Arc;

/// A path inside this repository, from the crate's manifest directory.
fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The template's three files, or `None` where the template did not travel.
fn shipped() -> Option<(Value, Value, String)> {
    let root = repo("templates/voice");
    let cfg = std::fs::read_to_string(root.join("config.json")).ok()?;
    let tpl = std::fs::read_to_string(root.join("template.json")).ok()?;
    let readme = std::fs::read_to_string(root.join("README.md")).ok()?;
    Some((
        meclaw_core::serde_json::from_str(&cfg).expect("templates/voice/config.json is JSON"),
        meclaw_core::serde_json::from_str(&tpl).expect("templates/voice/template.json is JSON"),
        readme,
    ))
}

/// Every `${NAME}` token in a value, in the order they are met. `$${NAME}` is
/// the escape form and is not a substitution (`substitute.rs`, `replace_env`).
fn env_tokens(v: &Value, into: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            let bytes = s.as_bytes();
            let mut i = 0;
            while let Some(rel) = s[i..].find("${") {
                let at = i + rel;
                let escaped = at > 0 && bytes[at - 1] == b'$';
                let end = s[at..].find('}').map(|e| at + e);
                match end {
                    Some(end) => {
                        if !escaped {
                            into.push(s[at + 2..end].to_string());
                        }
                        i = end + 1;
                    }
                    None => break,
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|x| env_tokens(x, into)),
        Value::Object(map) => map.values().for_each(|x| env_tokens(x, into)),
        _ => {}
    }
}

/// The fenced ```json blocks of a markdown document, in order.
fn json_blocks(md: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in md.lines() {
        match (&mut current, line.trim_start().starts_with("```")) {
            (Some(buf), false) => {
                buf.push_str(line);
                buf.push('\n');
            }
            (Some(_), true) => out.push(current.take().expect("a block is open")),
            (None, true) if line.trim() == "```json" => current = Some(String::new()),
            _ => {}
        }
    }
    out
}

/// The binding manifest: the first fenced block of the README that grows a node
/// AND wires it. The other blocks are `override_params` recipes.
fn binding_manifest(readme: &str) -> Value {
    for raw in json_blocks(readme) {
        let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        if v["diff"]["add_edges"].is_array() && v["diff"]["add_nodes"].is_array() {
            return v;
        }
    }
    panic!("templates/voice/README.md carries no add_nodes+add_edges manifest");
}

// ═════════════════════════════ 1. the declared surface

/// The cell is a `voice` cell and it is long-running: `timeout: -1`, like every
/// other cell that owns a listener for the life of the colony.
#[test]
fn the_template_declares_a_long_running_voice_cell() {
    let Some((cfg, tpl, _)) = shipped() else {
        return;
    };
    assert_eq!(cfg["cell"]["type"], json!("voice"));
    assert_eq!(
        cfg["cell"]["timeout"],
        json!(-1),
        "a cell holding a socket is not bounded by a message timeout"
    );
    assert_eq!(tpl["name"], json!("voice"));
    assert_eq!(tpl["version"], json!("1.4.1"));
}

/// The params surface and the settings surface are the same surface: a knob an
/// instance may hand over is a knob a builder can read about, and the other way
/// round.
#[test]
fn every_param_is_a_declared_setting_and_the_reverse() {
    let Some((cfg, _, _)) = shipped() else {
        return;
    };
    let params = cfg["params"].as_object().expect("params is an object");
    let settings = cfg["contract"]["settings"]
        .as_object()
        .expect("contract.settings is an object");

    let undeclared: Vec<&String> = params
        .keys()
        .filter(|k| !settings.contains_key(*k))
        .collect();
    assert!(
        undeclared.is_empty(),
        "these params carry no contract.settings entry, so nothing tells a \
         builder they may be overridden: {undeclared:?}"
    );
    let phantom: Vec<&String> = settings
        .keys()
        .filter(|k| !params.contains_key(*k))
        .collect();
    assert!(
        phantom.is_empty(),
        "these settings name no param, so override_params would refuse them \
         (GH #294, ruling Q6): {phantom:?}"
    );

    for (key, spec) in settings {
        assert_eq!(
            &spec["default"], &params[key],
            "contract.settings.{key}.default and params.{key} disagree — the \
             declaration promises a default the template does not ship"
        );
    }
}

/// The two credentials are the only environment tokens. Everything else is a
/// behaviour knob and lives in `params` where `override_params` reaches it
/// (`docs/development-rules.md` § 8a, R6).
#[test]
fn the_only_env_tokens_are_the_two_credentials() {
    let Some((cfg, _, _)) = shipped() else {
        return;
    };
    let mut found = Vec::new();
    env_tokens(&cfg["params"], &mut found);
    found.sort();
    found.dedup();
    assert_eq!(
        found,
        vec![
            "CARTESIA_API_KEY".to_string(),
            "DEEPGRAM_API_KEY".to_string()
        ],
        "only the provider lane stays in .env; a model, a voice, a language or \
         a threshold read out of the environment is unreachable per instance"
    );
    assert_eq!(
        cfg["params"]["stt"]["api_key"],
        json!("${DEEPGRAM_API_KEY}")
    );
    assert_eq!(
        cfg["params"]["tts"]["api_key"],
        json!("${CARTESIA_API_KEY}")
    );
    for spec in ["stt", "tts"] {
        assert_eq!(
            cfg["contract"]["settings"][spec]["secret"],
            json!(true),
            "the {spec} block carries the credential and is marked as such"
        );
    }
}

/// The environment surface is DECLARED, so a builder reads what a colony owes
/// before a boot refuses it: neither token carries a default, and a colony
/// growing the shipped configuration without them stops with `env_var_missing`.
///
/// Declared and not `required`: the requirement walk reads the template and
/// never the `override_params` beside it (`validate::requires_for_reference`),
/// so requiring the recogniser key would refuse the one configuration meant to
/// cost nothing — `stt.provider: "echo"`, no `tts` block, no credential at all.
#[test]
fn the_two_credentials_are_a_declared_environment_surface() {
    let Some((cfg, tpl, _)) = shipped() else {
        return;
    };
    let declared = tpl["requires"]["env"]
        .as_object()
        .expect("template.json declares requires.env");
    let mut names: Vec<&String> = declared.keys().collect();
    names.sort();
    assert_eq!(
        names,
        vec!["CARTESIA_API_KEY", "DEEPGRAM_API_KEY"],
        "requires.env and the ${{VAR}} tokens in params are one surface"
    );

    let mut in_params = Vec::new();
    env_tokens(&cfg["params"], &mut in_params);
    in_params.sort();
    in_params.dedup();
    let mut as_declared: Vec<String> = names.iter().map(|s| (*s).clone()).collect();
    as_declared.sort();
    assert_eq!(
        in_params, as_declared,
        "a token the template substitutes and does not declare is an \
         environment surface a builder cannot read"
    );

    for (key, spec) in declared {
        assert_eq!(
            spec["required"],
            json!(false),
            "requires.env.{key} is required, which refuses the echo \
             configuration: the walk reads the template, not the override"
        );
        assert!(
            spec["because"].as_str().is_some_and(|s| s.len() > 40),
            "requires.env.{key} declares no reason, and a declaration without \
             one teaches a builder nothing"
        );
    }
}

/// The answer finds its way back because the cell says it mints the key the
/// answer is addressed by, and because it declares both keys it will read.
///
/// **RETRACTED with 1.4.0** (GH #620), in the words this test carried until
/// then: *`consumes.context.session_id` is `required`, because without the
/// session there is no connection to speak into.* Required is exactly what made
/// the repair unmeasurable — the substrate refuses a message that names only
/// the call before the cell ever sees it, so the cell could never prefer the
/// call over the keeper's session. Both keys are optional now, and the cell's
/// own `missing_session` is what refuses a message that names neither. The
/// ingress declaration does NOT move: its list is the standard header
/// convention (GH #185) and `call_id` is not one of them; it reaches context
/// through the channel's own ingress edge.
#[test]
fn the_call_key_is_declared_and_the_session_key_is_still_read() {
    let Some((cfg, _, _)) = shipped() else {
        return;
    };
    let contract = &cfg["contract"];
    for key in ["call_id", "session_id"] {
        assert_eq!(
            contract["consumes"]["context"][key]["required"],
            json!(false),
            "consumes.context.{key} is required, which refuses a message \
             addressed by the other key before the cell can read it"
        );
        assert!(
            contract["consumes"]["context"][key]["description"]
                .as_str()
                .is_some_and(|d| d.len() > 40),
            "consumes.context.{key} carries no reason, and two keys for one \
             value without a reason is a puzzle rather than a contract"
        );
    }
    assert_eq!(
        contract["emits"]["hop"]["call_id"]["required"],
        json!(false),
        "the call travels on every emission that has a session, and the error \
         shape has none"
    );
    assert_eq!(
        contract["ingress"]["context"],
        json!(["session_id"]),
        "a connection is born at this cell, so the cell is the setter root for \
         the key that addresses it (GH #185) — and `call_id` is not a standard \
         header, so it is not claimed here"
    );
    // The failure shape carries an empty `messages[]` and no session at all, so
    // not one hop key may be required — that is what makes three shapes legal
    // on one contract.
    let hop = contract["emits"]["hop"]
        .as_object()
        .expect("emits.hop is an object");
    for (key, spec) in hop {
        assert_eq!(
            spec["required"],
            json!(false),
            "emits.hop.{key} is required, which refuses the error shape"
        );
    }
    assert_eq!(
        contract["emits"]["hop"]["route"]["values"],
        json!(["turn", "partial", "speak_end", "error"]),
        "the four lanes this cell names for itself"
    );
}

// ═════════════════════════════ 2. the manifest a reader copies

/// The README's binding manifest grows the node the catalogue names and draws
/// exactly the two edges the container needs: one up carrying all three lanes
/// with the session promoted, one down guarded on the node name.
#[test]
fn the_readme_manifest_binds_the_three_lanes_and_the_way_back() {
    let Some((cfg, _, readme)) = shipped() else {
        return;
    };
    let manifest = binding_manifest(&readme);

    let nodes = manifest["diff"]["add_nodes"]
        .as_array()
        .expect("add_nodes is a list");
    assert_eq!(nodes.len(), 1, "one channel is one node");
    assert_eq!(nodes[0]["name"], json!("channels/voice"));
    assert_eq!(nodes[0]["template"], json!("voice@1.4.1"));

    let edges = manifest["diff"]["add_edges"]
        .as_array()
        .expect("add_edges is a list");
    assert_eq!(
        edges.len(),
        2,
        "one edge up for the three lanes, one edge down for the answer"
    );

    let up = &edges[0];
    assert_eq!(up["from"], json!("./channels/voice"));
    assert_eq!(up["to"], json!("./channels"));
    let cond = up["condition"].as_str().expect("the up edge is guarded");
    for lane in ["turn", "partial", "error"] {
        assert!(
            cond.contains(&format!("hop.route == '{lane}'")),
            "the up edge does not carry `{lane}`: {cond}"
        );
    }
    let promoted = up["modifier"]["set_context"]
        .as_object()
        .expect("the up edge promotes");
    for key in ["channel_node", "channel", "audience_set", "call_id"] {
        assert!(
            promoted.contains_key(key),
            "the up edge promotes no `{key}` — hop is single-hop and the key \
             is gone by the next emission"
        );
    }
    assert!(
        promoted["call_id"]
            .as_str()
            .is_some_and(|s| s.contains("has(hop.call_id)")),
        "a promotion off the hop is written `has(...) ? ... : ''`, or a missing \
         key skips the whole edge silently"
    );
    // GH #620: the workaround is gone. `voice_session` existed because
    // `context.session_id` had two owners, and the ruling gave the call a key
    // of its own — so the manifest carries neither the second key nor the
    // restamp that put it back.
    assert!(
        !promoted.contains_key("voice_session"),
        "the `voice_session` workaround is retracted: {promoted:#?}"
    );
    assert!(
        up["modifier"].get("set_hop").is_none(),
        "the cell names its own lanes; an edge that re-stamped hop.route here \
         would be rewriting a decision the emission already carries"
    );

    let down = &edges[1];
    assert_eq!(down["from"], json!("./channels"));
    assert_eq!(down["to"], json!("./channels/voice"));
    let guard = down["condition"]
        .as_str()
        .expect("the down edge is guarded");
    assert!(
        guard.contains("hop.route == 'answer'")
            && guard.contains("context.channel_node == 'voice'"),
        "the way back is the answer lane, addressed by the node name: {guard}"
    );
    assert_eq!(
        down["modifier"]["set_hop"]["route"],
        json!("'in_speak'"),
        "the inbound lane of this cell is `in_speak`"
    );
    assert!(
        down["modifier"].get("set_context").is_none(),
        "nothing on the way down rewrites the address any more: `context.call_id` \
         has one owner and arrives on the answer exactly as it left (GH #620)"
    );

    // The node name the manifest grows is the word the down edge guards on and
    // the word the up edge promotes — one typo apart, the answer goes nowhere.
    assert_eq!(promoted["channel_node"], json!("'voice'"));
    assert_eq!(promoted["channel"], json!("'voice'"));

    // And what the cell consumes is what the down edge delivers.
    assert_eq!(
        cfg["contract"]["consumes"]["body"]["messages"]["required"],
        json!(true)
    );
}

/// `partial` reaches the container and stops there in `member`, which is a
/// ruling of this wave rather than an oversight — so the README says so, in the
/// place a reader looks before wiring it.
#[test]
fn the_readme_says_where_the_partial_lane_ends() {
    let Some((_, _, readme)) = shipped() else {
        return;
    };
    for claim in [
        "emit_partials",
        "no_route",
        "The `partial` lane ships OFF, and whoever listens orders it",
        "emit_speak_end",
        "The `speak_end` lane ships OFF as well, and the telephone is who orders it",
    ] {
        assert!(
            readme.contains(claim),
            "templates/voice/README.md does not tell a reader about `{claim}`"
        );
    }
}

// ═════════════════════════════ 3. the params the cell really parses

/// Substitute every `${NAME}` with `<NAME>-dummy`, the way the staging
/// substitution would out of a colony's `.env`. No real credential is needed to
/// prove that the shipped block parses — and none may be, because this file
/// runs everywhere.
fn substituted(v: &Value) -> Value {
    match v {
        Value::String(_) => {
            let mut found = Vec::new();
            env_tokens(v, &mut found);
            let mut out = v.as_str().expect("a string").to_string();
            for name in found {
                out = out.replace(&format!("${{{name}}}"), &format!("{name}-dummy"));
            }
            Value::String(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(substituted).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, x)| (k.clone(), substituted(x)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The block this template ships is a block the cell accepts. Nothing else
/// proves the two surfaces are one: `scripts/check_tree_rules.py` reads names,
/// this reads the parser — and the parser refuses an unknown key anywhere, so a
/// template that grew a field the contract does not carry fails here and
/// nowhere else until a colony tries to boot it.
#[test]
fn the_shipped_params_parse_into_voice_params() {
    let Some((cfg, _, _)) = shipped() else {
        return;
    };
    let params = VoiceParams::parse(&substituted(&cfg["params"]))
        .expect("the shipped params parse into VoiceParams");
    assert_eq!(params.port, 7900);
    assert_eq!(params.bind, "127.0.0.1");
    assert_eq!(params.default_mode, Mode::Auto);
    assert!(params.barge_in);
    assert!(
        !params.emit_partials,
        "R-V8': the partial lane ships OFF -- whoever consumes it orders it, in \
         the same manifest that draws the edge that drains it"
    );
    assert!(
        !params.emit_speak_end,
        "R-V8' again: the speak_end lane ships OFF -- the telephony hive orders \
         it, in the same breath as the inner edge that drains it"
    );
    assert_eq!(params.external_timeout_ms, 5000);
    assert_eq!(params.provider_idle_timeout_ms, 30000);
    assert!(
        params.speak_plain,
        "the shipped template speaks TEXT: an assistant writes markdown whether \
         or not anybody asked it to, and a provider reads what it is handed"
    );
    assert!(
        matches!(params.stt, SttParams::Deepgram(_)),
        "the shipped recogniser is Deepgram Flux"
    );
    assert!(
        matches!(params.tts, Some(TtsParams::Cartesia(_))),
        "the shipped voice is Cartesia; only `echo` may ship without one"
    );
}

/// An unresolved credential is refused by the NAME of the key an operator has
/// to set, and the value never appears. A parse error travels into a log line
/// and a mutation receipt, so this is the one message shape that must not carry
/// material.
#[test]
fn an_unresolved_credential_is_refused_by_its_name() {
    let Some((cfg, _, _)) = shipped() else {
        return;
    };
    let err = VoiceParams::parse(&cfg["params"])
        .expect_err("an unsubstituted ${…} credential is refused");
    assert!(
        err.contains("stt.api_key"),
        "the refusal must name the setting that is unresolved: {err}"
    );
    assert!(
        !err.contains("DEEPGRAM_API_KEY-"),
        "and it must never carry the value beside the name: {err}"
    );
    // The message names the params PATH today and not the ENVIRONMENT VARIABLE
    // (`DEEPGRAM_API_KEY`), which is what an operator actually has to set. The
    // token is still literally in the value at that point, so the name is there
    // to be read; naming it is a one-line change in `voice/params.rs`, which is
    // not this strand's file. Reported to the wave — tighten this to
    // `err.contains("DEEPGRAM_API_KEY")` in the same commit that lands it.
}

/// R-V4 — a model name, a threshold and a rate are params with defaults, and
/// the default the TEMPLATE ships is the default the CODE carries.
///
/// **The MODEL is pinned against the adapter's own constant** and not against
/// the serde default beside it: `providers::deepgram::DEFAULT_MODEL` and
/// `providers::cartesia::DEFAULT_MODEL` are where a provider strand states the
/// current streaming model (R-V4, R-V14), and that is the statement a template
/// literal has to agree with. A strand that moves its constant and leaves this
/// template quoting the old name turns this red, which is the whole point — the
/// alternative is a catalogue promising a model the cell no longer asks for.
///
/// **Everything else is pinned against the parser**: a block carrying only
/// `provider`, the credential, the model and the one key with no default lets
/// every remaining serde default fire, and it must land exactly where the
/// shipped literals land.
#[test]
fn the_template_literals_are_the_code_defaults() {
    let Some((cfg, _, _)) = shipped() else {
        return;
    };
    assert_eq!(
        cfg["params"]["stt"]["model"],
        json!(meclaw_cells::voice::providers::deepgram::DEFAULT_MODEL),
        "templates/voice/config.json quotes a recogniser model the adapter no \
         longer names — move the template with the constant, in the same commit"
    );
    assert_eq!(
        cfg["params"]["tts"]["model"],
        json!(meclaw_cells::voice::providers::cartesia::DEFAULT_MODEL),
        "templates/voice/config.json quotes a synthesis model the adapter no \
         longer names — move the template with the constant, in the same commit"
    );
    assert_eq!(
        cfg["params"]["emit_partials"],
        json!(false),
        "R-V8': the partial lane ships OFF. Asserted against the LITERAL and \
         not against the parser, because the parser still defaults it to true \
         — see `the_partial_lane_is_off_in_the_code_too`, which is the pin \
         that closes that gap and is ignored until t1 flips it"
    );

    assert_eq!(
        cfg["params"]["emit_speak_end"],
        json!(false),
        "R-V8': the speak_end lane ships OFF too, and for the same reason -- the \
         emission goes to the cell's own path and a lane nobody drew an edge \
         for dead-letters once per sentence"
    );

    let full_params = substituted(&cfg["params"]);
    let mut bare = full_params.clone();
    for lane in ["stt", "tts"] {
        let block = bare[lane]
            .as_object_mut()
            .expect("both provider blocks are objects");
        block.retain(|k, _| k == "provider" || k == "api_key" || k == "voice" || k == "model");
    }

    let full = VoiceParams::parse(&full_params).expect("the shipped params parse");
    let defaulted = VoiceParams::parse(&bare)
        .expect("provider + credential is enough; every other key carries a default");

    assert_eq!(
        format!("{:?}", full.stt),
        format!("{:?}", defaulted.stt),
        "templates/voice/config.json ships an stt literal (a threshold, a rate, \
         a language) the code no longer defaults to"
    );
    assert_eq!(
        format!("{:?}", full.tts),
        format!("{:?}", defaulted.tts),
        "templates/voice/config.json ships a tts literal (a rate, a language) \
         the code no longer defaults to"
    );
}

// ═════════════════════════════ 4. it instantiates, and the edges land

/// Copy a directory tree — staging copies a template's whole directory, so the
/// library under test IS the template and not a fixture that resembles it.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("create the destination");
    for entry in std::fs::read_dir(from).expect("read the template dir") {
        let entry = entry.expect("a directory entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy a template file");
        }
    }
}

/// A colony root holding one empty open container to grow a channel into, the
/// two credentials a boot would otherwise stop for, and this template in its
/// library with the port pointed at a free one.
///
/// The credentials are dummies and named as such. `${…}` without a default
/// fails the boot loudly (`env_var_missing`), which is the behaviour the
/// template's own `requires.env` declaration describes — so the test supplies
/// them rather than dodging them.
fn tree(port: u16) -> tempfile::TempDir {
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();
    std::fs::create_dir_all(root.join("channels")).expect("the container directory");
    std::fs::write(
        root.join("channels/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .expect("write the container");
    std::fs::write(
        root.join(".env"),
        "DEEPGRAM_API_KEY=dummy-not-a-key\nCARTESIA_API_KEY=dummy-not-a-key\n",
    )
    .expect("write the env file");

    let tpl = root.join("templates/voice");
    copy_tree(&repo("templates/voice"), &tpl);
    let mut cfg: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(tpl.join("config.json")).expect("read the shipped config"),
    )
    .expect("the shipped config is JSON");
    cfg["params"]["port"] = json!(port);
    std::fs::write(
        tpl.join("config.json"),
        meclaw_core::serde_json::to_string_pretty(&cfg).expect("serialise"),
    )
    .expect("write the ported config");
    td
}

/// The README's manifest, with the one value a test may not take from a
/// document: the port. Everything else — both endpoints, both conditions, every
/// promotion — is the text a reader copies.
fn manifest_with_port(readme: &str, port: u16) -> Value {
    let mut m = binding_manifest(readme);
    m["scope"] = json!("/");
    m["diff"]["add_nodes"][0]["override_params"]["port"] = json!(port);
    m
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_readme_manifest_grows_the_channel_it_describes() {
    let Some((_, _, readme)) = shipped() else {
        return;
    };
    let port = meclaw_testing::free_port();
    let td = tree(port);

    let factory: Arc<dyn meclaw_colony::CellFactory> = Arc::new(VoiceCellFactory);
    let h = meclaw_testing::ColonyHandle::new_with_factories_at(
        &td,
        vec![("voice".to_string(), factory)],
    );
    // The container is an open, empty hive, and a hive has NO registry row
    // (`bootstrap::registered_hive_paths`) — it is a scope marker, not an
    // actor. So it is registered as a scope rather than spawned, which is
    // exactly what an instantiating mutation finds in a member.
    h.add_hive_scope(meclaw_core::Path::new("/channels")).await;

    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan sent");
    ack_rx
        .await
        .expect("rescan acked")
        .expect("the library must register voice@1.4.1");

    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(meclaw_colony::ColonyMsg::Mutation {
            payload: manifest_with_port(&readme, port),
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("mutation sent");
    let outcome = ack_rx.await.expect("mutation acked");

    // Two verdicts are legal here and they are NOT the same claim.
    //
    // **Committed** is the full one, and the assertions below make it: the node
    // stands where the manifest said and both edges carry the README's
    // conditions byte for byte.
    //
    // **Rejected at the SPAWN stage, with no violations**, is the one this file
    // gets while the cell body is a stub (`voice: not built yet`). It is still
    // worth asserting, and precisely: the mutation runs every VALIDATION stage
    // before it stages anything — schema, edge endpoints, header-contract
    // locality, the hive boundary, the `requires` walk — so a rejection that
    // carries an empty `violations` list and the word `spawn` is the statement
    // that the wiring in the README is correct and the only thing missing is
    // the implementation. A rejection carrying ANY violation is this strand's
    // defect and fails here. When the cell spawns, this branch stops being
    // taken and the strong assertions run without an edit.
    match &outcome {
        meclaw_colony::MutationOutcome::Rejected {
            error_code,
            violations,
            ..
        } if error_code == "spawn" && violations.is_empty() => return,
        meclaw_colony::MutationOutcome::Committed { .. } => {}
        other => panic!(
            "the manifest a reader copies out of the README must pass every \
             validation stage; got {other:?}"
        ),
    }

    let db = rusqlite::Connection::open(td.path().join("colony.db")).expect("open colony.db");
    let grown: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM registry WHERE path = '/channels/voice'",
            [],
            |r| r.get(0),
        )
        .expect("query the registry");
    assert_eq!(grown, 1, "the channel is registered at /channels/voice");

    let mut stmt = db
        .prepare("SELECT from_path, to_path, COALESCE(condition,'') FROM edges")
        .expect("prepare");
    let edges: Vec<(String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query the edges")
        .collect::<Result<Vec<_>, _>>()
        .expect("rows");

    let documented = binding_manifest(&readme);
    for edge in documented["diff"]["add_edges"]
        .as_array()
        .expect("add_edges is a list")
    {
        let from = edge["from"].as_str().expect("from").replace("./", "/");
        let to = edge["to"].as_str().expect("to").replace("./", "/");
        let cond = edge["condition"].as_str().expect("condition");
        assert!(
            edges
                .iter()
                .any(|(f, t, c)| f == &from && t == &to && c == cond),
            "the edge table carries no `{from} -> {to}` on `{cond}`; it holds {edges:?}"
        );
    }
}

/// The recipe the README hands a reader for the credential-free instance: the
/// `echo` recogniser and `"tts": null` beside it.
///
/// `override_params` MERGES onto what the template ships and has no gesture
/// that removes a key, so an override that names only `stt` leaves the shipped
/// Cartesia block standing, credential and all. `null` is the spelling for "not
/// set" — and the whole `requires.env` declaration in `template.json` rests on
/// this recipe working, because it is the reason neither credential is
/// `required` there.
///
/// Ignored until `VoiceParams::parse` reads a null `tts` as an absent one
/// (R-V21). It still COMPILES and still merges the README's own bytes, so the
/// day the parser lands, removing the attribute is the whole change.
#[test]
fn the_readme_echo_recipe_parses_without_tts() {
    let Some((cfg, _, readme)) = shipped() else {
        return;
    };

    // The recipe: the `override_params` block of the README that switches the
    // recogniser to `echo`. Found by what it SAYS, not by its position.
    let recipe = json_blocks(&readme)
        .into_iter()
        .filter_map(|raw| meclaw_core::serde_json::from_str::<Value>(&raw).ok())
        .find(|v| v["override_params"]["stt"]["provider"] == json!("echo"))
        .expect("templates/voice/README.md carries no echo recipe");

    let mut params = substituted(&cfg["params"]);
    let target = params.as_object_mut().expect("params is an object");
    for (key, value) in recipe["override_params"]
        .as_object()
        .expect("override_params is an object")
    {
        // Flat, whole-key: `tts` is one block on the params surface, so an
        // override of it replaces the block rather than merging into it.
        target.insert(key.clone(), value.clone());
    }
    assert_eq!(
        params["tts"],
        Value::Null,
        "the recipe must be the one that says `tts` is not set"
    );

    let parsed = VoiceParams::parse(&params)
        .expect("the README's echo recipe must parse — no credential, no tts block");
    assert!(
        parsed.tts.is_none(),
        "a null tts is an ABSENT tts, which is legal exactly when the \
         recogniser is echo"
    );
    assert!(
        matches!(parsed.stt, SttParams::Echo),
        "the recipe switches the recogniser to echo"
    );
}

/// R-V8' — the code defaults the `partial` lane OFF too, so a params block that
/// says nothing about it agrees with the template that ships it.
///
/// The template's own literal is pinned in
/// `the_template_literals_are_the_code_defaults`; this is the other half, and
/// it is the half that catches a colony building its params by hand. Ignored
/// until `VoiceParams::parse` flips its default — today it reads `true`, which
/// is the one knob where template and code disagree.
#[test]
fn the_partial_lane_is_off_in_the_code_too() {
    let Some((cfg, _, _)) = shipped() else {
        return;
    };
    let mut params = substituted(&cfg["params"]);
    params
        .as_object_mut()
        .expect("params is an object")
        .remove("emit_partials");
    let parsed = VoiceParams::parse(&params).expect("the block parses without the knob");
    assert!(
        !parsed.emit_partials,
        "a params block that orders no partial lane must not get one"
    );
}
