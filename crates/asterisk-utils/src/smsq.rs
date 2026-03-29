//! smsq - SMS queue manager for Asterisk app_sms
//!
//! Queues SMS messages for delivery via Asterisk. Messages can be queued as
//! mobile-terminated (MT) or mobile-originated (MO), and for transmit (TX)
//! or receive (RX) processing. The tool creates spool files that Asterisk
//! picks up and processes through the SMS application.
//!
//! Port of asterisk/utils/smsq.c

use clap::Parser;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

/// SMS queue manager for Asterisk
#[derive(Parser, Debug)]
#[command(name = "smsq", about = "Queue SMS messages for Asterisk app_sms")]
struct Args {
    /// Queue name (including optional sub-address as queue-X)
    #[arg(short, long, default_value = "")]
    queue: String,

    /// Destination address (phone number)
    #[arg(short = 'd', long)]
    da: Option<String>,

    /// Origination address (phone number)
    #[arg(short = 'o', long)]
    oa: Option<String>,

    /// Message text (UTF-8)
    #[arg(short = 'm', long = "ud")]
    message: Option<String>,

    /// Message file
    #[arg(short = 'f', long = "ud-file")]
    ud_file: Option<PathBuf>,

    /// Mobile Terminated
    #[arg(short = 't', long)]
    mt: bool,

    /// Mobile Originated
    #[arg(long)]
    mo: bool,

    /// Send message (transmit)
    #[arg(long)]
    tx: bool,

    /// Queue for receipt
    #[arg(short = 'r', long)]
    rx: bool,

    /// Rx queue process command
    #[arg(short = 'e', long)]
    process_cmd: Option<String>,

    /// Do not dial
    #[arg(short = 'x', long)]
    no_dial: bool,

    /// Do not wait if already calling
    #[arg(long)]
    no_wait: bool,

    /// Number of concurrent calls to allow
    #[arg(long, default_value = "1")]
    concurrent: u32,

    /// Channel for MO TX calls
    #[arg(long, default_value = "Local/1709400X")]
    motx_channel: String,

    /// Caller ID for MO TX calls
    #[arg(long)]
    motx_callerid: Option<String>,

    /// Time to wait for MO TX call to answer (seconds)
    #[arg(long, default_value = "10")]
    motx_wait: u32,

    /// Time between MO TX call retries (seconds)
    #[arg(long, default_value = "1")]
    motx_delay: u32,

    /// Number of retries for MO TX calls
    #[arg(long, default_value = "10")]
    motx_retries: u32,

    /// Channel for MT TX calls
    #[arg(long)]
    mttx_channel: Option<String>,

    /// Caller ID for MT TX calls
    #[arg(long, default_value = "080058752X0")]
    mttx_callerid: String,

    /// Time to wait for MT TX call to answer (seconds)
    #[arg(long, default_value = "10")]
    mttx_wait: u32,

    /// Time between MT TX call retries (seconds)
    #[arg(long, default_value = "30")]
    mttx_delay: u32,

    /// Number of retries for MT TX calls
    #[arg(long, default_value = "100")]
    mttx_retries: u32,

    /// Message reference
    #[arg(short = 'n', long)]
    mr: Option<i32>,

    /// Protocol ID
    #[arg(short = 'p', long)]
    pid: Option<i32>,

    /// Data Coding Scheme
    #[arg(short = 'c', long)]
    dcs: Option<i32>,

    /// User data header (hex)
    #[arg(long)]
    udh: Option<String>,

    /// Status Report Request
    #[arg(long)]
    srr: bool,

    /// Return Path request
    #[arg(long)]
    rp: bool,

    /// Validity Period (seconds)
    #[arg(short = 'v', long, default_value = "0")]
    vp: u32,

    /// Timestamp (YYYY-MM-SSTHH:MM:SS)
    #[arg(long)]
    scts: Option<String>,

    /// Default sub-address character
    #[arg(long, default_value = "9")]
    default_sub_address: String,

    /// Asterisk spool directory
    #[arg(long, default_value = "/var/spool/asterisk")]
    spool_dir: PathBuf,
}

/// Get the SMS direction directory name.
fn sms_dir(mo: bool, rx: bool) -> &'static str {
    match (mo, rx) {
        (true, true) => "morx",
        (true, false) => "motx",
        (false, true) => "mtrx",
        (false, false) => "mttx",
    }
}

