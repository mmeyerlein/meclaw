//! GH #1061 (#801) — no shipped template or example reads a secret out of the
//! environment.
//!
//! Since 0.62 every secret a cell spends arrives SEALED from the vault over a
//! credential grant; `${VAR}` stays for what is not a secret (models, URLs).
//! Before this lock the library carried 32 `config.json` with a provider key as
//! a late-bound token, four `template.json` declaring one as an environment
//! requirement, a builder-librarian seed that taught it as THE way, and six
//! examples `scripts/start.sh` ships — and `start.sh` wrote the key into the
//! colony's `.env` to feed them. This test is the "grep empty" of #801.
//!
//! The class is by SUFFIX, not substring: a name ENDING in `_KEY`, `_TOKEN`,
//! `_SECRET`, `_PASSWORD`, `_PASS` or `_PASSPHRASE`. `STEWARD_NUMERIC_PARAM_KEYS`
//! is a list of param names and stays legal. A `contract.settings.<k>` marked
//! `secret: true` may not default to any `${…}` token either: its default IS
//! the value the cell spends.

use serde_json::Value;
use std::path::{Path, PathBuf};

const SUFFIXES: [&str; 6] = [
    "_KEY",
    "_TOKEN",
    "_SECRET",
    "_PASSWORD",
    "_PASS",
    "_PASSPHRASE",
];
const SENTENCE: &str = "name a credential_grant_id instead (#801)";

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Every file of the classes the issue names, under `dir`.
fn files(dir: &Path, out: &mut Vec<PathBuf>, examples: bool) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out, examples);
            continue;
        }
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let in_seed = p
            .parent()
            .and_then(|d| d.file_name())
            .is_some_and(|n| n == "seed");
        let take = if examples {
            name.ends_with(".json") || (in_seed && name.ends_with(".jsonl"))
        } else {
            name == "config.json"
                || name == "template.json"
                || (in_seed && name.ends_with(".jsonl"))
        };
        if take {
            out.push(p);
        }
    }
}

/// The names of the secret-class `${NAME…}` tokens in `s`.
fn secret_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        let after = &rest[i + 2..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let name = &after[..end];
        let closes = after[end..].starts_with('}') || after[end..].starts_with(":-");
        if closes && SUFFIXES.iter().any(|suf| name.ends_with(suf)) {
            out.push(name.to_string());
        }
        rest = &after[end..];
    }
    out
}

/// Walk every string of `v`; push `(json path, token)` per hit.
fn walk(v: &Value, path: &str, hits: &mut Vec<(String, String)>) {
    match v {
        Value::String(s) => {
            for t in secret_tokens(s) {
                hits.push((path.to_string(), t));
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                walk(x, &format!("{path}[{i}]"), hits);
            }
        }
        Value::Object(m) => {
            for (k, x) in m {
                walk(x, &format!("{path}.{k}"), hits);
            }
        }
        _ => {}
    }
}

/// `contract.settings.<k>` with `secret: true` and a `${…}` default.
fn secret_settings(v: &Value, hits: &mut Vec<(String, String)>) {
    let Some(settings) = v.pointer("/contract/settings").and_then(Value::as_object) else {
        return;
    };
    for (k, s) in settings {
        let secret = s.get("secret").and_then(Value::as_bool).unwrap_or(false);
        let default = s.get("default").and_then(Value::as_str).unwrap_or_default();
        if secret && default.contains("${") {
            hits.push((
                format!("$.contract.settings.{k}.default"),
                default.to_string(),
            ));
        }
    }
}

fn violations(file: &Path) -> Vec<String> {
    let raw = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let docs: Vec<(String, Value)> = if file.extension().is_some_and(|x| x == "jsonl") {
        raw.lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
            .map(|(i, l)| {
                let v = serde_json::from_str(l)
                    .unwrap_or_else(|e| panic!("{}:{}: {e}", file.display(), i + 1));
                (format!("line {}: $", i + 1), v)
            })
            .collect()
    } else {
        let v = serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        vec![("$".to_string(), v)]
    };
    let mut out = Vec::new();
    for (root, doc) in &docs {
        let mut hits = Vec::new();
        walk(doc, root, &mut hits);
        secret_settings(doc, &mut hits);
        for (path, name) in hits {
            out.push(format!("{} {path}: `{name}` -- {SENTENCE}", file.display()));
        }
    }
    out
}

#[test]
fn gh1061_no_template_reads_a_secret_from_the_environment() {
    let templates = repo("templates");
    if !templates.join("access/template.json").is_file() {
        return; // GH #49: a tree without the library is skipped, never judged
    }
    let mut all = Vec::new();
    files(&templates, &mut all, false);
    let examples = repo("examples");
    if examples.is_dir() {
        files(&examples, &mut all, true);
    }
    assert!(
        all.len() > 100,
        "the walk found only {} files -- the lint would pass by looking at nothing",
        all.len()
    );
    let found: Vec<String> = all.iter().flat_map(|f| violations(f)).collect();
    assert!(
        found.is_empty(),
        "{} secret(s) read from the environment:\n{}",
        found.len(),
        found.join("\n")
    );
}

