use blake3::hash as blake3_hash;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rollup_boost::{Authorization, FlashblocksP2PError};
use serde::{Deserialize, Serialize};
use serde_json;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authorized<T: Serialize> {
    pub payload: T,
    pub authorization: Authorization,
    pub builder_sig: Signature,
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