/// Get current Unix timestamp.
fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Encode a Unicode string as UCS-2 hex.
fn encode_ucs2_hex(text: &str) -> String {
    let mut hex = String::new();
    for ch in text.chars() {
        let code = ch as u32;
        if code <= 0xFFFF {
            hex.push_str(&format!("{code:04X}"));
        }
    }
    hex
}

/// Check if all characters in the message are printable ASCII.
fn is_printable_ascii(text: &str) -> bool {
    text.chars().all(|c| c >= ' ' && (c as u32) < 0x80)
}

/// Check if all characters fit in a single byte (UCS-1).
fn is_ucs1(text: &str) -> bool {
    text.chars().all(|c| (c as u32) < 0x100)
}

/// Parameters for writing a queue file.
struct QueueFileParams<'a> {
    spool_dir: &'a Path,
    dir: &'a str,
    queue: &'a str,
    oa: Option<&'a str>,
    da: Option<&'a str>,
    scts: Option<&'a str>,
    pid: Option<i32>,
    dcs: Option<i32>,
    mr: Option<i32>,
    srr: bool,
    rp: bool,
    udh: Option<&'a str>,
    vp: u32,
    message: &'a str,
}

/// Write a queue file for an SMS message.
fn write_queue_file(params: &QueueFileParams<'_>) -> Result<(), String> {
    let QueueFileParams {
        spool_dir, dir, queue, oa, da, scts, pid, dcs, mr, srr, rp, udh, vp, message,
    } = params;
    let sms_dir_path = spool_dir.join("sms");
    let full_dir = sms_dir_path.join(dir);

    // Ensure directories exist
    let _ = fs::create_dir_all(&full_dir);

    let temp_path = sms_dir_path.join(format!(".smsq-{}", process::id()));
    let queue_name = if queue.is_empty() { "0" } else { queue };
    let queue_file = full_dir.join(format!("{}.{}-{}", queue_name, unix_timestamp(), process::id()));

    let mut f = fs::File::create(&temp_path)
        .map_err(|e| format!("Cannot create temp file: {e}"))?;

    if let Some(oa) = oa {
        writeln!(f, "oa={oa}").map_err(|e| format!("Write error: {e}"))?;
    }
    if let Some(da) = da {
        writeln!(f, "da={da}").map_err(|e| format!("Write error: {e}"))?;
    }
    if let Some(scts) = scts {
        writeln!(f, "scts={scts}").map_err(|e| format!("Write error: {e}"))?;
    }
    if let Some(pid) = pid {
        writeln!(f, "pid={pid}").map_err(|e| format!("Write error: {e}"))?;
    }
    if let Some(dcs) = dcs {
        writeln!(f, "dcs={dcs}").map_err(|e| format!("Write error: {e}"))?;
    }
    if let Some(mr) = mr {
        writeln!(f, "mr={mr}").map_err(|e| format!("Write error: {e}"))?;
    }
    if *srr {
        writeln!(f, "srr=1").map_err(|e| format!("Write error: {e}"))?;
    }
    if *rp {
        writeln!(f, "rp=1").map_err(|e| format!("Write error: {e}"))?;
    }
    if let Some(udh) = udh {
        writeln!(f, "udh#{udh}").map_err(|e| format!("Write error: {e}"))?;
    }
    if *vp > 0 {
        writeln!(f, "vp={vp}").map_err(|e| format!("Write error: {e}"))?;
    }

    // Write user data
    if !message.is_empty() {
        if is_printable_ascii(message) {
            writeln!(f, "ud={message}").map_err(|e| format!("Write error: {e}"))?;
        } else if is_ucs1(message) {
            let hex: String = message
                .chars()
                .map(|c| format!("{:02X}", c as u32))
                .collect();
            writeln!(f, "ud#{hex}").map_err(|e| format!("Write error: {e}"))?;
        } else {
            let hex = encode_ucs2_hex(message);
            writeln!(f, "ud##{hex}").map_err(|e| format!("Write error: {e}"))?;
        }
    }

    drop(f);

    // Atomically move into place
    fs::rename(&temp_path, &queue_file).map_err(|e| {
        let _ = fs::remove_file(&temp_path);
        format!("Cannot rename to queue file: {e}")
    })?;

    eprintln!("Queued: {}", queue_file.display());
    Ok(())
}

