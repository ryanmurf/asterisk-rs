//! Asterisk Gateway Interface (AGI) resource module.
//!
//! Port of `res/res_agi.c`. Provides the AGI protocol implementation for
//! controlling Asterisk dialplan execution from external scripts or processes.
//! Supports standard AGI (stdin/stdout pipes), FastAGI (TCP sockets), and
//! AsyncAGI (AMI-driven control).

use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, Write as IoWrite};
use parking_lot::RwLock;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
use tokio::net::TcpStream;
use tracing::info;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum AgiError {
    #[error("AGI I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("AGI protocol error: {0}")]
    Protocol(String),
    #[error("AGI command not found: {0}")]
    CommandNotFound(String),
    #[error("AGI channel hung up")]
    Hangup,
    #[error("AGI invalid command: {0}")]
    InvalidCommand(String),
    #[error("AGI connection failed: {0}")]
    ConnectionFailed(String),
}

pub type AgiResult<T> = Result<T, AgiError>;

// ---------------------------------------------------------------------------
// AGI result (protocol response)
// ---------------------------------------------------------------------------

/// An AGI command response.
///
/// The AGI protocol returns responses in the format:
/// `<code> result=<value> [(<data>)]`
///
/// Common codes: 200 = success, 510 = invalid command, 520 = usage error.
#[derive(Debug, Clone)]
pub struct AgiResponse {
    /// Status code (200 = success, 510 = invalid, 520 = usage).
    pub code: u16,
    /// Result value (from `result=<value>`).
    pub result: String,
    /// Optional additional data (in parentheses).
    pub data: Option<String>,
}

impl AgiResponse {
    /// Create a successful response with a result value.
    pub fn success(result: &str) -> Self {
        Self {
            code: 200,
            result: result.to_string(),
            data: None,
        }
    }

    /// Create a successful response with result and data.
    pub fn success_with_data(result: &str, data: &str) -> Self {
        Self {
            code: 200,
            result: result.to_string(),
            data: Some(data.to_string()),
        }
    }

    /// Create an "invalid command" error response (510).
    pub fn invalid_command(msg: &str) -> Self {
        Self {
            code: 510,
            result: msg.to_string(),
            data: None,
        }
    }

    /// Create a "usage" error response (520).
    pub fn usage(msg: &str) -> Self {
        Self {
            code: 520,
            result: msg.to_string(),
            data: None,
        }
    }

    /// Create a failure response (result=-1).
    pub fn failure() -> Self {
        Self {
            code: 200,
            result: "-1".to_string(),
            data: None,
        }
    }

    /// Format as an AGI protocol response line.
    pub fn to_protocol_line(&self) -> String {
        if let Some(ref data) = self.data {
            format!("{} result={} ({})\n", self.code, self.result, data)
        } else {
            format!("{} result={}\n", self.code, self.result)
        }
    }

    /// Parse an AGI response from a protocol line.
    pub fn parse(line: &str) -> AgiResult<Self> {
        let line = line.trim();
        if line.is_empty() {
            return Err(AgiError::Protocol("Empty response line".into()));
        }

        // Format: "<code> result=<value> [(<data>)]"
        let code_end = line.find(' ').ok_or_else(|| {
            AgiError::Protocol(format!("No space in response: {}", line))
        })?;
        let code: u16 = line[..code_end].parse().map_err(|_| {
            AgiError::Protocol(format!("Invalid response code: {}", &line[..code_end]))
        })?;

        let rest = &line[code_end + 1..];

        // Extract result=<value>.
        let result_prefix = "result=";
        let result_start = rest.find(result_prefix).map(|i| i + result_prefix.len());

        let (result, data) = if let Some(start) = result_start {
            let remainder = &rest[start..];
            // Check for parenthesized data.
            if let Some(paren_start) = remainder.find('(') {
                let result_val = remainder[..paren_start].trim().to_string();
                let after_paren = &remainder[paren_start + 1..];
                let data_val = after_paren
                    .find(')')
                    .map(|end| after_paren[..end].to_string());
                (result_val, data_val)
            } else {
                (remainder.trim().to_string(), None)
            }
        } else {
            (rest.trim().to_string(), None)
        };

        Ok(Self { code, result, data })
    }

