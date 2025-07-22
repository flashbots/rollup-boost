use std::marker::PhantomData;

use alloy_primitives::{B64, Bytes};
use alloy_rlp::{Decodable, Encodable, Header};
use alloy_rpc_types_engine::PayloadId;
use bytes::{Buf as _, BufMut as _, BytesMut};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::{FlashblocksP2PError, FlashblocksPayloadV1};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authorization {
    pub payload_id: PayloadId,
    pub timestamp: u64,
    pub builder_pub: VerifyingKey,
    pub authorizer_sig: Signature,
}

#[derive(Copy, Clone, Debug, PartialEq, Deserialize, Serialize, Eq)]
pub struct InitiateBuildReq;

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Deserialize, Serialize, Eq)]
pub enum InitiateBuildRes {
    Granted = 0x00,
    Denied = 0x01,
}

/// A message that can be sent over the Flashblocks P2P network.
#[repr(u8)]
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, Eq)]
pub enum FlashblocksP2PMsg {
    Authorized(Authorized) = 0x00,
}

#[repr(u8)]
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, Eq)]
pub enum AuthorizedMsg {
    FlashblocksPayloadV1(FlashblocksPayloadV1) = 0x00,
    InnitiateBuildReq(InitiateBuildReq) = 0x01,
    InnitiateBuildRes(InitiateBuildRes) = 0x02,
}

impl From<FlashblocksPayloadV1> for AuthorizedMsg {
    fn from(payload: FlashblocksPayloadV1) -> Self {
        Self::FlashblocksPayloadV1(payload)
    }
}

impl From<InitiateBuildReq> for AuthorizedMsg {
    fn from(req: InitiateBuildReq) -> Self {
        Self::InnitiateBuildReq(req)
    }
}

impl From<InitiateBuildRes> for AuthorizedMsg {
    fn from(res: InitiateBuildRes) -> Self {
        Self::InnitiateBuildRes(res)
    }
}

impl Authorization {
    pub fn new(
        payload_id: PayloadId,
        timestamp: u64,
        authorizer_sk: &SigningKey,
        builder_pub: VerifyingKey,
    ) -> Self {
        let mut msg = payload_id.0.to_vec();
        msg.extend_from_slice(&timestamp.to_le_bytes());
        msg.extend_from_slice(builder_pub.as_bytes());
        let hash = blake3::hash(&msg);
        let sig = authorizer_sk.sign(hash.as_bytes());

        Self {
            payload_id,
            timestamp,
            builder_pub,
            authorizer_sig: sig,
        }
    }

    pub fn verify(&self, authorizer_pub: VerifyingKey) -> Result<(), FlashblocksP2PError> {
        let mut msg = self.payload_id.0.to_vec();
        msg.extend_from_slice(&self.timestamp.to_le_bytes());
        msg.extend_from_slice(self.builder_pub.as_bytes());
        let hash = blake3::hash(&msg);
        authorizer_pub
            .verify(hash.as_bytes(), &self.authorizer_sig)
            .map_err(|_| FlashblocksP2PError::InvalidAuthorizerSig)
    }
}

impl Encodable for Authorization {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        // pre-serialize the key & sig once so we can reuse the bytes & lengths
        let pub_bytes = Bytes::copy_from_slice(self.builder_pub.as_bytes()); // 33 bytes
        let sig_bytes = Bytes::copy_from_slice(&self.authorizer_sig.to_bytes()); // 64 bytes

        let payload_len = self.payload_id.0.length()
            + self.timestamp.length()
            + pub_bytes.length()
            + sig_bytes.length();

        Header {
            list: true,
            payload_length: payload_len,
        }
        .encode(out);

        // 1. payload_id (inner B64 already Encodable)
        self.payload_id.0.encode(out);
        // 2. timestamp
        self.timestamp.encode(out);
        // 3. builder_pub
        pub_bytes.encode(out);
        // 4. authorizer_sig
        sig_bytes.encode(out);
    }

    fn length(&self) -> usize {
        let pub_bytes = Bytes::copy_from_slice(self.builder_pub.as_bytes());
        let sig_bytes = Bytes::copy_from_slice(&self.authorizer_sig.to_bytes());

        let payload_len = self.payload_id.0.length()
            + self.timestamp.length()
            + pub_bytes.length()
            + sig_bytes.length();

        Header {
            list: true,
            payload_length: payload_len,
        }
        .length()
            + payload_len
    }
}

impl Decodable for Authorization {
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        let mut body = &buf[..header.payload_length as usize];

        // 1. payload_id
        let payload_id = alloy_rpc_types_engine::PayloadId(B64::decode(&mut body)?.into());

        // 2. timestamp
        let timestamp = u64::decode(&mut body)?;

