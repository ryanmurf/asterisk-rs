//! PRACK / 100rel (RFC 3262) -- Reliable provisional responses.
//!
//! Allows provisional SIP responses (1xx other than 100) to be sent
//! reliably with retransmission until acknowledged by a PRACK request.
//!
//! Key headers:
//! - `Require: 100rel` -- indicates provisional responses MUST be reliable
//! - `Supported: 100rel` -- indicates support for reliable provisionals
//! - `RSeq: N` -- sequence number on the provisional response
//! - `RAck: rseq cseq method` -- acknowledgment in the PRACK request

use std::time::Duration;

use crate::parser::{SipMessage, SipMethod};
use crate::transaction::timers;

// ---------------------------------------------------------------------------
// PRACK state
// ---------------------------------------------------------------------------

/// State for tracking reliable provisional response sequencing.
#[derive(Debug, Clone)]
pub struct PrackState {
    /// Current RSeq value (incremented for each new reliable provisional).
    pub rseq: u32,
    /// Whether we are waiting for a PRACK for the latest provisional.
    pub rack_pending: bool,
    /// The CSeq of the INVITE this relates to.
    pub invite_cseq: u32,
    /// Number of retransmissions sent for the current provisional.
    pub retransmit_count: u32,
    /// Maximum retransmissions before giving up.
    pub max_retransmits: u32,
}

impl PrackState {
    /// Create a new PRACK state for an INVITE with the given CSeq.
    pub fn new(invite_cseq: u32) -> Self {
        Self {
            rseq: 0,
            rack_pending: false,
            invite_cseq,
            retransmit_count: 0,
            max_retransmits: 7, // ~64*T1 with doubling
        }
    }

    /// Allocate the next RSeq value for a new reliable provisional response.
    pub fn next_rseq(&mut self) -> u32 {
        self.rseq += 1;
        self.rack_pending = true;
        self.retransmit_count = 0;
        self.rseq
    }

    /// Record that a retransmission was sent.
    /// Returns `false` if the maximum retransmissions have been reached.
    pub fn record_retransmit(&mut self) -> bool {
        self.retransmit_count += 1;
        self.retransmit_count <= self.max_retransmits
    }

    /// Handle receipt of a PRACK for a specific RSeq.
    ///
    /// Returns `true` if the PRACK matches the pending RSeq.
    pub fn handle_prack(&mut self, rack_rseq: u32, rack_cseq: u32, rack_method: &str) -> bool {
        if rack_rseq == self.rseq
            && rack_cseq == self.invite_cseq
            && rack_method.eq_ignore_ascii_case("INVITE")
        {
            self.rack_pending = false;
            self.retransmit_count = 0;
            true
        } else {
            false
        }
    }

    /// Whether a PRACK is expected (we sent a reliable provisional but
    /// haven't received the PRACK yet).
    pub fn is_prack_pending(&self) -> bool {
        self.rack_pending
    }

    /// Current retransmit interval (doubles each time, starting at T1).
    pub fn retransmit_interval(&self) -> Duration {
        let base = timers::T1;
        let multiplier = 1u64 << self.retransmit_count.min(6);
        let interval = base * multiplier as u32;
        // Cap at T2.
        if interval > timers::T2 {
            timers::T2
        } else {
            interval
        }
    }
}

// ---------------------------------------------------------------------------
// Header helpers
// ---------------------------------------------------------------------------

/// Check whether a SIP message indicates support for 100rel.
///
/// Looks for `100rel` in the `Supported` header.
pub fn supports_100rel(msg: &SipMessage) -> bool {
    msg.get_headers("Supported")
        .into_iter()
        .chain(msg.get_headers("k")) // compact form
        .any(|val| {
            val.split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("100rel"))
        })
}

/// Check whether a SIP message requires 100rel.
///
/// Looks for `100rel` in the `Require` header.
pub fn require_100rel(msg: &SipMessage) -> bool {
    msg.get_headers("Require")
        .into_iter()
        .any(|val| {
            val.split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("100rel"))
        })
}

/// Add `RSeq` header to a provisional response for reliable delivery.
pub fn add_rseq_header(msg: &mut SipMessage, rseq: u32) {
    msg.add_header("RSeq", &rseq.to_string());
}

