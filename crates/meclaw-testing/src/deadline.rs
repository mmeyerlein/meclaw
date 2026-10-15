//! Hang marks: the only deadlines a test may carry (VORGABEN TV-Z3/TV-Z4).
//!
//! A hang mark is never a verdict on speed. It turns "the awaited event never
//! came" into a readable failure (`HANG: <what> (waited <n> s)`) instead of a
//! runner kill minutes later. It sits far above any observed wait, scales with
//! `MECLAW_TEST_TIME_FACTOR` for slow hosts, and is never shorter than
//! [`FLOOR`].
//!
//! Everything here runs on `tokio::time`, so under a paused clock
//! (`#[tokio::test(start_paused = true)]`) the mark is virtual: it fires only
//! once every task is idle and nothing but the mark is left to wait for.

use std::fmt::Debug;
use std::future::Future;
use std::time::Duration;

/// The hang mark of an ordinary wait.
pub const HANG: Duration = Duration::from_secs(30);

/// The hang mark of a boot (colony, binary, browser).
pub const BOOT_HANG: Duration = Duration::from_secs(60);

/// The hang mark of a replay after a boot: turns a stop left open, answered
/// only after a fresh colony has caught up (GH #1216, tv-br M10i). The
/// organism's `proto_ms` (release to the key in the first brain request) ran
/// to 39.9 s on mm-os01 under load and is measured in the M10i acceptance under
/// a CPU quota; the mark stays at least 3 × its P95 (TV-Z3), which
/// [`BOOT_HANG`] (2.2 ×) did not.
pub const REPLAY_HANG: Duration = Duration::from_secs(120);

/// The hang mark of a one-shot `meclaw --apply` child: a whole boot of a fresh
/// binary with the shipped library, the apply and the shutdown, awaited as one
/// event (GH #423). Basis (TV-Z3, coordinator rule K9: >= 3 x the P95 of the
/// step under full-suite load on the weakest lane at factor 1, never a verdict):
/// - P95 under overbooking at factor 1 (measured lighter than the full suite
///   and than the 80 % quota at F = 3, so not a full-suite P95): 14 busy
///   loops on build01 (12 vCPU, the weakest lane class), no quota, beside the
///   other gh423 and gh974 tests of the run; n = 20 full applies (one-shot and
///   stdin) in 10 runs: p50 16.6 s, P95 21.7 s, max 22.5 s (wave validation 2,
///   strand v2-flake-b01, fix round F, 2026-10-15). 3 x P95 = 65 s.
/// - The full-suite re-gate v2-fix-e on build01 at factor 1 is a censored lower
///   bound, not a P95: the old test died at its own 60 s deadline after 61.5 s,
///   so the step took longer than that there. 3 x 61.5 s = 184.5 s.
///
/// The mark is the larger of the two, rounded up: 190 s at factor 1. Under the
/// 80 % CPU quota (F = 3) the step took p50 46.4-52.5 s, max 59.5 s.
/// [`BOOT_HANG`] (60 s) sits below the re-gate bound.
pub const APPLY_BOOT_HANG: Duration = Duration::from_secs(190);

/// The hang mark of one person's turn through a freshly grown organism, from
/// the sent turn to its answer on the channel (GH #974): the brain's tool
/// round, the recall and the channel's code cell in one wait. Basis (TV-Z3,
/// K9 as for [`APPLY_BOOT_HANG`]):
/// - P95 under overbooking at factor 1 (lighter than the full suite, as for
///   [`APPLY_BOOT_HANG`]): 14 busy loops on build01 (12 vCPU),
///   no quota, beside the other gh423 and gh974 tests of the run; n = 30 turns
///   in 10 runs: p50 13.4 s, P95 14.2 s, max 14.3 s; the longest turn of a
///   run P95 14.2 s (wave validation 2, strand v2-flake-b01, fix round F,
///   2026-10-15). 3 x P95 = 43 s.
/// - The full-suite re-gate v2-fix-e on build01 at factor 1 is a censored lower
///   bound: the old test waited 90 s on one answer and died at its own
///   deadline, so a turn there took longer than 90 s. 3 x 90 s = 270 s.
///
/// The mark is the larger of the two: 270 s at factor 1. Under the 80 % CPU
/// quota (F = 3) a turn took p50 62.2 s, max 66.4 s (n = 15, build02).
/// [`HANG`] (30 s), [`BOOT_HANG`] (60 s) and [`REPLAY_HANG`] (120 s) sit below it.
pub const TURN_HANG: Duration = Duration::from_secs(270);