/// The matcher itself: the suffix rule, the default form, and what it leaves alone.
#[test]
fn gh1061_the_secret_class_is_a_suffix_not_a_substring() {
    assert_eq!(
        secret_tokens("${OPENROUTER_API_KEY}"),
        vec!["OPENROUTER_API_KEY"]
    );
    assert_eq!(
        secret_tokens("x ${SLACK_BOT_TOKEN:-} y"),
        vec!["SLACK_BOT_TOKEN"]
    );
    assert_eq!(
        secret_tokens("${PEER_CLIENT_SECRET}"),
        vec!["PEER_CLIENT_SECRET"]
    );
    assert_eq!(
        secret_tokens("${VAULT_PASSPHRASE:-a}"),
        vec!["VAULT_PASSPHRASE"]
    );
    assert!(secret_tokens("${STEWARD_NUMERIC_PARAM_KEYS}").is_empty());
    assert!(secret_tokens("${MODEL_CORE} ${OPENROUTER_BASE_URL:-https://x}").is_empty());
    assert!(secret_tokens("${ctx.api_key}").is_empty());
    assert!(secret_tokens("$OPENROUTER_API_KEY").is_empty());
    let mut hits = Vec::new();
    secret_settings(
        &serde_json::json!({"contract": {"settings": {
            "api_key": {"secret": true, "default": "${SOMETHING}"},
            "model": {"secret": false, "default": "${MODEL_CORE}"}}}}),
        &mut hits,
    );
    assert_eq!(hits.len(), 1, "{hits:?}");
}

/// The provider keys `scripts/start.sh` asks for. A key it reads goes into a
/// vault; the daemon it starts must not inherit it (A3/T4: no provider key in
/// the daemon's environment, `/proc/<pid>/environ` included).
const ASKED_PROVIDER_KEYS: &[&str] = &["OPENROUTER_API_KEY"];

/// GH #1061 review — `start.sh` drops every provider key it read from the
/// exported environment before anything is started, in every flavour.
///
/// Runs the script's own prologue (everything before step 1, which is where
/// the key is captured and, without one, asked for) under `sh` with a dummy
/// key, then `env` as the stand-in for the first child it starts. Only NAMES
/// are compared, never a value. The rest of the script is held to not putting
/// the name back (no `export`, no assignment), and the generated
/// `deposit-key.sh` to dropping it before it starts its daemon.
#[test]
fn gh1061_start_sh_hands_its_daemon_no_provider_key() {
    let script = std::fs::read_to_string(repo("scripts/start.sh")).unwrap();
    let cut = script
        .find("\n# 1. Install.")
        .expect("start.sh lost its `# 1. Install.` marker");
    let (prologue, rest) = script.split_at(cut);
    for flavour in ["", "hard-shell", "meclaw-os", "organism"] {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(format!("{prologue}\nenv\n"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", std::env::temp_dir())
            .env("MECLAW_EXAMPLE", flavour)
            .stdin(std::process::Stdio::null());
        for k in ASKED_PROVIDER_KEYS {
            cmd.env(k, "dummy-not-a-key");
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "the prologue failed for flavour {flavour:?}"
        );
        let names: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.split_once('=').map(|(n, _)| n.to_string()))
            .collect();
        assert!(
            names.iter().any(|n| n == "MECLAW_EXAMPLE"),
            "env printed nothing"
        );
        for k in ASKED_PROVIDER_KEYS {
            assert!(
                !names.iter().any(|n| n == k),
                "flavour {flavour:?}: {k} is still exported to the daemon start.sh starts"
            );
            for (i, line) in rest.lines().enumerate() {
                let t = line.trim_start();
                assert!(
                    !(t.starts_with("export") && t.contains(k)) && !t.starts_with(&format!("{k}=")),
                    "start.sh:{}: puts {k} back into the environment",
                    prologue.lines().count() + i + 1
                );
            }
        }
    }
    let deposit = script
        .split("cat <<'DEPOSIT'")
        .nth(1)
        .and_then(|s| s.split("\nDEPOSIT\n").next())
        .expect("start.sh lost its deposit-key.sh body");
    let first_start = deposit
        .find("start_daemon\n")
        .expect("deposit-key.sh no longer starts its daemon");
    for k in ASKED_PROVIDER_KEYS {
        let unset = deposit
            .find(&format!("unset {k}"))
            .unwrap_or_else(|| panic!("deposit-key.sh never drops {k}"));
        assert!(
            unset < first_start,
            "deposit-key.sh drops {k} only after its daemon started"
        );
    }
}
