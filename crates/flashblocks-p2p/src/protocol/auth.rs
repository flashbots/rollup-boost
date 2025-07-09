use blake3::hash as blake3_hash;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use reth::payload::PayloadId;
use serde::{Deserialize, Serialize};
use serde_json;

use crate::protocol::error::FlashblocksP2PError;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authorized<T: Serialize> {
    payload: T,
    authorization: Authorization,
    builder_sig: Signature,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authorization {
    payload_id: PayloadId,
    builder_pub: VerifyingKey,
    authorizer_sig: Signature,
}

impl Authorization {
    pub fn new(
        authorizer_sk: &SigningKey,
        builder_pub: VerifyingKey,
        payload_id: PayloadId,
    ) -> Self {
        let mut msg = payload_id.0.to_vec();
        msg.extend_from_slice(builder_pub.as_bytes());
        let hash = blake3_hash(&msg);
        let sig = authorizer_sk.sign(hash.as_bytes());

        Self {
            payload_id,
            builder_pub,
            authorizer_sig: sig,
        }
    }

    pub fn verify(&self, authorizer_pub: VerifyingKey) -> Result<(), FlashblocksP2PError> {
        let mut msg = self.payload_id.0.to_vec();
        msg.extend_from_slice(self.builder_pub.as_bytes());
        let hash = blake3_hash(&msg);
        authorizer_pub
            .verify(hash.as_bytes(), &self.authorizer_sig)
            .map_err(|_| FlashblocksP2PError::InvalidAuthorizerSig)
    }
}

impl<T: Serialize> Authorized<T> {
    pub fn new(builder_sk: &SigningKey, authorization: Authorization, payload: T) -> Self {
        let hash = blake3_hash(&serde_json::to_vec(&payload).unwrap());
        let builder_sig = builder_sk.sign(hash.as_bytes());

        Self {
            payload,
            authorization,
            builder_sig,
        }
    }

    pub fn verify(&self, authorizer_pub: VerifyingKey) -> Result<(), FlashblocksP2PError> {
        self.authorization.verify(authorizer_pub)?;

        let hash = blake3_hash(&serde_json::to_vec(&self.payload).unwrap());

        self.authorization
            .builder_pub
            .verify(hash.as_bytes(), &self.builder_sig)
            .map_err(|_| FlashblocksP2PError::InvalidBuilderSig)
    }
}