/// [`HANG`] in milliseconds at factor 1. A config value or driver that takes
/// ms reads [`hang_ms`], which carries the factor.
pub const HANG_MS: u64 = 30_000;

/// The hang mark of one browser driver run over a colony line, before the factor:
/// 3 x 340 s, the loaded P95 of a line (272-330 s under `--load 12`, wave testing C1)
/// rounded up to the minute. The tests hand it to the driver scaled
/// (`MECLAW_WEBKIT_RUN_MS`); `RUN_HANG_MS.colony` in `workshop/tools/wait.mjs` is the
/// same number for a run without the tests.
pub const BROWSER_RUN_COLONY: Duration = Duration::from_secs(1020);

/// The hang mark of one browser driver run over a part of the sheet line, before the
/// factor (`RUN_HANG_MS.sheet`).
pub const BROWSER_RUN_SHEET: Duration = Duration::from_secs(330);

/// What a browser driver's child does outside its run mark and inside the tests' mark
/// over the child: node's start and the browser's launch before the driver's timer,
/// `browser.close()` and the report after it. A whole sheet part under `--load 12` took
/// at most 49.5 s, launch and close included, so the two ends fit three times.
pub const BROWSER_RUN_OUTSIDE: Duration = Duration::from_secs(60);

/// Every hang mark by name -- the one list the factor pin walks, so a mark
/// added here is scaled-and-pinned without a second list to forget.
pub const MARKS: [(&str, Duration); 8] = [
    ("HANG", HANG),
    ("BOOT_HANG", BOOT_HANG),
    ("REPLAY_HANG", REPLAY_HANG),
    ("APPLY_BOOT_HANG", APPLY_BOOT_HANG),
    ("TURN_HANG", TURN_HANG),
    ("BROWSER_RUN_COLONY", BROWSER_RUN_COLONY),
    ("BROWSER_RUN_SHEET", BROWSER_RUN_SHEET),
    ("BROWSER_RUN_OUTSIDE", BROWSER_RUN_OUTSIDE),
];

/// No positive wait is ever shorter than this, whatever the factor says.
pub const FLOOR: Duration = Duration::from_secs(10);

/// The environment variable that scales every hang mark (default 1) -- the
/// ONE time factor of the test suite (OR-T12-13). A measurement driver sets it
/// only for a run under a CPU quota and writes it into the receipt
/// (`scripts/flake_triage.py --time-factor`); the lane transport passes
/// exactly this name on (`scripts/lane_sync.sh test`, which
/// `scripts/pipeline/lane.py` delegates the test run to).
pub const TIME_FACTOR_VAR: &str = "MECLAW_TEST_TIME_FACTOR";

/// The quota mark: set by the lane transport, and only there, to the CPU
/// quota (whole percent) of the scope it runs the tier in
/// (`scripts/lane_sync.sh test` under `MECLAW_LANE_CPU_QUOTA`). A time factor
/// other than 1 belongs to a quota run only (OR-T12-13), and this is where
/// the rule is read (tv-br M10q-a, re-check R K3): a factor without the mark
/// -- a local `scripts/test-tier.sh`, a gate in the main tree, `cargo nextest`
/// by hand, a triage fallback without a delegated cpu controller -- is
/// refused at the first mark instead of stretching every mark unseen.
pub const CPU_QUOTA_MARK_VAR: &str = "MECLAW_TEST_CPU_QUOTA";

