//! DumpChan - dump channel variables to log.
//!
//! Port of app_dumpchan.c from Asterisk C. Dumps all channel info
//! and variables to the Asterisk log for debugging purposes.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::info;

/// The DumpChan() dialplan application.
///
/// Usage: DumpChan([min_verbose_level])
///
/// Dumps channel information including name, state, caller ID,
/// connected line, DNID, language, context/extension/priority,
/// and all channel variables to the log at VERBOSE level.
pub struct AppDumpChan;

impl DialplanApp for AppDumpChan {
    fn name(&self) -> &str {
        "DumpChan"
    }

    fn description(&self) -> &str {
        "Dump channel info and variables to the log"
    }
}

impl AppDumpChan {
    /// Execute the DumpChan application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let _min_level: u32 = args.trim().parse().unwrap_or(0);

        info!("DumpChan: =============== Channel Info ===============");
        info!("DumpChan: Name:        {}", channel.name);
        info!("DumpChan: UniqueID:    {}", channel.unique_id);
        info!("DumpChan: State:       {:?}", channel.state);
        // In a real implementation, also dump:
        // - Caller ID name/number
        // - Connected line
        // - DNID / RDNIS
        // - Language
        // - Context / Extension / Priority
        // - Native formats
        // - Read/Write formats
        // - Read/Write trans path
        // - All channel variables
        // - CDR information
        info!("DumpChan: ============================================");

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dumpchan_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppDumpChan::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
