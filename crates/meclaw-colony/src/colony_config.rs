//! `colony.json` — colony-wide behaviour defaults.
//!
//! Spec: `docs/meclaw-overview.md` § "`colony.json` — Schema" (Z.361–401). The file
//! is **optional**: absence yields [`ColonyConfig::default`] (spec Z.330/401, the
//! mandatory-file variant was rejected). A *present but invalid* file is a **hard
//! error** ([`ColonyConfig::parse_str`] returns [`ConfigError`]) — Phase-13.5 Slice-6
//! ruling (strict-fail, consistent with the edge-hydration hard-fail). Path/logging
//! operations stay CLI flags and are deliberately absent here.

use serde::Deserialize;

/// Current supported `colony.json` schema version. A file declaring any other
/// version is rejected (migration contract — spec Z.388).
pub const COLONY_CONFIG_SCHEMA_VERSION: u32 = 1;

/// Parsed `colony.json`. Every field carries a spec default, so a `{}` document
/// (or an absent file) deserialises to [`ColonyConfig::default`].
///
/// **Wiring status (Paket 1):**
/// - `blob_inline_max_bytes` (A8 offload): wired since Phase-13.5 Slice-6.
/// - `idle_timeout_default_ms`: wired since Phase-13.5 Slice-6 (single live
///   idle-default source for every spawn path; `DEFAULT_IDLE_TIMEOUT_MS` is only
///   its seed).
/// - `mailbox_default_capacity`: **now wired (Paket 1)** — all spawn paths resolve
///   `cell.mailbox_size ?? mailbox_default_capacity ?? 1000`.
/// - `message_default_ttl`: **now wired (TTL slice 2026-06-11)** — the colony
///   outputs-arm stamps it on source emissions (timer/proxy/mcp origin) and the
///   HTTP ingress uses it as the per-message default (overridable per initial
///   message via the `ttl` request field).
/// - `watchdog_threshold` / `watchdog_period_ms` / `watchdog_on_trip`: **now
///   wired (GH #84)** — `meclaw-cli` resolves the heartbeat-watchdog supervisor
///   from these unless a caller passes an explicit override (tests).
/// - All other fields parse but are not yet consumed — the boot reader emits a
///   `tracing::warn` per such field set away from its default (see
///   `read_colony_config`). Full wiring is deferred per consumer slice.
#[derive(Debug, Clone, PartialEq, Deserialize)]
// P1-7: reject genuinely-unknown keys hard (typo/forward-incompat guard). Known
// fields keep their `#[serde(default)]` fill; only real unknowns fail parsing.
#[serde(default, deny_unknown_fields)]
pub struct ColonyConfig {
    /// Schema version marker for migration compatibility (spec Z.388).
    pub schema_version: u32,
    /// Default capacity of the bounded mpsc mailboxes (cells and colony).
    pub mailbox_default_capacity: usize,
    /// Default substrate-backstop per `handle()` call, in ms (concept B).
    pub message_timeout_default_ms: u64,
    /// Default idle duration (ms) for stateful cells with `cell.timeout: 0`.
    pub idle_timeout_default_ms: u64,
    /// Default TTL for source messages (routing-loop guard).
    pub message_default_ttl: u32,
    /// GH #119 — opt in to the **terminal notice** of a TTL death.
    ///
    /// Off (the default) an expired TTL is terminal and silent exactly as the
    /// spec describes: the message dead-letters and takes no `reply_to`
    /// cascade. On, a TTL death that carries a reply anchor additionally sends
    /// ONE substrate error reply (`error_code: "ttl_expired"`) to that anchor,
    /// so a waiting fan-in has something to route on instead of parking
    /// forever.
    ///
    /// It is opt-in for the same reason `modifier.restore_ttl` is: the notice
    /// carries a fresh `message_default_ttl`, so a colony that turns it on has
    /// taken its loops out of the TTL guard and must bound them with an
    /// iteration counter instead. Everything that does not opt in keeps the
    /// sharp guard.
    pub ttl_notice: bool,
    /// GH #553 — opt in to the **mutation receipt**: where a committed knock at
    /// the mutation door leaves its event.
    ///
    /// A committed mutation is an event only its own caller could see: the
    /// verdict travels back on `reply_to`, and both `--apply` and
    /// `POST /colony/mutations` set no `reply_to` at all. Everything else that
    /// wanted to know "the graph moved" had to ask on a timer — a poll in an
    /// event-driven substrate, paid for in availability.
    ///
    /// With this set, the door emits ONE terminal message per committed knock
    /// (and one more right after the boot's `InitialApply`, so a restart fills
    /// what listens without waiting for the first mutation of the day). Off —
    /// the default, and the state of every colony that says nothing — nothing
    /// is emitted and nothing about a mutation changes.
    ///
    /// Opt-in for the same reason `ttl_notice` is: the receipt is new traffic
    /// in a topology that did not have it, and who hears it is the colony's
    /// decision, never the substrate's.
    pub mutation_receipts: Option<MutationReceipts>,
    /// Maximum `one_for_one` restarts per cell before `failed`.
    pub restart_max_retries: u32,
    /// Threshold (bytes) at/above which a UBF body is offloaded to a blob.
    pub blob_inline_max_bytes: usize,
    /// Hard limit for recursive blob-reference resolution.
    pub blob_max_recursion_depth: u32,
    /// GH #285 (W4 T12) — how many messages ONE `park` slot may hold while
    /// nothing is bound behind it.
    ///
    /// A `park` declaration promises the message a future, and a promise with
    /// no bound is a memory leak with a nice name: a slot nobody ever fills
    /// would grow its queue for as long as the colony runs. At the bound the
    /// substrate refuses the NEWEST arrival (`slot_park_overflow`) so the
    /// earliest context — the part a later reader cannot reconstruct — is the
    /// part that survives. `0` is a valid kill-switch: every message onto an
    /// unbound `park` slot is refused, and the declaration reads like `error`
    /// with a different code.
    pub slot_park_max: usize,
    /// Release-build default for JSON-schema validation against `emits`/`consumes`.
    pub strict_validation: bool,
    /// Tracing default level (overridable via `--log-level`).
    pub log_default_level: String,
    /// Consecutive silent supervisor periods before the heartbeat watchdog trips.
    ///
    /// GH #84: the deadline used to be hard-wired in `meclaw-cli`, reachable only
    /// from a test-facing entry point, so a box on which `5 × 100 ms` was too
    /// tight had no way to say so. Must be `>= 1`.
    pub watchdog_threshold: u32,
    /// Length of one heartbeat-watchdog supervisor period, in ms. Must be `>= 1`.
    pub watchdog_period_ms: u64,
    /// GH #47: how long the shutdown drain waits for quiescence before it cuts
    /// and names the rest. `0` disables the drain entirely and restores the
    /// pre-#47 teardown.
    ///
    /// The default is anchored on systemd, not on taste: the shipped unit runs
    /// `TimeoutStopSec=30`, and the drain is only the first of four budgets the
    /// process spends after the signal (drain, shutdown ack, colony join,
    /// bridge join). 10 s leaves the rest of that chain inside the window.
    #[serde(default = "default_shutdown_drain_timeout_ms")]
    pub shutdown_drain_timeout_ms: u64,
    /// What a watchdog trip does: end the process (`exit`, the default and the
    /// production contract of issue #6) or report it and keep running
    /// (`log-only`). See [`crate::watchdog::WatchdogOnTrip`].
    pub watchdog_on_trip: crate::watchdog::WatchdogOnTrip,
}

