//! muted - Desktop mute daemon for Asterisk
//!
//! Monitors Asterisk call state via the AMI (Asterisk Manager Interface) and
//! automatically mutes/unmutes the desktop audio when calls are active. This
//! is useful for softphone setups where you want your music or other audio
//! to be muted when a call comes in.
//!
//! This is a conceptual port - the original C code interfaced directly with
//! OSS/ALSA mixer APIs. This Rust version uses a platform-agnostic approach,
//! shelling out to system commands for audio control.

use clap::Parser;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::Command;
use std::thread;
use std::time::Duration;

/// Desktop audio mute daemon for Asterisk call state
#[derive(Parser, Debug)]
#[command(
    name = "muted",
    about = "Mute/unmute desktop audio based on Asterisk call state"
)]
struct Args {
    /// Asterisk Manager Interface host
    #[arg(short = 'H', long, default_value = "127.0.0.1")]
    host: String,

    /// AMI port
    #[arg(short, long, default_value = "5038")]
    port: u16,

    /// AMI username
    #[arg(short, long)]
    username: String,

    /// AMI secret (password)
    #[arg(short, long)]
    secret: String,

    /// Run in foreground (don't daemonize)
    #[arg(short, long)]
    foreground: bool,

    /// Mute command to execute when a call starts
    #[arg(long, default_value = "")]
    mute_cmd: String,

    /// Unmute command to execute when all calls end
    #[arg(long, default_value = "")]
    unmute_cmd: String,

    /// Reconnect delay in seconds
    #[arg(long, default_value = "5")]
    reconnect_delay: u64,
}

/// Audio control abstraction
struct AudioController {
    mute_cmd: String,
    unmute_cmd: String,
    is_muted: bool,
}

impl AudioController {
    fn new(mute_cmd: String, unmute_cmd: String) -> Self {
        Self {
            mute_cmd,
            unmute_cmd,
            is_muted: false,
        }
    }

    /// Get the default mute command for the current platform.
    fn default_mute_cmd() -> &'static str {
        if cfg!(target_os = "macos") {
            "osascript -e 'set volume with output muted'"
        } else {
            "amixer set Master mute"
        }
    }

    /// Get the default unmute command for the current platform.
    fn default_unmute_cmd() -> &'static str {
        if cfg!(target_os = "macos") {
            "osascript -e 'set volume without output muted'"
        } else {
            "amixer set Master unmute"
        }
    }

    fn mute(&mut self) {
        if self.is_muted {
            return;
        }
        let cmd = if self.mute_cmd.is_empty() {
            Self::default_mute_cmd().to_string()
        } else {
            self.mute_cmd.clone()
        };

        eprintln!("Muting audio");
        if let Err(e) = Command::new("sh").args(["-c", &cmd]).status() {
            eprintln!("Failed to mute: {e}");
        }
        self.is_muted = true;
    }

    fn unmute(&mut self) {
        if !self.is_muted {
            return;
        }
        let cmd = if self.unmute_cmd.is_empty() {
            Self::default_unmute_cmd().to_string()
        } else {
            self.unmute_cmd.clone()
        };

        eprintln!("Unmuting audio");
        if let Err(e) = Command::new("sh").args(["-c", &cmd]).status() {
            eprintln!("Failed to unmute: {e}");
        }
        self.is_muted = false;
    }
}

/// AMI event types we care about
#[derive(Debug)]
enum AmiEvent {
    /// A new channel has been created (call starting)
    Newchannel,
    /// A channel has been hung up (call ending)
    Hangup,
    /// Some other event we don't track
    Other,
}

/// Parse an AMI event from a block of header lines.
fn parse_ami_event(lines: &[String]) -> AmiEvent {
    for line in lines {
        if line.starts_with("Event: ") {
            let event_name = line.trim_start_matches("Event: ").trim();
            return match event_name {
                "Newchannel" => AmiEvent::Newchannel,
                "Hangup" => AmiEvent::Hangup,
                _ => AmiEvent::Other,
            };
        }
    }
    AmiEvent::Other
}

