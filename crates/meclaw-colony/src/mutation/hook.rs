//! Phase-6 crash-injection hook (cfg feature = "test-hooks").
//!
//! In release builds (no feature) `park_after_rename` is a no-op async fn.
//! In test builds, tests can `set_after_rename(notify)` to make handle_mutation
//! park between rename(2) and spawn-loop until `notify.notify_waiters()` fires.
//! Used by the Phase-6-Demo crash-recovery test (T26) to assert in_flight-row
//! durability before the simulated crash.

#[cfg(feature = "test-hooks")]
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(feature = "test-hooks")]
use tokio::sync::Notify;

#[cfg(feature = "test-hooks")]
static AFTER_RENAME: OnceLock<Mutex<Option<Arc<Notify>>>> = OnceLock::new();

#[cfg(feature = "test-hooks")]
pub fn set_after_rename(n: Arc<Notify>) {
    AFTER_RENAME
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .replace(n);
}

#[cfg(feature = "test-hooks")]
pub fn clear_after_rename() {
    if let Some(m) = AFTER_RENAME.get() {
        *m.lock().unwrap() = None;
    }
}

// --- Deep-Audit F2: deterministic mid-rename fault injection (test-hooks) ---

#[cfg(feature = "test-hooks")]
static FAIL_RENAME_AT: OnceLock<Mutex<Option<usize>>> = OnceLock::new();

/// Test-only: make `atomic_rename_or_overwrite_all` fail when its committed
/// counter reaches `committed_index` (e.g. `1` → fail the SECOND rename after the
/// first already landed, producing a `LiveTreeMutated` live-tree-partial state).
/// Binary-local static — set it only from a single-binary integration test.
#[cfg(feature = "test-hooks")]
pub fn set_fail_rename_at(committed_index: usize) {
    FAIL_RENAME_AT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .replace(committed_index);
}

#[cfg(feature = "test-hooks")]
pub fn clear_fail_rename() {
    if let Some(m) = FAIL_RENAME_AT.get() {
        *m.lock().unwrap() = None;
    }
}

/// Returns true iff a mid-rename failure is injected at this `committed` value.
/// Release build: const `false` (zero cost, no branch survives on the hot path).
#[inline]
pub fn should_fail_rename(committed: usize) -> bool {
    #[cfg(feature = "test-hooks")]
    {
        FAIL_RENAME_AT
            .get()
            .and_then(|m| *m.lock().unwrap())
            .is_some_and(|at| at == committed)
    }
    #[cfg(not(feature = "test-hooks"))]
    {
        let _ = committed;
        false
    }
}

/// Phase-6 apply sequence step 7 hook. In release no-op; in tests parks until released.
pub async fn park_after_rename() {
    #[cfg(feature = "test-hooks")]
    {
        let hook = AFTER_RENAME.get().and_then(|m| m.lock().unwrap().clone());
        if let Some(n) = hook {
            n.notified().await;
        }
    }
    #[cfg(not(feature = "test-hooks"))]
    {}
}

#[cfg(all(test, feature = "test-hooks"))]
mod tests {
    use super::*;

    /// Serialize hook-tests: both mutate the AFTER_RENAME static, parallel
    /// execution would race (set in test A, clear in test B → A's `park` never
    /// sees the Notify). Held across `.await`, so use `tokio::sync::Mutex` —
    /// `.lock().await` yields the task, never blocks a tokio worker.
    static HOOK_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn after_rename_hook_parks_until_notified() {
        let _guard = HOOK_TEST_LOCK.lock().await;
        let notify = Arc::new(Notify::new());
        set_after_rename(notify.clone());

        let park_task = tokio::spawn(async {
            park_after_rename().await;
            "released"
        });
        // Give park a tick to start awaiting.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!park_task.is_finished(), "park_after_rename must wait");

        notify.notify_waiters();
        let result = tokio::time::timeout(std::time::Duration::from_secs(30), park_task)
            .await
            .expect("park must complete after notify")
            .unwrap();
        assert_eq!(result, "released");

        clear_after_rename();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn after_rename_hook_passthrough_when_not_set() {
        let _guard = HOOK_TEST_LOCK.lock().await;
        clear_after_rename();
        // Without a set hook, park should return immediately.
        tokio::time::timeout(std::time::Duration::from_secs(30), park_after_rename())
            .await
            .expect("park without hook must not block");
    }
}
