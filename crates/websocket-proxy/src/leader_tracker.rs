use crate::flashblock_parser::{parse_flashblock, ParsedFlashblock};
use http::Uri;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Tracks the current block being built and which sequencer is the leader.
/// Uses a simple strategy: first sequencer to send a flashblock with a block_number
/// establishes itself as the leader for that block. Flashblocks from other sequencers
/// for the same block are dropped (conflict).
pub struct LeaderTracker {
    /// Current block state
    current_block: Arc<RwLock<Option<BlockState>>>,
}

/// State for the current block being built
#[derive(Debug, Clone)]
struct BlockState {
    /// The block number being built
    block_number: u64,
    /// The URI of the sequencer that is the leader for this block
    source_uri: Uri,
}

/// Reason why a flashblock was forwarded
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardReason {
    /// Failed to parse flashblock, forwarding for backwards compatibility
    ParseError,
    /// No leader established yet, forwarding
    NoLeaderYet,
    /// This message established a new leader
    NewLeader,
    /// Message is from the current leader
    FromLeader,
}

/// Reason why a flashblock was dropped
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// Message is not from the current leader
    NotFromLeader,
    /// Different source sent flashblock for same block number
    Conflict,
    /// Block number is older than current block
    Stale,
}

/// Result of checking whether a flashblock should be forwarded
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardDecision {
    /// Message should be forwarded
    Forward(ForwardReason),
    /// Message should be dropped
    Drop(DropReason),
}

impl LeaderTracker {
    /// Create a new leader tracker
    pub fn new() -> Self {
        Self {
            current_block: Arc::new(RwLock::new(None)),
        }
    }

    /// Check whether a flashblock message should be forwarded to downstream clients.
    ///
    /// This is the main entry point that handles parsing and filtering logic.
    ///
    /// # Arguments
    /// * `uri` - The URI of the upstream sequencer that sent the message
    /// * `message` - The raw flashblock message string
    ///
    /// # Returns
    /// A `ForwardDecision` indicating whether to forward or drop, and why.
    pub async fn should_forward_message(&self, uri: &Uri, message: &str) -> ForwardDecision {
        // Try to parse the flashblock
        let parsed = match parse_flashblock(message) {
            Ok(p) => p,
            Err(e) => {
                // If parsing fails, still forward (backwards compatibility)
                warn!(
                    "Failed to parse flashblock from {}, forwarding anyway: {}",
                    uri, e
                );
                return ForwardDecision::Forward(ForwardReason::ParseError);
            }
        };

        self.check_flashblock(uri, &parsed).await
    }