    /// Whether this response indicates success.
    pub fn is_success(&self) -> bool {
        self.code == 200
    }

    /// Get the result as an integer.
    pub fn result_int(&self) -> Option<i64> {
        self.result.parse().ok()
    }
}

impl fmt::Display for AgiResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_protocol_line().trim_end())
    }
}

// ---------------------------------------------------------------------------
// AGI environment variables
// ---------------------------------------------------------------------------

/// AGI environment variables sent at the start of an AGI session.
///
/// These are passed as `agi_<name>: <value>` lines.
#[derive(Debug, Clone, Default)]
pub struct AgiEnvironment {
    /// The AGI request (script path or agi:// URL).
    pub request: String,
    /// Channel name.
    pub channel: String,
    /// Channel language.
    pub language: String,
    /// Channel type (e.g., "SIP").
    pub channel_type: String,
    /// Channel unique ID.
    pub uniqueid: String,
    /// Asterisk version.
    pub version: String,
    /// Caller ID number.
    pub callerid: String,
    /// Caller ID name.
    pub calleridname: String,
    /// Calling presentation.
    pub callingpres: String,
    /// Calling ANI.
    pub callingani2: String,
    /// Calling TON.
    pub callington: String,
    /// Calling TNS.
    pub callingtns: String,
    /// DNID (dialed number identification).
    pub dnid: String,
    /// RDNIS (redirecting number).
    pub rdnis: String,
    /// Current context.
    pub context: String,
    /// Current extension.
    pub extension: String,
    /// Current priority.
    pub priority: String,
    /// Whether enhanced mode is enabled.
    pub enhanced: String,
    /// Account code.
    pub accountcode: String,
    /// Thread ID.
    pub threadid: String,
    /// Additional arguments passed to the AGI script.
    pub arguments: Vec<String>,
    /// Raw key-value pairs for any agi_ variables.
    pub extra: HashMap<String, String>,
}

impl AgiEnvironment {
    /// Serialize to AGI protocol format (one `agi_<key>: <value>` line per
    /// variable, terminated by an empty line).
    pub fn to_protocol_lines(&self) -> String {
        let mut lines = String::new();
        let mut add = |key: &str, val: &str| {
            lines.push_str(&format!("agi_{}: {}\n", key, val));
        };

        add("request", &self.request);
        add("channel", &self.channel);
        add("language", &self.language);
        add("type", &self.channel_type);
        add("uniqueid", &self.uniqueid);
        add("version", &self.version);
        add("callerid", &self.callerid);
        add("calleridname", &self.calleridname);
        add("callingpres", &self.callingpres);
        add("callingani2", &self.callingani2);
        add("callington", &self.callington);
        add("callingtns", &self.callingtns);
        add("dnid", &self.dnid);
        add("rdnis", &self.rdnis);
        add("context", &self.context);
        add("extension", &self.extension);
        add("priority", &self.priority);
        add("enhanced", &self.enhanced);
        add("accountcode", &self.accountcode);
        add("threadid", &self.threadid);

        for (i, arg) in self.arguments.iter().enumerate() {
            add(&format!("arg_{}", i + 1), arg);
        }

        for (key, val) in &self.extra {
            add(key, val);
        }

        lines.push('\n'); // Empty line terminates environment.
        lines
    }