/// Add `Require: 100rel` header to a provisional response.
pub fn add_require_100rel(msg: &mut SipMessage) {
    msg.add_header("Require", "100rel");
}

/// Parse the `RAck` header from a PRACK request.
///
/// Format: `RAck: rseq-number cseq-number method`
///
/// Returns `(rseq, cseq, method)` if parsing succeeds.
pub fn parse_rack_header(msg: &SipMessage) -> Option<(u32, u32, String)> {
    let rack_value = msg.get_header("RAck")?;
    let parts: Vec<&str> = rack_value.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }

    let rseq: u32 = parts[0].parse().ok()?;
    let cseq: u32 = parts[1].parse().ok()?;
    let method = parts[2].to_string();

    Some((rseq, cseq, method))
}

/// Build a PRACK request to acknowledge a reliable provisional response.
///
/// The PRACK includes a `RAck` header with the RSeq, CSeq, and method
/// from the response being acknowledged.
pub fn build_prack_request(
    request_uri: &str,
    from: &str,
    to: &str,
    call_id: &str,
    cseq: u32,
    rseq: u32,
    invite_cseq: u32,
) -> SipMessage {
    let mut msg = SipMessage::new_request(
        SipMethod::Prack,
        request_uri,
    );

    msg.add_header("From", from);
    msg.add_header("To", to);
    msg.add_header("Call-ID", call_id);
    msg.add_header("CSeq", &format!("{} PRACK", cseq));
    msg.add_header("RAck", &format!("{} {} INVITE", rseq, invite_cseq));
    msg.add_header("Max-Forwards", "70");
    msg.add_header("Content-Length", "0");

    msg
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prack_state_lifecycle() {
        let mut state = PrackState::new(1);
        assert!(!state.is_prack_pending());

        let rseq = state.next_rseq();
        assert_eq!(rseq, 1);
        assert!(state.is_prack_pending());

        // Retransmit.
        assert!(state.record_retransmit());
        assert_eq!(state.retransmit_count, 1);

        // Handle matching PRACK.
        assert!(state.handle_prack(1, 1, "INVITE"));
        assert!(!state.is_prack_pending());
    }

    #[test]
    fn test_prack_state_mismatched_rack() {
        let mut state = PrackState::new(1);
        state.next_rseq();

        // Wrong RSeq.
        assert!(!state.handle_prack(99, 1, "INVITE"));
        assert!(state.is_prack_pending());

        // Wrong CSeq.
        assert!(!state.handle_prack(1, 99, "INVITE"));
        assert!(state.is_prack_pending());

        // Wrong method.
        assert!(!state.handle_prack(1, 1, "BYE"));
        assert!(state.is_prack_pending());
    }

    #[test]
    fn test_rseq_sequencing() {
        let mut state = PrackState::new(5);

        let r1 = state.next_rseq();
        assert_eq!(r1, 1);

        // Simulate PRACK received.
        state.handle_prack(1, 5, "INVITE");

        let r2 = state.next_rseq();
        assert_eq!(r2, 2);
    }

    #[test]
    fn test_retransmit_interval_doubling() {
        let mut state = PrackState::new(1);
        state.next_rseq();

        // Initial interval should be T1 (500ms).
        assert_eq!(state.retransmit_interval(), timers::T1);

        state.record_retransmit();
        // After 1 retransmit: 2 * T1.
        assert_eq!(state.retransmit_interval(), timers::T1 * 2);

        state.record_retransmit();
        // After 2 retransmits: 4 * T1 = T2.
        state.record_retransmit();
        // After 3 retransmits: 8 * T1 = 4s = T2 (capped).
        assert!(state.retransmit_interval() <= timers::T2);
    }

    #[test]
    fn test_max_retransmits() {
        let mut state = PrackState::new(1);
        state.next_rseq();
        state.max_retransmits = 2;

        assert!(state.record_retransmit()); // 1
        assert!(state.record_retransmit()); // 2
        assert!(!state.record_retransmit()); // 3 > max
    }

    #[test]
    fn test_supports_100rel() {
        let mut msg = SipMessage::new_request(SipMethod::Invite, "sip:bob@example.com");
        msg.add_header("Supported", "100rel, timer");

        assert!(supports_100rel(&msg));
    }

    #[test]
    fn test_supports_100rel_not_present() {
        let mut msg = SipMessage::new_request(SipMethod::Invite, "sip:bob@example.com");
        msg.add_header("Supported", "timer");

        assert!(!supports_100rel(&msg));
    }

    #[test]
    fn test_require_100rel() {
        let mut msg = SipMessage::new_request(SipMethod::Invite, "sip:bob@example.com");
        msg.add_header("Require", "100rel");

        assert!(require_100rel(&msg));
    }

    #[test]
    fn test_parse_rack_header() {
        let mut msg = SipMessage::new_request(SipMethod::Prack, "sip:bob@example.com");
        msg.add_header("RAck", "1 1 INVITE");

        let (rseq, cseq, method) = parse_rack_header(&msg).unwrap();
        assert_eq!(rseq, 1);
        assert_eq!(cseq, 1);
        assert_eq!(method, "INVITE");
    }

    #[test]
    fn test_build_prack_request() {
        let msg = build_prack_request(
            "sip:bob@example.com",
            "<sip:alice@example.com>;tag=abc",
            "<sip:bob@example.com>;tag=def",
            "call-123@example.com",
            2,  // PRACK CSeq
            1,  // RSeq being acknowledged
            1,  // INVITE CSeq
        );

        assert_eq!(msg.get_header("RAck"), Some("1 1 INVITE"));
        assert_eq!(msg.get_header("CSeq"), Some("2 PRACK"));
    }

    // -----------------------------------------------------------------------
    // ADVERSARIAL PRACK TESTS
    // -----------------------------------------------------------------------

    #[test]
    fn test_rseq_monotonically_increasing() {
        let mut state = PrackState::new(1);
        let r1 = state.next_rseq();
        state.handle_prack(r1, 1, "INVITE");
        let r2 = state.next_rseq();
        state.handle_prack(r2, 1, "INVITE");
        let r3 = state.next_rseq();

        assert!(r1 < r2, "RSeq must be monotonically increasing");
        assert!(r2 < r3, "RSeq must be monotonically increasing");
    }

    #[test]
    fn test_prack_for_unknown_rseq_rejected() {
        let mut state = PrackState::new(1);
        state.next_rseq(); // rseq = 1

        // PRACK with rseq=99 (unknown) should be rejected
        assert!(!state.handle_prack(99, 1, "INVITE"), "Unknown RSeq should be rejected");
        assert!(state.is_prack_pending(), "PRACK should still be pending");
    }

    #[test]
    fn test_prack_retransmit_timer_doubling() {
        let mut state = PrackState::new(1);
        state.next_rseq();

        let t0 = state.retransmit_interval();
        state.record_retransmit();
        let t1 = state.retransmit_interval();
        state.record_retransmit();
        let t2 = state.retransmit_interval();

        assert_eq!(t1, t0 * 2, "Timer should double after first retransmit");
        assert_eq!(t2, t0 * 4, "Timer should double again after second retransmit");
    }

    #[test]
    fn test_prack_timer_caps_at_t2() {
        let mut state = PrackState::new(1);
        state.next_rseq();

        // Retransmit many times to hit the cap
        for _ in 0..10 {
            state.record_retransmit();
        }
        assert!(state.retransmit_interval() <= timers::T2,
            "Timer must cap at T2");
    }

    #[test]
    fn test_prack_method_case_insensitive() {
        let mut state = PrackState::new(1);
        state.next_rseq();

        // Method comparison should be case-insensitive
        assert!(state.handle_prack(1, 1, "invite"), "Method comparison should be case-insensitive");
    }

    #[test]
    fn test_parse_rack_header_invalid_format() {
        let mut msg = SipMessage::new_request(SipMethod::Prack, "sip:bob@example.com");
        msg.add_header("RAck", "invalid");
        assert!(parse_rack_header(&msg).is_none(), "Invalid RAck format should return None");

        let mut msg2 = SipMessage::new_request(SipMethod::Prack, "sip:bob@example.com");
        msg2.add_header("RAck", "1 2"); // Missing method
        assert!(parse_rack_header(&msg2).is_none(), "RAck with missing method should return None");
    }

    #[test]
    fn test_parse_rack_header_missing() {
        let msg = SipMessage::new_request(SipMethod::Prack, "sip:bob@example.com");
        assert!(parse_rack_header(&msg).is_none(), "Missing RAck header should return None");
    }
}
