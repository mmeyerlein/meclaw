//! Tuning a shipped template's knobs from a test, the way a mutation does.
//!
//! Since GH #138 a behaviour knob of a shipped template is a `params` key of the
//! cell that reads it, not a `${VAR}` substituted out of `.env`. A running
//! colony retunes such a knob with an `override_params` entry addressed by the
//! cell's path inside the template (GH #140), and the mutation door applies it
//! by writing the key into the staged `config.json`
//! (`mutation::stage::patch_and_substitute_config`).
//!
//! A test that boots a tree **from disk** has no mutation door in the way, so it
//! writes the same key into the same file itself -- that is what the two
//! functions here do. They exist as helpers rather than as a copy in twenty test
//! files because they replace a line that used to be one string in a `.env`, and
//! because a knob that quietly stops being read is SILENT: the nightly close
//! sweep those `.env` lines pushed away would simply have started firing into
//! test runs again, as a flake rather than as a red assert.

use std::path::Path;

use meclaw_core::serde_json::{self, Value};

/// A cron that fires on the first of January at midnight -- a date no test run
/// reaches, which is the whole of what "quiet" means here.
pub const NEVER_CRON: &str = "0 0 0 1 1 *";

/// Merge `params` into a cell's `config.json` on disk.
///
/// The disk-boot twin of an `override_params` entry: same keys, same file, same
/// last-write-wins semantics. `cell_dir` is the directory that holds the
/// `config.json` -- inside a copied template library, inside a seed, or inside a
/// tree a test wrote itself.
///
/// # Panics
/// If the config is missing or is not a JSON object -- a test that tuned a cell
/// which is not there has measured nothing, and should say so loudly.
pub fn override_params_on_disk(cell_dir: &Path, params: &Value) {
    let path = cell_dir.join("config.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut cfg: Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let Some(object) = params.as_object() else {
        panic!("override_params_on_disk takes a JSON object, got {params}")
    };
    for (key, value) in object {
        cfg["params"][key.as_str()] = value.clone();
    }
    let rendered =
        serde_json::to_string_pretty(&cfg).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    std::fs::write(&path, rendered).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

/// Push the session keeper's nightly close sweep out of a run's way.
///
/// `keeper_dir` is the `session-keeper` directory -- in a copied template
/// library, or inside an instantiated composite. The whole `schedules` array is
/// kept and only its cron is moved, which is what an `override_params` entry
/// naming `schedules` amounts to for a schedule that is otherwise unchanged.
///
/// Until `session-keeper@2.2.0` this was a `KEEPER_NIGHT_CRON` line in the
/// tree's `.env`. The shipped schedule fires through the local night, so a run
/// that leaves it alone behaves differently depending on the hour it started --
/// which is why eighteen setups wrote that line, and why the line becoming dead
/// would have been a flake rather than a failure.
///
/// # Panics
/// If the keeper's `night/config.json` is missing.
pub fn quiet_keeper_night(keeper_dir: &Path) {
    let path = keeper_dir.join("night/config.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut cfg: Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(
        cfg["params"]["schedules"][0]["cron"].is_string(),
        "{}: the keeper's night carries no schedule to quiet",
        path.display()
    );
    cfg["params"]["schedules"][0]["cron"] = Value::String(NEVER_CRON.to_string());
    let rendered =
        serde_json::to_string_pretty(&cfg).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    std::fs::write(&path, rendered).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}
