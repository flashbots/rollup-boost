use std::marker::PhantomData;

use alloy_primitives::{B64, Bytes};
use alloy_rlp::{Decodable, Encodable, Header};
use alloy_rpc_types_engine::PayloadId;
use bytes::{Buf as _, BufMut as _, BytesMut};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::{FlashblocksP2PError, FlashblocksPayloadV1};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authorization {
    pub payload_id: PayloadId,
    pub timestamp: u64,
    pub builder_vk: VerifyingKey,
    pub authorizer_sig: Signature,
}

#[derive(Copy, Clone, Debug, PartialEq, Deserialize, Serialize, Eq)]
pub struct StartPublish {
    pub block_number: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Deserialize, Serialize, Eq)]
pub struct StopPublish;

/// A message that can be sent over the Flashblocks P2P network.
#[repr(u8)]
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, Eq)]
pub enum FlashblocksP2PMsg {
    Authorized(Authorized) = 0x00,
}

#[allow(clippy::large_enum_variant)]
#[repr(u8)]
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, Eq)]
pub enum AuthorizedMsg {
    FlashblocksPayloadV1(FlashblocksPayloadV1) = 0x00,
    StartPublish(StartPublish) = 0x01,
    StopPublish(StopPublish) = 0x02,
}

impl From<FlashblocksPayloadV1> for AuthorizedMsg {
    fn from(payload: FlashblocksPayloadV1) -> Self {
        Self::FlashblocksPayloadV1(payload)
    }
}

impl From<StartPublish> for AuthorizedMsg {
    fn from(req: StartPublish) -> Self {
        Self::StartPublish(req)
    }
}

impl From<StopPublish> for AuthorizedMsg {
    fn from(res: StopPublish) -> Self {
        Self::StopPublish(res)
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
            builder_vk: builder_pub,
            authorizer_sig: sig,
        }
    }

    pub fn verify(&self, authorizer_pub: VerifyingKey) -> Result<(), FlashblocksP2PError> {
        let mut msg = self.payload_id.0.to_vec();
        msg.extend_from_slice(&self.timestamp.to_le_bytes());
        msg.extend_from_slice(self.builder_vk.as_bytes());
        let hash = blake3::hash(&msg);
        authorizer_pub
            .verify(hash.as_bytes(), &self.authorizer_sig)
            .map_err(|_| FlashblocksP2PError::InvalidAuthorizerSig)
    }
}

impl Encodable for Authorization {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        // pre-serialize the key & sig once so we can reuse the bytes & lengths
        let pub_bytes = Bytes::copy_from_slice(self.builder_vk.as_bytes()); // 33 bytes
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
        let pub_bytes = Bytes::copy_from_slice(self.builder_vk.as_bytes());
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
            builder_vk: builder_pub,
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
            .builder_vk
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
                let payload = Authorized::decode(buf)?;
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

impl AsRef<StartPublish> for AuthorizedMsg {
    fn as_ref(&self) -> &StartPublish {
        match self {
            Self::StartPublish(req) => req,
            _ => panic!("not a StartPublish message"),
        }
    }
}

impl AsRef<StopPublish> for AuthorizedMsg {
    fn as_ref(&self) -> &StopPublish {
        match self {
            Self::StopPublish(res) => res,
            _ => panic!("not a StopPublish message"),
        }
    }
}

impl Encodable for StartPublish {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.block_number.encode(out);
    }

    fn length(&self) -> usize {
        self.block_number.length()
    }
}

impl Decodable for StartPublish {
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        Ok(StartPublish {
            block_number: u64::decode(buf)?,
        })
    }
}

impl Encodable for StopPublish {
    fn encode(&self, _out: &mut dyn alloy_rlp::BufMut) {}

    fn length(&self) -> usize {
        0
    }
}

impl Decodable for StopPublish {
    fn decode(_buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        Ok(StopPublish)
    }
}