/// GH #553 — where the mutation door leaves its receipt.
///
/// `to` is a **hive** path. A hive is what makes the receipt an event rather
/// than a delivery: the message enters as a hive transit, so the hive's own
/// `{"from": "."}` edges decide who hears it and under which lane, and the
/// substrate never learns a single listener's address. Naming a cell works and
/// delivers to exactly that cell — it is simply the narrower choice.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationReceipts {
    /// Absolute path the receipt is addressed to (a hive, by intent).
    pub to: String,
}

/// Serde/`Default` seed for [`ColonyConfig::shutdown_drain_timeout_ms`] — one
/// source for both, so the two can never drift apart (GH #47).
fn default_shutdown_drain_timeout_ms() -> u64 {
    10_000
}

impl Default for ColonyConfig {
    fn default() -> Self {
        ColonyConfig {
            schema_version: COLONY_CONFIG_SCHEMA_VERSION,
            mailbox_default_capacity: 1000,
            message_timeout_default_ms: 60_000,
            // Sole consumer of the constant: the colony-wide idle default is
            // seeded here (Phase-13.5 Slice-6 Nachzieh-Fix — all live spawn-path
            // fallbacks now read this field, never the constant directly).
            idle_timeout_default_ms: crate::DEFAULT_IDLE_TIMEOUT_MS,
            // Seeded from the substrate constant (TTL slice 2026-06-11) — the
            // live consumers (outputs-arm source emissions, HTTP ingress) read
            // this field, never the constant directly.
            message_default_ttl: meclaw_core::MESSAGE_DEFAULT_TTL,
            // GH #119: OFF by default — turning it on changes what an expired
            // TTL means for a loop, so it is a colony's decision, never the
            // substrate's (same discipline as `modifier.restore_ttl`).
            ttl_notice: false,
            // GH #553: OFF by default — a receipt is traffic a topology has to
            // be built for, so the substrate never invents a listener.
            mutation_receipts: None,
            restart_max_retries: 5,
            blob_inline_max_bytes: 65_536,
            blob_max_recursion_depth: 64,
            slot_park_max: 64,
            strict_validation: false,
            log_default_level: "info".to_string(),
            // GH #84: exactly the values `meclaw-cli` used to hard-wire. Making
            // the knob reachable must not move the default by a single ms.
            watchdog_threshold: 5,
            watchdog_period_ms: 100,
            watchdog_on_trip: crate::watchdog::WatchdogOnTrip::Exit,
            // GH #47: `0` stays a valid, documented choice (ruling O7), so this
            // number is a default and never a floor — `parse_str` does not
            // reject it the way it rejects a zero watchdog period.
            shutdown_drain_timeout_ms: default_shutdown_drain_timeout_ms(),
        }
    }
}

