//! ADSI script programming application (stub).
//!
//! Port of app_adsiprog.c from Asterisk C. Programs ADSI (Analog
//! Display Services Interface) scripts into ADSI-capable phones.
//! ADSI allows controlling the phone's display, soft keys, and
//! other features from the PBX.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// The ADSIProg() dialplan application (stub).
///
/// Usage: ADSIProg(script_file)
///
/// Loads and programs an ADSI script into the connected phone.
/// The phone must support ADSI (e.g. certain Nortel, Lucent phones).
///
/// This is a deprecated feature but included for completeness.
pub struct AppAdsiProg;

impl DialplanApp for AppAdsiProg {
    fn name(&self) -> &str {
        "ADSIProg"
    }

    fn description(&self) -> &str {
        "Program ADSI scripts into a phone"
    }
}

impl AppAdsiProg {
    /// Execute the ADSIProg application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let script = args.trim();

        if script.is_empty() {
            warn!("ADSIProg: requires script file argument");
            return PbxExecResult::Failed;
        }

        info!("ADSIProg: channel '{}' programming script '{}'", channel.name, script);

        // In a real implementation:
        // 1. Parse ADSI script file
        // 2. Check if channel supports ADSI
        // 3. Send ADSI programming sequences to phone
        // 4. Wait for acknowledgment

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_adsiprog_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppAdsiProg::exec(&mut channel, "asterisk.adsi").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_adsiprog_empty() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppAdsiProg::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }
}