impl Encodable for AuthorizedMsg {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        match self {
            Self::FlashblocksPayloadV1(payload) => {
                Header {
                    list: true,
                    payload_length: 1 + payload.length(),
                }
                .encode(out);
                0u32.encode(out);
                payload.encode(out);
            }
            Self::StartPublish(start) => {
                Header {
                    list: true,
                    payload_length: 1 + start.length(),
                }
                .encode(out);
                1u32.encode(out);
                start.encode(out);
            }
            Self::StopPublish(stop) => {
                Header {
                    list: true,
                    payload_length: 1 + stop.length(),
                }
                .encode(out);
                2u32.encode(out);
                stop.encode(out);
            }
        };
    }

    fn length(&self) -> usize {
        let body_len = match self {
            Self::FlashblocksPayloadV1(payload) => 1 + payload.length(),
            Self::StartPublish(start) => 1 + start.length(),
            Self::StopPublish(stop) => 1 + stop.length(),
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
            1 => Self::StartPublish(StartPublish::decode(buf)?),
            2 => Self::StopPublish(StopPublish::decode(buf)?),
            _ => return Err(alloy_rlp::Error::Custom("unknown tag")),
        };

        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecutionPayloadBaseV1, ExecutionPayloadFlashblockDeltaV1};
    use alloy_primitives::{Address, B256, Bloom, U256};
    use alloy_rlp::{Decodable, Encodable, encode};
    use alloy_rpc_types_eth::Withdrawal;
    use bytes::{BufMut, BytesMut};

    fn key_pair(seed: u8) -> (SigningKey, VerifyingKey) {
        let bytes = [seed; 32];
        let sk = SigningKey::from_bytes(&bytes);
        let vk = sk.verifying_key();
        (sk, vk)
    }

    fn sample_authorization() -> (Authorization, VerifyingKey) {
        let (authorizer_sk, authorizer_vk) = key_pair(1);
        let (_, builder_vk) = key_pair(2);

        (
            Authorization::new(
                PayloadId::default(),
                1_700_000_001,
                &authorizer_sk,
                builder_vk,
            ),
            authorizer_vk,
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
            index: 42,
            diff: sample_diff(),
            metadata: serde_json::json!({ "ok": true }),
            base: Some(sample_base()),
        }
    }

    #[test]
    fn authorization_rlp_roundtrip_and_verify() {
        let (authorizer_sk, authorizer_vk) = key_pair(1);
        let (_, builder_vk) = key_pair(2);

        let auth = Authorization::new(
            PayloadId::default(),
            1_700_000_123,
            &authorizer_sk,
            builder_vk,
        );

        let encoded = encode(auth);
        assert_eq!(encoded.len(), auth.length(), "length impl correct");

        let mut slice = encoded.as_ref();
        let decoded = Authorization::decode(&mut slice).expect("decoding succeeds");
        assert!(slice.is_empty(), "decoder consumed all bytes");
        assert_eq!(decoded, auth, "round-trip preserves value");

        // Signature is valid
        decoded.verify(authorizer_vk).expect("signature verifies");
    }

    #[test]
    fn authorization_signature_tamper_is_detected() {
        let (authorizer_sk, authorizer_vk) = key_pair(1);
        let (_, builder_vk) = key_pair(2);

        let mut auth = Authorization::new(PayloadId::default(), 42, &authorizer_sk, builder_vk);

        let mut sig_bytes = auth.authorizer_sig.to_bytes();
        sig_bytes[0] ^= 1;
        auth.authorizer_sig = Signature::try_from(sig_bytes.as_ref()).unwrap();

        assert!(auth.verify(authorizer_vk).is_err());
    }

    #[test]
    fn authorized_rlp_roundtrip_and_verify() {
        let (builder_sk, _builder_vk) = key_pair(2);
        let (authorization, authorizer_vk) = sample_authorization();

        let payload = sample_flashblocks_payload();
        let msg = AuthorizedMsg::FlashblocksPayloadV1(payload);

        let authorized = Authorized::new(&builder_sk, authorization.clone(), msg);

        // Encode → decode
        let encoded = encode(&authorized);
        assert_eq!(encoded.len(), authorized.length());

        let mut slice = encoded.as_ref();
        let decoded = Authorized::decode(&mut slice).expect("decoding succeeds");
        assert!(slice.is_empty());
        assert_eq!(decoded, authorized);

        decoded
            .verify(authorizer_vk)
            .expect("composite verification succeeds");
    }

    #[test]
    fn authorized_builder_signature_tamper_is_detected() {
        let (builder_sk, _) = key_pair(2);
        let (authorization, authorizer_vk) = sample_authorization();
        let payload = sample_flashblocks_payload();
        let msg = AuthorizedMsg::FlashblocksPayloadV1(payload);

        let mut authorized = Authorized::new(&builder_sk, authorization, msg);

        let mut sig_bytes = authorized.actor_sig.to_bytes();
        sig_bytes[0] ^= 1;
        authorized.actor_sig = Signature::try_from(sig_bytes.as_ref()).unwrap();

        assert!(authorized.verify(authorizer_vk).is_err());
    }

    #[test]
    fn authorized_msg_variants_rlp_roundtrip() {
        let variants = [
            AuthorizedMsg::FlashblocksPayloadV1(sample_flashblocks_payload()),
            AuthorizedMsg::StartPublish(StartPublish { block_number: 100 }),
            AuthorizedMsg::StopPublish(StopPublish),
        ];

        for msg in variants {
            let encoded = encode(&msg);
            assert_eq!(encoded.len(), msg.length());

            let mut slice = encoded.as_ref();
            let decoded = AuthorizedMsg::decode(&mut slice).expect("decodes");
            assert!(slice.is_empty());
            assert_eq!(decoded, msg);
        }
    }

    #[test]
    fn p2p_msg_roundtrip() {
        let (builder_sk, _) = key_pair(2);
        let (authorization, _authorizer_vk) = sample_authorization();
        let payload = sample_flashblocks_payload();
        let msg = AuthorizedMsg::FlashblocksPayloadV1(payload);

        let authorized = Authorized::new(&builder_sk, authorization, msg);
        let p2p = FlashblocksP2PMsg::Authorized(authorized.clone());

        let encoded = p2p.encode();

        let mut view: &[u8] = &encoded;
        let decoded = FlashblocksP2PMsg::decode(&mut view).expect("decoding succeeds");
        assert!(view.is_empty(), "all bytes consumed");

        match decoded {
            FlashblocksP2PMsg::Authorized(inner) => assert_eq!(inner, authorized),
        }
    }

    #[test]
    fn p2p_msg_unknown_type_errors() {
        let mut buf = BytesMut::new();
        buf.put_u8(0xFF); // unknown discriminator

        let mut slice: &[u8] = &buf;
        let err =
            FlashblocksP2PMsg::decode(&mut slice).expect_err("should fail on unknown message type");
        assert_eq!(err, FlashblocksP2PError::UnknownMessageType);
    }
}