    /// Parse AGI environment variables from protocol lines.
    pub fn parse_lines(lines: &[String]) -> Self {
        let mut env = AgiEnvironment::default();
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                break;
            }
            if let Some(rest) = line.strip_prefix("agi_") {
                if let Some((key, value)) = rest.split_once(':') {
                    let key = key.trim();
                    let value = value.trim();
                    match key {
                        "request" => env.request = value.to_string(),
                        "channel" => env.channel = value.to_string(),
                        "language" => env.language = value.to_string(),
                        "type" => env.channel_type = value.to_string(),
                        "uniqueid" => env.uniqueid = value.to_string(),
                        "version" => env.version = value.to_string(),
                        "callerid" => env.callerid = value.to_string(),
                        "calleridname" => env.calleridname = value.to_string(),
                        "callingpres" => env.callingpres = value.to_string(),
                        "callingani2" => env.callingani2 = value.to_string(),
                        "callington" => env.callington = value.to_string(),
                        "callingtns" => env.callingtns = value.to_string(),
                        "dnid" => env.dnid = value.to_string(),
                        "rdnis" => env.rdnis = value.to_string(),
                        "context" => env.context = value.to_string(),
                        "extension" => env.extension = value.to_string(),
                        "priority" => env.priority = value.to_string(),
                        "enhanced" => env.enhanced = value.to_string(),
                        "accountcode" => env.accountcode = value.to_string(),
                        "threadid" => env.threadid = value.to_string(),
                        k if k.starts_with("arg_") => {
                            env.arguments.push(value.to_string());
                        }
                        _ => {
                            env.extra.insert(key.to_string(), value.to_string());
                        }
                    }
                }
            }
        }
        env
    }
}

// ---------------------------------------------------------------------------
// AGI session mode
// ---------------------------------------------------------------------------

/// The mode of an AGI session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgiMode {
    /// Standard AGI: communicates via stdin/stdout of a child process.
    Standard,
    /// Fast AGI: communicates via TCP socket.
    FastAgi,
    /// Async AGI: controlled via AMI events and actions.
    Async,
    /// Dead AGI: channel has hung up but AGI script is still running.
    DeadAgi,
}

impl AgiMode {
    /// Parse from the request URI scheme.
    pub fn from_request(request: &str) -> Self {
        if request.starts_with("agi://") {
            Self::FastAgi
        } else if request.starts_with("async:") || request.eq_ignore_ascii_case("agi:async") {
            Self::Async
        } else {
            Self::Standard
        }
    }
}

// ---------------------------------------------------------------------------
// AGI command definitions
// ---------------------------------------------------------------------------

/// An AGI command name paired with its description and expected arguments.
#[derive(Debug, Clone)]
pub struct AgiCommandDef {
    /// Command name (e.g., "ANSWER", "STREAM FILE").
    pub name: String,
    /// Brief synopsis of the command.
    pub synopsis: String,
    /// Usage/syntax string.
    pub usage: String,
}

/// Registry of known AGI commands.
///
/// In the C source, each AGI command is registered with a handler function.
/// Here we define the command set declaratively.
pub struct AgiCommandRegistry {
    commands: RwLock<HashMap<String, AgiCommandDef>>,
}

impl AgiCommandRegistry {
    /// Create a new registry populated with all standard AGI commands.
    pub fn new() -> Self {
        let reg = Self {
            commands: RwLock::new(HashMap::new()),
        };
        reg.register_standard_commands();
        reg
    }

    /// Register a single command definition.
    pub fn register(&self, def: AgiCommandDef) {
        self.commands.write().insert(def.name.to_uppercase(), def);
    }

    /// Look up a command by name (case-insensitive).
    pub fn get(&self, name: &str) -> Option<AgiCommandDef> {
        self.commands.read().get(&name.to_uppercase()).cloned()
    }

    /// Get all command names.
    pub fn command_names(&self) -> Vec<String> {
        self.commands.read().keys().cloned().collect()
    }