        // 3. builder_pub
        let pub_bytes = Bytes::decode(&mut body)?;
        let builder_pub = VerifyingKey::try_from(pub_bytes.as_ref())
            .map_err(|_| alloy_rlp::Error::Custom("bad builder_pub"))?;

        // 4. authorizer_sig
        let sig_bytes = Bytes::decode(&mut body)?;
        let authorizer_sig = Signature::try_from(sig_bytes.as_ref())
            .map_err(|_| alloy_rlp::Error::Custom("bad signature"))?;

        // advance caller’s slice cursor
        *buf = &buf[header.payload_length as usize..];

        Ok(Self {
            payload_id,
            timestamp,
            builder_pub,
            authorizer_sig,
        })
    }
}

/// A signed and authorized message that can be sent over the Flashblocks P2P network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizedPayload<T> {
    pub authorized: Authorized,
    /// The underlying message type
    pub _marker: PhantomData<T>,
}

impl<T> AuthorizedPayload<T>
where
    T: Into<AuthorizedMsg>,
{
    pub fn new(actor_sk: &SigningKey, authorization: Authorization, msg: T) -> Self {
        let msg = msg.into();
        let authorized = Authorized::new(actor_sk, authorization, msg);

        Self {
            authorized,
            _marker: PhantomData,
        }
    }
}

/// A signed and authorized message that can be sent over the Flashblocks P2P network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authorized {
    /// The msg that is being authorized and signed over.
    pub msg: AuthorizedMsg,
    /// The authorization that grants permission to send this message.
    pub authorization: Authorization,
    /// The signature of the actor, made over the hash of the message and authorization.
    pub actor_sig: Signature,
}

impl Authorized {
    pub fn new(actor_sk: &SigningKey, authorization: Authorization, msg: AuthorizedMsg) -> Self {
        let mut encoded = Vec::new();
        msg.encode(&mut encoded);
        authorization.encode(&mut encoded);

        let hash = blake3::hash(&encoded);
        let actor_sig = actor_sk.sign(hash.as_bytes());

        Self {
            msg,
            authorization,
            actor_sig,
        }
    }

    pub fn verify(&self, authorizer_pub: VerifyingKey) -> Result<(), FlashblocksP2PError> {
        self.authorization.verify(authorizer_pub)?;

        let mut encoded = Vec::new();
        self.msg.encode(&mut encoded);
        self.authorization.encode(&mut encoded);

        let hash = blake3::hash(&encoded);
        self.authorization
            .builder_pub
            .verify(hash.as_bytes(), &self.actor_sig)
            .map_err(|_| FlashblocksP2PError::InvalidBuilderSig)
    }

    pub fn into_unchecked<T>(self) -> AuthorizedPayload<T> {
        AuthorizedPayload::<T> {
            authorized: self,
            _marker: PhantomData,
        }
    }
}

impl<T> AuthorizedPayload<T>
where
    AuthorizedMsg: AsRef<T>,
{
    pub fn msg(&self) -> &T {
        self.authorized.msg.as_ref()
    }
}

impl Encodable for Authorized {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        // encode once so we know the length beforehand
        let sig_bytes = Bytes::copy_from_slice(&self.actor_sig.to_bytes());
        let payload_len = self.msg.length() + self.authorization.length() + sig_bytes.length();

        Header {
            list: true,
            payload_length: payload_len,
        }
        .encode(out);

        // 1. payload
        self.msg.encode(out);
        // 2. authorization
        self.authorization.encode(out);
        // 3. builder signature
        sig_bytes.encode(out);
    }

    fn length(&self) -> usize {
        let sig_bytes = Bytes::copy_from_slice(&self.actor_sig.to_bytes());
        let payload_len = self.msg.length() + self.authorization.length() + sig_bytes.length();

        Header {
            list: true,
            payload_length: payload_len,
        }
        .length()
            + payload_len
    }
}

impl Decodable for Authorized {
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }

        let mut body = &buf[..header.payload_length as usize];

        // 1. payload
        let payload = AuthorizedMsg::decode(&mut body)?;
        // 2. authorization
        let authorization = Authorization::decode(&mut body)?;
        // 3. builder signature
        let sig_bytes = Bytes::decode(&mut body)?;
        let builder_sig = Signature::try_from(sig_bytes.as_ref())
            .map_err(|_| alloy_rlp::Error::Custom("bad signature"))?;

        // advance caller’s cursor
        *buf = &buf[header.payload_length as usize..];

        Ok(Self {
            msg: payload,
            authorization,
            actor_sig: builder_sig,
        })
    }
}

impl FlashblocksP2PMsg {
    /// Creates a new `FlashblocksP2PError` with the given message ID and payload.
    pub fn encode(&self) -> BytesMut {
        let mut buf = BytesMut::new();
        match self {
            FlashblocksP2PMsg::Authorized(payload) => {
                buf.put_u8(0x00);
                payload.encode(&mut buf);
            }
        }
        buf
    }

