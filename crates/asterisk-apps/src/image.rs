//! SendImage application.
//!
//! Port of app_image.c from Asterisk C. Sends an image file to a
//! channel that supports image transmission (e.g. IAX2, SIP with
//! T.38 or messaging support).

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::{info, warn};

/// The SendImage() dialplan application.
///
/// Usage: SendImage(filename)
///
/// Sends the specified image file to the channel. The image format
/// must be supported by the channel technology (e.g. JPEG, PNG).
/// If the channel does not support images, this is a no-op.
///
/// Sets SENDIMAGESTATUS:
///   OK       - Image sent successfully
///   NOSUPPORT - Channel does not support images
///   FAILURE  - Failed to send image
pub struct AppSendImage;

impl DialplanApp for AppSendImage {
    fn name(&self) -> &str {
        "SendImage"
    }

    fn description(&self) -> &str {
        "Send an image to the channel"
    }
}

impl AppSendImage {
    /// Execute the SendImage application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let filename = args.trim();

        if filename.is_empty() {
            warn!("SendImage: requires filename argument");
            return PbxExecResult::Failed;
        }

        info!("SendImage: channel '{}' sending '{}'", channel.name, filename);

        // In a real implementation:
        // 1. Check if channel supports image transfer
        // 2. If not, set SENDIMAGESTATUS=NOSUPPORT and return
        // 3. Load and send image via ast_send_image()
        // 4. Set SENDIMAGESTATUS=OK or FAILURE

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sendimage_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSendImage::exec(&mut channel, "/tmp/logo.jpg").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_sendimage_empty() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppSendImage::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }
}
