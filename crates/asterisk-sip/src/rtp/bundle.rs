//! BUNDLE (RFC 8843) -- Grouping multiple media on one transport.
//!
//! Allows audio, video, and data channels to share a single ICE candidate
//! set and DTLS session. Demultiplexing is done by SSRC-to-MID mapping
//! learned from SDP or the `urn:ietf:params:rtp-hdrext:sdes:mid` RTP
//! header extension.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use super::RtpHeader;
use asterisk_types::AsteriskError;

// ---------------------------------------------------------------------------
// Bundle group
// ---------------------------------------------------------------------------

/// A BUNDLE group representing multiple media sections sharing one transport.
///
/// SDP example:
/// ```text
/// a=group:BUNDLE audio video
/// ...
/// m=audio 9 UDP/TLS/RTP/SAVPF 111
/// a=mid:audio
/// ...
/// m=video 9 UDP/TLS/RTP/SAVPF 96
/// a=mid:video
/// ```
#[derive(Debug, Clone)]
pub struct BundleGroup {
    /// MID values in this bundle group (e.g. ["audio", "video"]).
    pub mid_values: Vec<String>,
    /// Mapping from SSRC to MID for demultiplexing.
    ssrc_to_mid: HashMap<u32, String>,
    /// RTP header extension ID for `sdes:mid`.
    mid_ext_id: Option<u8>,
}

impl BundleGroup {
    /// Create a new bundle group with the given MID values.
    pub fn new(mid_values: Vec<String>) -> Self {
        Self {
            mid_values,
            ssrc_to_mid: HashMap::new(),
            mid_ext_id: None,
        }
    }

    /// Check if a MID value is part of this bundle group.
    pub fn contains_mid(&self, mid: &str) -> bool {
        self.mid_values.iter().any(|m| m == mid)
    }

    /// Set the RTP header extension ID used for `sdes:mid`.
    pub fn set_mid_ext_id(&mut self, ext_id: u8) {
        self.mid_ext_id = Some(ext_id);
    }

    /// Get the MID extension ID.
    pub fn mid_ext_id(&self) -> Option<u8> {
        self.mid_ext_id
    }

    /// Register an SSRC-to-MID mapping (learned from SDP `a=ssrc:` or signaling).
    pub fn map_ssrc_to_mid(&mut self, ssrc: u32, mid: String) {
        self.ssrc_to_mid.insert(ssrc, mid);
    }

    /// Look up the MID for an SSRC.
    pub fn mid_for_ssrc(&self, ssrc: u32) -> Option<&str> {
        self.ssrc_to_mid.get(&ssrc).map(|s| s.as_str())
    }

    /// Demultiplex an RTP packet to determine which MID it belongs to.
    ///
    /// Strategy:
    /// 1. Check SSRC-to-MID mapping table
    /// 2. If RTP header extension contains `sdes:mid`, use that
    /// 3. Return None if unable to determine
    pub fn demux_packet(
        &mut self,
        header: &RtpHeader,
        packet_data: &[u8],
    ) -> Option<String> {
        // Strategy 1: Known SSRC mapping.
        if let Some(mid) = self.ssrc_to_mid.get(&header.ssrc) {
            return Some(mid.clone());
        }

        // Strategy 2: Parse RTP header extension for sdes:mid.
        if header.extension {
            if let Some(ext_id) = self.mid_ext_id {
                if let Some(mid) = extract_mid_from_header_extension(packet_data, ext_id) {
                    // Learn and cache this SSRC-to-MID mapping.
                    self.ssrc_to_mid.insert(header.ssrc, mid.clone());
                    return Some(mid);
                }
            }
        }

        None
    }

    /// Number of media identifiers in this group.
    pub fn len(&self) -> usize {
        self.mid_values.len()
    }

    /// Whether the group is empty.
    pub fn is_empty(&self) -> bool {
        self.mid_values.is_empty()
    }
}

// ---------------------------------------------------------------------------
// SDP parsing helpers
// ---------------------------------------------------------------------------

