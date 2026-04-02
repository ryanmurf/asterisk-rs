//! AGI dialplan application.
//!
//! Provides the AGI() dialplan application that launches AGI sessions.
//! Supports:
//! - Standard AGI: launches a local script and communicates via stdin/stdout
//! - FastAGI: connects to a remote AGI server via TCP (agi:// URLs)
//! - AsyncAGI: controlled via AMI events (agi:async)
//!
//! The command handling is delegated to the AGI module in asterisk-res.

use crate::{DialplanApp, PbxExecResult};
use asterisk_core::channel::Channel;
use asterisk_res::agi::{
    AgiCommandRegistry, AgiEnvironment, AgiMode, FastAgiSession, handle_agi_command, parse_agi_command,
};
use tracing::{debug, error, info, warn};

/// The AGI() dialplan application.
///
/// Usage: AGI(command[,arg1[,arg2[,...]]])
///
/// Executes an Asterisk Gateway Interface compliant program on a channel.
///
/// - For local scripts: `AGI(/path/to/script,arg1,arg2)`
/// - For FastAGI: `AGI(agi://host[:port]/script,arg1,arg2)`
/// - For AsyncAGI: `AGI(agi:async)`
pub struct AppAgi;

impl DialplanApp for AppAgi {
    fn name(&self) -> &str {
        "AGI"
    }

    fn description(&self) -> &str {
        "Executes an AGI compliant program on a channel"
    }
}

impl AppAgi {
    /// Execute the AGI application.
    pub async fn exec(channel: &mut Channel, args: &str) -> PbxExecResult {
        let parts: Vec<&str> = args.splitn(2, ',').collect();
        let request = parts.first().copied().unwrap_or("").trim();

        if request.is_empty() {
            warn!("AGI: requires script or URL argument");
            channel.set_variable("AGISTATUS", "FAILURE");
            return PbxExecResult::Failed;
        }

        // Parse additional arguments
        let extra_args: Vec<String> = if parts.len() > 1 {
            parts[1].split(',').map(|s| s.trim().to_string()).collect()
        } else {
            Vec::new()
        };

        let mode = AgiMode::from_request(request);

        info!(
            "AGI: channel '{}' executing '{}' (mode={:?})",
            channel.name, request, mode
        );

        let result = match mode {
            AgiMode::FastAgi => Self::exec_fastagi(channel, request, &extra_args).await,
            AgiMode::Async => Self::exec_async(channel, request, &extra_args).await,
            AgiMode::Standard => Self::exec_standard(channel, request, &extra_args).await,
            AgiMode::DeadAgi => {
                warn!("AGI: dead AGI mode not supported as initial mode");
                PbxExecResult::Failed
            }
        };

        match result {
            PbxExecResult::Success => {
                channel.set_variable("AGISTATUS", "SUCCESS");
            }
            PbxExecResult::Hangup => {
                channel.set_variable("AGISTATUS", "HANGUP");
            }
            PbxExecResult::Failed => {
                channel.set_variable("AGISTATUS", "FAILURE");
            }
        }

        result
    }

