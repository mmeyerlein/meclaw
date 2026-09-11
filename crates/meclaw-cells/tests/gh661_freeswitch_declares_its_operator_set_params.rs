//! GH #661 — `freeswitch` declares the three params an operator has to set.
//!
//! The measured colony (GH #661) grew the signal half of a telephone channel
//! with every shipped default: `voice_ws_url` pointing at `127.0.0.1`, an empty
//! `line_user_id`, an empty `callers`. Each of the three is a SHAPE — the key
//! exists so a manifest can set it — and a channel carrying all three would
//! never have reached any switch.
//!
//! Exactly three, and the number is asserted (drift lock, § 2d). `dial_prefix`
//! is NOT one of them: `sofia/gateway/fs02/` is a working default for the
//! gateway this distribution is wired against, and a marking on a param that
//! works trains an operator to ignore the list.
//!
//! OR-A5: `voice` and `telegram-connector` are named in the issue as candidates
//! and carry no param of this form — their keys are env-bound
//! (`${DEEPGRAM_API_KEY}`, `${CARTESIA_API_KEY}`, `${TELEGRAM_BOT_TOKEN}`, and an
//! unset variable already fails loudly with `env_var_missing`) or are working
//! values. So only `freeswitch` is marked in this wave.

use meclaw_core::serde_json::Value;

const SIGNAL: &str = "templates/freeswitch/signal/config.json";

/// The three, and the reason each one is a shape rather than a value.
const OPERATOR_SET: &[&str] = &["callers", "line_user_id", "voice_ws_url"];

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn signal() -> Value {
    let raw = std::fs::read_to_string(repo(SIGNAL)).unwrap_or_else(|e| panic!("{SIGNAL}: {e}"));
    meclaw_core::serde_json::from_str(&raw).expect("the signal half parses")
}

#[test]
fn the_three_line_params_are_operator_set() {
    let cfg = signal();
    let settings = cfg["contract"]["settings"]
        .as_object()
        .expect("contract.settings");
    let mut marked: Vec<&str> = settings
        .iter()
        .filter(|(_, v)| v["operator_set"].as_bool() == Some(true))
        .map(|(k, _)| k.as_str())
        .collect();
    marked.sort_unstable();
    assert_eq!(
        marked, OPERATOR_SET,
        "exactly these three carry a shipped value that is a shape and not a \
         working one; the number stands once, here, so a fourth marking is a \
         decision rather than a side effect"
    );

    // And every one of them is a param the cell really has — a declaration on a
    // key that is not settable would be a rule nobody can satisfy.
    let params = cfg["params"].as_object().expect("params");
    for key in OPERATOR_SET {
        assert!(
            params.contains_key(*key),
            "`{key}` is declared operator-set and is not a param of this cell"
        );
    }

    // The counter-proof: the gateway prefix is a working default and stays one.
    assert!(
        settings["dial_prefix"]["operator_set"].as_bool() != Some(true),
        "`dial_prefix` ships a gateway that works; marking what works trains an \
         operator to ignore the list"
    );
}
