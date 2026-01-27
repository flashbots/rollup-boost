use eyre::Result;
use rollup_boost_types::flashblocks::FlashblocksPayloadV1;
use serde_json;

/// Parsed flashblock metadata extracted from a flashblock message.
/// Contains the essential fields needed for leader tracking.
#[derive(Debug, Clone)]
pub struct ParsedFlashblock {
    /// The block number being built (only present in flashblock index 0)
    pub block_number: Option<u64>,
}

/// Parse a flashblock message to extract relevant metadata.
///
/// This function deserializes the JSON message into a FlashblocksPayloadV1 structure
/// and extracts the payload_id, block_number (if present), and index.
///
/// # Arguments
/// * `message` - The JSON-encoded flashblock message string
///
/// # Returns
/// * `Ok(ParsedFlashblock)` - Successfully parsed flashblock metadata
/// * `Err(eyre::Error)` - Failed to parse the message (invalid JSON or structure)
///
/// # Notes
/// * The block_number is only present in flashblock index 0 (which contains the base payload)
/// * Subsequent flashblocks (index > 0) will have block_number = None
pub fn parse_flashblock(message: &str) -> Result<ParsedFlashblock> {
    let payload: FlashblocksPayloadV1 = serde_json::from_str(message)?;

    Ok(ParsedFlashblock {
        block_number: payload.base.map(|b| b.block_number),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_flashblock_with_base() {
        let message = r#"{
            "payload_id": "0x0000000000000001",
            "index": 0,
            "diff": {
                "state_root": "0x0000000000000000000000000000000000000000000000000000000000000001",
                "receipts_root": "0x0000000000000000000000000000000000000000000000000000000000000002",
                "logs_bloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
                "gas_used": "0x5208",
                "block_hash": "0x0000000000000000000000000000000000000000000000000000000000000003",
                "transactions": ["0xdeadbeef"],
                "withdrawals": [],
                "withdrawals_root": "0x0000000000000000000000000000000000000000000000000000000000000004"
            },
            "metadata": {},
            "base": {
                "parent_beacon_block_root": "0x0000000000000000000000000000000000000000000000000000000000000005",
                "parent_hash": "0x0000000000000000000000000000000000000000000000000000000000000006",
                "fee_recipient": "0x0000000000000000000000000000000000000000",
                "prev_randao": "0x0000000000000000000000000000000000000000000000000000000000000007",
                "block_number": "0x7b",
                "gas_limit": "0x1c9c380",
                "timestamp": "0x6563b900",
                "extra_data": "0x68656c6c6f",
                "base_fee_per_gas": "0x3b9aca00"
            }
        }"#;

        let parsed = parse_flashblock(message).expect("Failed to parse flashblock");
        assert_eq!(parsed.block_number, Some(123)); // 0x7b = 123
    }

    #[test]
    fn test_parse_flashblock_without_base() {
        let message = r#"{
            "payload_id": "0x0000000000000001",
            "index": 1,
            "diff": {
                "state_root": "0x0000000000000000000000000000000000000000000000000000000000000001",
                "receipts_root": "0x0000000000000000000000000000000000000000000000000000000000000002",
                "logs_bloom": "0x00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
                "gas_used": "0x5208",
                "block_hash": "0x0000000000000000000000000000000000000000000000000000000000000003",
                "transactions": ["0xdeadbeef"],
                "withdrawals": [],
                "withdrawals_root": "0x0000000000000000000000000000000000000000000000000000000000000004"
            },
            "metadata": {}
        }"#;

        let parsed = parse_flashblock(message).expect("Failed to parse flashblock");
        assert_eq!(parsed.block_number, None);
    }

    #[test]
    fn test_parse_invalid_json() {
        let message = "invalid json";
        let result = parse_flashblock(message);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_incomplete_message() {
        let message = r#"{"payload_id": "0x0000000000000001"}"#;
        let result = parse_flashblock(message);
        assert!(result.is_err());
    }
}
