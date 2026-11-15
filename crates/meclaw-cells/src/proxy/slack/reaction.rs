//! GH #1530: Slack's hook into the shared typing keeper (`proxy::typing`).
//!
//! The Events API has no bot typing, and `assistant.threads.setStatus` only
//! works in assistant threads, so a running turn shows the way a person in the
//! channel would: a reaction on the message the turn answers. Unlike
//! Telegram's status it does not decay -- it is shown once and stands until
//! `reactions.remove` takes it away, which the keeper does at the answer, at
//! a repeat or at its ceiling.
//!
//! The message is the person's own (`ts`), not the thread (`thread_ts`): in a
//! thread the two differ, and in a DM the address of the answer carries no
//! `ts` at all. So the hook carries channel and `ts` from the incoming event
//! to the end of the turn, in the keeper's task.
//!
//! What the connector guarantees (OR-d54g.acktyp.2): each reaction is a row
//! in the cell's own `cell.db` (`typing_reactions`), written before the add,
//! and the row goes only once Slack confirmed its removal (`ok`, or
//! `no_reaction`: nothing to take away is the goal reached). The keeper
//! reports that confirmation on the cell's report lane to the handler, which
//! owns the database. A removal that fails is tried again
//! (`typing_remove_retries` more times, `typing_remove_retry_ms` apart, or
//! after the `Retry-After` of Slack's rate limit when it names one); a
//! permanent refusal is not tried again in the same run. A removal still
//! unconfirmed after that -- like a reaction whose keeper died with a
//! restart -- keeps its row, and the next start takes it away, under the
//! same rule. The end of the turn takes the reaction away whether or not the
//! add was confirmed: an add that failed at its timeout may still have gone
//! through.
//!
//! The one limit: an add that timed out and that Slack carries out only
//! after the confirmed removal stands, because its row is gone by then.
//! Nothing the connector can observe tells it so.

use super::client::{SlackClient, SlackError};
use super::db::RememberedReaction;
use super::io::SlackEvent;
use std::time::Duration;
use tokio::sync::mpsc;

/// OR-d54g.acktyp.2: how a failed `reactions.remove` is tried again
/// (`params.typing_remove_retries`, `params.typing_remove_retry_ms`).
#[derive(Debug, Clone, Copy)]
pub struct RemoveRetry {
    /// Further tries after the first one failed.
    pub retries: u32,
    /// The wait before a further try, unless Slack named a `Retry-After`.
    pub gap: Duration,
}

/// OR-d54g.acktyp.2: takes `reaction` away until Slack confirms it, tried
/// again after a transient failure as `retry` allows. Every call carries the
/// client's `send_timeout` (hard rule 12); a rate limit's `Retry-After` wins
/// over the gap; a permanent refusal (scope, token, channel) is not tried
/// again in this run. Confirmed → reported on `reports`, where the handler
/// forgets the row; not confirmed → the row stays for the next start.
pub async fn remove_until_confirmed(
    client: &SlackClient,
    reaction: &RememberedReaction,
    retry: RemoveRetry,
    reports: &mpsc::UnboundedSender<SlackEvent>,
) -> Result<(), String> {
    let mut tries_left = retry.retries;
    loop {
        match client
            .reactions_remove(&reaction.channel, &reaction.ts, &reaction.name)
            .await
        {
            Ok(()) => {
                // A closed lane means the cell is gone; its row then waits
                // for the next start, whose removal of nothing is no failure.
                let _ = reports.send(SlackEvent::ReactionRemoved(reaction.clone()));
                return Ok(());
            }
            Err(f) => {
                let permanent = matches!(f.error, SlackError::Permanent(_));
                if permanent || tries_left == 0 {
                    return Err(f.to_string());
                }
                tries_left -= 1;
                let wait = f.retry_after.unwrap_or(retry.gap);
                tracing::debug!(
                    channel = %reaction.channel,
                    error = %f,
                    wait_ms = wait.as_millis() as u64,
                    tries_left,
                    "slack: taking a reaction away failed -- trying again"
                );
                tokio::time::sleep(wait).await;
            }
        }
    }
}

/// The reaction of one running turn.
pub struct SlackReaction {
    /// The bot's client (its `send_timeout` bounds each call).
    pub client: SlackClient,
    /// Channel and `ts` of the person's message, and the emoji
    /// (`params.typing_reaction`) -- the row the cell remembered.
    pub reaction: RememberedReaction,
    /// How a failed removal is tried again.
    pub retry: RemoveRetry,
    /// The cell's report lane: a confirmed removal goes there.
    pub reports: mpsc::UnboundedSender<SlackEvent>,
}

impl crate::proxy::typing::TypingSign for SlackReaction {
    fn kind(&self) -> &'static str {
        "reaction"
    }

    fn decays(&self) -> bool {
        false
    }

    fn needs_take_away(&self) -> bool {
        true
    }

    async fn show(&self) -> Result<(), String> {
        let r = &self.reaction;
        self.client
            .reactions_add(&r.channel, &r.ts, &r.name)
            .await
            .map_err(|e| e.to_string())
    }

    async fn take_away(&self) -> Result<(), String> {
        remove_until_confirmed(&self.client, &self.reaction, self.retry, &self.reports).await
    }
}
