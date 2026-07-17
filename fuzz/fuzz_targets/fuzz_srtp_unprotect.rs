#![no_main]
//! Fuzz the REAL SRTP/SRTCP unprotect path
//! (`asterisk_sip::srtp::SrtpCrypto::{unprotect_rtp, unprotect_rtcp}` via the
//! default pure-Rust backend). A fixed test key is installed once; each input
//! is treated as an inbound SRTP/SRTCP packet. Auth will (almost always) fail,
//! but the length/header arithmetic before the auth check runs on raw wire
//! bytes and must never panic. No real key/secret is used.
use libfuzzer_sys::fuzz_target;

use std::sync::{LazyLock, Mutex};

use asterisk_sip::srtp::{create_srtp_crypto, SrtpCrypto, SrtpCryptoSuite, SrtpKeyMaterial};

// A fixed, non-secret test key (16-byte key + 14-byte salt for AES_CM_128).
static CTX: LazyLock<Mutex<Box<dyn SrtpCrypto>>> = LazyLock::new(|| {
    let km = SrtpKeyMaterial::new(
        SrtpCryptoSuite::AesCm128HmacSha1_80,
        vec![0x11; 16],
        vec![0x22; 14],
    );
    Mutex::new(create_srtp_crypto(km).expect("build srtp ctx"))
});

fuzz_target!(|data: &[u8]| {
    let mut ctx = CTX.lock().unwrap();
    let mut pkt = data.to_vec();
    let _ = ctx.unprotect_rtp(&mut pkt);
    let mut pkt2 = data.to_vec();
    let _ = ctx.unprotect_rtcp(&mut pkt2);
});