    /// Decodes a `FlashblocksP2PError` from the given message buffer.
    pub fn decode(buf: &mut &[u8]) -> Result<Self, FlashblocksP2PError> {
        if buf.is_empty() {
            return Err(FlashblocksP2PError::InputTooShort);
        }
        let id = buf[0];
        buf.advance(1);
        match id {
            0x00 => {
                let payload = Authorized::decode(&mut &buf[..])?;
                Ok(FlashblocksP2PMsg::Authorized(payload))
            }
            _ => Err(FlashblocksP2PError::UnknownMessageType),
        }
    }
}

impl AsRef<FlashblocksPayloadV1> for AuthorizedMsg {
    fn as_ref(&self) -> &FlashblocksPayloadV1 {
        match self {
            Self::FlashblocksPayloadV1(p) => p,
            _ => panic!("not a FlashblocksPayloadV1 message"),
        }
    }
}

impl Encodable for AuthorizedMsg {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        match self {
            Self::FlashblocksPayloadV1(p) => {
                Header {
                    list: true,
                    payload_length: 1 + p.length(),
                }
                .encode(out);
                0u32.encode(out);
                p.encode(out);
            }
            Self::InnitiateBuildReq(_) => {
                Header {
                    list: true,
                    payload_length: 1,
                }
                .encode(out);
                1u32.encode(out);
            }
            Self::InnitiateBuildRes(r) => {
                Header {
                    list: true,
                    payload_length: 1 + r.length(),
                }
                .encode(out);
                2u32.encode(out);
                r.encode(out);
            }
        };
    }

    fn length(&self) -> usize {
        let body_len = match self {
            Self::FlashblocksPayloadV1(p) => 1 + p.length(),
            Self::InnitiateBuildReq(_) => 1,
            Self::InnitiateBuildRes(r) => 1 + r.length(),
        };

        Header {
            list: true,
            payload_length: body_len,
        }
        .length()
            + body_len
    }
}

impl Decodable for AuthorizedMsg {
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let hdr = Header::decode(buf)?;
        if !hdr.list {
            return Err(alloy_rlp::Error::Custom(
                "AuthorizedMsg must be an RLP list",
            ));
        }

        let tag = u8::decode(buf)?;
        let value = match tag {
            0 => Self::FlashblocksPayloadV1(FlashblocksPayloadV1::decode(buf)?),
            1 => Self::InnitiateBuildReq(InitiateBuildReq),
            2 => Self::InnitiateBuildRes(InitiateBuildRes::decode(buf)?),
            _ => return Err(alloy_rlp::Error::Custom("unknown tag")),
        };

        Ok(value)
    }
}

impl Encodable for InitiateBuildRes {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        (*self as u32).encode(out);
    }

    fn length(&self) -> usize {
        1
    }
}

