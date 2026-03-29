//! IVR demo skeleton application.
//!
//! Port of app_ivrdemo.c from Asterisk C. A simple IVR (Interactive
//! Voice Response) demonstration that shows how to build an IVR tree
//! using the Asterisk IVR engine.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use tracing::info;

/// An IVR menu option.
#[derive(Debug, Clone)]
pub struct IvrOption {
    /// DTMF digit that selects this option.
    pub digit: char,
    /// Description / prompt filename for this option.
    pub description: String,
    /// Action to take: play a file, jump to submenu, or exit.
    pub action: IvrAction,
}

/// Actions that an IVR option can trigger.
#[derive(Debug, Clone)]
pub enum IvrAction {
    /// Play a sound file.
    Playback(String),
    /// Jump to a named submenu.
    SubMenu(String),
    /// Exit the IVR.
    Exit,
}

/// A simple IVR menu.
#[derive(Debug, Clone)]
pub struct IvrMenu {
    /// Menu name/identifier.
    pub name: String,
    /// Prompt to play when entering this menu.
    pub prompt: String,
    /// Available options.
    pub options: Vec<IvrOption>,
}

/// The IVRDemo() dialplan application.
///
/// Usage: IVRDemo()
///
/// Runs a simple demonstration IVR that plays a welcome message and
/// offers options via DTMF. This is a skeleton/example application.
pub struct AppIvrDemo;

impl DialplanApp for AppIvrDemo {
    fn name(&self) -> &str {
        "IVRDemo"
    }

    fn description(&self) -> &str {
        "IVR Demo application skeleton"
    }
}

impl AppIvrDemo {
    /// Execute the IVRDemo application.
    pub async fn exec(channel: &mut Channel, _args: &str) -> PbxExecResult {
        info!("IVRDemo: channel '{}' starting demo IVR", channel.name);

        // Demo IVR structure (from app_ivrdemo.c):
        // Main menu: "demo-congrats" prompt
        //   1 -> play "digits/1"
        //   2 -> play "digits/2"
        //   * -> submenu:
        //        1 -> play "digits/1"
        //        * -> return to main
        //   # -> exit

        // In a real implementation:
        // ast_ivr_run(channel, &demo_ivr_tree)

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ivr_menu() {
        let menu = IvrMenu {
            name: "main".to_string(),
            prompt: "welcome".to_string(),
            options: vec![
                IvrOption {
                    digit: '1',
                    description: "Option 1".to_string(),
                    action: IvrAction::Playback("digits/1".to_string()),
                },
                IvrOption {
                    digit: '#',
                    description: "Exit".to_string(),
                    action: IvrAction::Exit,
                },
            ],
        };
        assert_eq!(menu.options.len(), 2);
    }

    #[tokio::test]
    async fn test_ivrdemo_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppIvrDemo::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Success);
    }
}
