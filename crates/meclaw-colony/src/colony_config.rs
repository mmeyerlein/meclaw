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
    /// Maximum `one_for_one` restarts per cell before `failed`.
    pub restart_max_retries: u32,
    /// Threshold (bytes) at/above which a UBF body is offloaded to a blob.
    pub blob_inline_max_bytes: usize,
    /// Hard limit for recursive blob-reference resolution.
    pub blob_max_recursion_depth: u32,
    /// Release-build default for JSON-schema validation against `emits`/`consumes`.
    pub strict_validation: bool,
    /// Tracing default level (overridable via `--log-level`).
    pub log_default_level: String,
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
            restart_max_retries: 5,
            blob_inline_max_bytes: 65_536,
            blob_max_recursion_depth: 64,
            strict_validation: false,
            log_default_level: "info".to_string(),
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
        Ok(cfg)
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
        // Only the three genuinely-unwired fields below remain forensic-only.
        if self.restart_max_retries != d.restart_max_retries {
            warn("restart_max_retries");
        }
        if self.blob_max_recursion_depth != d.blob_max_recursion_depth {
            warn("blob_max_recursion_depth");
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
    /// Paket 7) and have their own no-warn pins below.
    #[test]
    fn other_unwired_fields_still_warn() {
        let cfg = ColonyConfig::parse_str(
            r#"{
                "restart_max_retries": 1,
                "blob_max_recursion_depth": 1,
                "log_default_level": "debug"
            }"#,
        )
        .unwrap();
        let warns = capture_unwired_warns(&cfg);
        for field in &[
            "restart_max_retries",
            "blob_max_recursion_depth",
            "log_default_level",
        ] {
            assert!(
                warns.contains(&(*field).to_owned()),
                "expected unwired-warn for {field} but got: {warns:?}"
            );
        }
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
