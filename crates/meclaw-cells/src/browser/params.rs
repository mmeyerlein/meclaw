//! The `params` surface of the `browser` cell. Closed key set, no search path.
//!
//! Two of the rules here are rulings rather than hygiene, and both come from
//! the same sentence: **the browser is a prerequisite out of the distribution's
//! package system** (R-G11). So `chromium_path` has no default and no search
//! path (OR-G9) — a substrate that went looking for a browser would eventually
//! find one nobody vetted — and `--no-sandbox` is refused in `extra_args`,
//! because the sandbox knob was struck and an operator flag is the door it
//! would come back through.

use crate::sandbox::SandboxProfile;
use meclaw_core::JsonValue;
use std::path::PathBuf;

/// Keys accepted in a `browser` cell's `params`. Closed set.
const KNOWN_KEYS: [&str; 11] = [
    "mount",
    "chromium_path",
    "user_data_dir",
    "extra_args",
    "max_pages",
    "suspend_after_ms",
    "throttle_after_ms",
    "screencast",
    "sandbox",
    "external_timeout_ms",
    "startup_timeout_ms",
];

/// Keys accepted in `params.screencast`. Closed set.
const SCREENCAST_KEYS: [&str; 5] = ["format", "quality", "max_width", "max_height", "max_fps"];

/// Flags this cell owns. An operator writing one of them would be changing how
/// the cell talks to its own browser, not how the browser behaves.
const OWNED_FLAG_PREFIXES: [&str; 4] = [
    "--headless",
    "--remote-debugging-",
    "--user-data-dir",
    "--disk-cache-dir",
];

/// How the picture is cut and how often it is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreencastParams {
    /// `jpeg` or `png`. JPEG is the shipped one: a page is a photograph, not a
    /// diagram, and the difference on the wire is an order of magnitude.
    pub format: String,
    /// JPEG quality, 1..=100. Ignored for `png`.
    pub quality: u32,
    /// The widest frame the browser is asked for, in device pixels.
    pub max_width: u32,
    /// The tallest frame the browser is asked for, in device pixels.
    pub max_height: u32,
    /// The pace the cell acknowledges frames at (OR-G8). Never a promise about
    /// what a page actually produces.
    pub max_fps: u32,
}

impl Default for ScreencastParams {
    fn default() -> Self {
        Self {
            format: "jpeg".to_string(),
            quality: 60,
            max_width: 1280,
            max_height: 800,
            max_fps: 20,
        }
    }
}

/// A `browser` cell's parsed params.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserParams {
    /// The name a `page:` join finds this cell under.
    pub mount: String,
    /// The browser binary of the installed package. Required (OR-G9).
    pub chromium_path: String,
    /// The profile directory, or `None` for the cell's own default.
    pub user_data_dir: Option<String>,
    /// Extra flags, appended after the ones the cell owns.
    pub extra_args: Vec<String>,
    /// How many pages may be open at once.
    pub max_pages: u32,
    /// How long a page without viewers stays open. `0` never suspends.
    pub suspend_after_ms: u64,
    /// How long a page may exceed `max_fps` before the cell slows its acks.
    pub throttle_after_ms: u64,
    /// Screencast shape and pace.
    pub screencast: ScreencastParams,
    /// The substrate's sandbox block — the CELL's ceiling, never the browser's
    /// own sandbox, which belongs to the package (R-G11).
    pub sandbox: Option<SandboxProfile>,
    /// The A-timeout around every CDP round trip and around the spawn.
    pub external_timeout_ms: u64,
    /// How long the browser has to answer `Browser.getVersion`.
    pub startup_timeout_ms: u64,
}

