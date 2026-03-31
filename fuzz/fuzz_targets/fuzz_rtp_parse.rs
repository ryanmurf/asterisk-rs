#![no_main]
use libfuzzer_sys::fuzz_target;

/// Minimal RTP header representation for fuzzing
#[derive(Debug)]
pub struct RtpHeader {
    pub version: u8,
    pub padding: bool,
    pub extension: bool,
    pub cc: u8,
    pub marker: bool,
    pub payload_type: u8,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub csrcs: Vec<u32>,
}

#[derive(Debug)]
pub struct RtpError(pub String);

const RTP_HEADER_SIZE: usize = 12;

impl RtpHeader {
    pub fn parse(data: &[u8]) -> Result<Self, RtpError> {
        if data.len() < RTP_HEADER_SIZE {
            return Err(RtpError("RTP packet too short".to_string()));
        }

        let version = (data[0] >> 6) & 0x03;
        if version != 2 {
            return Err(RtpError(format!("Invalid RTP version: {}", version)));
        }

        let padding = (data[0] & 0x20) != 0;
        let extension = (data[0] & 0x10) != 0;
        let cc = data[0] & 0x0F;
        let marker = (data[1] & 0x80) != 0;
        let payload_type = data[1] & 0x7F;

        let sequence = u16::from_be_bytes([data[2], data[3]]);
        let timestamp = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let ssrc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

        // Parse CSRCs if present
        let mut csrcs = Vec::new();
        let csrc_count = cc as usize;
        if csrc_count > 0 {
            let csrc_start = 12;
            let csrc_end = csrc_start + (csrc_count * 4);
            
            if data.len() >= csrc_end {
                for i in 0..csrc_count {
                    let offset = csrc_start + (i * 4);
                    let csrc = u32::from_be_bytes([
                        data[offset], data[offset + 1], 
                        data[offset + 2], data[offset + 3]
                    ]);
                    csrcs.push(csrc);
                }
            } else {
                return Err(RtpError("Insufficient data for CSRCs".to_string()));
            }
        }

        // Check for extension header
        if extension {
            let ext_start = 12 + (csrc_count * 4);
            if data.len() >= ext_start + 4 {
                // Extension header has profile and length fields
                let _profile = u16::from_be_bytes([data[ext_start], data[ext_start + 1]]);
                let ext_length = u16::from_be_bytes([data[ext_start + 2], data[ext_start + 3]]);
                
                // Skip extension header for simplicity
                let _ext_end = ext_start + 4 + (ext_length as usize * 4);
            }
        }

        Ok(RtpHeader {
            version,
            padding,
            extension,
            cc,
            marker,
            payload_type,
            sequence,
            timestamp,
            ssrc,
            csrcs,
        })
    }

    pub fn header_size(&self) -> usize {
        12 + (self.cc as usize * 4)
    }
}

fuzz_target!(|data: &[u8]| {
    // Ensure we don't panic on any input
    let _ = RtpHeader::parse(data);
});