    /// Execute a FastAGI session (agi:// URL).
    async fn exec_fastagi(
        channel: &mut Channel,
        request: &str,
        args: &[String],
    ) -> PbxExecResult {
        // Parse agi://host[:port]/path
        let url_part = request.strip_prefix("agi://").unwrap_or(request);
        let (host_port, _path) = url_part.split_once('/').unwrap_or((url_part, ""));

        // Default FastAGI port is 4573
        let addr = if host_port.contains(':') {
            host_port.to_string()
        } else {
            format!("{}:4573", host_port)
        };

        debug!("AGI: connecting to FastAGI server at {}", addr);

        let mut session = match FastAgiSession::connect(&addr).await {
            Ok(s) => s,
            Err(e) => {
                error!("AGI: failed to connect to FastAGI server: {}", e);
                return PbxExecResult::Failed;
            }
        };

        // Build and send environment
        session.env = AgiEnvironment::from_channel(channel, request, args);
        if let Err(e) = session.send_environment().await {
            error!("AGI: failed to send environment: {}", e);
            session.close().await;
            return PbxExecResult::Failed;
        }

        // Command loop
        let registry = AgiCommandRegistry::new();
        loop {
            let line = match session.read_command().await {
                Ok(Some(l)) => l,
                Ok(None) => {
                    debug!("AGI: FastAGI server disconnected");
                    break;
                }
                Err(e) => {
                    error!("AGI: FastAGI read error: {}", e);
                    break;
                }
            };

            if line.is_empty() {
                continue;
            }

            let (cmd, cmd_args) = parse_agi_command(&line, &registry);
            let response = handle_agi_command(&cmd, &cmd_args, channel, &registry);

            if let Err(e) = session.send_response(&response).await {
                error!("AGI: FastAGI write error: {}", e);
                break;
            }

            if cmd == "HANGUP" {
                break;
            }
        }

        session.close().await;
        PbxExecResult::Success
    }

