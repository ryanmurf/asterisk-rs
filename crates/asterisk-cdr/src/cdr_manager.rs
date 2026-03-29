//! AMI CDR backend - send CDR events via the Asterisk Manager Interface.
//!
//! Port of cdr/cdr_manager.c from Asterisk C.
//!
//! Formats CDR records as AMI "Cdr" events with all standard CDR fields
//! as event headers. Custom field mappings can be configured.

use crate::{Cdr, CdrBackend, CdrError};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::debug;

/// Configuration for the Manager CDR backend.
#[derive(Debug, Clone)]
pub struct ManagerCdrConfig {
    /// Whether the backend is enabled
    pub enabled: bool,
    /// Custom field mappings: config_field_name -> CDR field name
    pub custom_fields: HashMap<String, String>,
}

impl Default for ManagerCdrConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            custom_fields: HashMap::new(),
        }
    }
}

/// AMI CDR backend.
///
/// Sends CDR records as AMI "Cdr" events. In production, this would
/// send the event via the AMI subsystem. In this port, it formats
/// the event and stores the last event for testing/inspection.
///
/// Standard AMI CDR event headers:
///   AccountCode, Source, Destination, DestinationContext,
///   CallerID, Channel, DestinationChannel, LastApplication,
///   LastData, StartTime, AnswerTime, EndTime, Duration,
///   BillableSeconds, Disposition, AMAFlags, UniqueID, UserField
pub struct ManagerCdrBackend {
    config: ManagerCdrConfig,
    /// Last formatted event (for testing/inspection)
    last_event: parking_lot::Mutex<Option<String>>,
}

impl ManagerCdrBackend {
    /// Create a new Manager CDR backend with default configuration.
    pub fn new() -> Self {
        Self {
            config: ManagerCdrConfig::default(),
            last_event: parking_lot::Mutex::new(None),
        }
    }

    /// Create with specific configuration.
    pub fn with_config(config: ManagerCdrConfig) -> Self {
        Self {
            config,
            last_event: parking_lot::Mutex::new(None),
        }
    }

    /// Get the last formatted AMI event string.
    pub fn last_event(&self) -> Option<String> {
        self.last_event.lock().clone()
    }

    /// Format a CDR as an AMI event string.
    fn format_event(cdr: &Cdr, custom_fields: &HashMap<String, String>) -> String {
        let mut lines = Vec::new();

        lines.push("Event: Cdr".to_string());
        lines.push(format!("AccountCode: {}", cdr.account_code));
        lines.push(format!("Source: {}", cdr.src));
        lines.push(format!("Destination: {}", cdr.dst));
        lines.push(format!("DestinationContext: {}", cdr.dst_context));
        lines.push(format!("CallerID: {}", cdr.caller_id));
        lines.push(format!("Channel: {}", cdr.channel));
        lines.push(format!("DestinationChannel: {}", cdr.dst_channel));
        lines.push(format!("LastApplication: {}", cdr.last_app));
        lines.push(format!("LastData: {}", cdr.last_data));
        lines.push(format!("StartTime: {}", format_time(cdr.start)));
        lines.push(format!(
            "AnswerTime: {}",
            cdr.answer.map(format_time).unwrap_or_default()
        ));
        lines.push(format!("EndTime: {}", format_time(cdr.end)));
        lines.push(format!("Duration: {}", cdr.duration));
        lines.push(format!("BillableSeconds: {}", cdr.billsec));
        lines.push(format!("Disposition: {}", cdr.disposition.as_str()));
        lines.push(format!("AMAFlags: {}", cdr.ama_flags.as_str()));
        lines.push(format!("UniqueID: {}", cdr.unique_id));
        lines.push(format!("UserField: {}", cdr.user_field));

        // Add custom fields from CDR variables
        for (header_name, cdr_field) in custom_fields {
            let value = Self::get_cdr_field(cdr, cdr_field);
            lines.push(format!("{}: {}", header_name, value));
        }

        lines.join("\r\n") + "\r\n"
    }

    /// Get a CDR field value by name.
    fn get_cdr_field(cdr: &Cdr, field: &str) -> String {
        match field.to_lowercase().as_str() {
            "src" | "source" => cdr.src.clone(),
            "dst" | "destination" => cdr.dst.clone(),
            "dcontext" | "dstcontext" => cdr.dst_context.clone(),
            "channel" => cdr.channel.clone(),
            "dstchannel" => cdr.dst_channel.clone(),
            "lastapp" => cdr.last_app.clone(),
            "lastdata" => cdr.last_data.clone(),
            "disposition" => cdr.disposition.as_str().to_string(),
            "amaflags" => cdr.ama_flags.as_str().to_string(),
            "accountcode" => cdr.account_code.clone(),
            "uniqueid" => cdr.unique_id.clone(),
            "userfield" => cdr.user_field.clone(),
            "linkedid" => cdr.linked_id.clone(),
            "peeraccount" => cdr.peer_account.clone(),
            "duration" => cdr.duration.to_string(),
            "billsec" => cdr.billsec.to_string(),
            "clid" | "callerid" => cdr.caller_id.clone(),
            "sequence" => cdr.sequence.to_string(),
            _ => cdr.variables.get(field).cloned().unwrap_or_default(),
        }
    }
}

