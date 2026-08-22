//! GH #204, third half — the seed is the third opinion about the embedding model.
//!
//! `gh204_declared_defaults_match_the_inline` pins the two halves that live in
//! one file: `params.script_inline`'s `${VAR:-default}` and the
//! `contract.settings.<x>.default` beside it. The `memory-hive` embedding lane
//! has a THIRD statement of the same value, in a different file and in a shape
//! no `${VAR}` sweep can see: `store/seed/emb_models.jsonl`, which is copied
//! into the store **verbatim** — the seed is not variable-substituted, so the
//! coupling is by hand and nothing was reading it.
//!
//! It went wrong exactly once and in the way the class predicts. Commit
//! `378befe7` moved the seed to `google/gemini-embedding-2`; the two literals in
//! `embed/config.json` stayed on the old model. The mechanism that makes this
//! worth a test rather than a proof-read:
//!
//!   1. `embed` stamps every row it writes with `model_id = MODEL`, the inline
//!      default.
//!   2. `recall` reads the ACTIVE row of `emb_models` and filters its `similar`
//!      operator on that `model_id`.
//!
//! Disagree, and the semantic leg selects over a model id no row carries: zero
//! rows, no error, and `semantic_degraded` stays `false` because nothing
//! failed — the lane answers, it just answers with nothing. A retrieval number
//! that quietly lost a quarter of its fan looks like a bad model, not like a
//! bad config, which is why this one gets a pin instead of a comment.
//!
//! `dim` rides along for the same reason: 1024 bits are packed into the 128-byte
//! blob the store compares, and a seed that declares a different width would
//! make `hamming()` measure noise.

use meclaw_core::serde_json::Value;

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// The default of one `${VAR:-default}` in a script, by the same substitution
/// rule the colony performs at instantiation and `gh204_declared_defaults…`
/// re-uses. A bare `${VAR}` declares no default and is therefore not an opinion
/// about one.
fn inline_default(script: &str, var: &str) -> String {
    let needle = format!("${{{var}:-");
    let start = script
        .find(&needle)
        .unwrap_or_else(|| panic!("the embed script carries no ${{{var}:-…}}"))
        + needle.len();
    let end = script[start..]
        .find('}')
        .unwrap_or_else(|| panic!("unterminated ${{{var}:-…}}"));
    script[start..start + end].to_string()
}

/// The active row of the shipped `emb_models` seed. The first line is the table
/// schema, every following line is a row; `active = 1` is the one `recall`
/// resolves the semantic leg against.
fn active_seed_row() -> Value {
    let raw =
        std::fs::read_to_string(templates_root().join("memory-hive/store/seed/emb_models.jsonl"))
            .expect("emb_models seed");
    let rows: Vec<Value> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| meclaw_core::serde_json::from_str(l).expect("a seed line is one JSON document"))
        .filter(|v: &Value| v.get("model_id").is_some())
        .collect();
    let active: Vec<&Value> = rows.iter().filter(|r| r["active"] == 1).collect();
    assert_eq!(
        active.len(),
        1,
        "exactly one seeded embedding model is active, found {}: {rows:?}",
        active.len()
    );
    active[0].clone()
}

fn embed_config() -> Value {
    let raw = std::fs::read_to_string(templates_root().join("memory-hive/embed/config.json"))
        .expect("embed config");
    meclaw_core::serde_json::from_str(&raw).expect("embed config json")
}

#[test]
fn the_shipped_embedding_generation_agrees() {
    let seed = active_seed_row();
    let cfg = embed_config();
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("embed script_inline");

    let seed_model = seed["model_id"].as_str().expect("seed model_id");
    let inline_model = inline_default(script, "MEMORY_EMBED_MODEL");
    let declared_model = cfg["contract"]["settings"]["model"]["default"]
        .as_str()
        .expect("declared model default");

    assert_eq!(
        inline_model, seed_model,
        "`embed` stamps rows with the inline default {inline_model:?}, the seed's active row says \
         {seed_model:?}, and `recall` filters `similar` on the seed's value. The semantic leg \
         would select over a model id no row carries: zero rows, no error, `semantic_degraded` \
         false."
    );
    assert_eq!(
        declared_model, seed_model,
        "`contract.settings.model.default` is the machine-readable answer to \"what does this \
         knob default to\" — it says {declared_model:?} while the shipped seed says \
         {seed_model:?}."
    );

    let seed_dim = seed["dim"].as_i64().expect("seed dim");
    let inline_dim: i64 = inline_default(script, "MEMORY_EMBED_DIM")
        .parse()
        .expect("MEMORY_EMBED_DIM default is a number");
    let declared_dim = cfg["contract"]["settings"]["dim"]["default"]
        .as_i64()
        .expect("declared dim default");

    assert_eq!(
        inline_dim, seed_dim,
        "the embedder requests {inline_dim} dimensions and the seed declares {seed_dim}; the \
         packed blob the store compares is one width, so the two cannot differ."
    );
    assert_eq!(
        declared_dim, seed_dim,
        "`contract.settings.dim.default` is {declared_dim}, the shipped seed declares {seed_dim}."
    );
}