/// Error reading or parsing `colony.json`. A present-but-broken file is a hard
/// boot failure (strict-fail ruling); absence is **not** an error (caller uses
/// the default instead).
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file is present but is not valid JSON for [`ColonyConfig`].
    #[error("colony.json parse error: {0}")]
    Parse(#[from] serde_json::Error),
    /// The file declares an unsupported `schema_version`.
    #[error("colony.json schema_version {found} unsupported (expected {expected})")]
    SchemaVersion {
        /// The version found in the file.
        found: u32,
        /// The version this build supports.
        expected: u32,
    },
    /// The file exists but could not be read (I/O error other than not-found).
    #[error("colony.json read error: {0}")]
    Io(std::io::Error),
    /// A field parsed but carries a value the substrate cannot run with.
    #[error("colony.json field `{field}` is invalid: {reason}")]
    InvalidField {
        /// The offending key.
        field: &'static str,
        /// Why the value cannot be used.
        reason: String,
    },
}

impl ColonyConfig {
    /// Parse a `colony.json` document string. Missing fields fall back to the
    /// spec defaults; an unsupported `schema_version` is rejected.
    pub fn parse_str(s: &str) -> Result<Self, ConfigError> {
        let cfg: ColonyConfig = serde_json::from_str(s)?;
        if cfg.schema_version != COLONY_CONFIG_SCHEMA_VERSION {
            return Err(ConfigError::SchemaVersion {
                found: cfg.schema_version,
                expected: COLONY_CONFIG_SCHEMA_VERSION,
            });
        }
        // GH #84: both watchdog numbers reach `tokio::time::interval`, which
        // panics on a zero period, and a threshold of 0 would trip on the first
        // tick. Strict-fail here (the `colony.json` ruling) rather than clamp
        // silently: a config that cannot mean what it says is a boot failure,
        // and `--validate` surfaces it before the daemon ever runs.
        if cfg.watchdog_threshold == 0 {
            return Err(ConfigError::InvalidField {
                field: "watchdog_threshold",
                reason: "must be >= 1 (0 would trip on the first supervisor period)".to_string(),
            });
        }
        if cfg.watchdog_period_ms == 0 {
            return Err(ConfigError::InvalidField {
                field: "watchdog_period_ms",
                reason: "must be >= 1 ms (a zero-length supervisor period is not a period)"
                    .to_string(),
            });
        }
        // GH #553: a receipt target that is not an absolute path can never be
        // routed, and a colony that asked for receipts and silently gets none
        // is the worst of the three outcomes. Strict-fail, like the two above.
        if let Some(r) = &cfg.mutation_receipts
            && !r.to.starts_with('/')
        {
            return Err(ConfigError::InvalidField {
                field: "mutation_receipts.to",
                reason: format!("must be an absolute path, got `{}`", r.to),
            });
        }
        Ok(cfg)
    }