impl BrowserParams {
    /// Parse and validate. Runs in `CellFactory::validate_params` and again in
    /// the build closure, so a broken document is a boot error and never a
    /// surprise at the first page.
    pub fn parse(raw: &JsonValue) -> Result<Self, String> {
        let obj = raw.as_object().ok_or("params must be a JSON object")?;
        for key in obj.keys() {
            if !KNOWN_KEYS.contains(&key.as_str()) {
                return Err(format!(
                    "params: unknown key {key:?} (allowed: {})",
                    KNOWN_KEYS.join(", ")
                ));
            }
        }

        let mount = match obj.get("mount") {
            None => "browser".to_string(),
            Some(v) => v
                .as_str()
                .ok_or("params.mount must be a string")?
                .to_string(),
        };
        if !meclaw_colony::surfaces::mount_is_valid(&mount) {
            return Err(format!(
                "params.mount {mount:?}: must be 1..=64 characters from [a-z0-9-] \
                 and none of the reserved names"
            ));
        }

        // No default and no search path (OR-G9). A browser is installed by the
        // operator, through the package system, and named here; looking for one
        // in a cache some tool left behind would be the substrate choosing a
        // binary nobody vetted.
        let chromium_path = match obj.get("chromium_path") {
            None => String::new(),
            // A number or an object here used to read as "no key at all", so
            // the refusal below talked about a missing param while the
            // operator was looking at one they had written.
            Some(v) => v
                .as_str()
                .ok_or_else(|| format!("params.chromium_path must be a string, got {v}"))?
                .to_string(),
        };
        if chromium_path.is_empty() {
            return Err(
                "params.chromium_path is required and has no default: install a \
                        Chromium-based browser from your distribution's package system and \
                        name its binary here (for example \"/usr/bin/chromium\"). The cell \
                        does not search for one."
                    .to_string(),
            );
        }

        let user_data_dir = match obj.get("user_data_dir").and_then(JsonValue::as_str) {
            None | Some("") => None,
            Some(s) => Some(s.to_string()),
        };

        let extra_args = match obj.get("extra_args") {
            None => Vec::new(),
            Some(v) => {
                let arr = v
                    .as_array()
                    .ok_or("params.extra_args must be an array of strings")?;
                let mut out = Vec::with_capacity(arr.len());
                for entry in arr {
                    let s = entry
                        .as_str()
                        .ok_or_else(|| {
                            format!("params.extra_args: every entry must be a string, got {entry}")
                        })?
                        .to_string();
                    check_flag(&s)?;
                    out.push(s);
                }
                out
            }
        };

        // Fail-closed, and the reason is the second sentence of the contract's
        // `sandbox` row: a shape without a cap would be `trusted` under
        // another name, so `limits` stays required, and a cap that cannot be
        // enforced is fail-closed. The shipped `config.json` carried the cap,
        // so only an `override_params` that left `sandbox` out got an uncapped
        // browser -- the one shape nobody writes on purpose (OR-G54).
        let sandbox = match SandboxProfile::parse(raw)? {
            None => {
                return Err(
                    "params.sandbox is required for a browser cell and has no default: a \
                     browser is a process tree with a renderer per site, and a cell that \
                     runs one without a ceiling can take the machine with it. Declare the \
                     cap -- {\"trust\": \"restricted\", \"network\": \"allow\", \"limits\": \
                     {\"memory_max_bytes\": 2000000000, \"cpu_max_percent\": 200, \
                     \"pids_max\": 512}} -- or say {\"trust\": \"trusted\"} and mean it."
                        .to_string(),
                );
            }
            Some(SandboxProfile::Restricted { limits: None, .. }) => {
                return Err(
                    "params.sandbox.limits is required for a browser cell: a restricted \
                     profile that caps nothing is trusted under another name, and this is \
                     the cell whose child forks a renderer per site."
                        .to_string(),
                );
            }
            Some(profile) => Some(profile),
        };

        Ok(Self {
            mount,
            chromium_path,
            user_data_dir,
            extra_args,
            max_pages: u32::try_from(positive(obj.get("max_pages"), "max_pages", 8)?)
                .map_err(|_| "params.max_pages is larger than a page count can be".to_string())?,
            suspend_after_ms: non_negative(
                obj.get("suspend_after_ms"),
                "suspend_after_ms",
                300_000,
            )?,
            throttle_after_ms: positive(obj.get("throttle_after_ms"), "throttle_after_ms", 30_000)?,
            screencast: parse_screencast(obj.get("screencast"))?,
            sandbox,
            external_timeout_ms: positive(
                obj.get("external_timeout_ms"),
                "external_timeout_ms",
                10_000,
            )?,
            startup_timeout_ms: positive(
                obj.get("startup_timeout_ms"),
                "startup_timeout_ms",
                20_000,
            )?,
        })
    }
}

/// Reject a flag the cell owns, and the one flag nobody owns.
fn check_flag(flag: &str) -> Result<(), String> {
    // R-G11, and it is the whole reason this check exists as its own arm: the
    // sandbox knob was struck, so a browser whose sandbox does not hold must
    // not start. An operator flag would be the one door left open.
    if flag == "--no-sandbox" || flag.starts_with("--no-sandbox=") {
        return Err(
            "params.extra_args: --no-sandbox is refused. The browser's sandbox \
                    belongs to the package it came from, and a cell that could switch it \
                    off would be a special right for one instance; install a browser whose \
                    sandbox works instead."
                .to_string(),
        );
    }
    for owned in OWNED_FLAG_PREFIXES {
        if flag == owned || flag.starts_with(&format!("{owned}=")) || flag.starts_with(owned) {
            return Err(format!(
                "params.extra_args: {owned} belongs to the cell — it is how the cell reaches \
                 its own browser, not how the browser behaves. Remove {flag:?}."
            ));
        }
    }
    Ok(())
}

