use alloy_primitives::{B256, Bytes};
use futures::{StreamExt as _, stream};
use moka::future::Cache;

use alloy_rpc_types_engine::{ExecutionPayload, ExecutionPayloadV3, PayloadId};
use op_alloy_rpc_types_engine::{
    OpExecutionPayloadEnvelopeV3, OpExecutionPayloadEnvelopeV4, OpExecutionPayloadV4,
};

const CACHE_SIZE: u64 = 100;

#[derive(Debug, Clone)]
pub enum OpExecutionPayloadEnvelope {
    V3(OpExecutionPayloadEnvelopeV3),
    V4(OpExecutionPayloadEnvelopeV4),
}

impl OpExecutionPayloadEnvelope {
    pub fn version(&self) -> PayloadVersion {
        match self {
            OpExecutionPayloadEnvelope::V3(_) => PayloadVersion::V3,
            OpExecutionPayloadEnvelope::V4(_) => PayloadVersion::V4,
        }
    }

    pub fn gas_used(&self) -> u64 {
        match self {
            OpExecutionPayloadEnvelope::V3(payload) => {
                payload
                    .execution_payload
                    .payload_inner
                    .payload_inner
                    .gas_used
            }

            OpExecutionPayloadEnvelope::V4(payload) => {
                payload
                    .execution_payload
                    .payload_inner
                    .payload_inner
                    .payload_inner
                    .gas_used
            }
        }
    }

    pub fn tx_count(&self) -> usize {
        match self {
            OpExecutionPayloadEnvelope::V3(payload) => payload
                .execution_payload
                .payload_inner
                .payload_inner
                .transactions
                .len(),
            OpExecutionPayloadEnvelope::V4(payload) => payload
                .execution_payload
                .payload_inner
                .payload_inner
                .payload_inner
                .transactions
                .len(),
        }
    }
}

impl From<OpExecutionPayloadEnvelope> for ExecutionPayload {
    fn from(envelope: OpExecutionPayloadEnvelope) -> Self {
        match envelope {
            OpExecutionPayloadEnvelope::V3(v3) => ExecutionPayload::from(v3.execution_payload),
            OpExecutionPayloadEnvelope::V4(v4) => {
                ExecutionPayload::from(v4.execution_payload.payload_inner)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewPayloadV3 {
    pub payload: ExecutionPayloadV3,
    pub versioned_hashes: Vec<B256>,
    pub parent_beacon_block_root: B256,
}

#[derive(Debug, Clone)]
pub struct NewPayloadV4 {
    pub payload: OpExecutionPayloadV4,
    pub versioned_hashes: Vec<B256>,
    pub parent_beacon_block_root: B256,
    pub execution_requests: Vec<Bytes>,
}

#[derive(Debug, Clone)]
pub enum NewPayload {
    V3(NewPayloadV3),
    V4(NewPayloadV4),
}

impl NewPayload {
    pub fn version(&self) -> PayloadVersion {
        match self {
            NewPayload::V3(_) => PayloadVersion::V3,
            NewPayload::V4(_) => PayloadVersion::V4,
        }
    }
}

impl From<OpExecutionPayloadEnvelope> for NewPayload {
    fn from(envelope: OpExecutionPayloadEnvelope) -> Self {
        match envelope {
            OpExecutionPayloadEnvelope::V3(v3) => NewPayload::V3(NewPayloadV3 {
                payload: v3.execution_payload,
                versioned_hashes: vec![],
                parent_beacon_block_root: v3.parent_beacon_block_root,
            }),
            OpExecutionPayloadEnvelope::V4(v4) => NewPayload::V4(NewPayloadV4 {
                payload: v4.execution_payload,
                versioned_hashes: vec![],
                parent_beacon_block_root: v4.parent_beacon_block_root,
                execution_requests: v4.execution_requests,
            }),
        }
    }
}

impl From<NewPayload> for ExecutionPayload {
    fn from(new_payload: NewPayload) -> Self {
        match new_payload {
            NewPayload::V3(v3) => ExecutionPayload::from(v3.payload),
            NewPayload::V4(v4) => ExecutionPayload::from(v4.payload.payload_inner),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PayloadVersion {
    V3,
    V4,
}

impl PayloadVersion {
    pub fn as_str(&self) -> &'static str {
        match self {
            PayloadVersion::V3 => "v3",
            PayloadVersion::V4 => "v4",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadSource {
    L2,
    Builder,
}

impl std::fmt::Display for PayloadSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PayloadSource::L2 => write!(f, "l2"),
            PayloadSource::Builder => write!(f, "builder"),
        }
    }
}

#[allow(dead_code)]
impl PayloadSource {
    pub fn is_builder(&self) -> bool {
        matches!(self, PayloadSource::Builder)
    }

    pub fn is_l2(&self) -> bool {
        matches!(self, PayloadSource::L2)
    }
}

#[derive(Debug, Clone)]
pub struct PayloadTrace {
    pub builder_has_payload: bool,
    pub trace_id: Option<tracing::Id>,
}

pub struct PayloadTraceContext {
    block_hash_to_payload_ids: Cache<B256, Vec<PayloadId>>,
    payload_id: Cache<PayloadId, PayloadTrace>,
}

impl Default for PayloadTraceContext {
    fn default() -> Self {
        Self::new()
    }
}

impl PayloadTraceContext {
    pub fn new() -> Self {
        PayloadTraceContext {
            block_hash_to_payload_ids: Cache::new(CACHE_SIZE),
            payload_id: Cache::new(CACHE_SIZE),
        }
    }

    pub async fn store(
        &self,
        payload_id: PayloadId,
        parent_hash: B256,
        builder_has_payload: bool,
        trace_id: Option<tracing::Id>,
    ) {
        self.payload_id
            .insert(
                payload_id,
                PayloadTrace {
                    builder_has_payload,
                    trace_id,
                },
            )
            .await;
        self.block_hash_to_payload_ids
            .entry(parent_hash)
            .and_upsert_with(|o| match o {
                Some(e) => {
                    let mut payloads = e.into_value();
                    payloads.push(payload_id);
                    std::future::ready(payloads)
                }
                None => std::future::ready(vec![payload_id]),
            })
            .await;
    }

    pub async fn trace_ids_from_parent_hash(&self, parent_hash: &B256) -> Option<Vec<tracing::Id>> {
        match self.block_hash_to_payload_ids.get(parent_hash).await {
            Some(payload_ids) => Some(
                stream::iter(payload_ids.iter())
                    .filter_map(|payload_id| async {
                        self.payload_id
                            .get(payload_id)
                            .await
                            .and_then(|x| x.trace_id)
                    })
                    .collect()
                    .await,
            ),
            None => None,
        }
    }

    pub async fn trace_id(&self, payload_id: &PayloadId) -> Option<tracing::Id> {
        self.payload_id
            .get(payload_id)
            .await
            .and_then(|x| x.trace_id)
    }

    pub async fn has_builder_payload(&self, payload_id: &PayloadId) -> bool {
        self.payload_id
            .get(payload_id)
            .await
            .map(|x| x.builder_has_payload)
            .unwrap_or_default()
    }

    pub async fn remove_by_parent_hash(&self, block_hash: &B256) {
        if let Some(payload_ids) = self.block_hash_to_payload_ids.remove(block_hash).await {
            for payload_id in payload_ids.iter() {
                self.payload_id.remove(payload_id).await;
            }
        }
    }
}