    /// Internal method to check if a parsed flashblock should be forwarded.
    ///
    /// # Logic
    /// 1. If no leader yet and message has block_number: establish leader, forward
    /// 2. If no leader yet and message has no block_number: forward (can't establish leader)
    /// 3. If leader exists and message is from leader: forward (and update block_number if higher)
    /// 4. If leader exists and message is from different source with higher block_number: new leader, forward
    /// 5. If leader exists and message is from different source with same block_number: drop (conflict)
    /// 6. If leader exists and message is from different source with lower block_number: drop (stale)
    /// 7. If leader exists and message is from different source with no block_number: drop (not from leader)
    async fn check_flashblock(&self, uri: &Uri, parsed: &ParsedFlashblock) -> ForwardDecision {
        let mut current_block = self.current_block.write().await;

        match current_block.as_ref() {
            None => {
                // No leader established yet
                if let Some(block_number) = parsed.block_number {
                    // Establish this source as leader
                    info!("Establishing leader for block {}: {}", block_number, uri);
                    *current_block = Some(BlockState {
                        block_number,
                        source_uri: uri.clone(),
                    });
                    ForwardDecision::Forward(ForwardReason::NewLeader)
                } else {
                    // No block_number and no leader - forward but can't establish leader
                    debug!(
                        "No leader yet, forwarding flashblock without block_number from {}",
                        uri
                    );
                    ForwardDecision::Forward(ForwardReason::NoLeaderYet)
                }
            }
            Some(block) => {
                // Leader exists - check if message is from leader
                let is_from_leader = &block.source_uri == uri;

                if is_from_leader {
                    // Message is from current leader - update block number if present
                    if let Some(block_number) = parsed.block_number {
                        if block_number > block.block_number {
                            debug!(
                                "Leader {} advanced to block {}",
                                uri, block_number
                            );
                            *current_block = Some(BlockState {
                                block_number,
                                source_uri: uri.clone(),
                            });
                        }
                    }
                    ForwardDecision::Forward(ForwardReason::FromLeader)
                } else {
                    // Message is from different source - check block_number
                    match parsed.block_number {
                        Some(block_number) if block_number > block.block_number => {
                            // New block from different source - new leader
                            info!(
                                "New leader: block {} -> {} from {} (was {})",
                                block.block_number, block_number, uri, block.source_uri
                            );
                            *current_block = Some(BlockState {
                                block_number,
                                source_uri: uri.clone(),
                            });
                            ForwardDecision::Forward(ForwardReason::NewLeader)
                        }
                        Some(block_number) if block_number == block.block_number => {
                            // Same block from different source - conflict
                            warn!(
                                "Conflict: block {} from {} (leader is {})",
                                block_number, uri, block.source_uri
                            );
                            ForwardDecision::Drop(DropReason::Conflict)
                        }
                        Some(block_number) => {
                            // Older block from different source - stale
                            debug!(
                                "Stale: block {} from {} (current is {})",
                                block_number, uri, block.block_number
                            );
                            ForwardDecision::Drop(DropReason::Stale)
                        }
                        None => {
                            // No block_number from non-leader - drop
                            debug!(
                                "Dropping flashblock without block_number from {} (leader is {})",
                                uri, block.source_uri
                            );
                            ForwardDecision::Drop(DropReason::NotFromLeader)
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_parsed_flashblock(block_number: Option<u64>) -> ParsedFlashblock {
        ParsedFlashblock { block_number }
    }

    fn make_uri(host: &str) -> Uri {
        format!("ws://{}:2545", host).parse().unwrap()
    }

    #[tokio::test]
    async fn test_first_flashblock_establishes_leader() {
        let tracker = LeaderTracker::new();
        let uri = make_uri("sequencer-0");
        let parsed = make_parsed_flashblock(Some(100));

        let decision = tracker.check_flashblock(&uri, &parsed).await;

        assert_eq!(decision, ForwardDecision::Forward(ForwardReason::NewLeader));
    }

    #[tokio::test]
    async fn test_same_source_forwards_with_no_block_number() {
        let tracker = LeaderTracker::new();
        let uri = make_uri("sequencer-0");
        let parsed1 = make_parsed_flashblock(Some(100));
        let parsed2 = make_parsed_flashblock(None);

        // Establish leader
        tracker.check_flashblock(&uri, &parsed1).await;

        // Same source with no block_number should forward
        let decision = tracker.check_flashblock(&uri, &parsed2).await;

        assert_eq!(decision, ForwardDecision::Forward(ForwardReason::FromLeader));
    }

    #[tokio::test]
    async fn test_different_source_no_block_number_drops() {
        let tracker = LeaderTracker::new();
        let uri1 = make_uri("sequencer-0");
        let uri2 = make_uri("sequencer-1");

        let parsed1 = make_parsed_flashblock(Some(100));
        let parsed2 = make_parsed_flashblock(None);

        // Establish leader
        tracker.check_flashblock(&uri1, &parsed1).await;

        // Different source with no block_number should be dropped
        let decision = tracker.check_flashblock(&uri2, &parsed2).await;

        assert_eq!(decision, ForwardDecision::Drop(DropReason::NotFromLeader));
    }

    #[tokio::test]
    async fn test_new_source_current_block_rejected_as_conflict() {
        let tracker = LeaderTracker::new();
        let leader = make_uri("sequencer-0");
        let new_source = make_uri("sequencer-1");

        // Establish leader for block 100
        tracker
            .check_flashblock(&leader, &make_parsed_flashblock(Some(100)))
            .await;

        // New source with same block number should be rejected as conflict
        let decision = tracker
            .check_flashblock(&new_source, &make_parsed_flashblock(Some(100)))
            .await;

        assert_eq!(decision, ForwardDecision::Drop(DropReason::Conflict));
    }

    #[tokio::test]
    async fn test_new_source_old_block_rejected_as_stale() {
        let tracker = LeaderTracker::new();
        let leader = make_uri("sequencer-0");
        let new_source = make_uri("sequencer-1");

        // Establish leader for block 100
        tracker
            .check_flashblock(&leader, &make_parsed_flashblock(Some(100)))
            .await;

        // New source with lower block number should be rejected as stale
        let decision = tracker
            .check_flashblock(&new_source, &make_parsed_flashblock(Some(99)))
            .await;

        assert_eq!(decision, ForwardDecision::Drop(DropReason::Stale));
    }

    #[tokio::test]
    async fn test_new_source_new_block_accepted_as_new_leader() {
        let tracker = LeaderTracker::new();
        let leader = make_uri("sequencer-0");
        let new_source = make_uri("sequencer-1");

        // Establish leader for block 100
        tracker
            .check_flashblock(&leader, &make_parsed_flashblock(Some(100)))
            .await;

        // New source with higher block number should be accepted as new leader
        let decision = tracker
            .check_flashblock(&new_source, &make_parsed_flashblock(Some(101)))
            .await;

        assert_eq!(decision, ForwardDecision::Forward(ForwardReason::NewLeader));
    }

    #[tokio::test]
    async fn test_no_leader_no_block_number_forwards() {
        let tracker = LeaderTracker::new();
        let uri = make_uri("sequencer-0");

        let parsed = make_parsed_flashblock(None);

        let decision = tracker.check_flashblock(&uri, &parsed).await;

        assert_eq!(
            decision,
            ForwardDecision::Forward(ForwardReason::NoLeaderYet)
        );
    }

    #[tokio::test]
    async fn test_leader_transition_scenario() {
        let tracker = LeaderTracker::new();
        let uri1 = make_uri("sequencer-0");
        let uri2 = make_uri("sequencer-1");

        // Block 100 from sequencer-0 (leader)
        let parsed1 = make_parsed_flashblock(Some(100));
        let decision = tracker.check_flashblock(&uri1, &parsed1).await;
        assert_eq!(decision, ForwardDecision::Forward(ForwardReason::NewLeader));

        // Block 100 from sequencer-1 (conflict)
        let parsed2 = make_parsed_flashblock(Some(100));
        let decision = tracker.check_flashblock(&uri2, &parsed2).await;
        assert_eq!(decision, ForwardDecision::Drop(DropReason::Conflict));

        // Block 101 from sequencer-1 (new leader)
        let parsed3 = make_parsed_flashblock(Some(101));
        let decision = tracker.check_flashblock(&uri2, &parsed3).await;
        assert_eq!(decision, ForwardDecision::Forward(ForwardReason::NewLeader));

        // Block 101 from old leader sequencer-0 (now conflict)
        let parsed4 = make_parsed_flashblock(Some(101));
        let decision = tracker.check_flashblock(&uri1, &parsed4).await;
        assert_eq!(decision, ForwardDecision::Drop(DropReason::Conflict));

        // No block_number from old leader sequencer-0 (not from leader)
        let parsed5 = make_parsed_flashblock(None);
        let decision = tracker.check_flashblock(&uri1, &parsed5).await;
        assert_eq!(decision, ForwardDecision::Drop(DropReason::NotFromLeader));

        // No block_number from current leader sequencer-1 (forwarded)
        let parsed6 = make_parsed_flashblock(None);
        let decision = tracker.check_flashblock(&uri2, &parsed6).await;
        assert_eq!(decision, ForwardDecision::Forward(ForwardReason::FromLeader));
    }

    #[tokio::test]
    async fn test_leader_advances_block_number() {
        let tracker = LeaderTracker::new();
        let uri1 = make_uri("sequencer-0");
        let uri2 = make_uri("sequencer-1");

        // Block 100 from sequencer-0 (leader)
        let parsed1 = make_parsed_flashblock(Some(100));
        let decision = tracker.check_flashblock(&uri1, &parsed1).await;
        assert_eq!(decision, ForwardDecision::Forward(ForwardReason::NewLeader));

        // Leader advances to block 101
        let parsed2 = make_parsed_flashblock(Some(101));
        let decision = tracker.check_flashblock(&uri1, &parsed2).await;
        assert_eq!(decision, ForwardDecision::Forward(ForwardReason::FromLeader));

        // Now sequencer-1 tries block 101 - should be conflict, not new leader
        let parsed3 = make_parsed_flashblock(Some(101));
        let decision = tracker.check_flashblock(&uri2, &parsed3).await;
        assert_eq!(decision, ForwardDecision::Drop(DropReason::Conflict));

        // Sequencer-1 with block 102 should become new leader
        let parsed4 = make_parsed_flashblock(Some(102));
        let decision = tracker.check_flashblock(&uri2, &parsed4).await;
        assert_eq!(decision, ForwardDecision::Forward(ForwardReason::NewLeader));
    }
}