/// The largest time factor a run may ask for (tv-br M10o, re-check B-M3).
/// OR-T12-13 measures under F = 3; a factor far above that no longer names a
/// hang (at 1e9 a mark waits for centuries and the runner kill ends the run
/// unnamed) and turns the "P95 <= mark x F / 3" receipt into a lever.
/// `scripts/flake_triage.py` reads this very line, so the bound is stated once.
pub const MAX_TIME_FACTOR: f64 = 10.0;

/// The time factor the quota profile of `.config/nextest.toml` is laid out
/// for (OR-T12-13, OR-T18-7): every quota run and measurement uses F = 3, so
/// the profile's leashes hold the factor-1 end of a test at this factor, not
/// at [`MAX_TIME_FACTOR`]. Pinned by `708_... the_run_marker_sits_under_the_leash_with_room`
/// (the 710 browser leashes) and `scripts/tests/test_gate_sh.py`
/// `TestTierQuotaLeash`, which reads this very line.
pub const QUOTA_TIME_FACTOR: f64 = 3.0;

/// The factor a raw `MECLAW_TEST_TIME_FACTOR` value stands for. Unset is 1;
/// a set value must be a finite number from 1 to [`MAX_TIME_FACTOR`]. Anything
/// else (empty, garbage, zero, negative, below 1, above the bound, inf, NaN)
/// is an error, never a silent 1 or a silent cap: a run that asked for a
/// factor and did not get it would measure under the wrong marks
/// (fail-closed). Below 1 is refused because a mark that shrinks below the
/// observed waits turns into a verdict on speed (TV-Z3).
pub fn parse_time_factor(raw: Option<&str>) -> Result<f64, String> {
    let Some(raw) = raw else { return Ok(1.0) };
    match raw.trim().parse::<f64>() {
        Ok(f) if f.is_finite() && (1.0..=MAX_TIME_FACTOR).contains(&f) => Ok(f),
        _ => Err(format!(
            "{TIME_FACTOR_VAR}={raw:?}: a finite number from 1 to {MAX_TIME_FACTOR} \
             (unset = 1), refused"
        )),
    }
}

/// The factor a run may use: [`parse_time_factor`] of `raw`, and a factor
/// other than 1 only next to a quota mark ([`CPU_QUOTA_MARK_VAR`], a whole
/// percent above 0). Without the mark such a factor is an error, never a
/// silent 1 (fail-closed, tv-br M10q-a).
pub fn admit_time_factor(raw: Option<&str>, quota_mark: Option<&str>) -> Result<f64, String> {
    let f = parse_time_factor(raw)?;
    if f == 1.0 {
        return Ok(f);
    }
    match quota_mark.map(|q| q.trim().parse::<u32>()) {
        Some(Ok(pct)) if pct > 0 => Ok(f),
        _ => Err(format!(
            "{TIME_FACTOR_VAR}={f} without a CPU quota ({CPU_QUOTA_MARK_VAR}={quota_mark:?}): \
             a factor belongs to a quota run of the lane transport only (OR-T12-13), refused"
        )),
    }
}

/// The factor of this process (read on every call; no shared state). An
/// invalid value, or a factor other than 1 without the quota mark, panics at
/// the first mark -- the test is red, not unscaled.
pub fn time_factor() -> f64 {
    let raw = std::env::var(TIME_FACTOR_VAR).ok();
    let mark = std::env::var(CPU_QUOTA_MARK_VAR).ok();
    match admit_time_factor(raw.as_deref(), mark.as_deref()) {
        Ok(f) => f,
        Err(e) => panic!("{e}"),
    }
}