    /// The heartbeat-watchdog supervisor period as a [`std::time::Duration`].
    ///
    /// One conversion site for the whole substrate; [`ColonyConfig::parse_str`]
    /// has already rejected `0`, so the value is always a real period.
    pub fn watchdog_period(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.watchdog_period_ms)
    }

    /// Warn (via `tracing`) for each field that is parsed but **not yet applied**
    /// in this version, when it has been set away from its spec default. Only the
    /// three genuinely-unwired fields remain: `restart_max_retries`,
    /// `blob_max_recursion_depth`, `log_default_level` (roadmap § Restliche
    /// `colony.json`-Feld-Verdrahtung). Every other field is consumed and must
    /// NOT warn — a warn on a wired field is a lying "ignored" diagnostic. Silent
    /// ignoring of the real gaps would contradict the strict-fail ruling, so we
    /// keep those three audible.
    pub(crate) fn warn_unwired_fields(&self) {
        let d = ColonyConfig::default();
        let warn = |name: &str| {
            tracing::warn!(
                field = name,
                "colony.json field parsed but not applied in this version"
            );
        };
        // Fully-wired fields do NOT warn (would be a lying "ignored" diagnostic):
        //   mailbox_default_capacity (Paket 1) — all spawn paths resolve it.
        //   message_default_ttl (TTL slice 2026-06-11) — outputs-arm + HTTP ingress.
        //   message_timeout_default_ms (Paket 3 / U1, P3-B-plumb-2) — B-backstop at
        //     every spawn call-site.
        //   strict_validation (Paket 7 / B5) — `resolve_validate_emits` consumes it.
        //   blob_inline_max_bytes + idle_timeout_default_ms (Slice-6 / A7-A8).
        //   blob_max_recursion_depth (GH #19 / D-025) — carried on the blob
        //     store and read at the cell-delivery boundary.
        //   slot_park_max (GH #285 / W4 T12) — read at both slot-delivery
        //     filters, where it bounds a `park` slot's queue.
        // Only the two genuinely-unwired fields below remain forensic-only.
        if self.restart_max_retries != d.restart_max_retries {
            warn("restart_max_retries");
        }
        if self.log_default_level != d.log_default_level {
            warn("log_default_level");
        }
    }
}

/// Read `colony.json` from the colony root.
///
/// **Absent** → [`ColonyConfig::default`] (spec Z.330/401, optional file).
/// **Present but broken / wrong schema** → `Err` (strict-fail; the caller turns
/// this into a hard boot failure and `--validate` surfaces it).
///
/// On success, [`ColonyConfig::warn_unwired_fields`] runs so set-but-unapplied
/// fields are audible in the log.
pub fn read_colony_config(root: &std::path::Path) -> Result<ColonyConfig, ConfigError> {
    let path = root.join("colony.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ColonyConfig::default()),
        Err(e) => return Err(ConfigError::Io(e)),
    };
    let cfg = ColonyConfig::parse_str(&raw)?;
    cfg.warn_unwired_fields();
    Ok(cfg)
}