/// Connect to AMI and process events.
fn ami_session(
    host: &str,
    port: u16,
    username: &str,
    secret: &str,
    audio: &mut AudioController,
) -> Result<(), String> {
    let addr = format!("{host}:{port}");
    let stream = TcpStream::connect_timeout(
        &addr.parse().map_err(|e| format!("Invalid address: {e}"))?,
        Duration::from_secs(10),
    )
    .map_err(|e| format!("Connection failed: {e}"))?;

    let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));

    let mut writer = stream
        .try_clone()
        .map_err(|e| format!("Clone failed: {e}"))?;
    let reader = BufReader::new(stream);

    eprintln!("Connected to AMI at {addr}");

    // Send login
    let login = format!(
        "Action: Login\r\nUsername: {username}\r\nSecret: {secret}\r\n\r\n"
    );
    writer
        .write_all(login.as_bytes())
        .map_err(|e| format!("Login write failed: {e}"))?;

    // Track active channel count
    let mut active_channels: u32 = 0;

    // Read events
    let mut event_lines: Vec<String> = Vec::new();

    for line_result in reader.lines() {
        let line = line_result.map_err(|e| format!("Read error: {e}"))?;

        if line.is_empty() {
            // End of event block
            if !event_lines.is_empty() {
                match parse_ami_event(&event_lines) {
                    AmiEvent::Newchannel => {
                        active_channels += 1;
                        eprintln!("Call started (active: {active_channels})");
                        audio.mute();
                    }
                    AmiEvent::Hangup => {
                        active_channels = active_channels.saturating_sub(1);
                        eprintln!("Call ended (active: {active_channels})");
                        if active_channels == 0 {
                            audio.unmute();
                        }
                    }
                    AmiEvent::Other => {}
                }
                event_lines.clear();
            }
        } else {
            event_lines.push(line);
        }
    }

    // Connection lost - unmute
    audio.unmute();
    Err("Connection lost".to_string())
}

fn main() {
    let args = Args::parse();

    eprintln!("muted - Asterisk desktop mute daemon");
    eprintln!("Connecting to {}:{}", args.host, args.port);

    let mut audio = AudioController::new(args.mute_cmd.clone(), args.unmute_cmd.clone());

    // Main reconnection loop
    loop {
        match ami_session(
            &args.host,
            args.port,
            &args.username,
            &args.secret,
            &mut audio,
        ) {
            Ok(()) => break,
            Err(e) => {
                eprintln!("AMI session error: {e}");
                eprintln!(
                    "Reconnecting in {} seconds...",
                    args.reconnect_delay
                );
                // Ensure we're unmuted on disconnect
                audio.unmute();
                thread::sleep(Duration::from_secs(args.reconnect_delay));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ami_event_newchannel() {
        let lines = vec![
            "Event: Newchannel".to_string(),
            "Channel: SIP/100-00000001".to_string(),
        ];
        assert!(matches!(parse_ami_event(&lines), AmiEvent::Newchannel));
    }

    #[test]
    fn test_parse_ami_event_hangup() {
        let lines = vec![
            "Event: Hangup".to_string(),
            "Channel: SIP/100-00000001".to_string(),
        ];
        assert!(matches!(parse_ami_event(&lines), AmiEvent::Hangup));
    }

    #[test]
    fn test_parse_ami_event_other() {
        let lines = vec!["Event: PeerStatus".to_string()];
        assert!(matches!(parse_ami_event(&lines), AmiEvent::Other));
    }

    #[test]
    fn test_parse_ami_event_empty() {
        let lines: Vec<String> = vec![];
        assert!(matches!(parse_ami_event(&lines), AmiEvent::Other));
    }

    #[test]
    fn test_audio_controller_state() {
        let mut ctrl = AudioController::new(
            "echo muted".to_string(),
            "echo unmuted".to_string(),
        );
        assert!(!ctrl.is_muted);

        ctrl.mute();
        assert!(ctrl.is_muted);

        ctrl.unmute();
        assert!(!ctrl.is_muted);
    }
}
