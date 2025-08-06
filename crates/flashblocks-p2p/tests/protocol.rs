use ed25519_dalek::SigningKey;
use flashblocks_p2p::protocol::handler::{FlashblocksHandle, PublishingStatus};
use futures::StreamExt as _;
use reth::payload::PayloadId;
use rollup_boost::{
    Authorization, AuthorizedPayload, ExecutionPayloadBaseV1, ExecutionPayloadFlashblockDeltaV1,
    FlashblocksPayloadV1,
};
use std::time::Duration;
use tokio::task;

const DUMMY_TIMESTAMP: u64 = 42;

/// Helper: deterministic ed25519 key made of the given byte.
fn signing_key(byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[byte; 32])
}

/// Helper: a minimal Flashblock (index 0) for the given payload-id.
fn payload(payload_id: reth::payload::PayloadId, idx: u64) -> FlashblocksPayloadV1 {
    FlashblocksPayloadV1 {
        payload_id,
        index: idx,
        base: Some(ExecutionPayloadBaseV1 {
            block_number: 0,
            ..Default::default()
        }),
        diff: ExecutionPayloadFlashblockDeltaV1::default(),
        metadata: serde_json::Value::Null,
    }
}

/// Build a fresh handle plus its broadcast receiver.
fn fresh_handle() -> FlashblocksHandle {
    // authorizer + builder keys
    let auth_sk = signing_key(1);
    let builder_sk = signing_key(2);

    FlashblocksHandle::new(auth_sk.verifying_key(), builder_sk)
}

#[tokio::test]
async fn publish_without_clearance_is_rejected() {
    let handle = fresh_handle();

    let payload_id = reth::payload::PayloadId::new([0; 8]);
    let auth = Authorization::new(
        payload_id,
        DUMMY_TIMESTAMP,
        &signing_key(1),
        handle.ctx.builder_sk.verifying_key(),
    );
    let payload = payload(payload_id, 0);
    let signed = AuthorizedPayload::new(&handle.ctx.builder_sk, auth, payload.clone());

    // We never called `start_publishing`, so this must fail.
    let err = handle.publish_new(signed).unwrap_err();
    assert!(matches!(
        err,
        flashblocks_p2p::protocol::error::FlashblocksP2PError::NotClearedToPublish
    ));
}

#[tokio::test]
async fn expired_authorization_is_rejected() {
    let handle = fresh_handle();

    // Step 1: obtain clearance with auth_1
    let payload_id = reth::payload::PayloadId::new([1; 8]);
    let auth_1 = Authorization::new(
        payload_id,
        DUMMY_TIMESTAMP,
        &signing_key(1),
        handle.ctx.builder_sk.verifying_key(),
    );
    handle.start_publishing(auth_1);

    // Step 2: craft a payload signed with *different* authorization → should fail
    let auth_2 = Authorization::new(
        payload_id,
        DUMMY_TIMESTAMP + 1,
        &signing_key(1),
        handle.ctx.builder_sk.verifying_key(),
    );
    let payload = payload(payload_id, 0);
    let signed = AuthorizedPayload::new(&handle.ctx.builder_sk, auth_2, payload);

    let err = handle.publish_new(signed).unwrap_err();
    assert!(matches!(
        err,
        flashblocks_p2p::protocol::error::FlashblocksP2PError::ExpiredAuthorization
    ));
}

#[tokio::test]
async fn flashblock_stream_is_ordered() {
    let handle = fresh_handle();

    // clearance
    let payload_id = reth::payload::PayloadId::new([2; 8]);
    let auth = Authorization::new(
        payload_id,
        DUMMY_TIMESTAMP,
        &signing_key(1),
        handle.ctx.builder_sk.verifying_key(),
    );
    handle.start_publishing(auth);

    // send index 1 first (out-of-order)
    for &idx in &[1u64, 0] {
        let p = payload(payload_id, idx);
        let signed = AuthorizedPayload::new(&handle.ctx.builder_sk, auth, p.clone());
        handle.publish_new(signed).unwrap();
    }

    let mut flashblock_stream = handle.flashblock_stream();

    // Expect to receive 0, then 1 over the ordered broadcast.
    let first = flashblock_stream.next().await.unwrap();
    let second = flashblock_stream.next().await.unwrap();
    assert_eq!(first.index, 0);
    assert_eq!(second.index, 1);
}