impl Decodable for InitiateBuildRes {
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let tag = u8::decode(buf)?;
        match tag {
            0x00 => Ok(InitiateBuildRes::Granted),
            0x01 => Ok(InitiateBuildRes::Denied),
            _ => Err(alloy_rlp::Error::Custom("unknown tag")),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{ExecutionPayloadBaseV1, ExecutionPayloadFlashblockDeltaV1};

    use super::*;
    use alloy_primitives::{Address, B256, Bloom, U256};
    use alloy_rlp::{Decodable, encode};
    use alloy_rpc_types_eth::Withdrawal;

    fn sample_keys() -> (SigningKey, VerifyingKey) {
        // deterministic keys for reproducible tests
        let bytes = [0u8; 32];
        let sk = SigningKey::from_bytes(&bytes);
        let vk = sk.verifying_key();
        (sk, vk)
    }

    fn sample_authorization() -> Authorization {
        let (authorizer_sk, _) = sample_keys();
        let (_, builder_vk) = sample_keys();
        Authorization::new(
            alloy_rpc_types_engine::PayloadId::default(),
            1_700_000_321,
            &authorizer_sk,
            builder_vk,
        )
    }

    fn sample_diff() -> ExecutionPayloadFlashblockDeltaV1 {
        ExecutionPayloadFlashblockDeltaV1 {
            state_root: B256::from([0x11; 32]),
            receipts_root: B256::from([0x22; 32]),
            logs_bloom: Bloom::default(),
            gas_used: 21_000,
            block_hash: B256::from([0x33; 32]),
            transactions: vec![Bytes::from_static(b"\xDE\xAD\xBE\xEF")],
            withdrawals: vec![Withdrawal::default()],
            withdrawals_root: B256::from([0x44; 32]),
        }
    }

    fn sample_base() -> ExecutionPayloadBaseV1 {
        ExecutionPayloadBaseV1 {
            parent_beacon_block_root: B256::from([0x55; 32]),
            parent_hash: B256::from([0x66; 32]),
            fee_recipient: Address::default(),
            prev_randao: B256::from([0x77; 32]),
            block_number: 1_234,
            gas_limit: 30_000_000,
            timestamp: 1_700_000_999,
            extra_data: Bytes::from_static(b"hi"),
            base_fee_per_gas: U256::from(1_000_000_000u64),
        }
    }

    fn sample_flashblocks_payload() -> FlashblocksPayloadV1 {
        FlashblocksPayloadV1 {
            payload_id: PayloadId::default(),
            index: 7,
            diff: sample_diff(),
            metadata: serde_json::json!({ "ok": true }),
            base: Some(sample_base()),
        }
    }

    #[test]
    fn authorization_roundtrip() {
        let (authorizer_sk, authorizer_vk) = sample_keys();
        let (_, builder_vk) = sample_keys();

        let auth = Authorization::new(
            PayloadId::default(),
            1_700_000_123,
            &authorizer_sk,
            builder_vk,
        );

        // RLP encode-then-decode
        let encoded = encode(&auth);
        assert_eq!(encoded.len(), auth.length());

        let mut slice = encoded.as_ref();
        let decoded = Authorization::decode(&mut slice).expect("decode succeeds");
        assert_eq!(auth, decoded);
        assert!(slice.is_empty());

        // signature verifies
        decoded.verify(authorizer_vk).expect("sig valid");
    }

    #[test]
    fn tampered_sig_is_rejected() {
        let (authorizer_sk, authorizer_vk) = sample_keys();
        let (_, builder_vk) = sample_keys();

        let mut auth = Authorization::new(PayloadId::default(), 42, &authorizer_sk, builder_vk);

        // flip one bit in the signature
        let mut auth_sig_bytes = auth.authorizer_sig.to_bytes();
        auth_sig_bytes[0] ^= 0x01;
        auth.authorizer_sig =
            Signature::try_from(auth_sig_bytes.as_ref()).expect("valid signature bytes");
        assert!(auth.verify(authorizer_vk).is_err());
    }

    #[test]
    fn authorized_roundtrip_and_verify() {
        let (builder_sk, builder_vk) = sample_keys();
        let authorization = sample_authorization();
        let payload = sample_flashblocks_payload();
        let msg = AuthorizedMsg::FlashblocksPayloadV1(payload.clone());

        let authorized = Authorized::new(&builder_sk, authorization.clone(), msg);

        // RLP round-trip
        let encoded = encode(&authorized);
        assert_eq!(encoded.len(), authorized.length());

        let mut slice = encoded.as_ref();
        let decoded = Authorized::decode(&mut slice).expect("decode ok");
        assert_eq!(decoded, authorized);
        assert!(slice.is_empty(), "decoder consumed all input");

        decoded
            .verify(authorization.builder_pub)
            .expect("verify succeeds");

        let hash = blake3::hash(&serde_json::to_vec(&payload).unwrap());
        builder_vk
            .verify(hash.as_bytes(), &decoded.actor_sig)
            .expect("builder sig valid");
    }

    #[test]
    fn builder_sig_tamper_fails() {
        let (builder_sk, _) = sample_keys();
        let authorization = sample_authorization();
        let payload = sample_flashblocks_payload();
        let msg = AuthorizedMsg::FlashblocksPayloadV1(payload.clone());

        let mut authorized = Authorized::new(&builder_sk, authorization, msg);
        // flip one bit
        let mut authorized_sig_bytes = authorized.actor_sig.to_bytes();
        authorized_sig_bytes[0] ^= 0x01;
        authorized.actor_sig =
            Signature::try_from(authorized_sig_bytes.as_ref()).expect("valid signature bytes");
        assert!(
            authorized
                .verify(authorized.authorization.builder_pub)
                .is_err(),
            "tampered sig must be rejected"
        );
    }

    #[test]
    fn p2p_msg_roundtrip() {
        let (builder_sk, _) = sample_keys();
        let authorization = sample_authorization();
        let payload = sample_flashblocks_payload();
        let msg = AuthorizedMsg::FlashblocksPayloadV1(payload.clone());
        let authorized = Authorized::new(&builder_sk, authorization, msg);

        let msg = FlashblocksP2PMsg::Authorized(authorized.clone());

        let encoded = msg.encode();

        let decoded = FlashblocksP2PMsg::decode(&mut &encoded[..]).expect("decode ok");

        match decoded {
            FlashblocksP2PMsg::Authorized(inner) => {
                assert_eq!(inner, authorized, "inner payload round-trips");
            }
        }
        assert_eq!(encoded.remaining(), 0, "decoder consumed all input");
    }
}