/// Parameters for creating an outgoing call file.
struct OutgoingCallParams<'a> {
    spool_dir: &'a Path,
    dir: &'a str,
    queue: &'a str,
    subaddress: char,
    channel: &'a str,
    callerid: Option<&'a str>,
    wait: u32,
    delay: u32,
    retries: u32,
    concurrent: u32,
}

/// Create an outgoing call file to trigger SMS delivery.
fn create_outgoing_call(params: &OutgoingCallParams<'_>) -> Result<bool, String> {
    let OutgoingCallParams {
        spool_dir, dir, queue, subaddress, channel, callerid, wait, delay, retries, concurrent,
    } = params;
    let temp_path = spool_dir.join(format!("sms/.smsq-{}", process::id()));

    // Check if there are any queued messages
    let sms_dir = spool_dir.join("sms").join(dir);
    if !sms_dir.is_dir() {
        return Ok(false);
    }

    let has_messages = fs::read_dir(&sms_dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|e| !e.file_name().to_string_lossy().starts_with('.'))
        })
        .unwrap_or(false);

    if !has_messages {
        return Ok(false);
    }

    // Determine queue name for channel
    let queue_short = if let Some(dash) = queue.find('-') {
        &queue[..dash]
    } else {
        queue
    };

    let mut f = fs::File::create(&temp_path)
        .map_err(|e| format!("Cannot create temp file: {e}"))?;

    // Write channel line
    let channel_line = if channel.contains('X') {
        channel.replacen('X', &subaddress.to_string(), 1)
    } else {
        channel.to_string()
    };

    writeln!(f, "Channel: {channel_line}").map_err(|e| format!("Write error: {e}"))?;

    // Write caller ID
    let cid = callerid
        .map(|c| {
            if c.contains('X') {
                c.replacen('X', &subaddress.to_string(), 1)
            } else {
                c.to_string()
            }
        })
        .unwrap_or_else(|| queue_short.to_string());
    writeln!(f, "Callerid: SMS <{cid}>").map_err(|e| format!("Write error: {e}"))?;

    writeln!(f, "Application: SMS").map_err(|e| format!("Write error: {e}"))?;

    let data_suffix = if dir.ends_with("tx") { "" } else { ",s" };
    writeln!(f, "Data: {queue}{data_suffix}").map_err(|e| format!("Write error: {e}"))?;
    writeln!(f, "MaxRetries: {retries}").map_err(|e| format!("Write error: {e}"))?;
    writeln!(f, "RetryTime: {delay}").map_err(|e| format!("Write error: {e}"))?;
    writeln!(f, "WaitTime: {wait}").map_err(|e| format!("Write error: {e}"))?;

    drop(f);

    // Try to link into outgoing directory
    let outgoing_dir = spool_dir.join("outgoing");
    for attempt in 1..=*concurrent {
        let og_name = outgoing_dir.join(format!("smsq.{dir}.{queue}.{attempt}"));
        // Use rename as atomic move (hard links may not work across filesystems)
        if fs::copy(&temp_path, &og_name).is_ok() {
            let _ = fs::remove_file(&temp_path);
            return Ok(true);
        }
    }

    let _ = fs::remove_file(&temp_path);
    Ok(false)
}