/// `base` scaled by `factor`, never below [`FLOOR`]. Only for positive waits
/// (hang marks) -- an absence window is never scaled (TV-Z4).
pub fn scaled_by(base: Duration, factor: f64) -> Duration {
    base.mul_f64(factor).max(FLOOR)
}

/// `base` scaled by this process's [`time_factor`], never below [`FLOOR`].
pub fn scaled(base: Duration) -> Duration {
    scaled_by(base, time_factor())
}

/// `d` in whole milliseconds, saturating at `u64::MAX` -- a mark past it is
/// no hang mark anyone waits out, and library code does not panic on it.
fn saturating_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// [`HANG`] scaled, in milliseconds.
pub fn hang_ms() -> u64 {
    saturating_ms(scaled(HANG))
}

/// A product deadline a test sets on the success path it awaits (TV-Z9: SUT
/// time by configuration) -- `credential_wait_ms`, `external_timeout_ms`, a
/// drain budget: ten scaled [`HANG`] marks, in milliseconds. Far above every
/// mark of the test, so a wait that never ends is named by the test's own
/// mark, never cut short by the product (OR-T12-13, tv-br M10m: one source
/// from the one factor instead of a literal per file).
pub fn product_wait_ms() -> u64 {
    product_wait_ms_by(time_factor())
}

/// [`product_wait_ms`] at `factor` -- the one formula, so a pin can walk every
/// admissible factor instead of restating it (tv-br M10q-a, re-check R K4).
pub fn product_wait_ms_by(factor: f64) -> u64 {
    saturating_ms(scaled_by(HANG, factor)).saturating_mul(10)
}

/// The `cell.message_timeout` (backstop B) of a cell whose operation timeout
/// A is [`product_wait_ms`] (CLAUDE.md rule 12, GH #1099: B generous, A
/// precise). Twice A clears the product rule `A + max(10 s, 10 %)` for every
/// factor, so the cell runs in its declared configuration -- no raise with a
/// WARN (`llm`), no B below A (`code`, default 121 s) -- and the test's own
/// mark still fires first (tv-br M10o, re-check B-M1). Pinned against the
/// product rule in `meclaw-cells`
/// (`a_shipped_llm_backstop_outlasts_its_own_call::the_test_backstop_clears_the_test_product_wait`).
pub fn product_backstop_ms() -> u64 {
    product_backstop_ms_by(time_factor())
}

/// [`product_backstop_ms`] at `factor`; pinned against the product rule at
/// every admissible factor up to [`MAX_TIME_FACTOR`] in `meclaw-cells`.
pub fn product_backstop_ms_by(factor: f64) -> u64 {
    product_wait_ms_by(factor).saturating_mul(2)
}

/// Awaits `fut` under the [`HANG`] mark; panics with `HANG: <desc> (waited <n> s)`.
pub async fn hang<F: Future>(desc: &str, fut: F) -> F::Output {
    mark(desc, scaled(HANG), fut, None::<fn() -> ()>).await
}

/// Awaits `fut` under the [`BOOT_HANG`] mark.
pub async fn boot_hang<F: Future>(desc: &str, fut: F) -> F::Output {
    mark(desc, scaled(BOOT_HANG), fut, None::<fn() -> ()>).await
}

/// Like [`hang`], and on a hang the panic carries the last observed state.
pub async fn hang_with<F, S, L>(desc: &str, fut: F, last_state: L) -> F::Output
where
    F: Future,
    S: Debug,
    L: FnOnce() -> S,
{
    mark(desc, scaled(HANG), fut, Some(last_state)).await
}

/// Like [`boot_hang`], and on a hang the panic carries the last observed state.
pub async fn boot_hang_with<F, S, L>(desc: &str, fut: F, last_state: L) -> F::Output
where
    F: Future,
    S: Debug,
    L: FnOnce() -> S,
{
    mark(desc, scaled(BOOT_HANG), fut, Some(last_state)).await
}