/// Parse `params.screencast`, filling every absent key from the default.
fn parse_screencast(raw: Option<&JsonValue>) -> Result<ScreencastParams, String> {
    let Some(v) = raw else {
        return Ok(ScreencastParams::default());
    };
    let obj = v
        .as_object()
        .ok_or("params.screencast must be a JSON object")?;
    for key in obj.keys() {
        if !SCREENCAST_KEYS.contains(&key.as_str()) {
            return Err(format!(
                "params.screencast: unknown key {key:?} (allowed: {})",
                SCREENCAST_KEYS.join(", ")
            ));
        }
    }
    let default = ScreencastParams::default();
    let format = match obj.get("format") {
        None => default.format.clone(),
        Some(v) => match v.as_str() {
            Some(f @ ("jpeg" | "png")) => f.to_string(),
            _ => {
                return Err(format!(
                    "params.screencast.format must be \"jpeg\" or \"png\", got {v}"
                ));
            }
        },
    };
    let quality = u32::try_from(positive(obj.get("quality"), "screencast.quality", 60)?)
        .map_err(|_| "params.screencast.quality must be 1..=100".to_string())?;
    if quality > 100 {
        return Err("params.screencast.quality must be 1..=100".to_string());
    }
    Ok(ScreencastParams {
        format,
        quality,
        max_width: small(
            obj.get("max_width"),
            "screencast.max_width",
            default.max_width,
        )?,
        max_height: small(
            obj.get("max_height"),
            "screencast.max_height",
            default.max_height,
        )?,
        max_fps: small(obj.get("max_fps"), "screencast.max_fps", default.max_fps)?,
    })
}

/// One `u64` knob that must be greater than zero.
fn positive(raw: Option<&JsonValue>, field: &str, default: u64) -> Result<u64, String> {
    let Some(v) = raw else { return Ok(default) };
    let n = v
        .as_u64()
        .ok_or_else(|| format!("params.{field} must be a positive integer, got {v}"))?;
    if n == 0 {
        return Err(format!("params.{field} must be greater than zero"));
    }
    Ok(n)
}

/// One `u64` knob where zero has a meaning of its own.
fn non_negative(raw: Option<&JsonValue>, field: &str, default: u64) -> Result<u64, String> {
    let Some(v) = raw else { return Ok(default) };
    v.as_u64()
        .ok_or_else(|| format!("params.{field} must be a non-negative integer, got {v}"))
}

/// One `u32` knob that must be greater than zero.
fn small(raw: Option<&JsonValue>, field: &str, default: u32) -> Result<u32, String> {
    let n = positive(raw, field, u64::from(default))?;
    u32::try_from(n).map_err(|_| format!("params.{field} is out of range"))
}

/// The profile directory this cell runs its browser in.
///
/// `/tmp/meclaw-browser-<cell_id>` by default, and deliberately not
/// `$XDG_RUNTIME_DIR`: measured on 2026-09-18, a snap-packaged browser is
/// refused `$XDG_RUNTIME_DIR`, `/dev/shm` and `~/.cache`, and what is writable
/// to it is `/tmp` and its own `~/snap/<package>/common`. OR-G4 fell on that
/// point.
///
/// The cell creates it and takes it away again at the end of its life (T9), so
/// it is never the cell directory: a directory that is removed must not be one
/// the No-Delete policy protects.
pub fn profile_dir_for(params: &BrowserParams, cell_id: &str) -> PathBuf {
    match &params.user_data_dir {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(format!("/tmp/meclaw-browser-{cell_id}")),
    }
}

/// The cell id out of the cell's OWN `config.json`, for the profile directory.
///
/// Its own directory, which is the only one a cell may read. A tree without the
/// key — a hand-written one, a fixture — falls back to the directory name,
/// which is unique inside a colony for the same reason a path is.
pub fn cell_id_of(cell_dir: &std::path::Path) -> String {
    let from_config = std::fs::read_to_string(cell_dir.join("config.json"))
        .ok()
        .and_then(|text| meclaw_core::serde_json::from_str::<JsonValue>(&text).ok())
        .and_then(|v| {
            v.get("cell")
                .and_then(|c| c.get("id"))
                .and_then(JsonValue::as_str)
                .map(str::to_string)
        });
    from_config.unwrap_or_else(|| {
        cell_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "cell".to_string())
    })
}
