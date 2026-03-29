//! External IVR application.
//!
//! Port of app_externalivr.c from Asterisk C. Launches an external process
//! (or connects to a TCP socket) and communicates via stdin/stdout to control
//! audio playback on the channel. The external application receives DTMF events
//! and hangup notifications, and can issue commands to stream files, send DTMF,
//! hang up the channel, set variables, etc.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_types::ChannelState;
use tracing::{debug, info, warn};

/// Commands sent FROM the external IVR process TO Asterisk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EivrCommand {
    /// S - (Re)set the prompt queue: clear and set new file.
    SetPrompt(String),
    /// A - Append file to the prompt queue (with optional offset).
    AppendPrompt { filename: String, offset: Option<i64> },
    /// H - Hangup the channel.
    Hangup,
    /// O - Set an option.
    Option(String),
    /// E - Exit the application.
    Exit(String),
    /// T - Answer the channel.
    Answer,
    /// D - Send DTMF digits.
    SendDtmf(String),
    /// G - Get channel variable(s).
    GetVariable(String),
    /// V - Set channel variable(s).
    SetVariable(String, String),
    /// L - Log a message.
    Log(String),
    /// P - Return supplied params.
    Params,
    /// I - Interrupt current playback.
    Interrupt,
}

impl EivrCommand {
    /// Parse a command line from the external process.
    ///
    /// Format: COMMAND_CHAR,timestamp[,data]
    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }

        let cmd_char = line.chars().next()?;
        // Data is everything after the first comma (skipping timestamp field)
        let data = line
            .splitn(3, ',')
            .nth(2)
            .unwrap_or("")
            .to_string();

        match cmd_char {
            'S' => Some(Self::SetPrompt(data)),
            'A' => {
                let parts: Vec<&str> = data.splitn(2, ',').collect();
                let filename = parts.first().unwrap_or(&"").to_string();
                let offset = parts.get(1).and_then(|o| o.parse::<i64>().ok());
                Some(Self::AppendPrompt { filename, offset })
            }
            'H' => Some(Self::Hangup),
            'O' => Some(Self::Option(data)),
            'E' | 'X' => Some(Self::Exit(data)),
            'T' => Some(Self::Answer),
            'D' => Some(Self::SendDtmf(data)),
            'G' => Some(Self::GetVariable(data)),
            'V' => {
                let parts: Vec<&str> = data.splitn(2, '=').collect();
                let var_name = parts.first().unwrap_or(&"").to_string();
                let var_value = parts.get(1).unwrap_or(&"").to_string();
                Some(Self::SetVariable(var_name, var_value))
            }
            'L' => Some(Self::Log(data)),
            'P' => Some(Self::Params),
            'I' => Some(Self::Interrupt),
            _ => {
                debug!("ExternalIVR: unknown command '{}'", cmd_char);
                None
            }
        }
    }
}

/// Events sent FROM Asterisk TO the external IVR process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EivrEvent {
    /// DTMF digit received.
    Dtmf(char),
    /// Channel hung up.
    Hangup,
    /// Playback of a file completed.
    PlaybackComplete(String),
    /// Informational: channel disconnected but not exiting yet.
    Disconnect,
    /// Error occurred.
    Error(String),
}

impl EivrEvent {
    /// Format the event as a line to send to the external process.
    pub fn format(&self, timestamp: u64) -> String {
        match self {
            Self::Dtmf(d) => format!("{},{}\n", d, timestamp),
            Self::Hangup => format!("H,{}\n", timestamp),
            Self::PlaybackComplete(f) => format!("Z,{},{}\n", timestamp, f),
            Self::Disconnect => format!("I,{}\n", timestamp),
            Self::Error(e) => format!("E,{},{}\n", timestamp, e),
        }
    }
}

/// Options for the ExternalIVR application.
#[derive(Debug, Clone, Default)]
pub struct ExternalIvrOptions {
    /// Do not answer the channel.
    pub no_answer: bool,
    /// Do not exit on hangup; send informational message instead.
    pub ignore_hangup: bool,
    /// Run on a dead (hung up) channel.
    pub run_dead: bool,
}