/// Awaits `fut` under the [`APPLY_BOOT_HANG`] mark; on a hang the panic
/// carries the last observed state.
pub async fn apply_boot_hang_with<F, S, L>(desc: &str, fut: F, last_state: L) -> F::Output
where
    F: Future,
    S: Debug,
    L: FnOnce() -> S,
{
    mark(desc, scaled(APPLY_BOOT_HANG), fut, Some(last_state)).await
}

/// Awaits `fut` under the [`TURN_HANG`] mark. On a hang the async
/// `last_state` runs once, after the mark and never while `fut` still runs, so
/// it may read what the wait would not (a fixture's dead letters, its round),
/// and the panic reads `HANG: <desc> (waited <n> s); last state: <state>`.
pub async fn turn_hang_with<F, L, LFut>(desc: &str, fut: F, last_state: L) -> F::Output
where
    F: Future,
    L: FnOnce() -> LFut,
    LFut: Future<Output = String>,
{
    let limit = scaled(TURN_HANG);
    match tokio::time::timeout(limit, fut).await {
        Ok(out) => out,
        Err(_) => {
            let state = last_state().await;
            panic!(
                "HANG: {desc} (waited {} s); last state: {state}",
                limit.as_secs()
            )
        }
    }
}

async fn mark<F, S, L>(desc: &str, limit: Duration, fut: F, last_state: Option<L>) -> F::Output
where
    F: Future,
    S: Debug,
    L: FnOnce() -> S,
{
    match tokio::time::timeout(limit, fut).await {
        Ok(out) => out,
        Err(_) => match last_state {
            Some(state) => panic!(
                "HANG: {desc} (waited {} s); last state: {:?}",
                limit.as_secs(),
                state()
            ),
            None => panic!("HANG: {desc} (waited {} s)", limit.as_secs()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A factor whose mark no longer fits in `u64` milliseconds saturates at
    /// `u64::MAX` instead of panicking in library code (unwrap budget, Gate R):
    /// 30 s x 1e15 is 3e19 ms, past `u64::MAX` (1.8e19), still a valid `Duration`.
    #[test]
    fn a_mark_past_u64_milliseconds_saturates_instead_of_panicking() {
        assert_eq!(product_wait_ms_by(1e15), u64::MAX);
        assert_eq!(product_backstop_ms_by(1e15), u64::MAX);
        assert_eq!(product_wait_ms_by(1.0), 300_000);
        assert_eq!(product_backstop_ms_by(1.0), 600_000);
    }

    #[test]
    fn deadline_scales_with_the_time_factor_and_never_below_ten_seconds() {
        assert_eq!(parse_time_factor(None), Ok(1.0));
        assert_eq!(parse_time_factor(Some("2.5")), Ok(2.5));
        assert_eq!(parse_time_factor(Some(" 3 ")), Ok(3.0));
        assert_eq!(parse_time_factor(Some("10")), Ok(MAX_TIME_FACTOR));
        for bad in [
            "0", "-3", "0.5", "abc", "", "inf", "NaN", "11", "10.5", "1e9",
        ] {
            let err = parse_time_factor(Some(bad)).expect_err(bad);
            assert!(err.contains(TIME_FACTOR_VAR), "{bad:?}: {err}");
        }
        assert_eq!(scaled_by(HANG, 1.0), Duration::from_secs(30));
        assert_eq!(scaled_by(HANG, 2.0), Duration::from_secs(60));
        assert_eq!(scaled_by(BOOT_HANG, 3.0), Duration::from_secs(180));
        assert_eq!(
            scaled_by(HANG, 0.1),
            FLOOR,
            "a small factor stops at the floor"
        );
        assert_eq!(scaled_by(Duration::from_secs(1), 1.0), FLOOR);
        assert_eq!(Duration::from_millis(HANG_MS), HANG);
    }

    /// OR-T12-13: the factor reaches EVERY mark, and factor 1 changes none.
    #[test]
    fn the_time_factor_scales_every_mark_and_one_changes_nothing() {
        for (name, mark) in MARKS {
            assert_eq!(
                scaled_by(mark, 1.0),
                mark,
                "{name}: factor 1 changes nothing"
            );
            assert_eq!(
                scaled_by(mark, 2.5),
                mark.mul_f64(2.5),
                "{name}: factor 2.5"
            );
            assert_eq!(scaled_by(mark, 4.0), mark * 4, "{name}: factor 4");
        }
    }

    /// tv-br M10o (B-M1/B-M3), M10q-a (K4): the backstop helper stays above
    /// the product wait by the product margin for every admissible factor, read
    /// through the helpers themselves (not a restated formula), and the
    /// process's helpers are the `_by` ones at its own factor. The product
    /// rule's own function is walked over the same factors in `meclaw-cells`
    /// (`the_test_backstop_clears_the_test_product_wait`).
    #[test]
    fn the_backstop_clears_the_product_wait_at_every_admissible_factor() {
        for f in [1.0, 3.0, MAX_TIME_FACTOR] {
            let (a, b) = (product_wait_ms_by(f), product_backstop_ms_by(f));
            assert_eq!(a, 10 * scaled_by(HANG, f).as_millis() as u64, "factor {f}");
            assert!(b >= a + (a / 10).max(10_000), "factor {f}: B {b} vs A {a}");
        }
        assert_eq!(product_wait_ms(), product_wait_ms_by(time_factor()));
        assert_eq!(product_backstop_ms(), product_backstop_ms_by(time_factor()));
    }

    /// tv-br M10q-a (re-check R K3): a factor other than 1 only with the quota
    /// mark the lane transport sets; factor 1 and an unset factor need none.
    #[test]
    fn a_time_factor_without_the_quota_mark_is_refused() {
        assert_eq!(admit_time_factor(None, None), Ok(1.0));
        assert_eq!(admit_time_factor(Some("1"), None), Ok(1.0));
        assert_eq!(admit_time_factor(Some("3"), Some("80")), Ok(3.0));
        assert_eq!(
            admit_time_factor(Some("10"), Some("400")),
            Ok(MAX_TIME_FACTOR)
        );
        for mark in [None, Some(""), Some("0"), Some("80%"), Some("x")] {
            let err = admit_time_factor(Some("3"), mark).expect_err("no quota");
            assert!(
                err.contains(CPU_QUOTA_MARK_VAR) && err.contains("refused"),
                "{mark:?}: {err}"
            );
        }
        assert!(
            admit_time_factor(Some("11"), Some("80")).is_err(),
            "the mark lifts no bound"
        );
    }

    /// The child side of [`the_factor_from_the_environment_reaches_every_mark`]:
    /// inert unless the parent started it with the probe variable.
    #[test]
    fn time_factor_probe_child() {
        let Ok(want) = std::env::var("MECLAW_DEADLINE_PROBE") else {
            return;
        };
        let want: f64 = want.parse().unwrap();
        for (name, mark) in MARKS {
            assert_eq!(
                scaled(mark),
                mark.mul_f64(want),
                "{name} under the environment"
            );
        }
        assert_eq!(hang_ms(), (HANG_MS as f64 * want) as u64);
    }

    /// The factor travels through the environment of a real process (this
    /// test binary, re-run on the probe): `3` triples every mark, an invalid
    /// value makes the probe red (fail-closed) instead of running unscaled.
    #[test]
    fn the_factor_from_the_environment_reaches_every_mark() {
        let exe = std::env::current_exe().unwrap();
        let probe_with = |raw: &str, want: &str, mark: Option<&str>| {
            let mut cmd = std::process::Command::new(&exe);
            cmd.args([
                "--exact",
                "deadline::tests::time_factor_probe_child",
                "--nocapture",
            ])
            .env(TIME_FACTOR_VAR, raw)
            .env("MECLAW_DEADLINE_PROBE", want);
            match mark {
                Some(m) => cmd.env(CPU_QUOTA_MARK_VAR, m),
                None => cmd.env_remove(CPU_QUOTA_MARK_VAR),
            };
            cmd.output().unwrap()
        };
        let probe = |raw: &str, want: &str| probe_with(raw, want, Some("80"));
        let said = |o: &std::process::Output| {
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            )
        };
        let ok = probe("3", "3");
        assert!(
            ok.status.success(),
            "factor 3 scales every mark: {}",
            said(&ok)
        );
        assert!(
            said(&ok).contains("1 passed"),
            "the probe ran: {}",
            said(&ok)
        );
        let bad = probe("fast", "1");
        assert!(
            !bad.status.success(),
            "an invalid factor is red, never factor 1"
        );
        assert!(
            said(&bad).contains("refused"),
            "the refusal names itself: {}",
            said(&bad)
        );
        // tv-br M10q-a (K3): the same factor without the quota mark -- a local
        // tier, a main-tree gate, `cargo nextest` by hand -- is refused.
        let local = probe_with("3", "3", None);
        assert!(
            !local.status.success(),
            "a factor without a quota is red, never x 3: {}",
            said(&local)
        );
        assert!(
            said(&local).contains(CPU_QUOTA_MARK_VAR),
            "the refusal names the mark: {}",
            said(&local)
        );
    }

    #[tokio::test(start_paused = true)]
    #[should_panic(expected = "HANG: the reply arrives (waited ")]
    async fn hang_names_what_never_came() {
        hang("the reply arrives", std::future::pending::<()>()).await;
    }

    #[tokio::test(start_paused = true)]
    #[should_panic(expected = "last state: \"3 of 5 rows\"")]
    async fn hang_with_carries_the_last_observed_state() {
        hang_with("five rows", std::future::pending::<()>(), || "3 of 5 rows").await;
    }

    #[tokio::test(start_paused = true)]
    async fn hang_passes_the_value_through() {
        assert_eq!(hang("ready", async { 7 }).await, 7);
    }

    /// K9 (GH #1199): a mark this strand added is at least 3 x the larger of its
    /// P95 under overbooking at factor 1 and the lower bound the full-suite
    /// re-gate observed at factor 1 -- the figures its doc names.
    #[test]
    fn the_apply_and_turn_marks_carry_three_times_their_basis() {
        for (name, mark, p95_s, gate_lower_s) in [
            ("APPLY_BOOT_HANG", APPLY_BOOT_HANG, 21.7_f64, 61.5_f64),
            ("TURN_HANG", TURN_HANG, 14.2, 90.0),
        ] {
            let basis = 3.0 * p95_s.max(gate_lower_s);
            assert!(
                mark.as_secs_f64() >= basis,
                "{name} = {} s is below 3 x its basis ({basis} s)",
                mark.as_secs()
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn turn_hang_with_carries_the_state_read_after_the_mark() {
        let hung = tokio::spawn(async {
            turn_hang_with("the answer", std::future::pending::<()>(), || async {
                tokio::task::yield_now().await;
                "dead letters: []".to_string()
            })
            .await
        })
        .await
        .expect_err("the mark fires");
        let said = hung.into_panic();
        let said = said
            .downcast_ref::<String>()
            .expect("a formatted panic")
            .clone();
        let waited = scaled(TURN_HANG).as_secs();
        assert_eq!(
            said,
            format!("HANG: the answer (waited {waited} s); last state: dead letters: []")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn turn_hang_with_passes_the_value_through() {
        let read = std::cell::Cell::new(false);
        let out = turn_hang_with("ready", async { 7 }, || async {
            read.set(true);
            String::new()
        })
        .await;
        assert_eq!(out, 7);
        assert!(!read.get(), "the state is read only after the mark");
    }
}