    /// Register all standard AGI commands from `res_agi.c`.
    fn register_standard_commands(&self) {
        let cmds = vec![
            ("ANSWER", "Answer channel", "ANSWER"),
            ("HANGUP", "Hangup a channel", "HANGUP [<channelname>]"),
            ("SET VARIABLE", "Sets a channel variable", "SET VARIABLE <variablename> <value>"),
            ("GET VARIABLE", "Gets a channel variable", "GET VARIABLE <variablename>"),
            ("GET FULL VARIABLE", "Evaluates a channel expression", "GET FULL VARIABLE <expression> [<channelname>]"),
            ("SET CONTEXT", "Sets channel context", "SET CONTEXT <context>"),
            ("SET EXTENSION", "Changes channel extension", "SET EXTENSION <extension>"),
            ("SET PRIORITY", "Set channel dialplan priority", "SET PRIORITY <priority>"),
            ("EXEC", "Executes a given application", "EXEC <application> <options>"),
            ("STREAM FILE", "Sends audio file on channel", "STREAM FILE <filename> <escape_digits> [<sample_offset>]"),
            ("CONTROL STREAM FILE", "Sends audio with playback control", "CONTROL STREAM FILE <filename> <escape_digits> [<skipms>] [<ffchar>] [<rewchar>] [<pausechar>] [<offsetms>]"),
            ("RECORD FILE", "Records to a given file", "RECORD FILE <filename> <format> <escape_digits> <timeout> [<offset_samples>] [BEEP] [s=<silence>]"),
            ("SAY NUMBER", "Say a given number", "SAY NUMBER <number> <escape_digits> [<gender>]"),
            ("SAY DIGITS", "Say a given digit string", "SAY DIGITS <number> <escape_digits>"),
            ("SAY ALPHA", "Say a given character string", "SAY ALPHA <number> <escape_digits>"),
            ("SAY PHONETIC", "Say a string phonetically", "SAY PHONETIC <string> <escape_digits>"),
            ("SAY DATE", "Say a given date", "SAY DATE <date> <escape_digits>"),
            ("SAY TIME", "Say a given time", "SAY TIME <time> <escape_digits>"),
            ("SAY DATETIME", "Say a given date/time", "SAY DATETIME <time> <escape_digits> [<format>] [<timezone>]"),
            ("GET DATA", "Prompts for DTMF on a channel", "GET DATA <file> [<timeout>] [<maxdigits>]"),
            ("GET OPTION", "Stream file, prompt for DTMF with timeout", "GET OPTION <filename> <escape_digits> [<timeout>]"),
            ("RECEIVE CHAR", "Receives one character", "RECEIVE CHAR <timeout>"),
            ("RECEIVE TEXT", "Receives text from channel", "RECEIVE TEXT <timeout>"),
            ("SEND TEXT", "Sends text to channel", "SEND TEXT \"<text to send>\""),
            ("WAIT FOR DIGIT", "Waits for a DTMF digit", "WAIT FOR DIGIT <timeout>"),
            ("SET MUSIC", "Enable/disable MOH", "SET MUSIC on|off [<class>]"),
            ("SET CALLERID", "Changes callerid of channel", "SET CALLERID <number>"),
            ("CHANNEL STATUS", "Returns status of the channel", "CHANNEL STATUS [<channelname>]"),
            ("DATABASE GET", "Gets database value", "DATABASE GET <family> <key>"),
            ("DATABASE PUT", "Adds/updates database value", "DATABASE PUT <family> <key> <value>"),
            ("DATABASE DEL", "Removes database key/value", "DATABASE DEL <family> <key>"),
            ("DATABASE DELTREE", "Removes database keytree", "DATABASE DELTREE <family> [<keytree>]"),
            ("VERBOSE", "Logs a message to verbose output", "VERBOSE <message> [<level>]"),
            ("NOOP", "Does nothing", "NOOP [<text>]"),
            ("SET AUTOHANGUP", "Autohangup channel in some time", "SET AUTOHANGUP <time>"),
            ("SPEECH CREATE", "Creates a speech object", "SPEECH CREATE <engine>"),
            ("SPEECH SET", "Sets a speech engine setting", "SPEECH SET <name> <value>"),
            ("SPEECH DESTROY", "Destroys a speech object", "SPEECH DESTROY"),
            ("SPEECH LOAD GRAMMAR", "Loads a grammar", "SPEECH LOAD GRAMMAR <grammar_name> <path>"),
            ("SPEECH UNLOAD GRAMMAR", "Unloads a grammar", "SPEECH UNLOAD GRAMMAR <grammar_name>"),
            ("SPEECH ACTIVATE GRAMMAR", "Activates a grammar", "SPEECH ACTIVATE GRAMMAR <grammar_name>"),
            ("SPEECH DEACTIVATE GRAMMAR", "Deactivates a grammar", "SPEECH DEACTIVATE GRAMMAR <grammar_name>"),
            ("SPEECH RECOGNIZE", "Recognizes speech", "SPEECH RECOGNIZE <prompt> <timeout> [<offset>]"),
            ("ASYNCAGI BREAK", "Interrupts Async AGI", "ASYNCAGI BREAK"),
            ("GOSUB", "Execute a dialplan subroutine", "GOSUB <context> <extension> <priority> [<args>]"),
        ];

        for (name, synopsis, usage) in cmds {
            self.register(AgiCommandDef {
                name: name.to_string(),
                synopsis: synopsis.to_string(),
                usage: usage.to_string(),
            });
        }
    }
}

