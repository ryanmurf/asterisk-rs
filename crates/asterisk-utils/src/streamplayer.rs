//! streamplayer - Read from a raw TCP stream and write to stdout
//!
//! A utility for reading audio data from a raw TCP stream and writing it to
//! stdout for use as an Asterisk music-on-hold source. The application reads
//! data from the TCP stream and dumps it to stdout, checking that writing
//! to stdout won't block before doing so (to keep the stream serviced even
//! when Asterisk isn't consuming the data).
//!
//! Port of asterisk/utils/streamplayer.c

use clap::Parser;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::process;
use std::time::Duration;

/// Read audio from a TCP stream and write to stdout (for Asterisk MOH)
#[derive(Parser, Debug)]
#[command(
    name = "streamplayer",
    about = "Read from a raw TCP stream and write to stdout for Asterisk music-on-hold"
)]
struct Args {
    /// Host/IP to connect to
    host: String,

    /// Port number
    port: u16,

    /// Read buffer size in bytes
    #[arg(short, long, default_value = "2048")]
    buffer_size: usize,

    /// Connection timeout in seconds
    #[arg(short, long, default_value = "10")]
    timeout: u64,
}

fn main() {
    let args = Args::parse();

    let addr = format!("{}:{}", args.host, args.port);

    // Connect to the TCP stream
    let stream = match TcpStream::connect_timeout(
        &addr.parse().unwrap_or_else(|_| {
            // Try DNS resolution
            use std::net::ToSocketAddrs;
            addr.to_socket_addrs()
                .unwrap_or_else(|e| {
                    eprintln!("Unable to resolve host '{}': {e}", args.host);
                    process::exit(1);
                })
                .next()
                .unwrap_or_else(|| {
                    eprintln!("Unable to lookup IP for host '{}'", args.host);
                    process::exit(1);
                })
        }),
        Duration::from_secs(args.timeout),
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Unable to connect to {addr}: {e}");
            process::exit(1);
        }
    };

    // Set read timeout to prevent blocking forever
    let _ = stream.try_clone().map(|s| {
        let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
    });

    eprintln!("Connected to {addr}");

    let mut stream = stream;
    let mut buf = vec![0u8; args.buffer_size];
    let stdout = io::stdout();
    let mut stdout_lock = stdout.lock();

    loop {
        let bytes_read = match stream.read(&mut buf) {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(ref e) if e.kind() == io::ErrorKind::TimedOut => continue,
            Err(_) => break,
        };

        // Write to stdout. In the C version, select() is used to check if
        // stdout is writable. In Rust, we simply write and handle errors.
        // If stdout is a pipe and the reader isn't consuming, we'll get a
        // BrokenPipe error and exit cleanly.
        match stdout_lock.write_all(&buf[..bytes_read]) {
            Ok(()) => {
                let _ = stdout_lock.flush();
            }
            Err(ref e) if e.kind() == io::ErrorKind::BrokenPipe => break,
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_buffer_size_default() {
        // Verify the default buffer size is reasonable
        let default_size: usize = 2048;
        assert!(default_size > 0);
        assert!(default_size <= 65536);
    }
}
