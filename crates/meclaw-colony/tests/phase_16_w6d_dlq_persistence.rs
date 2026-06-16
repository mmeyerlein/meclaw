//! Phase-16 W6d (Audit A6): the dead-letter queue is durable — it survives a
//! colony restart. Before W6d the DLQ was a volatile in-memory `VecDeque`; a
//! shutdown/crash lost it. Now it persists in `colony.db` (the single source of
//! truth) and a freshly booted colony reads the prior entries — full envelope
//! reconstructed (Ruling W6d Option 1: `message_json` column), so `dl.message.*`
//! is intact across the restart.

use meclaw_colony::DeadLetterReason;
use meclaw_core::Path;
use meclaw_testing::{ColonyHandle, MessageBuilder};
use tempfile::TempDir;

/// (b) A dead-letter written by one colony lifecycle is readable, in full, after
/// a clean restart on the same `colony.db`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dead_letter_survives_colony_restart() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("colony.db");

    // --- Boot #1: dead-letter an unresolved target, then shut down (flushes). ---
    {
        let colony = ColonyHandle::with_db_path(&db_path);
        colony
            .send_from(Path::new("/"), MessageBuilder::new("/gone").build())
            .await;
        // FIFO inbox: the Route is processed (→ unresolved_path DLQ → persisted)
        // before the Shutdown drain, whose pre-teardown flush is the final net.
        colony.shutdown().await;
    }

    // --- Boot #2 on the SAME colony.db: the entry survived the restart. ---
    {
        let colony = ColonyHandle::with_db_path(&db_path);
        let dls = colony.drain_dead_letters().await;
        assert_eq!(dls.len(), 1, "the dead letter survived the colony restart");
        let dl = &dls[0];
        assert_eq!(dl.reason, DeadLetterReason::UnresolvedPath);
        assert_eq!(dl.resolved_target.as_str(), "/gone");
        assert_eq!(dl.sender_path.as_str(), "/");
        // Envelope reconstructed from `message_json` (Option 1) — not just the
        // projection: target + body come back across the restart.
        assert_eq!(dl.message.target.as_str(), "/gone");
        assert!(matches!(dl.message.body, meclaw_core::Body::Inline(_)));
        // A second drain returns nothing — the first drain cleared the DB.
        let again = colony.drain_dead_letters().await;
        assert!(again.is_empty(), "drain cleared the persisted DLQ");
        colony.shutdown().await;
    }
}