/// Parse `a=group:BUNDLE` from session-level SDP attributes.
///
/// Returns a BundleGroup if the attribute is present.
pub fn parse_bundle_group(
    attributes: &[(String, Option<String>)],
) -> Option<BundleGroup> {
    for (name, value) in attributes {
        if name == "group" {
            if let Some(val) = value {
                if let Some(rest) = val.strip_prefix("BUNDLE ") {
                    let mids: Vec<String> =
                        rest.split_whitespace().map(|s| s.to_string()).collect();
                    if !mids.is_empty() {
                        return Some(BundleGroup::new(mids));
                    }
                } else if val == "BUNDLE" {
                    // Empty bundle (unusual but valid).
                    return Some(BundleGroup::new(Vec::new()));
                }
            }
        }
    }
    None
}

/// Parse `a=mid:` from media-level SDP attributes.
pub fn parse_mid(attributes: &[(String, Option<String>)]) -> Option<String> {
    for (name, value) in attributes {
        if name == "mid" {
            return value.clone();
        }
    }
    None
}

/// Parse `a=extmap:` to find the sdes:mid extension ID.
///
/// Looks for:
/// ```text
/// a=extmap:1 urn:ietf:params:rtp-hdrext:sdes:mid
/// ```
pub fn parse_mid_ext_id(
    attributes: &[(String, Option<String>)],
) -> Option<u8> {
    for (name, value) in attributes {
        if name == "extmap" {
            if let Some(val) = value {
                if val.contains("urn:ietf:params:rtp-hdrext:sdes:mid") {
                    // Format: "ID uri" or "ID/direction uri"
                    let parts: Vec<&str> = val.split_whitespace().collect();
                    if let Some(id_str) = parts.first() {
                        let id_part = id_str.split('/').next().unwrap_or(id_str);
                        if let Ok(id) = id_part.parse::<u8>() {
                            return Some(id);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Parse `a=ssrc:SSRC msid:...` or similar to extract SSRC values
/// and associate them with a MID value.
pub fn parse_ssrc_attributes(
    attributes: &[(String, Option<String>)],
) -> Vec<u32> {
    let mut ssrcs = Vec::new();
    for (name, value) in attributes {
        if name == "ssrc" {
            if let Some(val) = value {
                if let Some(ssrc_str) = val.split_whitespace().next() {
                    if let Ok(ssrc) = ssrc_str.parse::<u32>() {
                        if !ssrcs.contains(&ssrc) {
                            ssrcs.push(ssrc);
                        }
                    }
                }
            }
        }
    }
    ssrcs
}

/// Generate SDP attributes for a bundle group.
pub fn generate_bundle_attributes(group: &BundleGroup) -> Vec<(String, Option<String>)> {
    let mut attrs = Vec::new();

    // Session-level: a=group:BUNDLE
    attrs.push((
        "group".to_string(),
        Some(format!("BUNDLE {}", group.mid_values.join(" "))),
    ));

    attrs
}

// ---------------------------------------------------------------------------
// RTP header extension parsing
// ---------------------------------------------------------------------------

/// Extract the MID value from an RTP header extension.
///
/// Handles one-byte header extensions (RFC 5285).
fn extract_mid_from_header_extension(packet: &[u8], ext_id: u8) -> Option<String> {
    // RTP header is at least 12 bytes. Extension starts after CSRC.
    if packet.len() < 12 {
        return None;
    }

    let cc = (packet[0] & 0x0F) as usize;
    let has_extension = (packet[0] & 0x10) != 0;
    if !has_extension {
        return None;
    }

    let ext_offset = 12 + cc * 4;
    if packet.len() < ext_offset + 4 {
        return None;
    }

    let ext_profile = u16::from_be_bytes([packet[ext_offset], packet[ext_offset + 1]]);
    let ext_length_words =
        u16::from_be_bytes([packet[ext_offset + 2], packet[ext_offset + 3]]) as usize;

    let ext_data_start = ext_offset + 4;
    let ext_data_end = ext_data_start + ext_length_words * 4;

    if packet.len() < ext_data_end {
        return None;
    }

    let ext_data = &packet[ext_data_start..ext_data_end];

    // One-byte header format (RFC 5285, Section 4.2).
    if ext_profile == 0xBEDE {
        let mut pos = 0;
        while pos < ext_data.len() {
            let byte = ext_data[pos];
            if byte == 0 {
                // Padding byte.
                pos += 1;
                continue;
            }
            let id = (byte >> 4) & 0x0F;
            let len = (byte & 0x0F) as usize + 1;
            pos += 1;

            if id == 15 {
                // Terminator.
                break;
            }

            if pos + len > ext_data.len() {
                break;
            }

            if id == ext_id {
                return String::from_utf8(ext_data[pos..pos + len].to_vec()).ok();
            }

            pos += len;
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bundle_group_basic() {
        let group = BundleGroup::new(vec![
            "audio".to_string(),
            "video".to_string(),
        ]);
        assert_eq!(group.len(), 2);
        assert!(group.contains_mid("audio"));
        assert!(group.contains_mid("video"));
        assert!(!group.contains_mid("data"));
    }

    #[test]
    fn test_bundle_ssrc_mapping() {
        let mut group = BundleGroup::new(vec![
            "audio".to_string(),
            "video".to_string(),
        ]);

        group.map_ssrc_to_mid(12345, "audio".to_string());
        group.map_ssrc_to_mid(67890, "video".to_string());

        assert_eq!(group.mid_for_ssrc(12345), Some("audio"));
        assert_eq!(group.mid_for_ssrc(67890), Some("video"));
        assert_eq!(group.mid_for_ssrc(99999), None);
    }

    #[test]
    fn test_parse_bundle_group_sdp() {
        let attrs = vec![
            ("group".to_string(), Some("BUNDLE audio video".to_string())),
            ("msid-semantic".to_string(), Some("WMS".to_string())),
        ];

        let group = parse_bundle_group(&attrs).unwrap();
        assert_eq!(group.mid_values, vec!["audio", "video"]);
    }

    #[test]
    fn test_parse_bundle_group_three_streams() {
        let attrs = vec![(
            "group".to_string(),
            Some("BUNDLE 0 1 2".to_string()),
        )];

        let group = parse_bundle_group(&attrs).unwrap();
        assert_eq!(group.mid_values, vec!["0", "1", "2"]);
    }

    #[test]
    fn test_parse_bundle_group_missing() {
        let attrs = vec![("msid-semantic".to_string(), Some("WMS".to_string()))];
        assert!(parse_bundle_group(&attrs).is_none());
    }

    #[test]
    fn test_parse_mid() {
        let attrs = vec![
            ("rtpmap".to_string(), Some("111 opus/48000/2".to_string())),
            ("mid".to_string(), Some("audio".to_string())),
        ];
        assert_eq!(parse_mid(&attrs), Some("audio".to_string()));
    }

    #[test]
    fn test_parse_mid_ext_id() {
        let attrs = vec![
            (
                "extmap".to_string(),
                Some("1 urn:ietf:params:rtp-hdrext:sdes:mid".to_string()),
            ),
            (
                "extmap".to_string(),
                Some("2 urn:ietf:params:rtp-hdrext:toffset".to_string()),
            ),
        ];
        assert_eq!(parse_mid_ext_id(&attrs), Some(1));
    }

    #[test]
    fn test_parse_mid_ext_id_with_direction() {
        let attrs = vec![(
            "extmap".to_string(),
            Some("3/sendrecv urn:ietf:params:rtp-hdrext:sdes:mid".to_string()),
        )];
        assert_eq!(parse_mid_ext_id(&attrs), Some(3));
    }

    #[test]
    fn test_parse_ssrc_attributes() {
        let attrs = vec![
            (
                "ssrc".to_string(),
                Some("1234567890 cname:stream1".to_string()),
            ),
            (
                "ssrc".to_string(),
                Some("1234567890 msid:stream1 track1".to_string()),
            ),
            (
                "ssrc".to_string(),
                Some("987654321 cname:stream2".to_string()),
            ),
        ];
        let ssrcs = parse_ssrc_attributes(&attrs);
        assert_eq!(ssrcs.len(), 2);
        assert!(ssrcs.contains(&1234567890));
        assert!(ssrcs.contains(&987654321));
    }

    #[test]
    fn test_generate_bundle_attributes() {
        let group = BundleGroup::new(vec![
            "audio".to_string(),
            "video".to_string(),
        ]);
        let attrs = generate_bundle_attributes(&group);
        assert_eq!(attrs.len(), 1);
        assert_eq!(
            attrs[0],
            (
                "group".to_string(),
                Some("BUNDLE audio video".to_string())
            )
        );
    }

    #[test]
    fn test_demux_by_ssrc() {
        let mut group = BundleGroup::new(vec![
            "audio".to_string(),
            "video".to_string(),
        ]);
        group.map_ssrc_to_mid(111, "audio".to_string());
        group.map_ssrc_to_mid(222, "video".to_string());

        let header_audio = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: false,
            payload_type: 0,
            sequence: 1,
            timestamp: 0,
            ssrc: 111,
        };

        let header_video = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            csrc_count: 0,
            marker: false,
            payload_type: 96,
            sequence: 1,
            timestamp: 0,
            ssrc: 222,
        };

        assert_eq!(
            group.demux_packet(&header_audio, &[]),
            Some("audio".to_string())
        );
        assert_eq!(
            group.demux_packet(&header_video, &[]),
            Some("video".to_string())
        );
    }

    // -----------------------------------------------------------------------
    // ADVERSARIAL BUNDLE TESTS
    // -----------------------------------------------------------------------

    #[test]
    fn test_bundle_unknown_ssrc_no_extension_returns_none() {
        // Packet with unknown SSRC and no mid extension -> cannot demux, drop
        let mut group = BundleGroup::new(vec![
            "audio".to_string(),
            "video".to_string(),
        ]);
        group.map_ssrc_to_mid(111, "audio".to_string());

        let header = RtpHeader {
            version: 2, padding: false, extension: false, csrc_count: 0,
            marker: false, payload_type: 96, sequence: 1, timestamp: 0,
            ssrc: 99999, // Unknown SSRC
        };

        assert_eq!(group.demux_packet(&header, &[]), None,
            "Unknown SSRC without extension should return None");
    }

    #[test]
    fn test_bundle_valid_mid_extension_demux() {
        // Packet with valid mid extension -> correct demux
        let mut group = BundleGroup::new(vec![
            "audio".to_string(),
            "video".to_string(),
        ]);
        group.set_mid_ext_id(1);

        // Build an RTP packet with one-byte header extension containing mid="audio"
        let mid_bytes = b"audio";
        let ext_id: u8 = 1;
        let ext_byte = (ext_id << 4) | ((mid_bytes.len() as u8) - 1);

        let mut packet = vec![
            0x90, 0x00, // V=2, X=1, CC=0, M=0, PT=0
            0x00, 0x01, // seq
            0x00, 0x00, 0x00, 0xA0, // ts
            0x00, 0x00, 0x00, 0x01, // ssrc=1 (unknown)
            // Extension header
            0xBE, 0xDE, // one-byte header profile
            0x00, 0x02, // extension length = 2 words = 8 bytes
        ];
        // Extension data: ext_byte + mid value + padding
        packet.push(ext_byte);
        packet.extend_from_slice(mid_bytes);
        // Pad to 8 bytes total
        while packet.len() < 12 + 4 + 8 {
            packet.push(0);
        }

        let header = RtpHeader {
            version: 2, padding: false, extension: true, csrc_count: 0,
            marker: false, payload_type: 0, sequence: 1, timestamp: 160,
            ssrc: 1,
        };

        let result = group.demux_packet(&header, &packet);
        assert_eq!(result, Some("audio".to_string()));

        // Verify the SSRC-to-MID mapping was learned
        assert_eq!(group.mid_for_ssrc(1), Some("audio"));
    }

    #[test]
    fn test_bundle_two_media_same_ssrc_conflict() {
        // Two media sections mapped to the same SSRC -> last write wins (HashMap behavior)
        let mut group = BundleGroup::new(vec![
            "audio".to_string(),
            "video".to_string(),
        ]);
        group.map_ssrc_to_mid(111, "audio".to_string());
        group.map_ssrc_to_mid(111, "video".to_string()); // Overwrites!

        assert_eq!(group.mid_for_ssrc(111), Some("video"),
            "Last mapping should win on SSRC conflict");
    }

    #[test]
    fn test_bundle_empty_group() {
        let group = BundleGroup::new(Vec::new());
        assert!(group.is_empty());
        assert_eq!(group.len(), 0);
    }
}