impl Default for AgiCommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// AGI session (synchronous, pipe-based)
// ---------------------------------------------------------------------------

/// A synchronous AGI session communicating over pipes (stdin/stdout).
///
/// Corresponds to `agi_state` in the C source for standard AGI.
pub struct AgiSession<R: BufRead, W: IoWrite> {
    /// Reader (from AGI script's stdout).
    reader: R,
    /// Writer (to AGI script's stdin).
    writer: W,
    /// Environment variables for this session.
    pub env: AgiEnvironment,
    /// Whether the session is still alive.
    pub alive: bool,
    /// Session mode.
    pub mode: AgiMode,
}

impl<R: BufRead, W: IoWrite> AgiSession<R, W> {
    /// Create a new synchronous AGI session.
    pub fn new(reader: R, writer: W, env: AgiEnvironment, mode: AgiMode) -> Self {
        Self {
            reader,
            writer,
            env,
            alive: true,
            mode,
        }
    }

    /// Send the AGI environment to the script.
    pub fn send_environment(&mut self) -> AgiResult<()> {
        let lines = self.env.to_protocol_lines();
        self.writer.write_all(lines.as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    /// Read one command line from the AGI script.
    pub fn read_command(&mut self) -> AgiResult<Option<String>> {
        if !self.alive {
            return Err(AgiError::Hangup);
        }

        let mut line = String::new();
        let n = self.reader.read_line(&mut line)?;
        if n == 0 {
            self.alive = false;
            return Ok(None);
        }
        Ok(Some(line.trim_end().to_string()))
    }

    /// Send a response to the AGI script.
    pub fn send_response(&mut self, response: &AgiResponse) -> AgiResult<()> {
        let line = response.to_protocol_line();
        self.writer.write_all(line.as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }

    /// Mark the session as dead (channel hung up).
    pub fn set_dead(&mut self) {
        self.alive = false;
        self.mode = AgiMode::DeadAgi;
    }
}

impl<R: BufRead, W: IoWrite> fmt::Debug for AgiSession<R, W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgiSession")
            .field("mode", &self.mode)
            .field("alive", &self.alive)
            .field("channel", &self.env.channel)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// FastAGI session (async, TCP-based)
// ---------------------------------------------------------------------------

/// An async FastAGI session communicating over a TCP socket.
pub struct FastAgiSession {
    /// TCP stream to the FastAGI server.
    stream: TcpStream,
    /// Environment variables for this session.
    pub env: AgiEnvironment,
    /// Whether the session is still alive.
    pub alive: bool,
}

impl FastAgiSession {
    /// Connect to a FastAGI server at the given address.
    pub async fn connect(addr: &str) -> AgiResult<Self> {
        let stream = TcpStream::connect(addr).await.map_err(|e| {
            AgiError::ConnectionFailed(format!("Failed to connect to {}: {}", addr, e))
        })?;
        info!(addr, "FastAGI connection established");
        Ok(Self {
            stream,
            env: AgiEnvironment::default(),
            alive: true,
        })
    }

    /// Send the AGI environment over the TCP connection.
    pub async fn send_environment(&mut self) -> AgiResult<()> {
        let lines = self.env.to_protocol_lines();
        self.stream.write_all(lines.as_bytes()).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Read one command from the FastAGI server.
    pub async fn read_command(&mut self) -> AgiResult<Option<String>> {
        if !self.alive {
            return Err(AgiError::Hangup);
        }

        let mut reader = TokioBufReader::new(&mut self.stream);
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            self.alive = false;
            return Ok(None);
        }
        Ok(Some(line.trim_end().to_string()))
    }

    /// Send a response to the FastAGI server.
    pub async fn send_response(&mut self, response: &AgiResponse) -> AgiResult<()> {
        let line = response.to_protocol_line();
        self.stream.write_all(line.as_bytes()).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Close the session.
    pub async fn close(&mut self) {
        self.alive = false;
        let _ = self.stream.shutdown().await;
    }
}

impl fmt::Debug for FastAgiSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FastAgiSession")
            .field("alive", &self.alive)
            .field("channel", &self.env.channel)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Command parsing
// ---------------------------------------------------------------------------

/// Parse an AGI command line into command name and arguments.
///
/// AGI commands may be multi-word (e.g., "SET VARIABLE", "DATABASE GET").
/// We try matching the longest known command name first.
pub fn parse_agi_command(line: &str, registry: &AgiCommandRegistry) -> (String, Vec<String>) {
    let parts: Vec<&str> = line.splitn(10, ' ').collect();
    if parts.is_empty() {
        return (String::new(), Vec::new());
    }

    // Try longest prefix match (up to 3 words for commands like "SPEECH LOAD GRAMMAR").
    for n in (1..=3.min(parts.len())).rev() {
        let candidate = parts[..n].join(" ").to_uppercase();
        if registry.get(&candidate).is_some() {
            let args: Vec<String> = parts[n..].iter().map(|s| s.to_string()).collect();
            return (candidate, args);
        }
    }

    // No match -- return first word as command, rest as args.
    let cmd = parts[0].to_uppercase();
    let args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
    (cmd, args)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_agi_response_success() {
        let resp = AgiResponse::success("0");
        assert_eq!(resp.to_protocol_line(), "200 result=0\n");
        assert!(resp.is_success());
        assert_eq!(resp.result_int(), Some(0));
    }

    #[test]
    fn test_agi_response_with_data() {
        let resp = AgiResponse::success_with_data("1", "testvariable");
        assert_eq!(resp.to_protocol_line(), "200 result=1 (testvariable)\n");
    }

    #[test]
    fn test_agi_response_invalid_command() {
        let resp = AgiResponse::invalid_command("No such command");
        assert_eq!(resp.code, 510);
    }

    #[test]
    fn test_agi_response_parse() {
        let resp = AgiResponse::parse("200 result=1 (testvariable)").unwrap();
        assert_eq!(resp.code, 200);
        assert_eq!(resp.result, "1");
        assert_eq!(resp.data.as_deref(), Some("testvariable"));
    }

    #[test]
    fn test_agi_response_parse_no_data() {
        let resp = AgiResponse::parse("200 result=0").unwrap();
        assert_eq!(resp.code, 200);
        assert_eq!(resp.result, "0");
        assert!(resp.data.is_none());
    }

    #[test]
    fn test_agi_response_parse_510() {
        let resp = AgiResponse::parse("510 Invalid or unknown command").unwrap();
        assert_eq!(resp.code, 510);
    }

    #[test]
    fn test_agi_mode_from_request() {
        assert_eq!(AgiMode::from_request("/usr/bin/my_agi.sh"), AgiMode::Standard);
        assert_eq!(AgiMode::from_request("agi://192.168.1.1"), AgiMode::FastAgi);
        assert_eq!(AgiMode::from_request("agi:async"), AgiMode::Async);
    }

    #[test]
    fn test_agi_environment_roundtrip() {
        let env = AgiEnvironment {
            request: "/tmp/test.sh".to_string(),
            channel: "SIP/alice-0001".to_string(),
            language: "en".to_string(),
            channel_type: "SIP".to_string(),
            uniqueid: "1234567890.1".to_string(),
            version: "18.0.0".to_string(),
            callerid: "1005".to_string(),
            calleridname: "Alice".to_string(),
            context: "default".to_string(),
            extension: "100".to_string(),
            priority: "1".to_string(),
            arguments: vec!["arg1".to_string(), "arg2".to_string()],
            ..Default::default()
        };

        let lines = env.to_protocol_lines();
        assert!(lines.contains("agi_request: /tmp/test.sh"));
        assert!(lines.contains("agi_channel: SIP/alice-0001"));
        assert!(lines.contains("agi_arg_1: arg1"));
        assert!(lines.contains("agi_arg_2: arg2"));
        assert!(lines.ends_with("\n\n")); // Terminated by empty line.

        // Parse them back.
        let line_vec: Vec<String> = lines.lines().map(|l| l.to_string()).collect();
        let parsed = AgiEnvironment::parse_lines(&line_vec);
        assert_eq!(parsed.request, "/tmp/test.sh");
        assert_eq!(parsed.channel, "SIP/alice-0001");
        assert_eq!(parsed.arguments.len(), 2);
        assert_eq!(parsed.arguments[0], "arg1");
    }

    #[test]
    fn test_agi_command_registry() {
        let reg = AgiCommandRegistry::new();
        assert!(reg.get("ANSWER").is_some());
        assert!(reg.get("STREAM FILE").is_some());
        assert!(reg.get("SET VARIABLE").is_some());
        assert!(reg.get("DATABASE GET").is_some());
        assert!(reg.get("SPEECH RECOGNIZE").is_some());
        assert!(reg.get("NONEXISTENT").is_none());

        let names = reg.command_names();
        assert!(names.len() >= 30); // We registered many commands.
    }

    #[test]
    fn test_parse_agi_command() {
        let reg = AgiCommandRegistry::new();

        let (cmd, args) = parse_agi_command("ANSWER", &reg);
        assert_eq!(cmd, "ANSWER");
        assert!(args.is_empty());

        let (cmd, args) = parse_agi_command("SET VARIABLE foo bar", &reg);
        assert_eq!(cmd, "SET VARIABLE");
        assert_eq!(args, vec!["foo", "bar"]);

        let (cmd, args) = parse_agi_command("DATABASE GET /contacts alice", &reg);
        assert_eq!(cmd, "DATABASE GET");
        assert_eq!(args, vec!["/contacts", "alice"]);

        let (cmd, args) = parse_agi_command("STREAM FILE hello 1234", &reg);
        assert_eq!(cmd, "STREAM FILE");
        assert_eq!(args, vec!["hello", "1234"]);
    }

    #[test]
    fn test_agi_session_sync() {
        let input = b"ANSWER\nSET VARIABLE foo bar\n";
        let reader = Cursor::new(input.to_vec());
        let mut output = Vec::new();

        let env = AgiEnvironment {
            channel: "test-chan".to_string(),
            ..Default::default()
        };

        // Use a separate scope to drop the session before checking output.
        {
            let mut session = AgiSession::new(
                std::io::BufReader::new(reader),
                &mut output,
                env,
                AgiMode::Standard,
            );

            // Send environment.
            session.send_environment().unwrap();

            // Read first command.
            let cmd1 = session.read_command().unwrap().unwrap();
            assert_eq!(cmd1, "ANSWER");

            // Send response.
            session.send_response(&AgiResponse::success("0")).unwrap();

            // Read second command.
            let cmd2 = session.read_command().unwrap().unwrap();
            assert_eq!(cmd2, "SET VARIABLE foo bar");
        }

        let output_str = String::from_utf8(output).unwrap();
        assert!(output_str.contains("agi_channel: test-chan"));
        assert!(output_str.contains("200 result=0"));
    }
}