fn main() {
    let args = Args::parse();

    // Resolve MT/MO mode
    let mut mt = args.mt;
    let mut mo = args.mo;

    if !mt && !mo && args.process_cmd.is_some() {
        mt = true;
    }
    if !mt && !mo && args.oa.is_some() {
        mt = true;
    }
    if !mt {
        mo = true;
    }
    if mt && mo {
        eprintln!("Cannot be --mt and --mo");
        process::exit(1);
    }

    // Resolve TX/RX mode
    let mut tx = args.tx;
    let mut rx = args.rx;

    if !rx && !tx && args.process_cmd.is_some() {
        rx = true;
    }
    if !rx {
        tx = true;
    }
    if tx && rx {
        eprintln!("Cannot be --tx and --rx");
        process::exit(1);
    }

    let mut no_dial = args.no_dial;
    if rx {
        no_dial = true;
    }

    // Validate arguments
    if let Some(ref da) = args.da {
        if da.len() > 20 {
            eprintln!("--da too long");
            process::exit(1);
        }
    }
    if let Some(ref oa) = args.oa {
        if oa.len() > 20 {
            eprintln!("--oa too long");
            process::exit(1);
        }
    }
    if args.queue.len() > 20 {
        eprintln!("--queue name too long");
        process::exit(1);
    }
    if mo && args.scts.is_some() {
        eprintln!("scts is set by service centre");
        process::exit(1);
    }

    // Get message text
    let message = if let Some(ref msg) = args.message {
        msg.clone()
    } else if let Some(ref ud_file) = args.ud_file {
        match fs::read_to_string(ud_file) {
            Ok(content) => content,
            Err(e) => {
                eprintln!("Cannot read {}: {e}", ud_file.display());
                process::exit(1);
            }
        }
    } else {
        String::new()
    };

    // Truncate to 160 characters (SMS limit)
    let message: String = message.chars().take(160).collect();

    // Determine sub-address
    let subaddress = if let Some(dash_pos) = args.queue.rfind('-') {
        args.queue.as_bytes().get(dash_pos + 1).copied().unwrap_or(b'9') as char
    } else {
        args.default_sub_address.chars().next().unwrap_or('9')
    };

    let dir = sms_dir(mo, rx);

    // Queue message if we have an address
    if args.oa.is_some() || args.da.is_some() {
        if let Err(e) = write_queue_file(&QueueFileParams {
            spool_dir: &args.spool_dir,
            dir,
            queue: &args.queue,
            oa: args.oa.as_deref(),
            da: args.da.as_deref(),
            scts: args.scts.as_deref(),
            pid: args.pid,
            dcs: args.dcs,
            mr: args.mr,
            srr: args.srr,
            rp: args.rp,
            udh: args.udh.as_deref(),
            vp: args.vp,
            message: &message,
        }) {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }

    // Dial to send messages
    if !no_dial && tx && args.process_cmd.is_none() {
        let max_tries = if args.no_wait { 1 } else { 3 };
        let (channel, callerid, wait, delay, retries) = if mo {
            (
                args.motx_channel.as_str(),
                args.motx_callerid.as_deref(),
                args.motx_wait,
                args.motx_delay,
                args.motx_retries,
            )
        } else {
            (
                args.mttx_channel.as_deref().unwrap_or(""),
                Some(args.mttx_callerid.as_str()),
                args.mttx_wait,
                args.mttx_delay,
                args.mttx_retries,
            )
        };

        let tx_dir = if mo { "motx" } else { "mttx" };
        let mut queued = false;

        for attempt in 0..max_tries {
            match create_outgoing_call(&OutgoingCallParams {
                spool_dir: &args.spool_dir,
                dir: tx_dir,
                queue: &args.queue,
                subaddress,
                channel,
                callerid,
                wait,
                delay,
                retries,
                concurrent: args.concurrent,
            }) {
                Ok(true) => {
                    queued = true;
                    break;
                }
                Ok(false) => {
                    if attempt + 1 < max_tries {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    break;
                }
            }
        }

        if !queued && !args.no_wait {
            eprintln!("No call scheduled as already sending");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sms_dir() {
        assert_eq!(sms_dir(true, true), "morx");
        assert_eq!(sms_dir(true, false), "motx");
        assert_eq!(sms_dir(false, true), "mtrx");
        assert_eq!(sms_dir(false, false), "mttx");
    }

    #[test]
    fn test_is_printable_ascii() {
        assert!(is_printable_ascii("Hello World"));
        assert!(!is_printable_ascii("Hello\nWorld"));
        assert!(!is_printable_ascii("Hallo Welt\u{00FC}")); // u with umlaut
    }

    #[test]
    fn test_is_ucs1() {
        assert!(is_ucs1("Hello"));
        assert!(is_ucs1("\u{00FF}")); // Latin small letter y with diaeresis
        assert!(!is_ucs1("\u{0100}")); // Latin capital letter A with macron
    }

    #[test]
    fn test_encode_ucs2_hex() {
        assert_eq!(encode_ucs2_hex("AB"), "00410042");
        assert_eq!(encode_ucs2_hex(""), "");
    }

    #[test]
    fn test_unix_timestamp() {
        let ts = unix_timestamp();
        assert!(ts > 1_700_000_000); // After 2023
    }
}