#[tokio::test]
async fn stop_and_restart_updates_state() {
    let handle = fresh_handle();

    // 1) start publishing
    let payload_id_0 = reth::payload::PayloadId::new([3; 8]);
    let auth_0 = Authorization::new(
        payload_id_0,
        DUMMY_TIMESTAMP,
        &signing_key(1),
        handle.ctx.builder_sk.verifying_key(),
    );
    handle.start_publishing(auth_0);
    assert!(matches!(
        handle.publishing_status(),
        PublishingStatus::Publishing { .. }
    ));

    // 2) stop
    handle.stop_publishing();
    assert!(matches!(
        handle.publishing_status(),
        PublishingStatus::NotPublishing { .. }
    ));

    // 3) start again with a new payload
    let payload_id_1 = reth::payload::PayloadId::new([4; 8]);
    let auth_1 = Authorization::new(
        payload_id_1,
        DUMMY_TIMESTAMP + 5,
        &signing_key(1),
        handle.ctx.builder_sk.verifying_key(),
    );
    handle.start_publishing(auth_1);
    assert!(matches!(
        handle.publishing_status(),
        PublishingStatus::Publishing { .. }
    ));
}

#[tokio::test]
async fn stop_and_restart_with_active_publishers() {
    let timestamp = 1000;
    let handle = fresh_handle();

    // Pretend we already know about another publisher.
    let other_vk = signing_key(99).verifying_key();
    {
        let state = handle.state.lock();
        state
            .publishing_status
            .send_replace(PublishingStatus::NotPublishing {
                active_publishers: vec![(other_vk, timestamp - 1)],
            });
    }

    // Our own clearance → should transition to WaitingToPublish.
    let payload_id = PayloadId::new([6; 8]);
    let auth = Authorization::new(
        payload_id,
        timestamp,
        &signing_key(1),
        handle.ctx.builder_sk.verifying_key(),
    );
    handle.start_publishing(auth);
    match handle.publishing_status() {
        PublishingStatus::WaitingToPublish {
            active_publishers, ..
        } => {
            assert_eq!(active_publishers.len(), 1);
            assert_eq!(active_publishers[0].0, other_vk);
        }
        s => panic!("unexpected status: {s:?}"),
    }

    // Now we voluntarily stop.  We should end up back in NotPublishing,
    // still carrying the same active publisher entry.
    handle.stop_publishing();
    match handle.publishing_status() {
        PublishingStatus::NotPublishing { active_publishers } => {
            assert_eq!(active_publishers.len(), 1);
            assert_eq!(active_publishers[0].0, other_vk);
        }
        s => panic!("unexpected status after stop: {s:?}"),
    }
}

#[tokio::test]
async fn flashblock_stream_buffers_and_live() {
    let timestamp = 1000;
    let handle = fresh_handle();
    let pid = PayloadId::new([7; 8]);
    let auth = Authorization::new(
        pid,
        timestamp,
        &signing_key(1),
        handle.ctx.builder_sk.verifying_key(),
    );
    handle.start_publishing(auth);

    // publish index 0 before creating the stream
    let signed0 = AuthorizedPayload::new(&handle.ctx.builder_sk, auth, payload(pid, 0));
    handle.publish_new(signed0).unwrap();

    // now create the combined stream
    let mut stream = handle.flashblock_stream();

    // first item comes from the cached vector
    let first = stream.next().await.unwrap();
    assert_eq!(first.index, 0);

    // publish index 1 after the stream exists
    let signed1 = AuthorizedPayload::new(&handle.ctx.builder_sk, auth, payload(pid, 1));
    handle.publish_new(signed1).unwrap();

    // second item should be delivered live
    let second = stream.next().await.unwrap();
    assert_eq!(second.index, 1);
}

#[tokio::test]
async fn await_clearance_unblocks_on_publish() {
    let handle = fresh_handle();

    let waiter = {
        let h = handle.clone();
        task::spawn(async move {
            h.await_clearance().await;
        })
    };

    // give the waiter a chance to subscribe
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished(), "future must still be pending");

    // now grant clearance
    let payload_id = reth::payload::PayloadId::new([5; 8]);
    let auth = Authorization::new(
        payload_id,
        DUMMY_TIMESTAMP,
        &signing_key(1),
        handle.ctx.builder_sk.verifying_key(),
    );
    handle.start_publishing(auth);

    // waiter should finish very quickly
    tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("await_clearance did not complete")
        .unwrap();
}