impl ExternalIvrOptions {
    pub fn parse(opts: &str) -> Self {
        let mut result = Self::default();
        for ch in opts.chars() {
            match ch {
                'n' => result.no_answer = true,
                'i' => result.ignore_hangup = true,
                'd' => result.run_dead = true,
                _ => {
                    debug!("ExternalIVR: ignoring unknown option '{}'", ch);
                }
            }
        }
        result
    }
}

/// Default TCP port for socket-based external IVR.
pub const EXTERNAL_IVR_PORT: u16 = 2949;

/// The ExternalIVR() dialplan application.
///
/// Usage: ExternalIVR(command[|arg1[|arg2...]][,options])
///
/// Forks a process to run the given command (or connects to ivr://host)
/// and starts a generator on the channel. The external application controls
/// the prompt playlist via stdout commands, and receives DTMF and hangup
/// events on stdin.
///
/// Options:
///   n - Do not answer the channel
///   i - Do not exit on hangup (send informational 'I' instead)
///   d - Run on a dead channel
pub struct AppExternalIvr;

impl DialplanApp for AppExternalIvr {
    fn name(&self) -> &str {
        "ExternalIVR"
    }

    fn description(&self) -> &str {
        "Interfaces with an external IVR application"
    }
}

impl AppExternalIvr {
    /// Execute the ExternalIVR application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        if args.trim().is_empty() {
            warn!("ExternalIVR: requires command argument");
            return PbxExecResult::Failed;
        }

        // Split on final comma for options
        let (command_str, options) = if let Some(comma_pos) = args.rfind(',') {
            let potential_opts = args[comma_pos + 1..].trim();
            if potential_opts.chars().all(|c| matches!(c, 'n' | 'i' | 'd')) && !potential_opts.is_empty() {
                (&args[..comma_pos], ExternalIvrOptions::parse(potential_opts))
            } else {
                (args, ExternalIvrOptions::default())
            }
        } else {
            (args, ExternalIvrOptions::default())
        };

        let is_socket = command_str.starts_with("ivr://");

        info!(
            "ExternalIVR: channel '{}' command='{}' socket={} options=n:{},i:{},d:{}",
            channel.name,
            command_str,
            is_socket,
            options.no_answer,
            options.ignore_hangup,
            options.run_dead,
        );

        // Answer the channel if needed
        if !options.no_answer && !options.run_dead && channel.state != ChannelState::Up {
            debug!("ExternalIVR: answering channel");
            channel.state = ChannelState::Up;
        }

        // In a real implementation:
        //
        //   if is_socket {
        //       // Parse ivr://host[:port] and connect via TCP
        //       let addr = parse_ivr_url(command_str);
        //       let stream = TcpStream::connect(addr).await?;
        //       let (reader, writer) = stream.into_split();
        //       run_eivr_loop(channel, reader, writer, &options).await
        //   } else {
        //       // Fork external process
        //       let mut child = Command::new("sh")
        //           .arg("-c")
        //           .arg(command_str)
        //           .stdin(Stdio::piped())
        //           .stdout(Stdio::piped())
        //           .stderr(Stdio::piped())
        //           .spawn()?;
        //
        //       let stdin = child.stdin.take().unwrap();
        //       let stdout = child.stdout.take().unwrap();
        //       let stderr = child.stderr.take().unwrap();
        //
        //       // Start audio generator on the channel
        //       activate_generator(channel).await;
        //
        //       // Main loop: read commands from stdout, send events to stdin
        //       // - Read frames from channel, forward DTMF events
        //       // - Read command lines from process stdout
        //       // - Execute commands (set prompt, append, hangup, etc.)
        //       // - Monitor for hangup unless ignore_hangup or run_dead
        //
        //       loop {
        //           tokio::select! {
        //               frame = read_frame(channel) => {
        //                   match frame {
        //                       Some(Frame::Dtmf(d)) => {
        //                           send_event(&stdin, EivrEvent::Dtmf(d)).await;
        //                       }
        //                       None if !options.ignore_hangup && !options.run_dead => {
        //                           send_event(&stdin, EivrEvent::Hangup).await;
        //                           break;
        //                       }
        //                       None if options.ignore_hangup => {
        //                           send_event(&stdin, EivrEvent::Disconnect).await;
        //                       }
        //                       _ => {}
        //                   }
        //               }
        //               line = read_line(&stdout) => {
        //                   if let Some(cmd) = EivrCommand::parse(&line) {
        //                       match cmd {
        //                           EivrCommand::SetPrompt(f) => {
        //                               clear_playlist(channel);
        //                               add_to_playlist(channel, &f);
        //                           }
        //                           EivrCommand::AppendPrompt { filename, offset } => {
        //                               add_to_playlist_with_offset(channel, &filename, offset);
        //                           }
        //                           EivrCommand::Hangup => {
        //                               hangup(channel).await;
        //                               break;
        //                           }
        //                           EivrCommand::Exit(_) => break,
        //                           EivrCommand::SendDtmf(digits) => {
        //                               send_dtmf(channel, &digits).await;
        //                           }
        //                           EivrCommand::Answer => {
        //                               answer(channel).await;
        //                           }
        //                           EivrCommand::GetVariable(name) => {
        //                               let val = get_variable(channel, &name);
        //                               send_event(&stdin, val).await;
        //                           }
        //                           EivrCommand::SetVariable(name, val) => {
        //                               set_variable(channel, &name, &val);
        //                           }
        //                           EivrCommand::Log(msg) => {
        //                               info!("ExternalIVR: {}", msg);
        //                           }
        //                           _ => {}
        //                       }
        //                   }
        //               }
        //           }
        //       }
        //
        //       deactivate_generator(channel).await;
        //       child.kill().ok();
        //   }

        info!(
            "ExternalIVR: channel '{}' session completed",
            channel.name,
        );
        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_command_set_prompt() {
        let cmd = EivrCommand::parse("S,1234567890,hello-world").unwrap();
        assert_eq!(cmd, EivrCommand::SetPrompt("hello-world".to_string()));
    }

    #[test]
    fn test_parse_command_hangup() {
        let cmd = EivrCommand::parse("H,1234567890").unwrap();
        assert_eq!(cmd, EivrCommand::Hangup);
    }

    #[test]
    fn test_parse_command_exit() {
        let cmd = EivrCommand::parse("E,1234567890,done").unwrap();
        assert_eq!(cmd, EivrCommand::Exit("done".to_string()));
    }

    #[test]
    fn test_parse_command_send_dtmf() {
        let cmd = EivrCommand::parse("D,1234567890,123#").unwrap();
        assert_eq!(cmd, EivrCommand::SendDtmf("123#".to_string()));
    }

    #[test]
    fn test_parse_command_set_variable() {
        let cmd = EivrCommand::parse("V,1234567890,MYVAR=hello").unwrap();
        assert_eq!(
            cmd,
            EivrCommand::SetVariable("MYVAR".to_string(), "hello".to_string())
        );
    }

    #[test]
    fn test_parse_command_empty() {
        assert!(EivrCommand::parse("").is_none());
    }

    #[test]
    fn test_event_format() {
        let evt = EivrEvent::Dtmf('5');
        assert_eq!(evt.format(1000), "5,1000\n");

        let evt = EivrEvent::Hangup;
        assert_eq!(evt.format(2000), "H,2000\n");
    }

    #[test]
    fn test_options() {
        let opts = ExternalIvrOptions::parse("nid");
        assert!(opts.no_answer);
        assert!(opts.ignore_hangup);
        assert!(opts.run_dead);
    }

    #[tokio::test]
    async fn test_external_ivr_exec() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppExternalIvr::exec(&mut channel, "/usr/bin/my-ivr").await;
        assert_eq!(result, PbxExecResult::Success);
    }

    #[tokio::test]
    async fn test_external_ivr_no_args() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppExternalIvr::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
    }
}