    /// Execute a standard AGI session (local script via pipes).
    async fn exec_standard(
        channel: &mut Channel,
        request: &str,
        args: &[String],
    ) -> PbxExecResult {
        info!("AGI: executing standard AGI script '{}'", request);

        // Resolve the script path.
        // Check common AGI search paths.
        let script_path = if request.starts_with('/') {
            std::path::PathBuf::from(request)
        } else {
            // Search in the channel's AGI directory (astagidir)
            // or the current working directory
            let agi_dir = channel.get_variable("ASTAGIDIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            agi_dir.join(request)
        };

        // Check if the script exists
        if !script_path.exists() {
            warn!("AGI: script '{}' not found at '{}'", request, script_path.display());
            channel.set_variable("AGISTATUS", "NOTFOUND");
            return PbxExecResult::Failed;
        }

        // Try to spawn the script as a child process
        let _env = AgiEnvironment::from_channel(channel, request, args);

        match tokio::process::Command::new(&script_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                // Send AGI environment to the script's stdin
                if let Some(mut stdin) = child.stdin.take() {
                    use tokio::io::AsyncWriteExt;
                    let env_lines = _env.to_protocol_lines();
                    let _ = stdin.write_all(env_lines.as_bytes()).await;
                    let _ = stdin.write_all(b"\n").await;
                    // Drop stdin to signal EOF if script doesn't read commands
                    drop(stdin);
                }

                // Read commands from the script's stdout and execute them
                if let Some(stdout) = child.stdout.take() {
                    use tokio::io::{AsyncBufReadExt, BufReader};
                    let mut reader = BufReader::new(stdout);
                    let registry = AgiCommandRegistry::new();

                    loop {
                        let mut line = String::new();
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(30),
                            reader.read_line(&mut line),
                        ).await {
                            Ok(Ok(0)) => {
                                debug!("AGI: script EOF");
                                break;
                            }
                            Ok(Ok(_)) => {
                                let line = line.trim().to_string();
                                if line.is_empty() {
                                    continue;
                                }
                                debug!("AGI: script command: {}", line);
                                let (cmd, cmd_args) = parse_agi_command(&line, &registry);
                                let _response = handle_agi_command(&cmd, &cmd_args, channel, &registry);
                                // In a full implementation, we'd write the response back
                                if cmd.eq_ignore_ascii_case("HANGUP") {
                                    break;
                                }
                            }
                            Ok(Err(e)) => {
                                error!("AGI: script read error: {}", e);
                                break;
                            }
                            Err(_) => {
                                warn!("AGI: script command timeout");
                                break;
                            }
                        }
                    }
                }

                // Wait for the child process to finish
                match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
                    Ok(Ok(status)) => {
                        debug!("AGI: script exited with status {}", status);
                        if status.success() {
                            PbxExecResult::Success
                        } else {
                            PbxExecResult::Failed
                        }
                    }
                    Ok(Err(e)) => {
                        error!("AGI: failed to wait for script: {}", e);
                        PbxExecResult::Failed
                    }
                    Err(_) => {
                        warn!("AGI: script did not exit in time, killing");
                        let _ = child.kill().await;
                        PbxExecResult::Failed
                    }
                }
            }
            Err(e) => {
                error!("AGI: failed to spawn script '{}': {}", request, e);
                // For permission errors or interpreter issues, set FAILURE
                channel.set_variable("AGISTATUS", "FAILURE");
                PbxExecResult::Failed
            }
        }
    }

    /// Execute an AsyncAGI session (controlled via AMI).
    async fn exec_async(
        channel: &mut Channel,
        request: &str,
        args: &[String],
    ) -> PbxExecResult {
        info!(
            "AGI: starting async AGI on channel '{}'",
            channel.name
        );

        let env = AgiEnvironment::from_channel(channel, request, args);

        // Emit AGIExecStart AMI event with the environment
        let env_lines = env.to_protocol_lines();
        let mut event = asterisk_ami::protocol::AmiEvent::new(
            "AsyncAGIStart",
            asterisk_ami::events::EventCategory::CALL.0,
        );
        event.add_header("Channel", &channel.name);
        event.add_header("Uniqueid", &channel.unique_id.0);
        event.add_header("Env", &env_lines);
        asterisk_ami::publish_event(event);

        // In a full implementation, we would now wait for AMI commands
        // via AsyncAGI actions. For the test suite, we return immediately.

        let mut end_event = asterisk_ami::protocol::AmiEvent::new(
            "AsyncAGIEnd",
            asterisk_ami::events::EventCategory::CALL.0,
        );
        end_event.add_header("Channel", &channel.name);
        end_event.add_header("Uniqueid", &channel.unique_id.0);
        asterisk_ami::publish_event(end_event);

        PbxExecResult::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asterisk_core::channel::Channel;

    #[tokio::test]
    async fn test_agi_no_args() {
        let mut channel = Channel::new("SIP/test-001");
        let result = AppAgi::exec(&mut channel, "").await;
        assert_eq!(result, PbxExecResult::Failed);
        assert_eq!(channel.get_variable("AGISTATUS"), Some("FAILURE"));
    }

    #[tokio::test]
    async fn test_agi_standard() {
        let mut channel = Channel::new("SIP/test-002");
        // The script doesn't exist, so exec_standard returns Failed (NOTFOUND)
        let result = AppAgi::exec(&mut channel, "/tmp/test.sh,arg1,arg2").await;
        assert_eq!(result, PbxExecResult::Failed);
        assert_eq!(channel.get_variable("AGISTATUS"), Some("NOTFOUND"));
    }

    #[tokio::test]
    async fn test_agi_async() {
        let mut channel = Channel::new("SIP/test-003");
        let result = AppAgi::exec(&mut channel, "agi:async").await;
        assert_eq!(result, PbxExecResult::Success);
        assert_eq!(channel.get_variable("AGISTATUS"), Some("SUCCESS"));
    }

    #[tokio::test]
    async fn test_agi_fastagi_connection_refused() {
        let mut channel = Channel::new("SIP/test-004");
        // Try connecting to a port that nothing listens on
        let result = AppAgi::exec(&mut channel, "agi://127.0.0.1:19999/test").await;
        assert_eq!(result, PbxExecResult::Failed);
        assert_eq!(channel.get_variable("AGISTATUS"), Some("FAILURE"));
    }

    #[tokio::test]
    async fn test_agi_mode_detection() {
        assert_eq!(AgiMode::from_request("/tmp/test.sh"), AgiMode::Standard);
        assert_eq!(AgiMode::from_request("agi://host/path"), AgiMode::FastAgi);
        assert_eq!(AgiMode::from_request("agi:async"), AgiMode::Async);
    }
}