impl Default for ManagerCdrBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CdrBackend for ManagerCdrBackend {
    fn name(&self) -> &str {
        "manager"
    }

    fn log(&self, cdr: &Cdr) -> Result<(), CdrError> {
        if !self.config.enabled {
            return Ok(());
        }

        let event = Self::format_event(cdr, &self.config.custom_fields);

        debug!(
            "CDR Manager: sending event for channel '{}'",
            cdr.channel
        );

        // In production, this would call manager_event() to send via AMI.
        // For now, store the last event for inspection.
        *self.last_event.lock() = Some(event);

        Ok(())
    }
}

/// Format a SystemTime as a date/time string.
fn format_time(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let total_secs = duration.as_secs();

    let days = total_secs / 86400;
    let remaining_secs = total_secs % 86400;
    let hours = remaining_secs / 3600;
    let minutes = (remaining_secs % 3600) / 60;
    let seconds = remaining_secs % 60;

    let (year, month, day) = days_to_date(days as i64);

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        year, month, day, hours, minutes, seconds
    )
}

fn days_to_date(mut days: i64) -> (i64, u32, u32) {
    let mut year: i64 = 1970;
    loop {
        let diy = if is_leap_year(year) { 366 } else { 365 };
        if days < diy {
            break;
        }
        days -= diy;
        year += 1;
    }

    let leap = is_leap_year(year);
    let month_days: [i64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];

    let mut month = 1u32;
    for &md in &month_days {
        if days < md {
            break;
        }
        days -= md;
        month += 1;
    }

    (year, month, days as u32 + 1)
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CdrDisposition;

    #[test]
    fn test_format_event() {
        let mut cdr = Cdr::new("SIP/alice-001".to_string(), "uid-123".to_string());
        cdr.src = "5551234".to_string();
        cdr.dst = "100".to_string();
        cdr.dst_context = "default".to_string();
        cdr.caller_id = "Alice <5551234>".to_string();
        cdr.disposition = CdrDisposition::Answered;
        cdr.duration = 120;
        cdr.billsec = 90;

        let event = ManagerCdrBackend::format_event(&cdr, &HashMap::new());

        assert!(event.contains("Event: Cdr"));
        assert!(event.contains("Source: 5551234"));
        assert!(event.contains("Destination: 100"));
        assert!(event.contains("Channel: SIP/alice-001"));
        assert!(event.contains("Disposition: ANSWERED"));
        assert!(event.contains("Duration: 120"));
        assert!(event.contains("BillableSeconds: 90"));
        assert!(event.contains("UniqueID: uid-123"));
    }

    #[test]
    fn test_manager_log() {
        let backend = ManagerCdrBackend::new();
        let mut cdr = Cdr::new("SIP/bob-002".to_string(), "uid-456".to_string());
        cdr.src = "5559876".to_string();
        cdr.dst = "200".to_string();
        cdr.disposition = CdrDisposition::Busy;

        backend.log(&cdr).unwrap();

        let event = backend.last_event().unwrap();
        assert!(event.contains("Event: Cdr"));
        assert!(event.contains("Source: 5559876"));
        assert!(event.contains("Disposition: BUSY"));
    }

    #[test]
    fn test_manager_disabled() {
        let config = ManagerCdrConfig {
            enabled: false,
            custom_fields: HashMap::new(),
        };
        let backend = ManagerCdrBackend::with_config(config);
        let cdr = Cdr::new("SIP/test".to_string(), "uid".to_string());

        backend.log(&cdr).unwrap();
        assert!(backend.last_event().is_none());
    }

    #[test]
    fn test_custom_fields() {
        let mut custom = HashMap::new();
        custom.insert("X-CustomSrc".to_string(), "src".to_string());
        custom.insert("X-CustomDur".to_string(), "duration".to_string());

        let mut cdr = Cdr::new("SIP/test".to_string(), "uid".to_string());
        cdr.src = "1234".to_string();
        cdr.duration = 60;

        let event = ManagerCdrBackend::format_event(&cdr, &custom);
        assert!(event.contains("X-CustomSrc: 1234"));
        assert!(event.contains("X-CustomDur: 60"));
    }
}
