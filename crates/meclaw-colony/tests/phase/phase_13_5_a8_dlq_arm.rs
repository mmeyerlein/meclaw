//! Phase-13.5 A8 T6 (Step A): the `ColonyMsg::DeadLetterMessage` inbox arm.
//!
//! A cell-delivery-boundary blob resolution failure cannot push to the
//! colony-owned dead-letter `VecDeque` directly, so `cell_task` forwards the
//! undeliverable message as `ColonyMsg::DeadLetterMessage`. This test proves the
//! colony arm records it with the given reason (`blob_unavailable`). FIFO inbox
//! ordering makes the subsequent drain see the pushed entry deterministically.

use meclaw_colony::{ColonyMsg, DeadLetterReason};
use meclaw_core::{MessageBuilder, Path};
use meclaw_testing::ColonyHandle;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dead_letter_message_arm_pushes_with_blob_unavailable_reason() {
    let h = ColonyHandle::new();

    let message = MessageBuilder::new(Path::new("/target")).build();
    h.inbox_tx
        .send(ColonyMsg::DeadLetterMessage {
            message,
            reason: DeadLetterReason::BlobUnavailable,
        })
        .await
        .expect("colony inbox closed");

    // Drain is itself an inbox message → FIFO guarantees the push above ran first.
    let drained = h.drain_dead_letters().await;
    assert_eq!(
        drained.len(),
        1,
        "the undeliverable message is dead-lettered"
    );
    let dl = &drained[0];
    assert_eq!(dl.reason.as_code(), "blob_unavailable");
    assert_eq!(dl.original_target.as_str(), "/target");
    assert_eq!(dl.resolved_target.as_str(), "/target");

    h.shutdown().await;
}
