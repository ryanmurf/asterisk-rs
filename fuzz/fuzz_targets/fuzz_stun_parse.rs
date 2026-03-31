#![no_main]
use libfuzzer_sys::fuzz_target;
use std::collections::HashMap;

/// Minimal STUN message representation for fuzzing
#[derive(Debug)]
pub struct StunMessage {
    pub message_type: u16,
    pub length: u16,
    pub magic_cookie: u32,
    pub transaction_id: [u8; 12],
    pub attributes: HashMap<u16, Vec<u8>>,
}

#[derive(Debug)]
pub struct StunError(pub String);

const HEADER_SIZE: usize = 20;
const MAGIC_COOKIE: u32 = 0x2112A442;

impl StunMessage {
    pub fn parse(data: &[u8]) -> Result<Self, StunError> {
        if data.len() < HEADER_SIZE {
            return Err(StunError(format!(
                "message too short: {} bytes",
                data.len()
            )));
        }

        // Check for STUN: first two bits must be 0
        if data[0] & 0xC0 != 0 {
            return Err(StunError("not a STUN message (first 2 bits not 0)".into()));
        }

        let message_type = u16::from_be_bytes([data[0], data[1]]);
        let length = u16::from_be_bytes([data[2], data[3]]);
        let magic_cookie = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);

        // Validate magic cookie
        if magic_cookie != MAGIC_COOKIE {
            return Err(StunError(format!(
                "invalid magic cookie: 0x{:08x}",
                magic_cookie
            )));
        }

        // Extract transaction ID
        let mut transaction_id = [0u8; 12];
        transaction_id.copy_from_slice(&data[8..20]);

        // Parse attributes
        let mut attributes = HashMap::new();
        let mut offset = HEADER_SIZE;
        let payload_end = HEADER_SIZE + length as usize;

        if data.len() < payload_end {
            return Err(StunError("message length mismatch".into()));
        }

        while offset < payload_end {
            if offset + 4 > data.len() {
                break; // Not enough data for attribute header
            }

            let attr_type = u16::from_be_bytes([data[offset], data[offset + 1]]);
            let attr_length = u16::from_be_bytes([data[offset + 2], data[offset + 3]]);
            offset += 4;

            // Calculate padded length (attributes are padded to 4-byte boundaries)
            let padded_length = ((attr_length + 3) / 4) * 4;

            if offset + padded_length as usize > data.len() {
                break; // Not enough data for attribute value
            }

            let value = data[offset..offset + attr_length as usize].to_vec();
            attributes.insert(attr_type, value);

            offset += padded_length as usize;
        }

        Ok(StunMessage {
            message_type,
            length,
            magic_cookie,
            transaction_id,
            attributes,
        })
    }
}

fuzz_target!(|data: &[u8]| {
    // Ensure we don't panic on any input
    let _ = StunMessage::parse(data);
});