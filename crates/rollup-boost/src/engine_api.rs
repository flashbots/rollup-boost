use alloy_primitives::B256;
use alloy_rpc_types_engine::{ForkchoiceState, ForkchoiceUpdated, PayloadId, PayloadStatus};
use alloy_rpc_types_eth::{Block, BlockNumberOrTag};
use jsonrpsee::core::async_trait;
use op_alloy_rpc_types_engine::OpPayloadAttributes;

use crate::ClientResult;
use rollup_boost_types::payload::{NewPayload, OpExecutionPayloadEnvelope, PayloadVersion};

/// op-reth uses `EngineApiMessageVersion::default()` to calculate the payload id instead
/// since the FCU version is not injected into the payload id. This is a bug on op-reth
/// described in https://github.com/ethereum-optimism/optimism/issues/20226.
const BUILDER_PAYLOAD_ID_VERSION: u8 = 4;

#[async_trait]
pub trait EngineApiExt: std::fmt::Debug + Send + Sync + 'static {
    /// Computes the payload id the builder is expected to derive for the given
    /// payload attributes.
    fn builder_payload_id(parent_hash: &B256, attributes: &OpPayloadAttributes) -> PayloadId
    where
        Self: Sized,
    {
        attributes.payload_id(parent_hash, BUILDER_PAYLOAD_ID_VERSION)
    }

    async fn fork_choice_updated_v3(
        &self,
        fork_choice_state: ForkchoiceState,
        payload_attributes: Option<OpPayloadAttributes>,
    ) -> ClientResult<ForkchoiceUpdated>;

    async fn new_payload(&self, new_payload: NewPayload) -> ClientResult<PayloadStatus>;

    async fn get_payload(
        &self,
        payload_id: PayloadId,
        version: PayloadVersion,
    ) -> ClientResult<OpExecutionPayloadEnvelope>;

    async fn get_block_by_number(
        &self,
        number: BlockNumberOrTag,
        full: bool,
    ) -> ClientResult<Block>;
}