/// Resolve the effective B-backstop timeout for a cell.
///
/// `cell_field` is the `config.json` `cell.message_timeout` value
/// (`Option<i64>`). `colony_default` is `colony.json`
/// `message_timeout_default_ms` (`u64`). Returns `Some(Duration)` for a
/// positive effective value, `None` for `0` or `-1` (no backstop), per
/// `docs/config.md` Z.53 and `docs/meclaw-overview.md` § Timeouts B.
///
/// The per-cell field **overrides** the colony default when present.
///
/// The error code emitted by the substrate when this backstop fires is
/// `"message_timeout"` (`header.finish_reason: "error"`,
/// `header.error_code: "message_timeout"`).
pub fn resolve_message_timeout(
    cell_field: Option<i64>,
    colony_default: u64,
) -> Option<std::time::Duration> {
    let ms = match cell_field {
        Some(n) if n > 0 => n as u64,
        Some(_) => return None, // 0 or negative → no backstop
        None => {
            if colony_default == 0 {
                return None;
            }
            colony_default
        }
    };
    Some(std::time::Duration::from_millis(ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- warn_unwired_fields tests ---
    //
    // `warn_unwired_fields` is `pub(crate)` so we can call it directly.
    // We use a minimal `tracing::Subscriber` (no external dep) to capture
    // warning events.  The collector stores warn-event field names into a
    // `Mutex<Vec<String>>` that the test then inspects.

    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::subscriber::Subscriber;
    use tracing::{Event, Level, Metadata, span};

    /// Minimal tracing subscriber that records the `field` value from every
    /// WARN-level event emitted by `colony_config`.
    struct WarnCapture {
        fields: Arc<Mutex<Vec<String>>>,
    }

    impl Subscriber for WarnCapture {
        fn enabled(&self, meta: &Metadata<'_>) -> bool {
            *meta.level() == Level::WARN
        }
        fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(1)
        }
        fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
        fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
        fn event(&self, event: &Event<'_>) {
            let mut visitor = FieldVisitor { captured: None };
            event.record(&mut visitor);
            if let Some(name) = visitor.captured {
                self.fields.lock().unwrap().push(name);
            }
        }
        fn enter(&self, _: &span::Id) {}
        fn exit(&self, _: &span::Id) {}
    }

    struct FieldVisitor {
        captured: Option<String>,
    }

    impl Visit for FieldVisitor {
        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "field" {
                self.captured = Some(value.to_owned());
            }
        }
        fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
    }

    fn capture_unwired_warns(cfg: &ColonyConfig) -> Vec<String> {
        let fields = Arc::new(Mutex::new(Vec::<String>::new()));
        let subscriber = WarnCapture {
            fields: Arc::clone(&fields),
        };
        tracing::subscriber::with_default(subscriber, || {
            cfg.warn_unwired_fields();
        });
        // Read the captured fields via the lock — NOT `Arc::try_unwrap`. Under
        // full-workspace parallel load `tracing`'s `with_default` dispatch can
        // briefly outlive this scope (the subscriber's `fields` Arc-clone lingers),
        // so `try_unwrap` intermittently sees >1 strong ref and panics. The lock
        // read is deterministic regardless of how many Arc refs exist (the
        // captured Vec is fully populated synchronously inside the closure above).
        let guard = fields.lock().unwrap();
        guard.clone()
    }

    /// message_default_ttl is now wired (TTL slice, 2026-06-11): consumed by the
    /// outputs-arm source-emission TTL and the HTTP-ingress default. A config
    /// that deviates only in this field must NOT produce a "parsed but not
    /// applied" warning.
    #[test]
    fn message_default_ttl_no_unwired_warn() {
        let cfg = ColonyConfig::parse_str(r#"{"message_default_ttl": 128}"#).unwrap();
        let warns = capture_unwired_warns(&cfg);
        assert!(
            !warns.contains(&"message_default_ttl".to_owned()),
            "unexpected unwired-warn for message_default_ttl: {warns:?}"
        );
    }

    /// The colony-wide TTL default is seeded from the substrate constant —
    /// single live source, same pattern as `idle_timeout_default_ms`.
    #[test]
    fn message_default_ttl_default_seeded_from_constant() {
        assert_eq!(
            ColonyConfig::default().message_default_ttl,
            meclaw_core::MESSAGE_DEFAULT_TTL
        );
    }

    /// mailbox_default_capacity is now wired (Paket 1).  A config that deviates
    /// only in this field must NOT produce a "parsed but not applied" warning.
    #[test]
    fn mailbox_default_capacity_no_unwired_warn() {
        let cfg = ColonyConfig::parse_str(r#"{"mailbox_default_capacity": 500}"#).unwrap();
        let warns = capture_unwired_warns(&cfg);
        assert!(
            !warns.contains(&"mailbox_default_capacity".to_owned()),
            "unexpected unwired-warn for mailbox_default_capacity: {warns:?}"
        );
    }

    /// The genuinely-unwired fields must continue to produce warnings when they
    /// deviate from their defaults. `message_timeout_default_ms` and
    /// `strict_validation` are NOT in this set — both are fully wired (Paket 3 /
    /// Paket 7) — and `blob_max_recursion_depth` left it with GH #19.
    #[test]
    fn other_unwired_fields_still_warn() {
        let cfg = ColonyConfig::parse_str(
            r#"{
                "restart_max_retries": 1,
                "log_default_level": "debug"
            }"#,
        )
        .unwrap();
        let warns = capture_unwired_warns(&cfg);
        for field in &["restart_max_retries", "log_default_level"] {
            assert!(
                warns.contains(&(*field).to_owned()),
                "expected unwired-warn for {field} but got: {warns:?}"
            );
        }
    }

    /// GH #19: `blob_max_recursion_depth` is wired — it rides on the blob store
    /// and bounds in-message pointer resolution at the delivery boundary. A
    /// config that deviates only in this field must NOT warn any more; a warn
    /// here would be the lying "ignored" diagnostic the doc comment on
    /// `warn_unwired_fields` warns about.
    #[test]
    fn blob_max_recursion_depth_no_unwired_warn() {
        let cfg = ColonyConfig::parse_str(r#"{"blob_max_recursion_depth": 8}"#).unwrap();
        let warns = capture_unwired_warns(&cfg);
        assert!(
            !warns.contains(&"blob_max_recursion_depth".to_owned()),
            "unexpected unwired-warn for blob_max_recursion_depth: {warns:?}"
        );
    }

    /// message_timeout_default_ms is fully wired (Paket 3 / U1, P3-B-plumb-2):
    /// the colony.json B-backstop default resolved at every spawn call-site. A
    /// config that deviates only in this field must NOT produce a "parsed but
    /// not applied" warning.
    #[test]
    fn message_timeout_default_ms_no_unwired_warn() {
        let cfg = ColonyConfig::parse_str(r#"{"message_timeout_default_ms": 1}"#).unwrap();
        let warns = capture_unwired_warns(&cfg);
        assert!(
            !warns.contains(&"message_timeout_default_ms".to_owned()),
            "unexpected unwired-warn for message_timeout_default_ms: {warns:?}"
        );
    }

    /// strict_validation is fully wired (Paket 7 / B5): `resolve_validate_emits`
    /// consumes it on every spawn/mutation validate-emits resolution. A config
    /// that deviates only in this field must NOT produce a "parsed but not
    /// applied" warning.
    #[test]
    fn strict_validation_no_unwired_warn() {
        let cfg = ColonyConfig::parse_str(r#"{"strict_validation": true}"#).unwrap();
        let warns = capture_unwired_warns(&cfg);
        assert!(
            !warns.contains(&"strict_validation".to_owned()),
            "unexpected unwired-warn for strict_validation: {warns:?}"
        );
    }

    /// GH #553 — the receipt target is opt-in, and a target that cannot be
    /// routed is a boot failure rather than a silence. A colony that ordered
    /// receipts and quietly gets none is the worst of the three outcomes, so
    /// this fails the same way a zero watchdog period does.
    #[test]
    fn a_receipt_target_is_optional_and_must_be_absolute() {
        assert!(
            ColonyConfig::default().mutation_receipts.is_none(),
            "off by default — a receipt is traffic a topology has to be built for"
        );
        assert!(
            ColonyConfig::parse_str("{}")
                .unwrap()
                .mutation_receipts
                .is_none()
        );

        let on = ColonyConfig::parse_str(r#"{"mutation_receipts": {"to": "/os"}}"#)
            .expect("an absolute target parses");
        assert_eq!(on.mutation_receipts.expect("set").to, "/os");

        let err = ColonyConfig::parse_str(r#"{"mutation_receipts": {"to": "os"}}"#)
            .expect_err("a relative target can never be routed");
        assert!(
            matches!(err, ConfigError::InvalidField { field, .. } if field == "mutation_receipts.to"),
            "the refusal names the field: {err:?}"
        );

        assert!(
            ColonyConfig::parse_str(r#"{"mutation_receipts": {"to": "/os", "who": "x"}}"#).is_err(),
            "the block denies unknown keys, like every other one"
        );
    }

    #[test]
    fn default_has_spec_values() {
        let c = ColonyConfig::default();
        assert_eq!(c.schema_version, 1);
        assert_eq!(c.blob_inline_max_bytes, 65_536);
        assert_eq!(c.idle_timeout_default_ms, 60_000);
        assert_eq!(c.message_default_ttl, 64);
        assert!(!c.strict_validation);
        assert_eq!(c.log_default_level, "info");
    }

    // --- GH #84: the watchdog knobs ---

    /// The load-bearing half of "make the tuning reachable": reachable must not
    /// mean changed. The defaults are the values `meclaw-cli` hard-wired, to the
    /// millisecond, and the default policy is the issue-#6 process exit.
    #[test]
    fn watchdog_defaults_are_the_hard_wired_values_unchanged() {
        let c = ColonyConfig::default();
        assert_eq!(c.watchdog_threshold, 5);
        assert_eq!(c.watchdog_period_ms, 100);
        assert_eq!(c.watchdog_period(), Duration::from_millis(100));
        assert_eq!(c.watchdog_on_trip, crate::watchdog::WatchdogOnTrip::Exit);
        // And an absent file (the common case) means exactly the same thing.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_colony_config(dir.path()).unwrap(), c);
    }

    /// GH #119: the TTL-notice guarantee is opt-in and OFF by default — a
    /// colony that says nothing keeps the sharp, silent TTL guard.
    #[test]
    fn ttl_notice_is_off_unless_the_colony_asks_for_it() {
        assert!(
            !ColonyConfig::default().ttl_notice,
            "default must stay off — turning it on changes what TTL bounds"
        );
        assert!(!ColonyConfig::parse_str("{}").unwrap().ttl_notice);
        assert!(
            ColonyConfig::parse_str(r#"{"ttl_notice": true}"#)
                .unwrap()
                .ttl_notice,
            "and it parses from colony.json when asked for"
        );
    }

    /// The three knobs parse from `colony.json`, including the kebab-case policy
    /// spelling an operator actually writes.
    #[test]
    fn watchdog_knobs_parse_from_colony_json() {
        let c = ColonyConfig::parse_str(
            r#"{"watchdog_threshold": 20, "watchdog_period_ms": 250,
                "watchdog_on_trip": "log-only"}"#,
        )
        .unwrap();
        assert_eq!(c.watchdog_threshold, 20);
        assert_eq!(c.watchdog_period(), Duration::from_millis(250));
        assert_eq!(c.watchdog_on_trip, crate::watchdog::WatchdogOnTrip::LogOnly);
        // Untouched neighbours keep their defaults.
        assert_eq!(
            c.message_default_ttl,
            ColonyConfig::default().message_default_ttl
        );
    }

    /// `exit` is spelled the obvious way and stays available explicitly.
    #[test]
    fn watchdog_on_trip_exit_parses() {
        let c = ColonyConfig::parse_str(r#"{"watchdog_on_trip": "exit"}"#).unwrap();
        assert_eq!(c.watchdog_on_trip, crate::watchdog::WatchdogOnTrip::Exit);
    }

    /// An unknown policy is a hard error, not a silent fallback to `exit`: an
    /// operator who typed `log_only` must learn it, not run production semantics
    /// while believing otherwise.
    #[test]
    fn an_unknown_trip_policy_is_rejected() {
        assert!(ColonyConfig::parse_str(r#"{"watchdog_on_trip": "log_only"}"#).is_err());
        assert!(ColonyConfig::parse_str(r#"{"watchdog_on_trip": "ignore"}"#).is_err());
    }

    /// Zero is not a period and zero is not a threshold. Both reach
    /// `tokio::time::interval` / the miss counter, so both are refused at parse
    /// time rather than at the first tick.
    #[test]
    fn zero_watchdog_values_are_refused() {
        let err = ColonyConfig::parse_str(r#"{"watchdog_threshold": 0}"#).unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::InvalidField {
                    field: "watchdog_threshold",
                    ..
                }
            ),
            "was: {err:?}"
        );
        let err = ColonyConfig::parse_str(r#"{"watchdog_period_ms": 0}"#).unwrap_err();
        assert!(
            matches!(
                err,
                ConfigError::InvalidField {
                    field: "watchdog_period_ms",
                    ..
                }
            ),
            "was: {err:?}"
        );
    }

    /// The knobs are wired, so they must not appear in the "parsed but not
    /// applied" warning set — that diagnostic would be a lie.
    #[test]
    fn watchdog_fields_do_not_warn_as_unwired() {
        let cfg = ColonyConfig::parse_str(
            r#"{"watchdog_threshold": 50, "watchdog_period_ms": 200,
                "watchdog_on_trip": "log-only"}"#,
        )
        .unwrap();
        let warns = capture_unwired_warns(&cfg);
        assert!(warns.is_empty(), "unexpected unwired-warns: {warns:?}");
    }

    // --- GH #47: the shutdown drain budget ---

    /// GH #47: the drain budget is configurable and has an honest default.
    #[test]
    fn shutdown_drain_timeout_defaults_to_ten_seconds() {
        let c = ColonyConfig::default();
        assert_eq!(c.shutdown_drain_timeout_ms, 10_000);
    }

    /// GH #47 ruling O7: `0` is the documented off switch, not an error. It
    /// restores the pre-drain teardown byte for byte, which is what makes this
    /// change revertible without a redeploy.
    #[test]
    fn a_zero_shutdown_drain_timeout_is_accepted_and_means_off() {
        let c = ColonyConfig::parse_str(r#"{"shutdown_drain_timeout_ms": 0}"#)
            .expect("zero must parse");
        assert_eq!(c.shutdown_drain_timeout_ms, 0);
    }

    #[test]
    fn shutdown_drain_timeout_is_read_from_colony_json() {
        let c =
            ColonyConfig::parse_str(r#"{"shutdown_drain_timeout_ms": 2500}"#).expect("must parse");
        assert_eq!(c.shutdown_drain_timeout_ms, 2_500);
    }

    #[test]
    fn empty_object_equals_default() {
        assert_eq!(
            ColonyConfig::parse_str("{}").unwrap(),
            ColonyConfig::default()
        );
    }

    #[test]
    fn partial_override_keeps_other_defaults() {
        let c = ColonyConfig::parse_str(r#"{"blob_inline_max_bytes": 1024}"#).unwrap();
        assert_eq!(c.blob_inline_max_bytes, 1024);
        assert_eq!(c.idle_timeout_default_ms, 60_000); // untouched
    }

    #[test]
    fn broken_json_is_parse_error() {
        let err = ColonyConfig::parse_str("{ kaputt").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn unsupported_schema_version_is_rejected() {
        let err = ColonyConfig::parse_str(r#"{"schema_version": 2}"#).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::SchemaVersion {
                found: 2,
                expected: 1
            }
        ));
    }

    #[test]
    fn read_absent_file_yields_default() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = read_colony_config(dir.path()).unwrap();
        assert_eq!(cfg, ColonyConfig::default());
    }

    #[test]
    fn read_valid_file_applies_override() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("colony.json"),
            r#"{"blob_inline_max_bytes": 2048}"#,
        )
        .unwrap();
        let cfg = read_colony_config(dir.path()).unwrap();
        assert_eq!(cfg.blob_inline_max_bytes, 2048);
    }

    #[test]
    fn read_broken_file_is_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("colony.json"), "{ not json").unwrap();
        assert!(read_colony_config(dir.path()).is_err());
    }

    // --- resolve_message_timeout ---

    use std::time::Duration;

    /// Positive per-cell value → that duration, ignoring colony default.
    #[test]
    fn resolve_cell_positive_uses_cell_value() {
        assert_eq!(
            resolve_message_timeout(Some(5_000), 60_000),
            Some(Duration::from_millis(5_000))
        );
    }

    /// Per-cell 0 → None (no backstop), colony default irrelevant.
    #[test]
    fn resolve_cell_zero_is_none() {
        assert_eq!(resolve_message_timeout(Some(0), 60_000), None);
    }

    /// Per-cell -1 → None (no backstop).
    #[test]
    fn resolve_cell_minus_one_is_none() {
        assert_eq!(resolve_message_timeout(Some(-1), 60_000), None);
    }

    /// Any negative per-cell value → None.
    #[test]
    fn resolve_cell_negative_is_none() {
        assert_eq!(resolve_message_timeout(Some(-42), 60_000), None);
    }

    /// cell_field = None, colony_default > 0 → colony default duration.
    #[test]
    fn resolve_none_cell_falls_back_to_colony_default() {
        assert_eq!(
            resolve_message_timeout(None, 30_000),
            Some(Duration::from_millis(30_000))
        );
    }

    /// cell_field = None, colony_default = 0 → None (backstop disabled colony-wide).
    #[test]
    fn resolve_none_cell_colony_zero_is_none() {
        assert_eq!(resolve_message_timeout(None, 0), None);
    }

    /// Per-cell value overrides colony default even when colony default is larger.
    #[test]
    fn resolve_cell_positive_overrides_colony_default() {
        assert_eq!(
            resolve_message_timeout(Some(1_000), 120_000),
            Some(Duration::from_millis(1_000))
        );
    }

    /// Large per-cell value (e.g. 300 s) is handled correctly.
    #[test]
    fn resolve_large_value() {
        assert_eq!(
            resolve_message_timeout(Some(300_000), 60_000),
            Some(Duration::from_millis(300_000))
        );
    }
}
