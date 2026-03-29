//! PIDF presence XML generation.
//!
//! Implements RFC 3863 (Presence Information Data Format) XML document
//! generation for SIP NOTIFY bodies used by the presence event package
//! (RFC 3856).

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Basic status
// ---------------------------------------------------------------------------

/// PIDF basic status element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidfBasicStatus {
    /// The entity is available (on-hook, ready).
    Open,
    /// The entity is unavailable (busy, offline).
    Closed,
}

impl PidfBasicStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }

    pub fn from_str_value(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "open" => Some(Self::Open),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

impl fmt::Display for PidfBasicStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// PIDF tuple
// ---------------------------------------------------------------------------

/// A tuple element within a PIDF document.
///
/// Each tuple represents a "service" or endpoint associated with the
/// presentity, along with its status.
#[derive(Debug, Clone)]
pub struct PidfTuple {
    /// Tuple ID (unique within the document).
    pub id: String,
    /// Basic status (open/closed).
    pub status: PidfBasicStatus,
    /// Contact URI for this tuple.
    pub contact: Option<String>,
    /// Timestamp (ISO 8601).
    pub timestamp: Option<String>,
    /// Optional note.
    pub note: Option<String>,
    /// Contact priority (0.0 to 1.0).
    pub priority: Option<f32>,
}

impl PidfTuple {
    /// Create a new tuple with the given ID and status.
    pub fn new(id: &str, status: PidfBasicStatus) -> Self {
        Self {
            id: id.to_string(),
            status,
            contact: None,
            timestamp: None,
            note: None,
            priority: None,
        }
    }

    pub fn with_contact(mut self, contact: &str) -> Self {
        self.contact = Some(contact.to_string());
        self
    }

    pub fn with_timestamp(mut self, ts: &str) -> Self {
        self.timestamp = Some(ts.to_string());
        self
    }

    pub fn with_note(mut self, note: &str) -> Self {
        self.note = Some(note.to_string());
        self
    }

    pub fn with_priority(mut self, priority: f32) -> Self {
        self.priority = Some(priority.clamp(0.0, 1.0));
        self
    }

    /// Generate the XML fragment for this tuple.
    fn to_xml(&self) -> String {
        let mut xml = format!("  <tuple id=\"{}\">\n", xml_escape(&self.id));
        xml.push_str(&format!(
            "    <status>\n      <basic>{}</basic>\n    </status>\n",
            self.status.as_str()
        ));
        if let Some(ref contact) = self.contact {
            xml.push_str("    <contact");
            if let Some(priority) = self.priority {
                xml.push_str(&format!(" priority=\"{:.1}\"", priority));
            }
            xml.push_str(&format!(">{}</contact>\n", xml_escape(contact)));
        }
        if let Some(ref note) = self.note {
            xml.push_str(&format!("    <note>{}</note>\n", xml_escape(note)));
        }
        if let Some(ref ts) = self.timestamp {
            xml.push_str(&format!("    <timestamp>{}</timestamp>\n", ts));
        }
        xml.push_str("  </tuple>\n");
        xml
    }
}

// ---------------------------------------------------------------------------
// PIDF document
// ---------------------------------------------------------------------------

/// A PIDF presence document (RFC 3863).
#[derive(Debug, Clone)]
pub struct PidfDocument {
    /// The entity URI (presentity).
    pub entity: String,
    /// Tuples describing presence state.
    pub tuples: Vec<PidfTuple>,
}

impl PidfDocument {
    /// Create a new PIDF document for the given entity.
    pub fn new(entity: &str) -> Self {
        Self {
            entity: entity.to_string(),
            tuples: Vec::new(),
        }
    }

    /// Add a tuple.
    pub fn add_tuple(&mut self, tuple: PidfTuple) {
        self.tuples.push(tuple);
    }

    /// Builder: add a tuple.
    pub fn with_tuple(mut self, tuple: PidfTuple) -> Self {
        self.tuples.push(tuple);
        self
    }

    /// Generate the complete PIDF XML document.
    pub fn generate_pidf_xml(&self) -> String {
        let mut xml = String::with_capacity(512);
        xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        xml.push_str("<presence xmlns=\"urn:ietf:params:xml:ns:pidf\"\n");
        xml.push_str(&format!(
            "  entity=\"{}\">\n",
            xml_escape(&self.entity)
        ));
        for tuple in &self.tuples {
            xml.push_str(&tuple.to_xml());
        }
        xml.push_str("</presence>\n");
        xml
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Generate an ISO 8601 timestamp from the current system time.
pub fn iso_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    // Simple UTC formatting without pulling in chrono.
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Approximate date calculation (sufficient for XML timestamps).
    let (year, month, day) = epoch_days_to_ymd(days_since_epoch);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds,
    )
}

/// Convert days since Unix epoch to (year, month, day).
fn epoch_days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from https://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Basic XML escaping.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_status() {
        assert_eq!(PidfBasicStatus::Open.as_str(), "open");
        assert_eq!(PidfBasicStatus::Closed.as_str(), "closed");
        assert_eq!(
            PidfBasicStatus::from_str_value("open"),
            Some(PidfBasicStatus::Open)
        );
        assert_eq!(PidfBasicStatus::from_str_value("invalid"), None);
    }

    #[test]
    fn test_pidf_document_generation() {
        let doc = PidfDocument::new("sip:alice@example.com").with_tuple(
            PidfTuple::new("t1", PidfBasicStatus::Open)
                .with_contact("sip:alice@192.168.1.100")
                .with_note("On the phone")
                .with_priority(0.8),
        );

        let xml = doc.generate_pidf_xml();
        assert!(xml.contains("<?xml version=\"1.0\""));
        assert!(xml.contains("entity=\"sip:alice@example.com\""));
        assert!(xml.contains("<basic>open</basic>"));
        assert!(xml.contains("<contact priority=\"0.8\">sip:alice@192.168.1.100</contact>"));
        assert!(xml.contains("<note>On the phone</note>"));
    }

    #[test]
    fn test_pidf_closed_status() {
        let doc = PidfDocument::new("sip:bob@example.com")
            .with_tuple(PidfTuple::new("t1", PidfBasicStatus::Closed));

        let xml = doc.generate_pidf_xml();
        assert!(xml.contains("<basic>closed</basic>"));
    }

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("a<b>c&d"), "a&lt;b&gt;c&amp;d");
    }

    #[test]
    fn test_iso_timestamp() {
        let ts = iso_timestamp();
        // Should end with Z and contain T separator.
        assert!(ts.ends_with('Z'));
        assert!(ts.contains('T'));
    }

    #[test]
    fn test_multiple_tuples() {
        let doc = PidfDocument::new("sip:alice@example.com")
            .with_tuple(PidfTuple::new("desk", PidfBasicStatus::Open))
            .with_tuple(PidfTuple::new("mobile", PidfBasicStatus::Closed));

        let xml = doc.generate_pidf_xml();
        assert!(xml.contains("id=\"desk\""));
        assert!(xml.contains("id=\"mobile\""));
    }
}
